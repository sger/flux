"""Flux values in lldb (proposal 0188 S6).

A native Flux build with `-g` describes each variable as a 64-bit word whose
typedef says how to read it:

- `FluxValue`: a tagged value. An odd word is an Int (`word >> 1`); a small
  even word is a sentinel (None, false, true, `[]`); anything else is a
  pointer to a heap object, whose header names its kind.
- `FluxCaptured`: a `FluxValue` the function captured from where it was made.
- `FluxInt`, `FluxBool`: a raw Int or Bool, in a function's unboxed worker.
- `FluxWord`: a word in a worker whose representation lowering did not know:
  a heap value if it points at one, otherwise a raw Int.

This script prints them as Flux values, the way `flux debug` does, and adds a
`flux` command:

    flux bt               the Flux frames of the current thread
    flux locals           the current frame's variables
    flux print <name>[.<part>...]
                          a variable, or a part of it, and its parts
    flux forget-steps     drop steps a breakpoint interrupted, so that
                          continuing does not finish them

Load it with `command script import flux_lldb.py`. On macOS, a `-g` build's
dSYM carries it, and lldb loads it from there once
`settings set target.load-script-from-symbol-file true` is set.
"""

import struct

import lldb

# What a debug slot holds before its variable is bound, and after its value
# was dropped (`src/lir/emit_debug.rs`).
UNBOUND_MARKER = 0x0DEB0DEB00000004
DROPPED_MARKER = 0x0DEB0DEB00000002

NONE, FALSE, TRUE, EMPTY_LIST, UNINIT, YIELD = 0, 2, 4, 6, 8, 10
MIN_PTR = 12
# Far above any user-space address on the platforms Flux supports.
MAX_PTR = 1 << 56

OBJ_STRING = 0xF1
OBJ_ADT = 0xF2
OBJ_TUPLE = 0xF3
OBJ_ARRAY = 0xF4
OBJ_CLOSURE = 0xF5
OBJ_BIGINT = 0xF6
OBJ_EVIDENCE = 0xF7
OBJ_FLOAT = 0xF8
OBJ_THUNK = 0xF9
KNOWN_OBJECTS = {
    OBJ_STRING,
    OBJ_ADT,
    OBJ_TUPLE,
    OBJ_ARRAY,
    OBJ_CLOSURE,
    OBJ_BIGINT,
    OBJ_EVIDENCE,
    OBJ_FLOAT,
    OBJ_THUNK,
}

SOME, LEFT, RIGHT, CONS = 1, 2, 3, 4
BUILTIN_CTORS = {SOME: "Some", LEFT: "Left", RIGHT: "Right", CONS: "Cons"}

# How much of a value one line shows before it is cut, as in the VM.
RENDER_LIMIT = 120
# How many parts of a value a variables view lists.
CHILD_LIMIT = 1000
# How many parts `flux print` lists.
PRINT_CHILDREN = 16
STRING_LIMIT = 4096


def _signed(word):
    word &= (1 << 64) - 1
    return word - (1 << 64) if word >= 1 << 63 else word


class Memory:
    """Reads the paused process's memory. Every read can fail: a variable
    may hold a stale or invalid word, and reading it must not error."""

    def __init__(self, process):
        self.process = process

    def read(self, address, size):
        if address <= 0 or address + size > MAX_PTR:
            return None
        error = lldb.SBError()
        data = self.process.ReadMemory(address, size, error)
        if not error.Success() or data is None or len(data) != size:
            return None
        return data

    def u8(self, address):
        data = self.read(address, 1)
        return None if data is None else data[0]

    def u32(self, address):
        data = self.read(address, 4)
        return None if data is None else struct.unpack("<I", data)[0]

    def i32(self, address):
        data = self.read(address, 4)
        return None if data is None else struct.unpack("<i", data)[0]

    def word(self, address):
        data = self.read(address, 8)
        return None if data is None else struct.unpack("<Q", data)[0]

    def double(self, address):
        data = self.read(address, 8)
        return None if data is None else struct.unpack("<d", data)[0]


class Constructors:
    """Constructor names by tag, and field names by constructor, from the
    `FluxCtor` enumeration and `FluxCtor.<Name>` structures every Flux
    compile unit carries."""

    _by_target = {}

    def __init__(self, target):
        self.target = target
        self.names = dict(BUILTIN_CTORS)
        self.fields = {}
        types = target.FindTypes("FluxCtor")
        for index in range(types.GetSize()):
            members = types.GetTypeAtIndex(index).GetEnumMembers()
            for member_index in range(members.GetSize()):
                member = members.GetTypeEnumMemberAtIndex(member_index)
                self.names.setdefault(member.GetValueAsSigned(), member.GetName())

    @classmethod
    def of(cls, target):
        key = target.GetExecutable().fullpath if target.IsValid() else None
        found = cls._by_target.get(key)
        if found is None:
            found = cls(target)
            cls._by_target[key] = found
        return found

    def field_names(self, name):
        """The names of `name`'s fields, or None when it has none."""
        if name not in self.fields:
            names = None
            ty = self.target.FindFirstType("FluxCtor." + name)
            if ty.IsValid():
                # The first two members are the tag and the field count.
                names = [
                    ty.GetFieldAtIndex(index).GetName()
                    for index in range(2, ty.GetNumberOfFields())
                ]
            self.fields[name] = names
        return self.fields[name]


class Value:
    """One Flux value: a word and how to read it."""

    def __init__(self, memory, ctors, word, kind="FluxValue"):
        self.memory = memory
        self.ctors = ctors
        self.word = word & ((1 << 64) - 1)
        self.kind = kind

    # ── What the word is ─────────────────────────────────────────────

    def marker(self):
        if self.word == UNBOUND_MARKER:
            return "unbound"
        if self.word == DROPPED_MARKER:
            return "dropped"
        return None

    def object_kind(self):
        """The heap object kind the word points at, or None."""
        word = self.word
        if word & 1 or word < MIN_PTR or word >= MAX_PTR or word % 8:
            return None
        tag = self.memory.u8(word - 3)
        if tag in KNOWN_OBJECTS:
            return tag
        if tag == 0:
            return "map"
        return None

    def tagged(self):
        """Whether the word is a tagged value rather than a raw one."""
        if self.kind in ("FluxValue", "FluxCaptured"):
            return True
        if self.kind == "FluxWord":
            return self.object_kind() is not None
        return False

    def adt(self):
        """(tag, field count) of an ADT, or None."""
        if not self.tagged() or self.object_kind() != OBJ_ADT:
            return None
        tag = self.memory.i32(self.word)
        count = self.memory.i32(self.word + 4)
        if tag is None or count is None or count < 0 or count > 1 << 16:
            return None
        return tag, count

    def at(self, address):
        word = self.memory.word(address)
        return None if word is None else Value(self.memory, self.ctors, word)

    # ── Parts ────────────────────────────────────────────────────────

    def children(self, limit, named=False):
        """The parts of a compound value, as (label, Value), labelled `0`,
        `1`, … as `print` addresses them; with `named`, a constructor's
        named fields by their names."""
        if self.marker() or not self.tagged():
            return []
        kind = self.object_kind()
        items = []
        if kind == OBJ_TUPLE:
            count = self.memory.u32(self.word + 4) or 0
            items = [self.at(self.word + 8 + 8 * index) for index in range(min(count, limit))]
        elif kind == OBJ_ARRAY:
            count = self.memory.u32(self.word + 4) or 0
            items = [self.at(self.word + 16 + 8 * index) for index in range(min(count, limit))]
        elif kind == OBJ_ADT:
            shape = self.adt()
            if shape is None:
                return []
            tag, count = shape
            if tag == CONS:
                items = self.list_items(limit)
            else:
                items = [self.at(self.word + 8 + 8 * index) for index in range(min(count, limit))]
                names = self.ctors.field_names(self.ctors.names.get(tag, "")) if named else None
                if names and len(names) == len(items):
                    return [(name, item) for name, item in zip(names, items) if item is not None]
        return [(str(index), item) for index, item in enumerate(items) if item is not None]

    def list_items(self, limit):
        items = []
        current = self
        while len(items) < limit:
            shape = current.adt()
            if shape is None or shape[0] != CONS:
                break
            head = current.at(current.word + 8)
            tail = current.at(current.word + 16)
            if head is None or tail is None:
                break
            items.append(head)
            current = tail
        return items

    def child(self, label):
        for name, item in self.children(CHILD_LIMIT, named=True):
            if name == label:
                return item
        if label.isdigit():
            for name, item in self.children(int(label) + 1):
                if name == label:
                    return item
        return None

    # ── Rendering ────────────────────────────────────────────────────

    def render(self):
        """One line, cut at the VM's limit."""
        out = []
        self._render(out, [0])
        text = "".join(out)
        if len(text) <= RENDER_LIMIT:
            return text
        return text[:RENDER_LIMIT] + "…"

    def _render(self, out, length, depth=0):
        def put(text):
            out.append(text)
            length[0] += len(text)

        if length[0] > RENDER_LIMIT * 4 or depth > 32:
            return
        marker = self.marker()
        if marker == "dropped":
            put("<dropped>")
            return
        if marker == "unbound":
            put("<unbound>")
            return
        if self.kind == "FluxInt" or (self.kind == "FluxWord" and not self.tagged()):
            put(str(_signed(self.word)))
            return
        if self.kind == "FluxBool":
            put("false" if self.word == 0 else "true")
            return

        word = self.word
        if word & 1:
            put(str(_signed(word) >> 1))
            return
        sentinels = {
            NONE: "None",
            FALSE: "false",
            TRUE: "true",
            EMPTY_LIST: "[]",
            UNINIT: "<uninit>",
            YIELD: "<yield>",
        }
        if word in sentinels:
            put(sentinels[word])
            return

        kind = self.object_kind()
        if kind is None:
            put("<invalid 0x%x>" % word)
        elif kind == OBJ_FLOAT:
            value = self.memory.double(word)
            put("<invalid>" if value is None else rust_float(value))
        elif kind == OBJ_STRING:
            size = self.memory.u32(word + 4)
            data = None if size is None else self.memory.read(word + 8, min(size, STRING_LIMIT))
            if data is None:
                put("<invalid string>")
            else:
                put('"' + data.decode("utf-8", "replace") + ("…" if size > STRING_LIMIT else "") + '"')
        elif kind == OBJ_TUPLE:
            items = [item for _, item in self.children(CHILD_LIMIT)]
            put("(")
            self._sequence(items, out, length, depth, put)
            if len(items) == 1:
                put(",")
            put(")")
        elif kind == OBJ_ARRAY:
            put("[|")
            self._sequence([item for _, item in self.children(CHILD_LIMIT)], out, length, depth, put)
            put("|]")
        elif kind == OBJ_ADT:
            shape = self.adt()
            if shape is None:
                put("<invalid adt>")
                return
            tag, count = shape
            if tag == CONS:
                put("[")
                self._sequence(self.list_items(CHILD_LIMIT), out, length, depth, put)
                put("]")
                return
            name = self.ctors.names.get(tag, "<adt tag=%d>" % tag)
            if count == 0:
                put(name)
                return
            put(name + "(")
            self._sequence([item for _, item in self.children(CHILD_LIMIT)], out, length, depth, put)
            put(")")
        elif kind == OBJ_CLOSURE:
            put("<closure>")
        elif kind == OBJ_BIGINT:
            put("<bigint>")
        elif kind == OBJ_THUNK:
            put("<thunk>")
        elif kind == OBJ_EVIDENCE:
            put("<internal>")
        elif kind == "map":
            put("{…}")
        else:
            put("<invalid 0x%x>" % word)

    def _sequence(self, items, out, length, depth, put):
        for index, item in enumerate(items):
            if index > 0:
                put(", ")
            item._render(out, length, depth + 1)
            if length[0] > RENDER_LIMIT * 4:
                break


def rust_float(value):
    """`value` as Rust's `{:?}` writes an f64, as the VM shows floats:
    shortest round-trip digits, always with a decimal point or exponent."""
    if value != value:
        return "NaN"
    if value in (float("inf"), float("-inf")):
        return "inf" if value > 0 else "-inf"
    if value == 0:
        return "-0.0" if str(value).startswith("-") else "0.0"
    text = repr(value)
    sign = ""
    if text.startswith("-"):
        sign, text = "-", text[1:]
    mantissa, _, exponent = text.partition("e")
    exponent = int(exponent) if exponent else 0
    whole, _, fraction = mantissa.partition(".")
    digits = (whole + fraction).lstrip("0")
    # The position of the decimal point relative to the digits.
    point = len(whole) + exponent - (len(whole + fraction) - len((whole + fraction).lstrip("0")))
    digits = digits.rstrip("0") or "0"
    magnitude = point - 1
    if -5 <= magnitude < 17:
        if point <= 0:
            return sign + "0." + "0" * -point + digits
        if point >= len(digits):
            return sign + digits + "0" * (point - len(digits)) + ".0"
        return sign + digits[:point] + "." + digits[point:]
    rest = digits[1:]
    return sign + digits[0] + ("." + rest if rest else "") + "e" + str(magnitude)


# ── lldb providers ───────────────────────────────────────────────────────


def _value_of(valobj):
    target = valobj.GetTarget()
    process = valobj.GetProcess()
    kind = valobj.GetType().GetName()
    return Value(Memory(process), Constructors.of(target), valobj.GetValueAsUnsigned(), kind)


def summary(valobj, internal_dict):
    """The one-line form of a Flux variable."""
    try:
        return _value_of(valobj.GetNonSyntheticValue()).render()
    except Exception as error:  # A summary must never fail.
        return "<error: %s>" % error


class FluxChildren:
    """The parts of a Flux value, so that a variables view can expand it."""

    def __init__(self, valobj, internal_dict):
        self.valobj = valobj
        self.items = []

    def update(self):
        self.items = []
        try:
            value = _value_of(self.valobj)
            self.items = value.children(CHILD_LIMIT, named=True)
        except Exception:
            self.items = []
        return False

    def num_children(self, max_children=None):
        return len(self.items)

    def has_children(self):
        try:
            return bool(_value_of(self.valobj).children(1))
        except Exception:
            return False

    def get_child_index(self, name):
        for index, (label, _) in enumerate(self.items):
            if label == name:
                return index
        return -1

    def get_child_at_index(self, index):
        if index < 0 or index >= len(self.items):
            return None
        label, item = self.items[index]
        target = self.valobj.GetTarget()
        ty = target.FindFirstType("FluxValue")
        if not ty.IsValid():
            ty = self.valobj.GetType()
        data = lldb.SBData.CreateDataFromUInt64Array(
            target.GetByteOrder(), target.GetAddressByteSize(), [item.word]
        )
        return self.valobj.CreateValueFromData(label, data, ty)


# ── The `flux` command ───────────────────────────────────────────────────


def _is_flux_frame(frame):
    entry = frame.GetLineEntry()
    name = entry.GetFileSpec().GetFilename() or ""
    return name.endswith(".flx") and entry.GetLine() != 0


def flux_frames(thread):
    """The thread's Flux frames, newest first, down to the Flux `main`."""
    frames = []
    for frame in thread:
        if not _is_flux_frame(frame):
            continue
        frames.append(frame)
        if frame.GetFunctionName() == "main":
            break
    return frames


def _place(frame):
    entry = frame.GetLineEntry()
    return "%s:%d:%d" % (
        entry.GetFileSpec().GetFilename(),
        entry.GetLine(),
        entry.GetColumn(),
    )


def frame_variables(frame):
    """The frame's visible variables, as (name, lldb value, captured):
    those bound by now, the innermost of each name, with captured ones
    last."""
    line = frame.GetLineEntry().GetLine()
    chosen = {}
    order = []
    variables = frame.GetVariables(True, True, False, True)
    for index in range(variables.GetSize()):
        variable = variables.GetValueAtIndex(index)
        name = variable.GetName()
        if not name or name.startswith("__"):
            continue
        raw = variable.GetNonSyntheticValue().GetValueAsUnsigned()
        if raw == UNBOUND_MARKER:
            continue
        declared = variable.GetDeclaration().GetLine()
        if declared > line and name in chosen:
            continue
        previous = chosen.get(name)
        if previous is None:
            order.append(name)
        if previous is None or declared >= previous[1]:
            chosen[name] = (variable, declared)
    result = []
    for name in order:
        variable = chosen[name][0]
        captured = variable.GetNonSyntheticValue().GetType().GetName() == "FluxCaptured"
        result.append((name, variable, captured))
    return [entry for entry in result if not entry[2]] + [entry for entry in result if entry[2]]


def _command_bt(debugger, args, result):
    thread = debugger.GetSelectedTarget().GetProcess().GetSelectedThread()
    frames = flux_frames(thread)
    if not frames:
        result.AppendMessage("no Flux frames")
        return
    for number, frame in enumerate(frames):
        result.AppendMessage("#%d %s at %s" % (number, frame.GetFunctionName(), _place(frame)))


def _selected_frame(debugger):
    thread = debugger.GetSelectedTarget().GetProcess().GetSelectedThread()
    frame = thread.GetSelectedFrame()
    if _is_flux_frame(frame):
        return frame
    frames = flux_frames(thread)
    return frames[0] if frames else None


def _command_locals(debugger, args, result):
    frame = _selected_frame(debugger)
    if frame is None:
        result.AppendMessage("no Flux frame")
        return
    variables = frame_variables(frame)
    if not variables:
        result.AppendMessage("no variables")
        return
    for name, variable, captured in variables:
        text = _value_of(variable.GetNonSyntheticValue()).render()
        result.AppendMessage("%s = %s%s" % (name, text, " (captured)" if captured else ""))


def _command_print(debugger, args, result):
    path = args.strip()
    parts = path.split(".")
    if not parts or not parts[0]:
        result.AppendMessage("usage: flux print <name>[.<n>...]")
        return
    frame = _selected_frame(debugger)
    if frame is None:
        result.AppendMessage("no Flux frame")
        return
    found = [variable for name, variable, _ in frame_variables(frame) if name == parts[0]]
    if not found:
        result.AppendMessage("no variable `%s` here" % parts[0])
        return
    value = _value_of(found[0].GetNonSyntheticValue())
    for part in parts[1:]:
        child = value.child(part)
        if child is None:
            result.AppendMessage("`%s` has no part `%s`" % (path, part))
            return
        value = child
    result.AppendMessage("%s = %s" % (path, value.render()))
    for label, child in value.children(PRINT_CHILDREN):
        result.AppendMessage("  .%s = %s" % (label, child.render()))


def drop_interrupted_steps(debugger):
    """Forgets the steps a breakpoint interrupted, so that continuing runs
    to the next breakpoint, as on the VM. lldb would otherwise finish such a
    step on the next continue and stop where it ends."""
    interpreter = debugger.GetCommandInterpreter()
    for _ in range(64):
        result = lldb.SBCommandReturnObject()
        interpreter.HandleCommand("thread plan discard 1", result)
        if not result.Succeeded():
            break


def _command_forget_steps(debugger, args, result):
    drop_interrupted_steps(debugger)


COMMANDS = {
    "bt": _command_bt,
    "locals": _command_locals,
    "print": _command_print,
    "p": _command_print,
    "forget-steps": _command_forget_steps,
}
USAGE = "usage: flux bt | flux locals | flux print <name>[.<part>...] | flux forget-steps"


def flux_command(debugger, command, result, internal_dict):
    words = command.strip().split(None, 1)
    if not words or words[0] not in COMMANDS:
        result.AppendMessage(USAGE)
        return
    try:
        COMMANDS[words[0]](debugger, words[1] if len(words) > 1 else "", result)
    except Exception as error:
        result.SetError("flux %s: %s" % (words[0], error))


def __lldb_init_module(debugger, internal_dict):
    module = __name__
    types = "FluxValue FluxCaptured FluxInt FluxBool FluxWord"
    debugger.HandleCommand("type category define flux")
    debugger.HandleCommand(
        "type summary add -v -w flux -F %s.summary %s" % (module, types)
    )
    debugger.HandleCommand(
        "type synthetic add -w flux -l %s.FluxChildren FluxValue FluxCaptured FluxWord" % module
    )
    debugger.HandleCommand("type category enable flux")
    debugger.HandleCommand(
        'command script add -o -h "Flux frames, variables and values." -f %s.flux_command flux'
        % module
    )
    # Step over the runtime, which is built with debug information, rather
    # than into it. Flux functions carry their Flux names, never these.
    debugger.HandleCommand(
        "settings set target.process.thread.step-avoid-regexp "
        "'^(flux_|_?rust_|__rust|std::|core::|alloc::)'"
    )
