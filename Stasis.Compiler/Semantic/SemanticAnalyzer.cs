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
                    AddSymbol(e.Name.Text, SymbolKind.Enum, new NamedTypeSymbol(e.Name.Text), e.Name.Span);
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
        }
    }

    private void DeclareConstants(CompilationUnitSyntax compilationUnit)
    {
        foreach (var decl in compilationUnit.Declarations.OfType<ConstDeclarationSyntax>())
        {
            var type = ResolveType(decl.Type);
            AddSymbol(decl.Name.Text, SymbolKind.Const, type, decl.Name.Span);

            // Validate that the initializer is a compile-time constant expression
            // For now, we allow any expression - more sophisticated constant folding can be added later
            // TODO: Add validation that initializer is a literal or constant expression
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
        if (v.Type is null)
        {
            _diagnostics.Add(new Diagnostic("Local variables must declare a type; use 'let name: type = value;' to initialize.", v.Name.Span));
        }
        else
        {
            var type = ResolveType(v.Type);
            AddLocal(scope, v.Name.Text, SymbolKind.Local, type, v.Name.Span);
            EnsurePrimitiveLocal(type, v.Name.Span);
        }

        if (v.Initializer is null)
        {
            _diagnostics.Add(new Diagnostic("Local variables must be initialized with a value.", v.Name.Span));
        }
        else
        {
            AnalyzeExpression(v.Initializer, scope);
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
                break;
            case BinaryExpressionSyntax bin:
                AnalyzeExpression(bin.Left, scope);
                AnalyzeExpression(bin.Right, scope);
                ValidateBinary(bin);
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

    private void ValidateBinary(BinaryExpressionSyntax bin)
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
}
