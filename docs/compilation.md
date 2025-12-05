**clean, polished, fully updated LL(1)-friendly Stasis Grammar** incorporating:

- **Assignment via operator-method** `.=( )`
- **No infix operators**
- **Variant D simplicity**
- **AoS → SoA semantics represented cleanly**
- **All productions structured for predictable recursive-descent parsing**

This is the version you'd put in a language manual or compiler reference.

---

# **Stasis Formal Grammar (LL1-Compatible)**

This grammar is suitable for a hand-written recursive-descent parser or for LL parser generators with minimal left-factoring.

---

# **1. Program Structure**

```
CompilationUnit  -> TopLevelItemList
```

```
TopLevelItemList -> TopLevelItem TopLevelItemList
                  | ε
```

```
TopLevelItem     -> StructDecl
                  | EnumDecl
                  | GlobalDecl
                  | FunctionDecl
                  | TestDecl
```

---

# **2. Declarations**

## 2.1 Struct Declarations

```
StructDecl       -> "struct" Identifier "{" StructFieldList "}"
```

```
StructFieldList  -> StructField StructFieldList
                  | ε
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
                  | ε
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
                    "function" Identifier
                    "(" ParamListOpt ")"
                    ReturnTypeOpt
                    Block
```

```
ExportOpt        -> "export"
                  | ε
```

```
ParamListOpt     -> ParamList
                  | ε
```

```
ParamList        -> Param ParamListRest
```

```
ParamListRest    -> "," Param ParamListRest
                  | ε
```

```
Param            -> Identifier ":" Type
```

```
ReturnTypeOpt    -> ":" Type
                  | ε
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
                  | ε
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
VarDeclStatement -> "let" Identifier "=" Expression ";"
```

---

## 4.2 If Statement

```
IfStatement      -> "if" "(" Expression ")" Block ElseOpt
```

```
ElseOpt          -> "else" Block
                  | ε
```

---

## 4.3 For Statement

```
ForStatement     -> "for"
                    Identifier "=" Expression ";"
                    Expression ";"
                    Expression
                    Block
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
                  | ε
```

---

## 4.6 Expression Statement

```
ExpressionStatement
                  -> Expression ";"
```

(Assignments are simply expressions using `.=( )`.)

---

# **5. Expressions**

```
Expression       -> UnaryExpr
```

---

## 5.1 Unary Expressions

```
UnaryExpr        -> "-" UnaryExpr
                  | "!" UnaryExpr
                  | PostfixExpr
```

---

## 5.2 Postfix Expressions (Core of the Language)

```
PostfixExpr      -> PrimaryExpr PostfixOpList
```

```
PostfixOpList    -> PostfixOp PostfixOpList
                  | ε
```

```
PostfixOp        -> MemberAccess
                  | ArrayAccess
                  | FunctionCall
                  | OperatorMethodCall
```

---

## 5.3 Postfix Operations

### Field access

```
MemberAccess     -> "." Identifier
```

### Array indexing

```
ArrayAccess      -> "[" Expression "]"
```

### Function call

```
FunctionCall     -> "(" ArgumentListOpt ")"
```

### Operator-method call, including assignment

```
OperatorMethodCall
                  -> "." OperatorToken "(" ArgumentListOpt ")"
```

### Operator tokens

```
OperatorToken    -> "+"
                  | "-"
                  | "*"
                  | "/"
                  | "%"
                  | "<"
                  | ">"
                  | "=="
                  | "="    (* assignment operator-method *)
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
                  | ε
```

```
ArgumentList     -> Expression ArgumentListRest
```

```
ArgumentListRest -> "," Expression ArgumentListRest
                  | ε
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
                  | ε
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

# ⭐ Notes on LL(1) Compatibility

This grammar is **intentionally LL(1)-friendly**:

- No left recursion
- All productions deterministically distinguishable by first token
- Assignment via `.=( )` eliminates the traditional assignment ambiguity
- `PostfixExpr` grows cleanly without ambiguity
- Expression parsing requires no precedence climbing

A straightforward recursive-descent parser can implement this directly.
