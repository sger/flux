# Error Code Reference

> Source: `src/diagnostics/compiler_errors.rs`, `src/diagnostics/runtime_errors.rs`, `src/diagnostics/registry.rs`

Flux uses stable error codes for all diagnostics. Codes are prefixed `E` (error) or `W` (warning).

## Code Ranges

| Range | Category | Source |
|-------|----------|--------|
| `E001–E060` | Compiler — semantic checks | `compiler_errors.rs` |
| `E061–E070` | Internal compiler errors (ICE) | `compiler_errors.rs` |
| `E071–E077` | Lexer / parser errors | `compiler_errors.rs` |
| `E440–E459`, `E476–E489` | Type classes and instances | `compiler_errors.rs` |
| `E472–E475` | Kind checking (Proposal 0179 Stage 1) | `compiler_errors.rs` |
| `E1000–E1021` | Runtime errors | `runtime_errors.rs` |
| `W2xx` | Warnings (linter) | `compiler_errors.rs` |

---

## Compiler Errors (E001–E077)

### Variable and Binding

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e001"></a>`E001` | `DUPLICATE_NAME` | Name already declared in this scope |
| <a id="e002"></a>`E002` | `IMMUTABLE_BINDING` | Attempt to reassign an immutable `let` binding |
| <a id="e003"></a>`E003` | `OUTER_ASSIGNMENT` | Assignment to a variable in an outer scope |
| <a id="e004"></a>`E004` | `UNDEFINED_VARIABLE` | Variable used before declaration |
| <a id="e007"></a>`E007` | `DUPLICATE_PARAMETER` | Parameter name used more than once in a function |

### Operators

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e005"></a>`E005` | `UNKNOWN_PREFIX_OPERATOR` | Unrecognized prefix operator |
| <a id="e006"></a>`E006` | `UNKNOWN_INFIX_OPERATOR` | Unrecognized infix operator |

### Module System

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e008"></a>`E008` | `INVALID_MODULE_NAME` | Module name does not match file path or naming rules |
| <a id="e009"></a>`E009` | `MODULE_NAME_CLASH` | Two modules share the same name |
| <a id="e010"></a>`E010` | `INVALID_MODULE_CONTENT` | Illegal declaration inside a module body |
| <a id="e011"></a>`E011` | `PRIVATE_MEMBER` | Accessing a non-public module member from outside the module |
| <a id="e012"></a>`E012` | `UNKNOWN_MODULE_MEMBER` | Member does not exist on the module |
| <a id="e013"></a>`E013` | `MODULE_NOT_IMPORTED` | Qualified access to a module that was not imported |
| <a id="e017"></a>`E017` | `IMPORT_SCOPE` | `import` used inside a function or block (top-level only) |
| <a id="e018"></a>`E018` | `IMPORT_NOT_FOUND` | Imported module file could not be found |
| <a id="e019"></a>`E019` | `IMPORT_READ_FAILED` | Imported module file could not be read |
| <a id="e021"></a>`E021` | `IMPORT_CYCLE` | Import cycle detected |
| <a id="e022"></a>`E022` | `SCRIPT_NOT_IMPORTABLE` | Importing a script file (no `module` declaration) |
| <a id="e023"></a>`E023` | `MULTIPLE_MODULES` | File contains more than one module declaration |
| <a id="e024"></a>`E024` | `MODULE_PATH_MISMATCH` | Module name in source does not match the file path |
| <a id="e025"></a>`E025` | `MODULE_SCOPE` | Module declaration is not at top level |
| <a id="e026"></a>`E026` | `INVALID_MODULE_ALIAS` | Alias name in `import ... as` is invalid |
| <a id="e027"></a>`E027` | `DUPLICATE_MODULE` | Same module found in multiple roots |
| <a id="e028"></a>`E028` | `INVALID_MODULE_FILE` | Module file is malformed |
| <a id="e029"></a>`E029` | `IMPORT_NAME_COLLISION` | Two imports resolve to the same name |
| <a id="e044"></a>`E044` | `CIRCULAR_DEPENDENCY` | Circular dependency between constants or definitions |

### Pattern Matching

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e014"></a>`E014` | `EMPTY_MATCH` | `match` expression has no arms |
| <a id="e015"></a>`E015` | `NON_EXHAUSTIVE_MATCH` | `match` does not cover all cases |
| <a id="e016"></a>`E016` | `CATCHALL_NOT_LAST` | Wildcard `_` arm is not the last arm |
| <a id="e020"></a>`E020` | `INVALID_PATTERN` | Pattern is not valid in this context |
| <a id="e035"></a>`E035` | `INVALID_PATTERN_LEGACY` | Legacy pattern syntax error |
| <a id="e075"></a>`E075` | `DUPLICATE_PATTERN_BINDING` | Same name bound twice in one pattern |

### Either / Option

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e041"></a>`E041` | `EITHER_CONSTRUCTOR_ERROR` | Misuse of `Left` / `Right` constructor |
| <a id="e042"></a>`E042` | `EITHER_VALUE_ERROR` | Invalid value inside `Left` / `Right` |
| <a id="e053"></a>`E053` | `EITHER_UNWRAP_ERROR_LEFT` | Unwrapping `Left` as `Right` at compile time |
| <a id="e054"></a>`E054` | `EITHER_UNWRAP_ERROR_RIGHT` | Unwrapping `Right` as `Left` at compile time |

### Type Errors

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e055"></a>`E055` | `TYPE_MISMATCH` | Types are incompatible |
| <a id="e056"></a>`E056` | `TYPE_ERROR` | Invalid type for this operation |
| <a id="e057"></a>`E057` | `INCOMPATIBLE_TYPES` | Two operand types cannot be combined |

### Constant Evaluation

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e045"></a>`E045` | `CONST_EVAL_ERROR` | Error during compile-time constant evaluation |
| <a id="e046"></a>`E046` | `CONST_NOT_FOUND` | Constant reference not found |
| <a id="e047"></a>`E047` | `CONST_NOT_PUBLIC` | Constant is private (`_` prefix) |
| <a id="e048"></a>`E048` | `CONST_INVALID_EXPR` | Expression cannot be evaluated at compile time |
| <a id="e049"></a>`E049` | `CONST_TYPE_ERROR` | Type error in constant expression |
| <a id="e050"></a>`E050` | `CONST_SCOPE_ERROR` | Constant used outside of valid scope |
| <a id="e051"></a>`E051` | `DIVISION_BY_ZERO_COMPILE` | Division by zero in constant expression |
| <a id="e052"></a>`E052` | `MODULO_BY_ZERO_COMPILE` | Modulo by zero in constant expression |
| <a id="e058"></a>`E058` | `CONST_RUNTIME_ERROR` | Runtime error during constant evaluation |
| <a id="e059"></a>`E059` | `CONST_DIVISION_BY_ZERO` | Division by zero in constant fold |
| <a id="e060"></a>`E060` | `CONST_OVERFLOW` | Integer overflow in constant expression |

### Pipe and Short-Circuit

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e039"></a>`E039` | `PIPE_OPERATOR_ERROR` | Invalid use of pipe operator `\|>` |
| <a id="e040"></a>`E040` | `PIPE_TARGET_ERROR` | Pipe target is not callable |
| <a id="e043"></a>`E043` | `SHORT_CIRCUIT_ERROR` | Invalid use of `&&` / `\|\|` |

### Type Classes and Instances

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e440"></a>`E440` | `DUPLICATE_CLASS` | Class already declared |
| <a id="e441"></a>`E441` | `INSTANCE_UNKNOWN_CLASS` | Instance names a class that does not exist |
| <a id="e442"></a>`E442` | `INSTANCE_MISSING_METHOD` | Instance omits a required class method |
| <a id="e443"></a>`E443` | `DUPLICATE_INSTANCE` | Instance already declared for this type |
| <a id="e444"></a>`E444` | `NO_INSTANCE` | No instance satisfies a class obligation |
| <a id="e445"></a>`E445` | `MISSING_SUPERCLASS_INSTANCE` | Required superclass instance is missing |
| <a id="e446"></a>`E446` | `INSTANCE_EXTRA_METHOD` | Instance defines a method the class does not declare |
| <a id="e447"></a>`E447` | `INSTANCE_TYPE_ARG_ARITY` | Instance head has the wrong number of type arguments |
| <a id="e448"></a>`E448` | `INSTANCE_METHOD_ARITY` | Instance method has the wrong number of parameters |
| <a id="e449"></a>`E449` | `ORPHAN_INSTANCE` | Instance declared outside the class's or type's module |
| <a id="e450"></a>`E450` | `PUBLIC_INSTANCE_OF_PRIVATE_CLASS` | Public instance implements a private class |
| <a id="e451"></a>`E451` | `PUBLIC_CLASS_LEAKS_PRIVATE_TYPE` | Public class signature exposes a private type |
| <a id="e452"></a>`E452` | `INSTANCE_METHOD_EFFECT_FLOOR` | Instance method demands more effects than the class allows |
| <a id="e453"></a>`E453` | `SEALED_CLASS_INSTANCE` | Instance declared for a sealed class |
| <a id="e454"></a>`E454` | `OVERLAPPING_INSTANCES` | Multiple instances match one predicate |
| <a id="e455"></a>`E455` | `PUBLIC_INSTANCE_HAS_PRIVATE_HEAD` | Public instance's head type is private |
| <a id="e456"></a>`E456` | `AMBIGUOUS_CLASS_CONSTRAINT` | Two visible classes share the constraint's short name |
| <a id="e459"></a>`E459` | `UNDETERMINED_CLASS_PARAMETER` | A class parameter the call does not fix leaves several instances compatible |
| <a id="e477"></a>`E477` | `SUPERCLASS_CYCLE` | A class reaches itself through its own superclasses |
| <a id="e478"></a>`E478` | `STALE_CLASS_INTERFACE` | A cached interface describes a public class that cannot be rebuilt |
| <a id="e479"></a>`E479` | `DUPLICATE_ASSOCIATED_TYPE` | An instance defines the same associated type twice |
| <a id="e480"></a>`E480` | `MISSING_ASSOCIATED_TYPE` | An instance omits an associated type its class declares |
| <a id="e481"></a>`E481` | `UNBOUND_ASSOCIATED_TYPE_VARIABLE` | An equation's body mentions a variable its head does not bind |
| <a id="e482"></a>`E482` | `ASSOCIATED_TYPE_KIND_MISMATCH` | An equation applies an associated type at the wrong arity |
| <a id="e483"></a>`E483` | `RECURSIVE_ASSOCIATED_TYPE` | An associated type reduces to a type mentioning itself |
| <a id="e484"></a>`E484` | `UNKNOWN_ASSOCIATED_TYPE` | An instance defines an associated type its class does not declare |
| <a id="e485"></a>`E485` | `AMBIGUOUS_DICTIONARY_SELECTION` | A call cannot say which of several dictionaries for one class it needs |
| <a id="e486"></a>`E486` | `UNDERIVABLE_CLASS` | A `deriving` clause names a class no method body can be generated for |
| <a id="e487"></a>`E487` | `OPERATOR_CLASS_NOT_IN_SCOPE` | An operator was used where the class it desugars to is not in scope |
| <a id="e488"></a>`E488` | `INSTANCE_SEARCH_EXHAUSTED` | Dictionary resolution exceeded the instance-context depth limit |
| <a id="e489"></a>`E489` | `COULD_NOT_DEDUCE` | A body needs a class predicate its own signature does not declare |
| <a id="e476"></a>`E476` | `AMBIGUOUS_TYPE_VARIABLE` | A declared bound constrains a variable the signature never mentions |

`E444`, `E454`, `E456`, `E459` and `E476` are easy to confuse. They differ in what is
known and what the remedy is:

- `E456` is a *name-resolution* failure — two classes share a short name, and
  the remedy is to qualify the class.
- `E444` and `E454` both concern a *fully known* predicate: nothing matches it
  (`E444`, add an instance), or several instances match it (`E454`, remove or
  narrow one).
- `E476` concerns a *signature*: a declared bound constrains a variable the
  signature's own type never mentions, so every call to it is affected.
- `E459` concerns an *incomplete* predicate — the call leaves a class parameter
  undetermined and more than one instance stays compatible. The remedy is to
  supply the missing type, usually with an annotation, rather than to change
  the instances. A single compatible instance is not an error: its head
  supplies the missing type.

### Kind Checking

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e472"></a>`E472` | `TYPE_CONSTRUCTOR_KIND_ARITY` | Type constructor applied to the wrong number of arguments |
| <a id="e473"></a>`E473` | `INSTANCE_HEAD_KIND_MISMATCH` | Instance head's kind does not match the class parameter |
| <a id="e474"></a>`E474` | `CONSTRAINT_KIND_MISMATCH` | Constraint argument has the wrong kind |
| <a id="e475"></a>`E475` | `CLASS_PARAMETER_KIND_CONFLICT` | Class parameter used at two different kinds |

### Internal Compiler Errors (ICE)

These indicate a bug in the compiler, not user code:

| Code | Constant |
|------|----------|
| <a id="e061"></a>`E061` | `ICE_SYMBOL_SCOPE_LET` |
| <a id="e062"></a>`E062` | `ICE_SYMBOL_SCOPE_ASSIGN` |
| <a id="e063"></a>`E063` | `ICE_TEMP_SYMBOL_MATCH` |
| <a id="e064"></a>`E064` | `ICE_TEMP_SYMBOL_SOME_PATTERN` |
| <a id="e065"></a>`E065` | `ICE_SYMBOL_SCOPE_PATTERN` |
| <a id="e066"></a>`E066` | `ICE_TEMP_SYMBOL_SOME_BINDING` |
| <a id="e067"></a>`E067` | `ICE_TEMP_SYMBOL_LEFT_PATTERN` |
| <a id="e068"></a>`E068` | `ICE_TEMP_SYMBOL_RIGHT_PATTERN` |
| <a id="e069"></a>`E069` | `ICE_TEMP_SYMBOL_LEFT_BINDING` |
| <a id="e070"></a>`E070` | `ICE_TEMP_SYMBOL_RIGHT_BINDING` |

### Lexer / Parser

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e030"></a>`E030` | `UNKNOWN_KEYWORD` | Unrecognized keyword (e.g. `fun` instead of `fn`) |
| <a id="e031"></a>`E031` | `EXPECTED_EXPRESSION` | Expected an expression, found something else |
| <a id="e032"></a>`E032` | `INVALID_INTEGER` | Integer literal out of range or malformed |
| <a id="e033"></a>`E033` | `INVALID_FLOAT` | Float literal malformed |
| <a id="e034"></a>`E034` | `UNEXPECTED_TOKEN` | Token not valid in this position |
| <a id="e036"></a>`E036` | `LAMBDA_SYNTAX_ERROR` | Lambda `\` syntax error |
| <a id="e037"></a>`E037` | `LAMBDA_PARAMETER_ERROR` | Invalid parameter in lambda |
| <a id="e038"></a>`E038` | `LAMBDA_BODY_ERROR` | Invalid lambda body |
| <a id="e071"></a>`E071` | `UNTERMINATED_STRING` | String literal not closed |
| <a id="e072"></a>`E072` | `UNTERMINATED_INTERPOLATION` | `#{` not closed |
| <a id="e073"></a>`E073` | `MISSING_COMMA` | Missing comma in expression list |
| <a id="e074"></a>`E074` | `UNTERMINATED_BLOCK_COMMENT` | `/*` not closed |
| <a id="e076"></a>`E076` | `UNCLOSED_DELIMITER` | `(`, `[`, or `{` not closed |
| <a id="e077"></a>`E077` | `LEGACY_LIST_TAIL_NONE` | Old-style list tail syntax |

---

## Runtime Errors (E1000–E1021)

| Code | Constant | Description |
|------|----------|-------------|
| <a id="e1000"></a>`E1000` | `WRONG_NUMBER_OF_ARGUMENTS` | Function called with wrong arity |
| <a id="e1001"></a>`E1001` | `NOT_A_FUNCTION` | Calling a non-function value |
| <a id="e1002"></a>`E1002` | `FUNCTION_NOT_FOUND` | Named function could not be resolved |
| <a id="e1003"></a>`E1003` | `BASE_FUNCTION_ERROR` | A Base function returned an error |
| <a id="e1004"></a>`E1004` | `RUNTIME_TYPE_ERROR` | Wrong type for a runtime operation |
| <a id="e1005"></a>`E1005` | `NOT_INDEXABLE` | Indexing a value that doesn't support `[]` |
| <a id="e1006"></a>`E1006` | `KEY_NOT_HASHABLE` | Hash map key is not a hashable type |
| <a id="e1007"></a>`E1007` | `NOT_ITERABLE` | Iterating over a non-iterable value |
| <a id="e1008"></a>`E1008` | `DIVISION_BY_ZERO_RUNTIME` | Division by zero at runtime |
| <a id="e1009"></a>`E1009` | `INVALID_OPERATION` | Operation not supported for this value |
| <a id="e1010"></a>`E1010` | `INTEGER_OVERFLOW` | Integer arithmetic overflow |
| <a id="e1011"></a>`E1011` | `MODULO_BY_ZERO_RUNTIME` | Modulo by zero at runtime |
| <a id="e1012"></a>`E1012` | `INDEX_OUT_OF_BOUNDS` | Array or tuple index out of bounds |
| <a id="e1013"></a>`E1013` | `KEY_NOT_FOUND` | Key missing from hash map |
| <a id="e1014"></a>`E1014` | `NEGATIVE_INDEX` | Negative index used |
| <a id="e1015"></a>`E1015` | `INVALID_SLICE` | `slice(arr, lo, hi)` bounds are invalid |
| <a id="e1016"></a>`E1016` | `MATCH_ERROR` | No match arm matched the value |
| <a id="e1017"></a>`E1017` | `OPTION_UNWRAP_ERROR` | Unwrapping `None` as `Some` |
| <a id="e1018"></a>`E1018` | `EITHER_UNWRAP_ERROR` | Unwrapping the wrong `Left`/`Right` variant |
| <a id="e1019"></a>`E1019` | `STRING_INDEX_ERROR` | String character index out of range |
| <a id="e1020"></a>`E1020` | `STRING_ENCODING_ERROR` | Invalid UTF-8 in string operation |
| <a id="e1021"></a>`E1021` | `INVALID_SUBSTRING` | `substring` bounds are invalid |

---

## Adding a New Error Code

1. Define the constant in `compiler_errors.rs` or `runtime_errors.rs`:
   ```rust
   pub const MY_ERROR: ErrorCode = ErrorCode {
       code: "E078",
       error_type: ErrorType::Compiler,
   };
   ```

2. Register it in `registry.rs` with a description and hint template.

3. Use `diagnostic_for(&MY_ERROR)` to create the diagnostic, then chain `with_*` builder methods:
   ```rust
   use crate::diagnostics::{Diagnostic, DiagnosticBuilder};
   use crate::diagnostics::compiler_errors::MY_ERROR;

   diagnostic_for(&MY_ERROR)
       .with_span(span)
       .with_message("what went wrong")
       .with_hint("how to fix it")
   ```
