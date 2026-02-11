**updated Stasis grammar and parser notes** incorporating:

- **Assignment via infix `=`**
- **Pratt parser for all expressions** (assignment is right-associative)
- **Operator-method calls stay for arithmetic/comparison**
- **AoS -> SoA semantics represented cleanly**

The reference compiler targets C# with LLVMSharp bindings for IR generation and emission.

---

# **Stasis Grammar & Parsing Notes**

Declarations remain LL(1)-friendly and work with recursive-descent; expressions use a Pratt parser.

---

# **1. Program Structure**

```
CompilationUnit  -> TopLevelItemList
```

```
TopLevelItemList -> TopLevelItem TopLevelItemList
                  | <empty>
```

```
TopLevelItem     -> StructDecl
                  | ImportDecl
                  | EnumDecl
                  | GlobalDecl
                  | FunctionDecl
                  | TestDecl
```

---

# **2. Declarations**

## 2.0 Import Declarations

```
ImportDecl       -> "import" StringLiteral ";"
```

Parsing note:
- Imports introduce modules (file = module). The parser records import paths so later passes can build a per-file module table and resolve identifiers against:
  - local declarations first
  - then imported module members (in scope by default)
  - and `ModuleName.symbol` for disambiguation
  - receiver-scoped callable candidates are resolved semantically by `(name, parameter-0 type)` at each call site
  - imports are transitive for compilation (the build graph includes imported files and their imports)

## 2.1 Struct Declarations

```
StructDecl       -> "struct" Identifier "{" StructFieldList "}"
```

```
StructFieldList  -> StructField StructFieldList
                  | <empty>
```

```
StructField      -> Identifier ":" Type ";"
```

---

## 2.2 Enum Declarations

```
EnumDecl         -> "enum" Identifier "{" EnumMemberList "}"
```

```
EnumMemberList   -> EnumMember EnumMemberRest
```

```
EnumMember       -> Identifier EnumValueOpt
```

```
EnumValueOpt     -> "=" IntegerLiteral
                  | <empty>
```

```
EnumMemberRest   -> "," EnumMember EnumMemberRest
                  | ","     (* optional trailing comma *)
                  | <empty>
```

---

## 2.3 Global Declarations

```
GlobalDecl       -> "global" Identifier ":" Type ";"
```

---

## 2.3.1 Link Directives

```
LinkDirective    -> "@" Identifier "(" StringLiteral ")" ";"
```

---

## 2.4 Function Declarations

```
FunctionDecl     -> ExportOpt ExternOpt
                    "function" AttributeListOpt Identifier
                    "(" ParamListOpt ")"
                    ReturnTypeOpt
                    FunctionBody
```

```
AttributeListOpt -> Attribute AttributeListOpt
                  | <empty>
Attribute        -> "@" Identifier AttributeValueOpt
AttributeValueOpt -> "(" StringLiteral ")"
                   | <empty>
```

```
ExportOpt        -> "export"
                  | <empty>
```

```
ExternOpt        -> "extern"
                  | <empty>
```

```
FunctionBody     -> Block
                  | ";"
```

```
ParamListOpt     -> ParamList
                  | <empty>
```

```
ParamList        -> Param ParamListRest
```

```
ParamListRest    -> "," Param ParamListRest
                  | <empty>
```

```
Param            -> Identifier ":" Type
```

```
ReturnTypeOpt    -> ":" Type
                  | <empty>
```

Semantic notes for receiver-scoped callables:

- The parser grammar for functions is unchanged.
- A callable key is `(name, parameter-0 type)`.
- General overloading by non-receiver parameter lists remains unsupported.
- Distinct parameter-0 types may reuse the same function name.
- Both call forms are valid:
  - receiver form: `recv.name(args...)`
  - function form: `name(recv, args...)`
- Resolution is deterministic:
  1. Determine receiver type from `recv` (receiver form) or argument 0 (function form).
  2. Match by callable key.
  3. Validate arity and remaining argument types.
  4. Require exactly one match; never tie-break by import order.

---

## 2.5 Test Declarations

```
TestDecl         -> "test" TestName
                    "(" ParamListOpt ")"
                    ReturnTypeOpt
                    Block
```

```
TestName         -> Identifier
                  | BacktickString
```

---

# **3. Types**

```
Type             -> PrimitiveType
                  | ArrayType
                  | StringType
                  | Identifier
```

```
PrimitiveType    -> "u8" | "u16" | "u32"
                  | "i32"
                  | "f32" | "f64"
                  | "bool"
```

```
ArrayType        -> Type "[" IntegerLiteral "]"
```

```
StringType       -> "string" "[" IntegerLiteral "]"
```

---

# **4. Statements**

```
Block            -> "{" StatementList "}"
```

```
StatementList    -> Statement StatementList
                  | <empty>
```

```
Statement        -> Block
                  | VarDeclStatement
                  | IfStatement
                  | ForStatement
                  | ForeachStatement
                  | ReturnStatement
                  | ExpressionStatement
```

---

## 4.1 Variable Declaration

```
VarDeclStatement -> "let" Identifier ":" Type ";"
```

---

## 4.2 If Statement

```
IfStatement      -> "if" "(" Expression ")" Block ElseOpt
```

```
ElseOpt          -> "else" Block
                  | <empty>
```

---

## 4.3 For Statement

```
ForStatement     -> "for"
                    ExpressionOpt ";"
                    ExpressionOpt ";"
                    ExpressionOpt
                    Block

ExpressionOpt    -> Expression
                  | <empty>
```

---

## 4.4 Foreach Statement

```
ForeachStatement -> "foreach" "(" Identifier "in" Expression ")" Block
```

---

## 4.5 Return Statement

```
ReturnStatement  -> "return" ReturnValueOpt ";"
```

```
ReturnValueOpt   -> Expression
                  | <empty>
```

---

## 4.6 Expression Statement

```
ExpressionStatement
                  -> Expression ";"
```

---

# **5. Expressions (Pratt)**

Expressions use a Pratt parser with the following precedence (low -> high):

- Assignment `=` `+=` `-=` `*=` `/=` `%=` (right-associative)
- Logical or `||`
- Logical and `&&`
- Unary prefix `-`, `!`
- Postfix/member/call/operator-method

Reference grammar mirroring those precedences:

```
Expression       -> Assignment
Assignment       -> LogicalOr (AssignOp Assignment)?
AssignOp         -> "=" | "+=" | "-=" | "*=" | "/=" | "%="
LogicalOr        -> LogicalAnd ("||" LogicalAnd)*
LogicalAnd       -> UnaryExpr ("&&" UnaryExpr)*
UnaryExpr        -> "-" UnaryExpr
                  | "!" UnaryExpr
                  | PostfixExpr
PostfixExpr      -> PrimaryExpr PostfixOp*
PostfixOp        -> MemberAccess
                  | ArrayAccess
                  | FunctionCall
                  | OperatorMethodCall
```

### Postfix Operations

```
MemberAccess         -> "." Identifier
ArrayAccess          -> "[" Expression "]"
FunctionCall         -> "(" ArgumentListOpt ")"
OperatorMethodCall   -> "." OperatorToken "(" ArgumentListOpt ")"
```

Semantic note:

- `recv.name(args...)` is parsed as member access plus call, then bound as a receiver-scoped callable using receiver type + name.
- `name(recv, args...)` is bound through the same receiver-scoped callable key.
- After binding, both forms lower through the same function call path.

### Operator tokens (method-style)

```
OperatorToken    -> "+"
                  | "-"
                  | "*"
                  | "/"
                  | "%"
                  | "<"
                  | "<="
                  | ">"
                  | ">="
                  | "=="
                  | "!="
```

---

# **6. Primary Expressions**

```
PrimaryExpr      -> Literal
                  | Identifier
                  | "(" Expression ")"
                  | StructInitializer
```

```
StructInitializer -> "{" StructInitFieldsOpt "}"
StructInitFieldsOpt
                 -> StructInitFields
                  | <empty>
StructInitFields -> StructInitField StructInitFieldsRest
StructInitFieldsRest
                 -> "," StructInitField StructInitFieldsRest
                  | ","              # trailing comma
                  | <empty>
StructInitField  -> Identifier "=" Expression
```

---

# **7. Argument Lists**

```
ArgumentListOpt  -> ArgumentList
                  | <empty>
```

```
ArgumentList     -> Expression ArgumentListRest
```

```
ArgumentListRest -> "," Expression ArgumentListRest
                  | <empty>
```

---

# **8. Literals**

```
Literal          -> IntegerLiteral
                  | FloatLiteral
                  | StringLiteral
                  | BoolLiteral
```

```
BoolLiteral      -> "true" | "false"
```

(Numeric and string literal forms defined lexically.)

---

# **9. Lexical Rules**

```
Identifier       -> Letter IdentifierRest
IdentifierRest   -> LetterOrDigitOrUnderscore IdentifierRest
                  | <empty>
```

```
LetterOrDigitOrUnderscore
                  -> Letter | Digit | "_"
```

```
Letter           -> "A" | ... | "Z" | "a" | ... | "z"
Digit            -> "0" | ... | "9"
```

```
StringLiteral    -> '"' StringChar* '"'
BacktickString   -> '`' BacktickChar* '`'
```

---

# Notes on parsing

- Declarations and statement shapes stay LL(1)-friendly; expressions rely on the Pratt precedence table above.
- Assignment is written with infix `=`/compound forms; arithmetic/comparison can be infix with TypeScript-style precedence or expressed as operator-method calls (e.g., `hp.+(1)`, `hp.==(0)`).
- Pratt precedence levels: assignment < `||` < `&&` < prefix < postfix.
