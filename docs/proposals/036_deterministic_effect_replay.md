# Proposal: Deterministic Effect Replay

**Status:** Draft  
**Date:** 2026-02-20

---

## 1. Motivation

Flux now has explicit effectful operation boundaries (`PrimOp` effects + builtin call boundaries), but reproducing failures is still difficult when programs depend on:

- file IO (`read_file`, `read_lines`, `read_stdin`)
- time (`now_ms`, `clock_now`)
- output ordering (`print`, `println`)

Deterministic Effect Replay provides a record/replay mode so the same program execution can be reproduced exactly across runs and across backends (VM and JIT policy path).

---

## 2. User-Facing Behavior

Add two CLI modes:

- `flux run --record <trace_file> <program.flx>`
- `flux run --replay <trace_file> <program.flx>`

Semantics:

- `--record`: execute normally and append effect events to trace.
- `--replay`: block real-world effects and satisfy them from trace.
- mismatch in replay produces deterministic failure with source location + event index.

Non-goals (v1):

- cryptographic integrity/signing of traces
- cross-version trace compatibility guarantees
- replay of nondeterminism from random/network APIs not yet modeled in Flux

---

## 3. Effect Event Model

Define a compact event log with strict order:

1. `ReadFile { path, result }`
2. `ReadLines { path, result }`
3. `ReadStdin { result }`
4. `NowMs { result }`
5. `Print { rendered }`
6. `Panic { message }`

Notes:

- `Print`/`Panic` are recorded to validate ordering and exact output stream behavior.
- Pure primops and pure builtins are never logged.

Suggested on-disk format (JSON Lines v1):

```json
{"v":1,"idx":0,"event":"ReadFile","path":"input.txt","result":"..."}
{"v":1,"idx":1,"event":"NowMs","result":1700000123456}
{"v":1,"idx":2,"event":"Print","rendered":"42"}
```

---

## 4. Runtime Design

Introduce runtime mode:

- `ReplayMode::Off`
- `ReplayMode::Record { sink }`
- `ReplayMode::Replay { source, cursor }`

Add an `EffectRuntime` trait used by effectful primops/builtins:

- `on_read_file(path) -> Result<String, Error>`
- `on_read_lines(path) -> Result<Vec<String>, Error>`
- `on_read_stdin() -> Result<String, Error>`
- `on_now_ms() -> Result<i64, Error>`
- `on_print(rendered) -> Result<(), Error>`
- `on_panic(message) -> Result<(), Error>`

Behavior:

- `Off`: call current runtime behavior directly.
- `Record`: call real behavior + persist event/result.
- `Replay`: validate next event kind/payload; return recorded result and never touch OS.

---

## 5. VM/JIT Parity Strategy

Policy parity requirement:

- both backends must route effectful operations through the same `EffectRuntime` interface.

VM:

- hook `OpPrimOp` effectful cases (`Println`, `ReadFile`, `ClockNow`, `Panic`)
- hook effectful builtins (`print`, `read_*`, `now_ms`, `time` if kept effectful)

JIT:

- keep existing runtime helper path
- pass replay context through helper ABI
- helper performs record/replay checks identically to VM

Result:

- same trace should replay identically under VM and JIT for supported effect set.

---

## 6. Compiler and Bytecode Impact

No language syntax changes.

No bytecode format changes required for v1.

Optional debug metadata improvement:

- include instruction offset + source span in replay mismatch diagnostics.

---

## 7. Failure Modes and Diagnostics

Replay must fail fast for:

- event kind mismatch
- payload mismatch (e.g., different file path argument)
- trace exhausted before program finishes
- leftover trace events after program exits (strict mode)

Diagnostic template:

```
replay mismatch at event #17
expected: ReadFile(path="input.txt")
actual:   NowMs()
at: examples/aoc/2024/day04.flx:12:9
```

---

## 8. Security and Privacy

Trace files may contain sensitive data (`read_file` payloads, stdin).

v1 requirements:

- explicit opt-in (`--record`)
- clear warning in docs
- no automatic upload/export

Future:

- redaction policy and selective field hashing

---

## 9. Test Plan

1. Unit tests:
- event encoder/decoder roundtrip
- mismatch detection cases
- cursor/EOF behavior

2. Integration tests:
- record + replay on deterministic fixture program
- replay mismatch reports event index and source span

3. Cross-backend tests:
- same trace replays under VM and JIT
- output parity for supported effect events

4. CI:
- add replay regression test suite

---

## 10. Rollout Plan

Phase A:
- implement `--record/--replay` for VM only
- validate correctness and diagnostics

Phase B:
- connect JIT helper path to shared `EffectRuntime`
- enforce VM/JIT parity tests

Phase C:
- add strict replay mode in CI for flaky scenario capture
- publish user docs and examples under `examples/effects/`

---

## 11. Real Benefits

1. Debugging:
- deterministic reproduction for “works on my machine” bugs.

2. CI stability:
- capture flaky runs once, replay forever.

3. Backend confidence:
- direct parity signal between VM and JIT under identical effect streams.

4. Performance analysis:
- benchmark pure execution differences without IO/time noise.

5. Foundation for type/effect roadmap:
- turns effect boundaries into enforceable runtime contracts now, before full algebraic handlers.
