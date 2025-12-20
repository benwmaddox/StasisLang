**updated Stasis grammar and parser notes** incorporating:

- **Assignment via infix `=`** instead of `.=( )`
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
EnumMemberList   -> Identifier EnumMemberRest
```

```
EnumMemberRest   -> "," Identifier EnumMemberRest
                  | ","     (* optional trailing comma *)
                  | <empty>
```

---

## 2.3 Global Declarations

```
GlobalDecl       -> "global" Identifier ":" Type ";"
```

---

## 2.4 Function Declarations

```
FunctionDecl     -> ExportOpt
                    "function" AttributeListOpt Identifier
                    "(" ParamListOpt ")"
                    ReturnTypeOpt
                    Block
```

```
AttributeListOpt -> Attribute AttributeListOpt
                  | <empty>
Attribute        -> "@" Identifier
```

```
ExportOpt        -> "export"
                  | <empty>
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
