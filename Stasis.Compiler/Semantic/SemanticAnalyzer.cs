using System.Linq;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

public sealed class SemanticAnalyzer
{
    private static readonly Dictionary<string, TypeSymbol> BuiltInTypes = new(StringComparer.Ordinal)
    {
        { "u8", new PrimitiveTypeSymbol("u8") },
        { "u16", new PrimitiveTypeSymbol("u16") },
        { "u32", new PrimitiveTypeSymbol("u32") },
        { "i32", new PrimitiveTypeSymbol("i32") },
        { "f32", new PrimitiveTypeSymbol("f32") },
        { "f64", new PrimitiveTypeSymbol("f64") },
        { "bool", new PrimitiveTypeSymbol("bool") },
        { "string", new PrimitiveTypeSymbol("string") },
        { "utf8", new PrimitiveTypeSymbol("utf8") },
        { "ascii", new PrimitiveTypeSymbol("ascii") },
        { "void", new VoidTypeSymbol() }
    };

    private readonly Dictionary<string, Symbol> _symbols = new(StringComparer.Ordinal);
    private readonly List<Diagnostic> _diagnostics = new();
    private readonly Dictionary<string, StructDeclarationSyntax> _structs = new(StringComparer.Ordinal);

    public SemanticResult Analyze(CompilationUnitSyntax compilationUnit)
    {
        DeclareBuiltIns();
        DeclareTypes(compilationUnit);
        DeclareGlobals(compilationUnit);
        DeclareConstants(compilationUnit);
        DeclareFunctions(compilationUnit);

        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case FunctionDeclarationSyntax fn:
                    AnalyzeFunction(fn);
                    break;
                case TestDeclarationSyntax test:
                    AnalyzeTest(test);
                    break;
            }
        }

        return new SemanticResult(_diagnostics, new Dictionary<string, Symbol>(_symbols));
    }

    private void DeclareBuiltIns()
    {
        foreach (var (name, type) in BuiltInTypes)
        {
            _symbols[name] = new Symbol(name, SymbolKind.Struct, type);
        }

        // Legacy I/O functions (to be deprecated)
        AddSymbol("print_string", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_int", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_char", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_cell", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_prompt", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_invalid", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_clue_error", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("print_solved", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("read_char", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("read_int", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));

        // Legacy math functions (to be renamed to math_*)
        AddSymbol("sin", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("cos", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("sin_fast", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("cos_fast", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));

        // Legacy system functions (to be renamed to sys_*)
        AddSymbol("time", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("get_time_ms", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sleep_ms", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));

        // Legacy graphics functions (external runtime)
        AddSymbol("init_window", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("begin_frame", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("end_frame", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("clear", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("draw_line", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("gfx_load_sprite", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("gfx_draw_sprite", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("gfx_poll_reload", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("gfx_debug_bake_hash", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("is_key_down", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("should_quit", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("get_window_size", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("set_fullscreen", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("set_postfx", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("load_font", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("draw_text", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("measure_text", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("list_directory", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("dir_list_entry_is_dir", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("dir_list_entry_copy_name", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));

        // ============================================================
        // Standard Library: char_* module (character/byte utilities)
        // ============================================================

        // Classification
        AddSymbol("char_is_digit", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_alpha", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_alnum", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_space", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_upper", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_lower", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_hex", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("char_is_print", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));

        // Conversion
        AddSymbol("char_to_upper", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));
        AddSymbol("char_to_lower", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));
        AddSymbol("char_to_digit", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("char_from_digit", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));
        AddSymbol("char_to_hex", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("char_from_hex", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));

        // ============================================================
        // Standard Library: str_* module (string operations)
        // ============================================================

        // Length & Capacity
        AddSymbol("str_len", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_is_empty", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));

        // Character Access
        AddSymbol("str_get", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));
        AddSymbol("str_set", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));

        // Comparison
        AddSymbol("str_eq", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("str_cmp", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_starts_with", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("str_ends_with", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));

        // Search
        AddSymbol("str_find", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_find_char", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_find_last_char", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_contains", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));

        // Modification (in-place)
        AddSymbol("str_clear", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("str_copy", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_append", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_append_char", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));

        // Substring
        AddSymbol("str_substr", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));

        // Trimming
        AddSymbol("str_trim_start", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_trim_end", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_trim", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));

        // Case conversion
        AddSymbol("str_to_upper", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("str_to_lower", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));

        // Number conversion
        AddSymbol("str_from_i32", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_from_f32", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_to_i32", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("str_to_f32", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
    }

    private void DeclareTypes(CompilationUnitSyntax compilationUnit)
    {
        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case StructDeclarationSyntax s:
                    AddSymbol(s.Name.Text, SymbolKind.Struct, new NamedTypeSymbol(s.Name.Text), s.Name.Span);
                    _structs[s.Name.Text] = s;
                    ValidateStructFields(s);
                    break;
                case EnumDeclarationSyntax e:
                    var enumType = new NamedTypeSymbol(e.Name.Text);
                    AddSymbol(e.Name.Text, SymbolKind.Enum, enumType, e.Name.Span);
                    // Add each enum member as a constant with the enum type (not i32)
                    for (int i = 0; i < e.Members.Count; i++)
                    {
                        var member = e.Members[i];
                        var memberName = $"{e.Name.Text}.{member.Identifier.Text}";
                        AddSymbol(memberName, SymbolKind.Const, enumType, member.Identifier.Span);
                    }
                    break;
            }
        }
    }

    private void DeclareGlobals(CompilationUnitSyntax compilationUnit)
    {
        var globals = compilationUnit.Declarations.OfType<GlobalDeclarationSyntax>().ToList();

        foreach (var decl in globals)
        {
            var type = ResolveType(decl.Type);
            AddSymbol(decl.Name.Text, SymbolKind.Global, type, decl.Name.Span);
            EnsureGlobalType(type, decl.Type.Span);
            RegisterFlattenedStructGlobals(decl.Name.Text, decl.Type);
        }
    }

    private void DeclareConstants(CompilationUnitSyntax compilationUnit)
    {
        foreach (var decl in compilationUnit.Declarations.OfType<ConstDeclarationSyntax>())
        {
            var type = ResolveType(decl.Type);
            AddSymbol(decl.Name.Text, SymbolKind.Const, type, decl.Name.Span);

            if (decl.Initializer is not LiteralExpressionSyntax)
            {
                _diagnostics.Add(new Diagnostic("Const initializers must be literal values for now.", decl.Initializer.Span));
            }
        }
    }

    private void DeclareFunctions(CompilationUnitSyntax compilationUnit)
    {
        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case FunctionDeclarationSyntax fn:
                    {
                        var returnType = fn.ReturnType is null ? null : ResolveType(fn.ReturnType);
                        AddSymbol(fn.Name.Text, SymbolKind.Function, returnType, fn.Name.Span);
                        break;
                    }
                case TestDeclarationSyntax test:
                    {
                        var returnType = test.ReturnType is null ? null : ResolveType(test.ReturnType);
                        AddSymbol(test.Name.Text, SymbolKind.Test, returnType, test.Name.Span);
                        break;
                    }
            }
        }
    }

    private void AnalyzeFunction(FunctionDeclarationSyntax fn)
    {
        var scope = new Dictionary<string, Symbol>(StringComparer.Ordinal);
        foreach (var param in fn.Parameters)
        {
            var type = ResolveType(param.Type);
            AddLocal(scope, param.Name.Text, SymbolKind.Parameter, type, param.Name.Span);
            EnsurePrimitiveLocal(type, param.Name.Span);
        }

        AnalyzeBlock(fn.Body, scope);
    }

    private void AnalyzeTest(TestDeclarationSyntax test)
    {
        var scope = new Dictionary<string, Symbol>(StringComparer.Ordinal);
        foreach (var param in test.Parameters)
        {
            var type = ResolveType(param.Type);
            AddLocal(scope, param.Name.Text, SymbolKind.Parameter, type, param.Name.Span);
            EnsurePrimitiveLocal(type, param.Name.Span);
        }

        AnalyzeBlock(test.Body, scope);
    }

    private void AnalyzeBlock(BlockStatementSyntax block, Dictionary<string, Symbol> scope)
    {
        foreach (var stmt in block.Statements)
        {
            AnalyzeStatement(stmt, scope);
        }
    }

    private TypeSymbol? ResolveIterableElementType(ExpressionSyntax iterable, IReadOnlyDictionary<string, Symbol> scope)
    {
        if (iterable is IdentifierExpressionSyntax id)
        {
            if (scope.TryGetValue(id.Identifier.Text, out var localSym) && localSym.Type is ArrayTypeSymbol localArray)
            {
                return localArray.ElementType;
            }

            if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Type is ArrayTypeSymbol array)
            {
                return array.ElementType;
            }
        }

        if (iterable is MemberAccessExpressionSyntax member &&
            member.Receiver is IdentifierExpressionSyntax recv &&
            _symbols.TryGetValue(recv.Identifier.Text, out var recvSym) &&
            recvSym.Type is NamedTypeSymbol named &&
            _structs.TryGetValue(named.TypeName, out var structDecl))
        {
            var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
            if (field?.Type is ArrayTypeSyntax arraySyntax)
            {
                return ResolveType(arraySyntax.ElementType);
            }
        }

        return null;
    }

    private void AnalyzeStatement(StatementSyntax stmt, Dictionary<string, Symbol> scope)
    {
        switch (stmt)
        {
            case BlockStatementSyntax block:
                AnalyzeBlock(block, new Dictionary<string, Symbol>(scope, StringComparer.Ordinal));
                break;
            case VariableDeclarationSyntax v:
                AnalyzeVariableDeclaration(v, scope);
                break;
            case IfStatementSyntax ifs:
                AnalyzeExpression(ifs.Condition, scope);
                if (ifs.ThenBlock is not null)
                {
                    AnalyzeBlock(ifs.ThenBlock, new Dictionary<string, Symbol>(scope, StringComparer.Ordinal));
                }

                if (ifs.ElseBlock is not null)
                {
                    AnalyzeBlock(ifs.ElseBlock, new Dictionary<string, Symbol>(scope, StringComparer.Ordinal));
                }

                break;
            case ForStatementSyntax fs:
                if (fs.Initializer is not null)
                {
                    AnalyzeExpression(fs.Initializer, scope);
                }
                if (fs.Condition is not null)
                {
                    AnalyzeExpression(fs.Condition, scope);
                }
                if (fs.Step is not null)
                {
                    AnalyzeExpression(fs.Step, scope);
                }

                AnalyzeBlock(fs.Body, new Dictionary<string, Symbol>(scope, StringComparer.Ordinal));
                break;
            case ForeachStatementSyntax fes:
                AnalyzeExpression(fes.Iterable, scope);
                var foreachScope = new Dictionary<string, Symbol>(scope, StringComparer.Ordinal);
                if (fes.BindByElement)
                {
                    var elementType = ResolveIterableElementType(fes.Iterable, scope);
                    if (elementType is null)
                    {
                        _diagnostics.Add(new Diagnostic("foreach target must be an array.", fes.Iterable.Span));
                        break;
                    }

                    AddLocal(foreachScope, fes.Iterator.Text, SymbolKind.Local, elementType, fes.Iterator.Span);
                    EnsurePrimitiveLocal(elementType, fes.Iterator.Span);
                }
                else
                {
                    var iteratorType = BuiltInTypes["i32"];
                    AddLocal(foreachScope, fes.Iterator.Text, SymbolKind.Local, iteratorType, fes.Iterator.Span);
                    EnsurePrimitiveLocal(iteratorType, fes.Iterator.Span);
                }

                // If an index variable is provided, add it to scope as i32
                if (fes.IndexVariable is not null)
                {
                    var indexType = BuiltInTypes["i32"];
                    AddLocal(foreachScope, fes.IndexVariable.Text, SymbolKind.Local, indexType, fes.IndexVariable.Span);
                }

                AnalyzeBlock(fes.Body, foreachScope);
                break;
            case ReturnStatementSyntax rs:
                if (rs.Expression is not null)
                {
                    AnalyzeExpression(rs.Expression, scope);
                }
                break;
            case ExpressionStatementSyntax es:
                AnalyzeExpression(es.Expression, scope);
                break;
        }
    }

    private void AnalyzeVariableDeclaration(VariableDeclarationSyntax v, Dictionary<string, Symbol> scope)
    {
        TypeSymbol? varType = null;

        if (v.Type is null)
        {
            _diagnostics.Add(new Diagnostic("Local variables must declare a type; use 'let name: type = value;' to initialize.", v.Name.Span));
        }
        else
        {
            varType = ResolveType(v.Type);
            AddLocal(scope, v.Name.Text, SymbolKind.Local, varType, v.Name.Span);
            EnsurePrimitiveLocal(varType, v.Name.Span);
        }

        if (v.Initializer is null)
        {
            _diagnostics.Add(new Diagnostic("Local variables must be initialized with a value.", v.Name.Span));
        }
        else
        {
            AnalyzeExpression(v.Initializer, scope);
            // Type check: ensure initializer type matches variable type
            var initType = ResolveExpressionType(v.Initializer, scope);
            if (varType is not null && initType is not null && !AreTypesCompatible(varType, initType))
            {
                _diagnostics.Add(new Diagnostic($"Cannot assign value of type '{FormatType(initType)}' to variable of type '{FormatType(varType)}'.", v.Initializer.Span));
            }
        }
    }

    private void AnalyzeExpression(ExpressionSyntax expr, Dictionary<string, Symbol> scope)
    {
        switch (expr)
        {
            case IdentifierExpressionSyntax id:
                ResolveIdentifier(id.Identifier.Text, id.Span, scope);
                break;
            case LiteralExpressionSyntax:
                break;
            case ParenthesizedExpressionSyntax p:
                AnalyzeExpression(p.Expression, scope);
                break;
            case UnaryExpressionSyntax u:
                AnalyzeExpression(u.Operand, scope);
                break;
            case MemberAccessExpressionSyntax m:
                // Check if this is an enum member access (e.g., State.Idle)
                if (m.Receiver is IdentifierExpressionSyntax idExpr)
                {
                    var enumName = idExpr.Identifier.Text;
                    if (_symbols.TryGetValue(enumName, out var enumSymbol) && enumSymbol.Kind == SymbolKind.Enum)
                    {
                        // This is an enum member access - verify the member exists
                        var memberName = $"{enumName}.{m.Member.Text}";
                        if (!_symbols.ContainsKey(memberName))
                        {
                            _diagnostics.Add(new Diagnostic($"Enum '{enumName}' does not have a member named '{m.Member.Text}'.", m.Member.Span));
                        }
                        // Don't recursively analyze the receiver since it's just the enum type name
                        return;
                    }
                }
                // Not an enum member access - regular struct field access
                AnalyzeExpression(m.Receiver, scope);
                break;
            case ArrayAccessExpressionSyntax a:
                AnalyzeExpression(a.Receiver, scope);
                AnalyzeExpression(a.Index, scope);
                break;
            case CallExpressionSyntax c:
                AnalyzeExpression(c.Callee, scope);
                foreach (var arg in c.Arguments)
                {
                    AnalyzeExpression(arg, scope);
                }
                break;
            case OperatorCallExpressionSyntax op:
                AnalyzeExpression(op.Receiver, scope);
                foreach (var arg in op.Arguments)
                {
                    AnalyzeExpression(arg, scope);
                }

                ValidateOperatorCall(op);
                break;
            case AssignmentExpressionSyntax assign:
                AnalyzeExpression(assign.Left, scope);
                AnalyzeExpression(assign.Right, scope);
                ValidateAssignment(assign.Left, assign.OperatorToken);
                ValidateSingleAssignment(assign);
                // Type check: ensure right side type matches left side type
                var leftType = ResolveExpressionType(assign.Left, scope);
                var rightType = ResolveExpressionType(assign.Right, scope);
                if (leftType is not null && rightType is not null && !AreTypesCompatible(leftType, rightType))
                {
                    _diagnostics.Add(new Diagnostic($"Cannot assign value of type '{FormatType(rightType)}' to target of type '{FormatType(leftType)}'.", assign.Right.Span));
                }
                break;
            case BinaryExpressionSyntax bin:
                AnalyzeExpression(bin.Left, scope);
                AnalyzeExpression(bin.Right, scope);
                ValidateBinary(bin, scope);
                break;
        }
    }

    private void ValidateOperatorCall(OperatorCallExpressionSyntax op)
    {
        var opText = op.OperatorToken.Text;
        if (op.Arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic($"Operator '.{opText}()' requires exactly one argument.", op.Span));
        }

        if (opText == "=")
        {
            _diagnostics.Add(new Diagnostic("Use infix '=' for assignment.", op.Span));
            if (!IsAssignableReceiver(op.Receiver))
            {
                _diagnostics.Add(new Diagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", op.Receiver.Span));
            }
        }
    }

    private bool IsAssignableReceiver(ExpressionSyntax receiver) =>
        receiver is IdentifierExpressionSyntax
        or MemberAccessExpressionSyntax
        or ArrayAccessExpressionSyntax;

    private void ValidateAssignment(ExpressionSyntax target, Token opToken)
    {
        if (!IsAssignableReceiver(target))
        {
            _diagnostics.Add(new Diagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", target.Span));
            return;
        }

        // Check if trying to assign to a constant
        if (target is IdentifierExpressionSyntax id && _symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Const)
        {
            _diagnostics.Add(new Diagnostic($"Cannot assign to constant '{id.Identifier.Text}'. Constants are immutable.", target.Span));
            return;
        }

        if (opToken.Kind is TokenKind.Equal)
        {
            return;
        }

        if (opToken.Kind is TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual or TokenKind.PercentEqual)
        {
            return;
        }

        _diagnostics.Add(new Diagnostic($"Unsupported assignment operator '{opToken.Text}'.", opToken.Span));
    }

    private void ValidateSingleAssignment(AssignmentExpressionSyntax assign)
    {
        if (assign.Left is AssignmentExpressionSyntax or BinaryExpressionSyntax { OperatorToken.Kind: TokenKind.Equal or TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual or TokenKind.PercentEqual })
        {
            _diagnostics.Add(new Diagnostic("Only one assignment is permitted per expression.", assign.Left.Span));
        }

        if (assign.Right is AssignmentExpressionSyntax rightAssign)
        {
            _diagnostics.Add(new Diagnostic("Only one assignment is permitted per expression.", rightAssign.Span));
        }
    }

    private void ValidateBinary(BinaryExpressionSyntax bin, IReadOnlyDictionary<string, Symbol> scope)
    {
        var kind = bin.OperatorToken.Kind;
        if (kind is TokenKind.AmpAmp
            or TokenKind.PipePipe
            or TokenKind.Plus
            or TokenKind.Minus
            or TokenKind.Star
            or TokenKind.Slash
            or TokenKind.Percent
            or TokenKind.Less
            or TokenKind.LessEqual
            or TokenKind.Greater
            or TokenKind.GreaterEqual
            or TokenKind.EqualEqual
            or TokenKind.BangEqual)
        {
            // For comparison operators, check type compatibility
            if (kind is TokenKind.EqualEqual or TokenKind.BangEqual
                or TokenKind.Less or TokenKind.LessEqual
                or TokenKind.Greater or TokenKind.GreaterEqual)
            {
                var leftType = ResolveExpressionType(bin.Left, scope);
                var rightType = ResolveExpressionType(bin.Right, scope);

                // Check if either side is an enum type
                if (leftType is NamedTypeSymbol leftNamed && _symbols.TryGetValue(leftNamed.TypeName, out var leftSymbol) && leftSymbol.Kind == SymbolKind.Enum)
                {
                    // Left is an enum - right must be the same enum type
                    if (rightType is not NamedTypeSymbol rightNamed || !string.Equals(leftNamed.TypeName, rightNamed.TypeName, StringComparison.Ordinal))
                    {
                        _diagnostics.Add(new Diagnostic($"Cannot compare enum '{leftNamed.TypeName}' with type '{FormatType(rightType ?? new PrimitiveTypeSymbol("unknown"))}'.", bin.Right.Span));
                    }
                }
                else if (rightType is NamedTypeSymbol rightNamed && _symbols.TryGetValue(rightNamed.TypeName, out var rightSymbol) && rightSymbol.Kind == SymbolKind.Enum)
                {
                    // Right is an enum - left must be the same enum type
                    if (leftType is not NamedTypeSymbol leftNamed2 || !string.Equals(rightNamed.TypeName, leftNamed2.TypeName, StringComparison.Ordinal))
                    {
                        _diagnostics.Add(new Diagnostic($"Cannot compare type '{FormatType(leftType ?? new PrimitiveTypeSymbol("unknown"))}' with enum '{rightNamed.TypeName}'.", bin.Left.Span));
                    }
                }
            }
            return;
        }

        _diagnostics.Add(new Diagnostic($"Unsupported infix operator '{bin.OperatorToken.Text}'.", bin.OperatorToken.Span));
    }

    private TypeSymbol? ResolveType(TypeSyntax typeSyntax)
    {
        switch (typeSyntax)
        {
            case NamedTypeSyntax named:
                if (_symbols.TryGetValue(named.Name, out var sym) && sym.Type is not null)
                {
                    return sym.Type;
                }

                _diagnostics.Add(new Diagnostic($"Unknown type '{named.Name}'.", named.Span));
                return new NamedTypeSymbol(named.Name);
            case ArrayTypeSyntax array:
                var elementType = ResolveType(array.ElementType);
                if (string.IsNullOrEmpty(array.SizeText))
                {
                    return elementType is null ? null : new ArrayTypeSymbol(elementType, -1);
                }

                if (int.TryParse(array.SizeText, out var size) && size > 0)
                {
                    return elementType is null ? null : new ArrayTypeSymbol(elementType, size);
                }

                var span = array.SizeToken?.Span ?? array.Span;
                _diagnostics.Add(new Diagnostic("Array size must be a positive integer literal.", span));
                return elementType;
            default:
                return null;
        }
    }

    private void ResolveIdentifier(string name, SourceSpan span, Dictionary<string, Symbol> scope)
    {
        if (scope.ContainsKey(name))
        {
            return;
        }

        if (_symbols.ContainsKey(name))
        {
            return;
        }

        _diagnostics.Add(new Diagnostic($"Undefined identifier '{name}'.", span));
    }

    private void EnsurePrimitiveLocal(TypeSymbol? type, SourceSpan span)
    {
        if (type is null)
        {
            return;
        }

        if (type is PrimitiveTypeSymbol primitive && primitive.PrimitiveName != "void")
        {
            return;
        }

        if (type is NamedTypeSymbol)
        {
            return;
        }

        if (type is ArrayTypeSymbol)
        {
            return;
        }

        _diagnostics.Add(new Diagnostic("Locals and parameters must be primitive types, struct references, or arrays.", span));
    }

    private void EnsureGlobalType(TypeSymbol? type, SourceSpan span)
    {
        if (type is null)
        {
            return;
        }

        if (type is ArrayTypeSymbol arr)
        {
            if (arr.Size > 0)
            {
                return;
            }

            _diagnostics.Add(new Diagnostic("Global arrays must declare a positive length.", span));
            return;
        }

        if (type is PrimitiveTypeSymbol or NamedTypeSymbol)
        {
            return;
        }

        _diagnostics.Add(new Diagnostic("Globals must be primitive, struct, or array types.", span));
    }

    private void ValidateStructFields(StructDeclarationSyntax structDecl)
    {
        foreach (var field in structDecl.Fields)
        {
            if (field.Type is ArrayTypeSyntax array && string.IsNullOrEmpty(array.SizeText))
            {
                _diagnostics.Add(new Diagnostic("Struct array fields must declare a positive length.", field.Type.Span));
            }
        }
    }

    private void RegisterFlattenedStructGlobals(string baseName, TypeSyntax typeSyntax)
    {
        if (typeSyntax is NamedTypeSyntax named && _structs.TryGetValue(named.Name, out var structDecl))
        {
            RegisterFlattenedFields(baseName, structDecl, 1);
            return;
        }

        if (typeSyntax is ArrayTypeSyntax array && array.ElementType is NamedTypeSyntax element && _structs.TryGetValue(element.Name, out var nestedStruct))
        {
            var count = ParseArraySize(array);
            RegisterFlattenedFields(baseName, nestedStruct, count);
        }
    }

    private void RegisterFlattenedFields(string baseName, StructDeclarationSyntax structDecl, int multiplier)
    {
        foreach (var field in structDecl.Fields)
        {
            RegisterFlattenedField(baseName, field, multiplier);
        }
    }

    private void RegisterFlattenedField(string prefix, StructFieldSyntax field, int multiplier)
    {
        var symbolName = $"{prefix}_{field.Identifier.Text}";

        if (field.Type is ArrayTypeSyntax array && array.ElementType is NamedTypeSyntax nestedNamed && _structs.TryGetValue(nestedNamed.Name, out var nestedStruct))
        {
            var count = ParseArraySize(array);
            RegisterFlattenedFields(symbolName, nestedStruct, count * multiplier);
            return;
        }

        if (field.Type is NamedTypeSyntax named && _structs.TryGetValue(named.Name, out var innerStruct))
        {
            RegisterFlattenedFields(symbolName, innerStruct, multiplier);
            return;
        }

        var fieldType = ResolveType(field.Type);
        if (fieldType is null)
        {
            return;
        }
        if (multiplier > 1)
        {
            fieldType = new ArrayTypeSymbol(fieldType, multiplier);
        }

        AddSymbol(symbolName, SymbolKind.Global, fieldType, field.Identifier.Span);
    }

    private static int ParseArraySize(ArrayTypeSyntax array)
    {
        if (array.SizeToken is not null && int.TryParse(array.SizeToken.Text, out var size) && size > 0)
        {
            return size;
        }

        return 1;
    }

    private void AddSymbol(string name, SymbolKind kind, TypeSymbol? type, SourceSpan span)
    {
        if (_symbols.ContainsKey(name))
        {
            _diagnostics.Add(new Diagnostic($"Duplicate symbol '{name}'.", span));
            return;
        }

        _symbols[name] = new Symbol(name, kind, type);
    }

    private void AddLocal(Dictionary<string, Symbol> scope, string name, SymbolKind kind, TypeSymbol? type, SourceSpan span)
    {
        if (scope.ContainsKey(name))
        {
            _diagnostics.Add(new Diagnostic($"Duplicate local '{name}'.", span));
            return;
        }

        scope[name] = new Symbol(name, kind, type);
    }

    private TypeSymbol? ResolveExpressionType(ExpressionSyntax expr, IReadOnlyDictionary<string, Symbol> scope)
    {
        switch (expr)
        {
            case LiteralExpressionSyntax lit:
                return lit.Literal.Kind switch
                {
                    TokenKind.IntegerLiteral => new PrimitiveTypeSymbol("i32"),
                    TokenKind.FloatLiteral => new PrimitiveTypeSymbol("f32"),
                    TokenKind.StringLiteral => new PrimitiveTypeSymbol("string_literal"),
                    TokenKind.TrueKeyword or TokenKind.FalseKeyword => new PrimitiveTypeSymbol("bool"),
                    _ => null
                };
            case IdentifierExpressionSyntax id:
                if (scope.TryGetValue(id.Identifier.Text, out var localSym))
                {
                    return localSym.Type;
                }
                if (_symbols.TryGetValue(id.Identifier.Text, out var globalSym))
                {
                    return globalSym.Type;
                }
                return null;
            case MemberAccessExpressionSyntax member:
                // Check if this is an enum member access (e.g., State.Idle)
                if (member.Receiver is IdentifierExpressionSyntax enumId &&
                    _symbols.TryGetValue(enumId.Identifier.Text, out var enumSymbol) &&
                    enumSymbol.Kind == SymbolKind.Enum)
                {
                    // Return the enum type, not i32
                    var memberName = $"{enumId.Identifier.Text}.{member.Member.Text}";
                    if (_symbols.TryGetValue(memberName, out var memberSymbol))
                    {
                        return memberSymbol.Type;
                    }
                }

                // Struct field access (including nested chains like state.ship.weapon.x)
                return ResolveMemberAccessType(member, scope);
            case BinaryExpressionSyntax bin when bin.OperatorToken.Kind is TokenKind.EqualEqual or TokenKind.BangEqual
                or TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual:
                // Comparison operators return bool
                return new PrimitiveTypeSymbol("bool");
            case BinaryExpressionSyntax:
                // Arithmetic operators - would need proper type inference
                return new PrimitiveTypeSymbol("i32");
            default:
                return null;
        }
    }

    private TypeSymbol? ResolveMemberAccessType(MemberAccessExpressionSyntax member, IReadOnlyDictionary<string, Symbol> scope)
    {
        var chain = new List<string>();
        ExpressionSyntax current = member;
        while (current is MemberAccessExpressionSyntax m)
        {
            chain.Add(m.Member.Text);
            current = m.Receiver;
        }

        if (current is not IdentifierExpressionSyntax rootId)
        {
            return null;
        }

        if (!scope.TryGetValue(rootId.Identifier.Text, out var rootSym) &&
            !_symbols.TryGetValue(rootId.Identifier.Text, out rootSym))
        {
            return null;
        }

        var currentType = rootSym.Type;
        chain.Reverse();

        foreach (var memberName in chain)
        {
            if (currentType is not NamedTypeSymbol named)
            {
                return null;
            }

            if (!_structs.TryGetValue(named.TypeName, out var structDecl))
            {
                // Not a struct type (could be an enum or unknown)
                return null;
            }

            var field = structDecl.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, memberName, StringComparison.Ordinal));
            if (field is null)
            {
                return null;
            }

            currentType = ResolveType(field.Type);
        }

        return currentType;
    }

    private bool AreTypesCompatible(TypeSymbol target, TypeSymbol source)
    {
        if (source is PrimitiveTypeSymbol sourceLiteral && sourceLiteral.PrimitiveName == "string_literal")
        {
            return target is PrimitiveTypeSymbol targetPrim
                && (targetPrim.PrimitiveName == "string"
                    || targetPrim.PrimitiveName == "utf8"
                    || targetPrim.PrimitiveName == "ascii");
        }

        // Exact type match
        if (target.GetType() == source.GetType())
        {
            if (target is PrimitiveTypeSymbol targetPrim && source is PrimitiveTypeSymbol sourcePrim)
            {
                // Allow implicit conversions between numeric primitives
                if (IsNumericType(targetPrim.PrimitiveName) && IsNumericType(sourcePrim.PrimitiveName))
                {
                    return true;
                }
                return string.Equals(targetPrim.PrimitiveName, sourcePrim.PrimitiveName, StringComparison.Ordinal);
            }
            if (target is NamedTypeSymbol targetNamed && source is NamedTypeSymbol sourceNamed)
            {
                // Enums and structs must have exact type match
                return string.Equals(targetNamed.TypeName, sourceNamed.TypeName, StringComparison.Ordinal);
            }
        }

        // Check if target is an enum - enums are NOT compatible with any primitive types
        if (target is NamedTypeSymbol targetEnum && _symbols.TryGetValue(targetEnum.TypeName, out var targetSym) && targetSym.Kind == SymbolKind.Enum)
        {
            // Enums can only be assigned from the same enum type (checked above)
            return false;
        }

        // Check if source is an enum - enum values can NOT be assigned to non-enum types
        if (source is NamedTypeSymbol sourceEnum && _symbols.TryGetValue(sourceEnum.TypeName, out var sourceSym) && sourceSym.Kind == SymbolKind.Enum)
        {
            // Enum values can only be assigned to the same enum type (checked above)
            return false;
        }

        // Allow i32 -> struct reference (index-based storage)
        if (target is NamedTypeSymbol && source is PrimitiveTypeSymbol sourcePrim2 && sourcePrim2.PrimitiveName == "i32")
        {
            return true;
        }

        return false;
    }

    private bool IsNumericType(string typeName)
    {
        return typeName is "i32" or "u8" or "u16" or "u32" or "f32" or "f64" or "bool";
    }

    private string FormatType(TypeSymbol type)
    {
        return type switch
        {
            PrimitiveTypeSymbol prim => prim.PrimitiveName,
            NamedTypeSymbol named => named.TypeName,
            ArrayTypeSymbol arr => $"{FormatType(arr.ElementType)}[{arr.Size}]",
            VoidTypeSymbol => "void",
            _ => "unknown"
        };
    }
}
