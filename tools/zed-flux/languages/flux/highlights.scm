; Starter highlighting for Flux using the built-in JavaScript grammar.
; This is intentionally lightweight until a dedicated tree-sitter-flux grammar exists.

(comment) @comment

(string) @string
(template_string) @string
(number) @number

[(true) (false)] @boolean
[(null) (undefined)] @constant

; Common Flux identifiers and variants often parse as identifiers under JS grammar.
(identifier) @keyword
  (#match? @keyword "^(fun|let|if|else|match|return|module|import|as)$")

(identifier) @type
  (#match? @type "^(Some|None|Left|Right)$")

(call_expression
  function: (identifier) @function)

(member_expression
  property: (property_identifier) @property)

["=" "==" "!=" "<=" ">=" "<" ">" "+" "-" "*" "/" "%" "&&" "||"] @operator

["{" "}" "(" ")" "[" "]"] @punctuation.bracket
["," "." ";" ":"] @punctuation.delimiter
