### Docs
- Added proposal [0170](../docs/proposals/0170_polymorphic_effect_operations.md) (Polymorphic Effect Operations), surfaced by a spike of proposal 0165 (IO primop migration to effect handlers). 0170 generalizes effect-op signatures at collection time (`effect_op_signatures` stores `Scheme` instead of `TypeExpr`) and instantiates fresh at each `perform` site, closing the gap that blocked 0165's Console slice.
- Updated 0165's status to "Blocked" and appended a "Spike findings" section documenting the two attempted paths (`a -> ()` rigid skolem, `to_string` coercion) and why neither is viable without op polymorphism.
- Added 0165 and 0170 rows to the proposal index.
