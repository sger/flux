# Backend Representation Contracts

Flux maintains backend parity by keeping semantic lowering shared through Core
and Aether, then requiring both maintained backends to classify runtime values
into the same representation families before any layout-specific dispatch.

## Runtime Families

Distinct runtime families at the backend boundary:

- `None`
- `[]` / empty list sentinel
- cons/list cells
- tuples
- built-in ADTs (`Some`, `Left`, `Right`, `Cons`)
- user ADTs
- arrays
- strings
- floats
- closures
- HAMTs / maps

## Contract

- Constructor-pattern dispatch must only decode constructor tags after proving
  the boxed value belongs to the correct object family.
- List patterns may only match:
  - the empty-list sentinel
  - a cons/list cell
- Tuple patterns may only match tuple objects.
- ADT constructor patterns may only match ADT objects.
- Arrays, closures, strings, floats, and HAMTs must never be decoded through
  ADT or tuple layouts by coincidence.

## Backend Proof Obligations

VM:

- Shape opcodes such as `OpIsCons`, `OpIsEmptyList`, tuple tests, and ADT tests
  must only succeed for the corresponding `Value` variants.
- The VM is the concrete runtime baseline for collection-family separation.

Native:

- `MatchCtor` and constructor-tag dispatch must guard on heap `obj_tag` before
  reading layout-specific fields.
- Native must not reinterpret arbitrary boxed pointers as `FluxAdt` or
  `FluxTuple` payloads without an object-family proof.

## Debugging Contract

- `core_mismatch` means frontend/Core lowering diverged before backend lowering.
- `aether_mismatch` means ownership lowering diverged before backend lowering.
- Matching Core and Aether with divergent runtime classification means a backend
  representation bug.

## Practical Rule

Do not fix parity by broadening one backend to accept ambiguous programs.
If a fixture relies on mixing `Flow.List` behavior with arrays, rewrite the
fixture to use the intended library family instead of normalizing around the
backend accident.
