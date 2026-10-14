"""Flux values in lldb (proposal 0188 S6, S7, S8).

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

    flux bt               the Flux frames of the current thread, with the
                          frames an effect handler's `resume` would run
    flux locals           the current frame's variables
    flux print <name>[.<part>...]
                          a variable, or a part of it, and its parts
    flux handlers         the active effect handlers, innermost first
    flux threads          the fibers of `run_async`, and what each waits for
    flux thread <n>       look at thread <n>: `flux bt` reads it
    flux catch [panic|division|unhandled|fibers|errors|all|none]...
                          the runtime errors to stop at; alone, lists them
    flux where            where the program stopped, and why
    flux next, flux step, flux finish
                          step over, into or out, as `flux debug` does:
                          following a perform to where its frame goes on,
                          and stopping once per line
    flux forget-steps     drop steps a breakpoint interrupted, so that
                          continuing does not finish them

Load it with `command script import flux_lldb.py`. On macOS, a `-g` build's
dSYM carries it, and lldb loads it from there once
`settings set target.load-script-from-symbol-file true` is set.
"""

import json
import re
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


# ── Stacks that follow continuations (0188 S7) ──────────────────────────
#
# A perform on the general path returns from every frame between it and the
# handler, each leaving a `$cont$` closure (named `f (resumed after L:C)` in
# the DWARF) that runs the rest of it. The handler's arm gets them as
# `resume`: one such closure, or the runtime's compose trampoline holding
# an array of them, innermost first. While the arm runs, they are the
# frames it suspended; while `resume` runs them, the ones still to run sit
# in that array.

RESUMED = re.compile(r"^(.*) \(resumed after (\d+):(\d+)\)$")
TRAMPOLINE = "flux_compose_trampoline.closure_entry"
CLOSURE_ENTRY = ".closure_entry"
# The header before a closure's captures (`FluxClosure`, flux_rt.c).
CLOSURE_PAYLOAD = 24
MAX_CONTINUATIONS = 256


class Shown:
    """One line of the logical stack."""

    def __init__(self, kind, name, file, line, column, frame=None, path=None):
        self.kind = kind  # "live", "suspended" or "label"
        self.name = name
        self.file = file
        self.line = line
        self.column = column
        self.frame = frame
        self.path = path

    def place(self):
        return "%s:%d:%d" % (self.file, self.line, self.column)


def flux_name(name):
    """A function's Flux name: a continuation is the function it resumes."""
    match = RESUMED.match(name or "")
    return match.group(1) if match else (name or "")


def _symbol_name(symbol):
    name = symbol.GetName() if symbol.IsValid() else None
    return (name or "").lstrip("_") if name and name.startswith("_flux") else name


def _function_at(target, address):
    """The symbol at a code address, and the Flux function it runs: a
    closure's entry wrapper has no DWARF of its own, the function it wraps
    does."""
    resolved = target.ResolveLoadAddress(address)
    name = _symbol_name(resolved.GetSymbol())
    if not name:
        return None, None
    function = resolved.GetFunction()
    if name.endswith(CLOSURE_ENTRY) and name != TRAMPOLINE:
        found = target.FindSymbols(name[: -len(CLOSURE_ENTRY)])
        function = None
        for index in range(found.GetSize()):
            candidate = found.GetContextAtIndex(index).GetSymbol().GetStartAddress().GetFunction()
            if candidate.IsValid():
                function = candidate
                break
    if function is not None and not function.IsValid():
        function = None
    return name, function


def continuations(memory, target, word, depth=0):
    """The continuations a `resume` value runs, innermost first, as Shown
    lines: each at the perform or call it resumes after."""
    value = Value(memory, None, word)
    if depth > 8 or value.object_kind() != OBJ_CLOSURE:
        return []
    code = memory.word(word)
    if code is None:
        return []
    name, function = _function_at(target, code)
    if name == TRAMPOLINE:
        array = memory.word(word + CLOSURE_PAYLOAD)
        if array is None or Value(memory, None, array).object_kind() != OBJ_ARRAY:
            return []
        count = min(memory.u32(array + 4) or 0, MAX_CONTINUATIONS)
        found = []
        for index in range(count):
            item = memory.word(array + 16 + 8 * index)
            if item is not None:
                found.extend(continuations(memory, target, item, depth + 1))
        return found
    if function is None:
        return []
    match = RESUMED.match(function.GetName() or "")
    if not match:
        return []
    spec = function.GetStartAddress().GetLineEntry().GetFileSpec()
    return [
        Shown(
            "suspended",
            match.group(1),
            spec.GetFilename() or "?",
            int(match.group(2)),
            int(match.group(3)),
            path=spec.fullpath,
        )
    ]


def _resume_word(frame):
    """An arm's `resume` argument, its first."""
    arguments = frame.GetVariables(True, False, False, True)
    if arguments.GetSize() == 0:
        return None
    return arguments.GetValueAtIndex(0).GetNonSyntheticValue().GetValueAsUnsigned()


def _is_arm(name):
    return (name or "").startswith("<handler ")


def logical_stack(thread):
    """The thread's Flux stack, newest first, down to the Flux `main`: each
    handler arm with the frames it suspended below it, and the continuations
    a running `resume` has still to run above it."""
    target = thread.GetProcess().GetTarget()
    memory = Memory(thread.GetProcess())
    shown = []
    resumed = []  # continuation frames above the next arm, newest first
    for frame in thread:
        if not _is_flux_frame(frame):
            continue
        name = frame.GetFunctionName() or ""
        entry = frame.GetLineEntry()
        if _is_arm(name):
            word = _resume_word(frame)
            pending = continuations(memory, target, word) if word is not None else []
            if resumed:
                # `resume` is running the continuations: the ones after the
                # running one wait in the trampoline, below it.
                running = flux_name(resumed[-1])
                names = [cont.name for cont in pending]
                if running in names:
                    for cont in pending[names.index(running) + 1 :]:
                        cont.kind = "live"
                        shown.append(cont)
                resumed = []
            shown.append(
                Shown("live", name, entry.GetFileSpec().GetFilename(), entry.GetLine(),
                      entry.GetColumn(), frame)
            )
            if not resumed and pending and not _resuming(shown):
                performed = pending[0]
                shown.append(
                    Shown("label", "handling %s — performed at %s"
                          % (name[len("<handler "):-1], performed.place()), None, 0, 0)
                )
                shown.extend(pending)
        else:
            if RESUMED.match(name):
                resumed.append(name)
            shown.append(
                Shown("live", flux_name(name), entry.GetFileSpec().GetFilename(),
                      entry.GetLine(), entry.GetColumn(), frame)
            )
        if name == "main":
            break
    return shown


def _resuming(shown):
    """Whether the arm just added is running `resume`: a continuation frame
    sits above it."""
    for line in reversed(shown[:-1]):
        if line.frame is None:
            continue
        name = line.frame.GetFunctionName() or ""
        if _is_arm(name):
            return False
        if RESUMED.match(name):
            return True
    return False


def _command_bt(debugger, args, result):
    process = debugger.GetSelectedTarget().GetProcess()
    words = args.split()
    number = None
    if "--thread" in words:
        # `--thread N`: thread N's stack, without selecting it (0188 S8).
        index = words.index("--thread")
        try:
            number = int(words[index + 1])
        except (IndexError, ValueError):
            result.AppendMessage("usage: flux bt [--json] [--thread <n>]")
            return
    shown = thread_stack(process, number)
    if "--json" in words:
        # For the debug adapter, which splices the frames lldb does not
        # see into lldb-dap's stack: each line, with its lldb frame index
        # when it is a frame lldb has.
        result.AppendMessage(
            json.dumps(
                [
                    {
                        "kind": line.kind,
                        "name": line.name,
                        "path": line.path,
                        "line": line.line,
                        "column": line.column,
                        "index": line.frame.GetFrameID() if line.frame is not None else None,
                    }
                    for line in shown
                ]
            )
        )
        return
    if not shown:
        result.AppendMessage("no Flux frames")
        return
    number = 0
    for line in shown:
        if line.kind == "label":
            result.AppendMessage("   \u27c2 %s" % line.name)
            continue
        note = "   (suspended)" if line.kind == "suspended" else ""
        result.AppendMessage("#%d %s at %s%s" % (number, line.name, line.place(), note))
        number += 1


# ── Fibers as threads (0188 S8) ─────────────────────────────────────────
#
# Native fibers are not OS threads: the runtime's scheduler runs them on
# its workers, and a fiber that waits is an entry in its tables holding a
# continuation, the closure `resume` would be (S7). `flux_debug_fibers()`
# answers what those tables hold, as JSON. It is called in the stopped
# program, the one call into it these commands make, and only while the
# stop is inside `run_async`; it takes no lock another thread may hold.
#
# Thread 1 is the program and its root fiber, each other fiber the next
# number in spawn order, as in `flux debug`. `flux thread N` makes `flux
# bt` read thread N until the program runs again.

FIBER_ENTRY = "execute_fiber"
FIBERS_FUNCTION = "flux_debug_fibers"
SCHEDULER_ROOT = "flux_async_run_root"
# The thread looked at, per process: (stop id, thread number).
SELECTED = {}


def _fiber_entry_index(thread):
    """The index of the scheduler frame that runs the thread's fiber, or
    None when the thread runs no fiber."""
    for index in range(thread.GetNumFrames()):
        if FIBER_ENTRY in (thread.GetFrameAtIndex(index).GetFunctionName() or ""):
            return index
    return None


def _in_async(process):
    return any(_fiber_entry_index(thread) is not None for thread in process) or any(
        SCHEDULER_ROOT in (frame.GetFunctionName() or "") for thread in process for frame in thread
    )


def fibers(process):
    """The fibers of the active `run_async`, as the runtime lists them, or
    [] outside one."""
    if not _in_async(process):
        return []
    target = process.GetTarget()
    found = target.FindSymbols(FIBERS_FUNCTION)
    address = None
    for index in range(found.GetSize()):
        symbol = found.GetContextAtIndex(index).GetSymbol()
        if symbol.IsValid():
            address = symbol.GetStartAddress().GetLoadAddress(target)
            break
    if not address or address == lldb.LLDB_INVALID_ADDRESS:
        return []
    options = lldb.SBExpressionOptions()
    options.SetTryAllThreads(False)
    options.SetUnwindOnError(True)
    options.SetIgnoreBreakpoints(True)
    options.SetTimeoutInMicroSeconds(2000000)
    frame = process.GetSelectedThread().GetFrameAtIndex(0)
    value = frame.EvaluateExpression(
        "(const char *)((const char *(*)(void))%d)()" % address, options
    )
    if not value.IsValid() or not value.GetError().Success():
        return []
    error = lldb.SBError()
    text = process.ReadCStringFromMemory(value.GetValueAsUnsigned(), 1 << 16, error)
    if not error.Success() or not text:
        return []
    try:
        answer = json.loads(text)
    except ValueError:
        return []
    return sorted(answer.get("fibers", []), key=lambda fiber: fiber["thread"])


def _library_file(path):
    return "/Flow/" in (path or "").replace("\\", "/")


def _fiber_name(target, fiber):
    if fiber["thread"] == 1:
        return "main"
    code = fiber.get("code") or 0
    _, function = _function_at(target, code) if code else (None, None)
    if function is None:
        return "<fiber>"
    return flux_name(function.GetName())


def _thread_by_id(process, os_id):
    for thread in process:
        if thread.GetThreadID() == os_id:
            return thread
    return None


def stopped_thread(process, listed=None):
    """The number of the thread the program stopped in: the fiber running
    on the selected OS thread, else 1."""
    listed = fibers(process) if listed is None else listed
    os_id = process.GetSelectedThread().GetThreadID()
    for fiber in listed:
        if fiber["state"] == "running" and fiber.get("os") == os_id:
            return fiber["thread"]
    return 1


def selected_thread(process, listed=None):
    chosen = SELECTED.get(process.GetUniqueID())
    if chosen is not None and chosen[0] == process.GetStopID():
        return chosen[1]
    return stopped_thread(process, listed)


def _start_line(target, code):
    """A function that has not run, at its first line."""
    _, function = _function_at(target, code) if code else (None, None)
    if function is None:
        return None
    entry = function.GetStartAddress().GetLineEntry()
    spec = entry.GetFileSpec()
    return Shown(
        "suspended",
        flux_name(function.GetName()),
        spec.GetFilename() or "?",
        entry.GetLine(),
        entry.GetColumn(),
        path=spec.fullpath,
    )


def _below_scheduler(process):
    """The Flux frames below the scheduler: the code that called
    `run_async`, newest first."""
    for thread in process:
        frames = list(thread)
        for index, frame in enumerate(frames):
            if SCHEDULER_ROOT in (frame.GetFunctionName() or ""):
                shown = []
                for below in frames[index + 1 :]:
                    if not _is_flux_frame(below):
                        continue
                    entry = below.GetLineEntry()
                    name = below.GetFunctionName() or ""
                    shown.append(
                        Shown("live", flux_name(name), entry.GetFileSpec().GetFilename(),
                              entry.GetLine(), entry.GetColumn(), below,
                              path=entry.GetFileSpec().fullpath)
                    )
                    if name == "main":
                        break
                return shown
    return []


def _live_stack(thread, fiber_number):
    """A running thread's stack: a child fiber's ends where the scheduler
    starts it; the root fiber's goes on down to `main`."""
    shown = logical_stack(thread)
    entry = _fiber_entry_index(thread)
    if entry is None or fiber_number == 1:
        return shown
    kept = []
    for line in shown:
        if line.frame is not None and line.frame.GetFrameID() > entry:
            break
        kept.append(line)
    return kept


def thread_stack(process, number=None):
    """Thread `number`'s Flux stack, newest first: the selected thread's
    when none is given."""
    listed = fibers(process)
    if not listed:
        return logical_stack(process.GetSelectedThread())
    number = selected_thread(process, listed) if number is None else number
    fiber = next((fiber for fiber in listed if fiber["thread"] == number), None)
    if fiber is None:
        return []
    target = process.GetTarget()
    if fiber["state"] == "running":
        thread = _thread_by_id(process, fiber.get("os"))
        return _live_stack(thread, number) if thread is not None else []
    if fiber["state"] == "not started":
        line = _start_line(target, fiber.get("code"))
        return [line] if line is not None else []
    shown = continuations(Memory(process), target, fiber.get("work") or 0)
    # The library functions it parked in (`sleep`, `Channel.recv`, ...):
    # the fiber waits at its own call to them.
    while shown and _library_file(shown[0].path):
        shown.pop(0)
    if number == 1:
        shown.extend(_below_scheduler(process))
    return shown


def threads(process):
    """Each thread: (number, name, state, top line or None, the OS thread a
    running fiber is on or None)."""
    listed = fibers(process)
    target = process.GetTarget()
    if not listed:
        shown = logical_stack(process.GetSelectedThread())
        return [(1, "main", "stopped", shown[0] if shown else None,
                 process.GetSelectedThread().GetThreadID())]
    stopped = stopped_thread(process, listed)
    found = []
    for fiber in listed:
        state = fiber["state"]
        if state == "running":
            state = "stopped" if fiber["thread"] == stopped else "running"
        elif state == "waiting":
            state = "waiting %s" % fiber.get("blocked", "a completion")
        shown = thread_stack(process, fiber["thread"])
        found.append((fiber["thread"], _fiber_name(target, fiber), state,
                      shown[0] if shown else None,
                      fiber.get("os") if fiber["state"] == "running" else None))
    return found


def _command_threads(debugger, args, result):
    process = debugger.GetSelectedTarget().GetProcess()
    found = threads(process)
    if args.strip() == "--json":
        # For the debug adapter: a running fiber is the OS thread `os`.
        result.AppendMessage(json.dumps([
            {"id": number, "name": name, "state": state, "os": os}
            for number, name, state, _, os in found
        ]))
        return
    looking_at = selected_thread(process)
    for number, name, state, top, _ in found:
        mark = "*" if number == looking_at else " "
        place = " at %s" % top.place() if top is not None else ""
        result.AppendMessage("%s %d %s [%s]%s" % (mark, number, name, state, place))


def _command_thread(debugger, args, result):
    process = debugger.GetSelectedTarget().GetProcess()
    try:
        number = int(args.strip())
    except ValueError:
        result.AppendMessage("usage: flux thread <n>; `flux threads` lists them")
        return
    found = {entry[0]: entry for entry in threads(process)}
    if number not in found:
        result.AppendMessage("no thread %d; `flux threads` lists them" % number)
        return
    listed = fibers(process)
    fiber = next((fiber for fiber in listed if fiber["thread"] == number), None)
    if fiber is not None and fiber["state"] == "running":
        process.SetSelectedThreadByID(fiber["os"])
    SELECTED[process.GetUniqueID()] = (process.GetStopID(), number)
    _, name, state, _, _ = found[number]
    result.AppendMessage("thread %d: %s [%s]" % (number, name, state))


# ── Handlers (0188 S7) ──────────────────────────────────────────────────
#
# The runtime keeps the active handlers in the thread's evidence vector,
# `flux_thread_ctx.current_evv` (effects.c): a count, then five words per
# handler, oldest first.

EVV_ENTRY_WORDS = 5
# The effects the compiler handles around `main`: internals, as in
# `flux debug`.
DEFAULT_HANDLED = {
    "Console", "FileSystem", "Stdin", "Env", "Process", "Clock", "Time", "IO",
}
SHOW_INTERNALS = [False]


def _current_evidence(frame):
    """`flux_thread_ctx.current_evv`, a thread-local, read in this frame's
    thread. The runtime is built with debug information, so the expression
    is a read lldb answers itself, running nothing in the program."""
    options = lldb.SBExpressionOptions()
    options.SetTryAllThreads(False)
    options.SetUnwindOnError(True)
    value = frame.EvaluateExpression("flux_thread_ctx.current_evv", options)
    if not value.IsValid() or not value.GetError().Success():
        return None
    return value.GetValueAsUnsigned()


def handlers(thread):
    """The active handlers, innermost first, as (effect, state Value or
    None)."""
    process = thread.GetProcess()
    target = process.GetTarget()
    memory = Memory(process)
    ctors = Constructors.of(target)
    frame = thread.GetFrameAtIndex(0)
    word = _current_evidence(frame)
    if not word or Value(memory, ctors, word).object_kind() != OBJ_EVIDENCE:
        return []
    count = memory.i32(word) or 0
    found = []
    for index in range(min(count, 1024)):
        base = word + 8 + 8 * EVV_ENTRY_WORDS * index
        clause = memory.word(base + 16)
        state = memory.word(base + 32)
        code = memory.word(clause) if clause else None
        _, function = _function_at(target, code) if code else (None, None)
        arm = function.GetName() if function is not None else ""
        effect = arm[len("<handler "):-1].rsplit(".", 1)[0] if _is_arm(arm) else "<effect>"
        found.append((effect, arm, Value(memory, ctors, state) if state else None))
    found.reverse()
    # In a handler's arm, the handlers inside it are suspended with the
    # frames that performed: the arm runs under its own handler and those
    # outside it.
    shown = logical_stack(thread)
    if len(shown) > 1 and shown[1].kind == "label" and shown[0].frame is not None:
        arm = shown[0].frame.GetFunctionName()
        arms = [entry[1] for entry in found]
        if arm in arms:
            found = found[arms.index(arm):]
    return [
        (effect, state)
        for effect, _, state in found
        if effect not in DEFAULT_HANDLED or SHOW_INTERNALS[0]
    ]


def _command_handlers(debugger, args, result):
    thread = debugger.GetSelectedTarget().GetProcess().GetSelectedThread()
    found = handlers(thread)
    if not found:
        result.AppendMessage("no handlers")
        return
    for number, (effect, state) in enumerate(found):
        text = " state = %s" % state.render() if state is not None else ""
        result.AppendMessage("#%d %s%s" % (number, effect, text))


# ── Runtime errors (0188 S7) ────────────────────────────────────────────
#
# The runtime calls an empty hook before it reports an error:
# `flux_debug_on_panic` from `flux_panic`, which user panics, division by
# zero and the runtime's other errors all go through, and
# `flux_debug_on_unhandled_effect`. `flux catch` breaks on them; the
# breakpoint's callback tells the kinds apart and goes on past the ones not
# asked for.

PANIC_HOOK = "flux_debug_on_panic"
UNHANDLED_HOOK = "flux_debug_on_unhandled_effect"
FIBER_FAILURE_HOOK = "flux_debug_on_fiber_failure"
HOOKS = (PANIC_HOOK, UNHANDLED_HOOK, FIBER_FAILURE_HOOK)
KINDS = {
    "panic": "panic",
    "division": "division_by_zero",
    "division_by_zero": "division_by_zero",
    "unhandled": "unhandled_effect",
    "unhandled_effect": "unhandled_effect",
    "fibers": "fiber_failure",
    "fiber_failure": "fiber_failure",
    "errors": "runtime_error",
    "runtime_error": "runtime_error",
}
ALL_KINDS = ["panic", "division_by_zero", "unhandled_effect", "fiber_failure", "runtime_error"]
LABELS = {
    "panic": "panic",
    "division_by_zero": "division by zero",
    "unhandled_effect": "unhandled effect",
    "fiber_failure": "fiber failure",
    "runtime_error": "runtime error",
}
CAUGHT = set()
# The error the program last stopped at: (kind, message), per process.
EXCEPTION = {}


def _classify(thread):
    """The kind and message of the error a hook stopped at."""
    top_frame = thread.GetFrameAtIndex(0)
    top = top_frame.GetFunctionName() or ""
    if top.startswith(UNHANDLED_HOOK):
        return "unhandled_effect", "unhandled effect"
    if top.startswith(FIBER_FAILURE_HOOK):
        memory = Memory(thread.GetProcess())
        word = _first_argument(top_frame, "err")
        text = Value(memory, Constructors.of(thread.GetProcess().GetTarget()),
                     word).render() if word is not None else ""
        return "fiber_failure", text or LABELS["fiber_failure"]
    message = ""
    kind = "runtime_error"
    memory = Memory(thread.GetProcess())
    for index in range(1, min(thread.GetNumFrames(), 8)):
        frame = thread.GetFrameAtIndex(index)
        name = frame.GetFunctionName() or ""
        if name.startswith("flux_panic") and not message:
            msg = frame.FindVariable("msg")
            if msg.IsValid():
                text = Value(memory, None, msg.GetValueAsUnsigned()).render()
                message = text[1:-1] if len(text) >= 2 and text[0] == '"' else text
            continue
        if "division_by_zero" in name or name.startswith(("flux_rt_div", "flux_rt_mod")):
            return "division_by_zero", "division by zero"
        if _is_flux_frame(frame):
            kind = "panic"
            break
    return kind, message or LABELS[kind]


def _first_argument(frame, name):
    """A hook's first argument: its variable when the optimised runtime
    kept it, else the register it arrived in, at the hook's entry."""
    variable = frame.FindVariable(name)
    if variable.IsValid() and variable.GetError().Success() and variable.GetValue() is not None:
        return variable.GetValueAsUnsigned()
    for register in ("x0", "rdi"):
        value = frame.FindRegister(register)
        if value.IsValid() and value.GetValue() is not None:
            return value.GetValueAsUnsigned()
    return None


def _first_flux_frame(thread):
    for index in range(thread.GetNumFrames()):
        if _is_flux_frame(thread.GetFrameAtIndex(index)):
            return index
    return None


def exception_stop(frame, location, *rest):
    """The hooks' breakpoint callback: stop for the kinds caught, at the
    Flux frame the error happened in."""
    thread = frame.GetThread()
    kind, message = _classify(thread)
    EXCEPTION[thread.GetProcess().GetUniqueID()] = (kind, message)
    if kind not in CAUGHT:
        return False
    index = _first_flux_frame(thread)
    if index is not None:
        thread.SetSelectedFrame(index)
    return True


def _hook_breakpoints(target):
    found = []
    for index in range(target.GetNumBreakpoints()):
        breakpoint = target.GetBreakpointAtIndex(index)
        if breakpoint.MatchesName("flux-exception"):
            found.append(breakpoint)
    return found


def set_caught(debugger, kinds):
    """Stops at these kinds of runtime errors, and no others."""
    CAUGHT.clear()
    CAUGHT.update(kinds)
    target = debugger.GetSelectedTarget()
    existing = _hook_breakpoints(target)
    if CAUGHT and not existing:
        for hook in HOOKS:
            breakpoint = target.BreakpointCreateByName(hook)
            breakpoint.AddName("flux-exception")
            breakpoint.SetScriptCallbackFunction("%s.exception_stop" % __name__)
    elif not CAUGHT:
        for breakpoint in existing:
            target.BreakpointDelete(breakpoint.GetID())


def _command_catch(debugger, args, result):
    words = args.split()
    if words:
        kinds = set()
        for word in words:
            if word == "all":
                kinds.update(ALL_KINDS)
            elif word == "none":
                continue
            elif word in KINDS:
                kinds.add(KINDS[word])
            else:
                result.AppendMessage(
                    "unknown error kind `%s`; use panic, division, unhandled, fibers, errors, all or none"
                    % word
                )
                return
        set_caught(debugger, kinds)
    caught = [LABELS[kind] for kind in ALL_KINDS if kind in CAUGHT]
    if caught:
        result.AppendMessage("stopping at: %s" % ", ".join(caught))
    else:
        result.AppendMessage("stopping at no runtime errors")


def stop_description(thread):
    """`stopped: <why> at <file:line:col> in <function>`, as `flux debug`
    says it, or None when the thread is not stopped in Flux code."""
    if not thread.IsValid():
        return None
    top_frame = thread.GetFrameAtIndex(0)
    top_name = top_frame.GetFunctionName() or ""
    exception = EXCEPTION.get(thread.GetProcess().GetUniqueID())
    at_error = top_name.startswith(HOOKS) and exception is not None
    # Stepping out of `main` stops in the runtime: not a Flux stop.
    if not at_error and not _is_flux_frame(top_frame):
        return None
    shown = [line for line in logical_stack(thread) if line.kind != "label"]
    if not shown:
        return None
    if at_error:
        kind, message = exception
        label = LABELS[kind]
        why = "exception (%s)" % message if message == label else "exception (%s: %s)" % (
            label,
            message,
        )
    elif thread.GetStopReason() == lldb.eStopReasonBreakpoint and thread.GetProcess().GetTarget().FindBreakpointByID(thread.GetStopReasonDataAtIndex(0)).IsValid():
        # A breakpoint `flux next` or `flux step` set for itself is gone
        # by now: that stop is a step.
        why = "breakpoint"
    else:
        why = "step"
    top = shown[0]
    process = thread.GetProcess()
    listed = fibers(process)
    number = stopped_thread(process, listed) if listed else 1
    if number > 1:
        top = next(iter(_live_stack(thread, number)), top)
        return "stopped: %s at %s in %s (thread %d)" % (why, top.place(), top.name, number)
    return "stopped: %s at %s in %s" % (why, top.place(), top.name)


def _command_where(debugger, args, result):
    thread = debugger.GetSelectedTarget().GetProcess().GetSelectedThread()
    text = stop_description(thread)
    result.AppendMessage(text if text else "not stopped in Flux code")


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


# ── Stepping across effects (0188 S7) ───────────────────────────────────
#
# A perform on the general path returns from the performing frames before
# the handler runs: lldb's step over a perform line ends in the frame that
# installed the handler. `flux next` and `flux step` step the way `flux
# debug` does. When a step leaves its frame while the runtime is yielding,
# they break where the frame goes on, its continuation (or, stepping in,
# the handler's arm), and keep stepping over in the frame they landed in, so
# a handler that never resumes ends at that frame's next line instead.

MAX_STEPS = 256


def _context_word(frame, field):
    """A field of the thread's `flux_thread_ctx`, read in the first frame
    from `frame` down that can name it: a closure's entry wrapper has no
    debug information to read it in."""
    options = lldb.SBExpressionOptions()
    options.SetTryAllThreads(False)
    thread = frame.GetThread()
    for index in range(frame.GetFrameID(), min(thread.GetNumFrames(), frame.GetFrameID() + 16)):
        value = thread.GetFrameAtIndex(index).EvaluateExpression(
            "flux_thread_ctx.%s" % field, options
        )
        if value.IsValid() and value.GetError().Success():
            return value.GetValueAsUnsigned()
    return None


def _frame_alive(thread, cfa):
    for frame in thread:
        if frame.GetCFA() == cfa:
            return True
    return False


def _functions_named(target, prefix):
    found = target.FindGlobalFunctions("^" + re.escape(prefix), 64, lldb.eMatchTypeRegex)
    return [found.GetContextAtIndex(i).GetFunction() for i in range(found.GetSize())]


def _break_at_functions(target, functions):
    breakpoints = []
    for function in functions:
        if function is None or not function.IsValid():
            continue
        address = function.GetStartAddress()
        breakpoint = target.BreakpointCreateBySBAddress(address)
        # Past the prologue, where the function's first line starts.
        prologue = function.GetPrologueByteSize()
        if prologue:
            target.BreakpointDelete(breakpoint.GetID())
            breakpoint = target.BreakpointCreateByAddress(
                address.GetLoadAddress(target) + prologue
            )
        breakpoint.SetOneShot(False)
        breakpoints.append(breakpoint)
    return breakpoints


def _step(debugger, result, into, out=False):
    target = debugger.GetSelectedTarget()
    process = target.GetProcess()
    thread = process.GetSelectedThread()
    origin = thread.GetSelectedFrame()
    if not _is_flux_frame(origin):
        index = _first_flux_frame(thread)
        origin = thread.GetFrameAtIndex(index) if index is not None else origin
    name = origin.GetFunctionName() or ""
    line = origin.GetLineEntry().GetLine()
    cfa = origin.GetCFA()
    was_async = debugger.GetAsync()
    debugger.SetAsync(False)
    # In a fiber, the step stays in it while it runs (0188 S8): another
    # fiber running the same function does not end it.
    origin_fiber = stopped_thread(process) if _fiber_entry_index(thread) is not None else None
    anywhere = []
    # One stop per line, as in `flux debug`: returning into a caller, on
    # the line of its call, is not a new place to stop.
    seen = {
        (frame.GetCFA(), frame.GetLineEntry().GetLine())
        for frame in thread
        if _is_flux_frame(frame) and frame.GetCFA() != cfa
    }
    breakpoints = []
    if into and _is_arm(name):
        # Into `resume`: the runtime calls the continuations, which lldb
        # steps over with the rest of the runtime. Break where they start.
        word = _resume_word(origin)
        if word is not None:
            functions = []
            for cont in continuations(Memory(process), target, word):
                functions.extend(
                    _functions_named(target, "%s (resumed after %d:" % (cont.name, cont.line))
                )
            breakpoints = _break_at_functions(target, functions)
    if RESUMED.match(name):
        # A resumed frame returns into the runtime's trampoline, which runs
        # the continuations after it: the frames below it on the logical
        # stack. Break where they start.
        shown = logical_stack(thread)
        waiting = []
        for index, entry in enumerate(shown):
            if entry.frame is not None and entry.frame.GetCFA() == cfa:
                for below in shown[index + 1 :]:
                    if below.frame is not None or below.kind != "live":
                        break
                    waiting.append(below)
                break
        functions = []
        for cont in waiting:
            functions.extend(
                _functions_named(target, "%s (resumed after %d:" % (cont.name, cont.line))
            )
        breakpoints.extend(_break_at_functions(target, functions))
    if out:
        thread.StepOut()
    elif into:
        thread.StepInto()
    else:
        thread.StepOver()
    try:
        armed = False
        for _ in range(MAX_STEPS):
            if process.GetState() != lldb.eStateStopped:
                return
            thread = process.GetSelectedThread()
            frame = thread.GetFrameAtIndex(0)
            hit_id = (
                thread.GetStopReasonDataAtIndex(0)
                if thread.GetStopReason() == lldb.eStopReasonBreakpoint
                else None
            )
            hit = any(hit_id == bp.GetID() for bp in breakpoints + anywhere)
            if (
                hit
                and origin_fiber is not None
                and not any(hit_id == bp.GetID() for bp in anywhere)
                and stopped_thread(process) != origin_fiber
            ):
                # The same function, in another fiber: go on.
                process.Continue()
                continue
            if hit:
                # A continuation starts on the line of the call it resumes
                # after, which has run: go on to the next.
                match = RESUMED.match(frame.GetFunctionName() or "")
                if not _is_flux_frame(frame) or (
                    match and frame.GetLineEntry().GetLine() == int(match.group(2))
                ):
                    thread.StepOver()
                return
            if _at_hook(frame):
                return
            alive = _frame_alive(thread, cfa)
            yielding = _context_word(frame, "yielding")
            if yielding and not alive and not armed:
                # The frame returned to let a handler run: break where it
                # goes on, and, stepping in, where the handler starts.
                armed = True
                targets = []
                if into:
                    clause = _context_word(frame, "clause")
                    code = Memory(process).word(clause) if clause else None
                    if code:
                        _, function = _function_at(target, code)
                        targets.append(function)
                targets.extend(
                    _functions_named(target, "%s (resumed after %d:" % (flux_name(name), line))
                )
                breakpoints.extend(_break_at_functions(target, targets))
            if origin_fiber is not None and not alive and _fiber_finished(thread):
                # Out of the fiber's code, in the scheduler. Parked, it
                # goes on where its frame does; finished, the step stops
                # where the next fiber goes on, as `flux debug`'s does.
                mine = next(
                    (fiber for fiber in fibers(process) if fiber["thread"] == origin_fiber), None
                )
                if mine is not None and mine["state"] == "running" and not yielding:
                    # Returning its result to the scheduler: finished.
                    mine = None
                if mine is not None and not armed:
                    armed = True
                    breakpoints.extend(
                        _break_at_functions(
                            target,
                            _functions_named(
                                target, "%s (resumed after %d:" % (flux_name(name), line)
                            ),
                        )
                    )
                    process.Continue()
                    continue
                if mine is None and not anywhere:
                    anywhere = _break_where_fibers_go_on(process)
                    if anywhere:
                        process.Continue()
                        continue
            if not _is_flux_frame(frame):
                # The runtime: out to the Flux code that called it.
                thread.StepOut()
                continue
            here = (frame.GetCFA(), frame.GetLineEntry().GetLine())
            if here in seen:
                thread.StepOver()
                continue
            if any(here[0] == caller for caller, _ in seen) and here[1] == _declared_line(frame):
                # A caller's return, at the line of its `fn`: no place to
                # stop in `flux debug`.
                thread.StepOver()
                continue
            if here == (cfa, line):
                # Back on the line it started from, out of the runtime lldb
                # steps over: a call to `resume`, or a perform, still to
                # come. Step on.
                if into:
                    thread.StepInto()
                else:
                    thread.StepOver()
                continue
            if armed and yielding:
                thread.StepOver()
                continue
            return
    finally:
        for breakpoint in breakpoints + anywhere:
            target.BreakpointDelete(breakpoint.GetID())
        debugger.SetAsync(was_async)
        # Where it stopped, for a debug adapter: it reports the stop, which
        # this synchronous run kept from it.
        if process.GetState() == lldb.eStateStopped:
            result.AppendMessage("thread %d" % process.GetSelectedThread().GetThreadID())


def _declared_line(frame):
    """The line of the frame's function's `fn`."""
    function = frame.GetFunction()
    if not function.IsValid():
        return None
    return function.GetStartAddress().GetLineEntry().GetLine()


def _fiber_finished(thread):
    """Whether the thread is in the scheduler with its fiber's Flux code
    returned: no Flux frame above the frame that ran the fiber."""
    entry = _fiber_entry_index(thread)
    if entry is None:
        return False
    return not any(_is_flux_frame(thread.GetFrameAtIndex(index)) for index in range(entry))


def _break_where_fibers_go_on(process):
    """Breakpoints where each other fiber goes on: a parked one at its
    continuation, one that has not started at its function."""
    target = process.GetTarget()
    memory = Memory(process)
    functions = []
    for fiber in fibers(process):
        if fiber["state"] == "not started":
            _, function = _function_at(target, fiber.get("code") or 0)
            functions.append(function)
        elif fiber["state"] in ("waiting", "ready"):
            conts = [
                cont
                for cont in continuations(memory, target, fiber.get("work") or 0)
                if not _library_file(cont.path)
            ]
            if conts:
                functions.extend(
                    _functions_named(
                        target, "%s (resumed after %d:" % (conts[0].name, conts[0].line)
                    )
                )
    return _break_at_functions(target, functions)


def _at_hook(frame):
    return (frame.GetFunctionName() or "").startswith(HOOKS)


def _command_next(debugger, args, result):
    _step(debugger, result, into=False)


def _command_step(debugger, args, result):
    _step(debugger, result, into=True)


def _command_finish(debugger, args, result):
    _step(debugger, result, into=False, out=True)


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
    "handlers": _command_handlers,
    "threads": _command_threads,
    "thread": _command_thread,
    "catch": _command_catch,
    "where": _command_where,
    "next": _command_next,
    "step": _command_step,
    "finish": _command_finish,
    "forget-steps": _command_forget_steps,
}
USAGE = (
    "usage: flux bt | flux locals | flux print <name>[.<part>...] | flux handlers"
    " | flux threads | flux thread <n>"
    " | flux catch [kind...] | flux where | flux next | flux step | flux finish"
    " | flux forget-steps"
)


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
