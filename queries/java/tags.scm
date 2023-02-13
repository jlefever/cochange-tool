(class_declaration
  name: (identifier) @name) @tag.class

(record_declaration
  name: (identifier) @name) @tag.record

(enum_declaration
  name: (identifier) @name) @tag.enum

(interface_declaration
  name: (identifier) @name) @tag.interface

(annotation_type_declaration
  name: (identifier) @name) @tag.annotation

(method_declaration
  name: (identifier) @name
  parameters: (_) @disc) @tag.method

(constructor_declaration
  name: (identifier) @name
  parameters: (_) @disc) @tag.constructor

; According to the tree-sitter playground, this query works.
; But, I can't seem to get tree-sitter to return multiple captures of @disc
;(method_declaration
;  name: (identifier) @name
;  parameters: (formal_parameters
;    (formal_parameter type: (_) @disc))) @tag.method

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @tag.field