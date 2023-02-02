(class_declaration
  name: (identifier) @name) @tag.class

(record_declaration
  name: (identifier) @name) @tag.record

(enum_declaration
  name: (identifier) @name) @tag.enum

(interface_declaration
  name: (identifier) @name) @tag.interface

(method_declaration
  name: (identifier) @name) @tag.method

(constructor_declaration
  name: (identifier) @name) @tag.constructor

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @tag.field

; TODO: What about annotations?