using System.Globalization;
using System.Linq;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler;

public sealed record SemanticAnalyzerOptions(bool EnableGraphicsBuiltins = true, bool EnableAudioBuiltins = true);

public sealed class SemanticAnalyzer
{
    private readonly SemanticAnalyzerOptions _options;
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

    private bool AtDiagnosticLimit => _diagnostics.Count >= DiagnosticPolicy.MaxErrors;

    public SemanticAnalyzer(SemanticAnalyzerOptions? options = null)
    {
        _options = options ?? new SemanticAnalyzerOptions();
    }

    public SemanticResult Analyze(CompilationUnitSyntax compilationUnit)
    {
        DeclareBuiltIns();
        DeclareTypes(compilationUnit);
        if (AtDiagnosticLimit) return new SemanticResult(_diagnostics, new Dictionary<string, Symbol>(_symbols));
        DeclareGlobals(compilationUnit);
        if (AtDiagnosticLimit) return new SemanticResult(_diagnostics, new Dictionary<string, Symbol>(_symbols));
        DeclareConstants(compilationUnit);
        if (AtDiagnosticLimit) return new SemanticResult(_diagnostics, new Dictionary<string, Symbol>(_symbols));
        DeclareFunctions(compilationUnit);
        if (AtDiagnosticLimit) return new SemanticResult(_diagnostics, new Dictionary<string, Symbol>(_symbols));
        ValidateFunctionDeclarations(compilationUnit);
        if (AtDiagnosticLimit) return new SemanticResult(_diagnostics, new Dictionary<string, Symbol>(_symbols));

        foreach (var decl in compilationUnit.Declarations)
        {
            if (AtDiagnosticLimit)
            {
                break;
            }

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

    private void AddDiagnostic(string message, SourceSpan span)
    {
        if (AtDiagnosticLimit)
        {
            return;
        }

        _diagnostics.Add(new Diagnostic(message, span));
    }

    private void ValidateFunctionDeclarations(CompilationUnitSyntax compilationUnit)
    {
        foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            var hasExternAttr = fn.Attributes.Any(a => string.Equals(a.Text, "extern", StringComparison.Ordinal));

            if (fn.Body is null)
            {
                if (!fn.IsExtern && !hasExternAttr)
                {
                    AddDiagnostic($"Function '{fn.Name.Text}' is missing a body. Add a body or mark it as extern.", fn.Name.Span);
                }
            }
            else
            {
                if (hasExternAttr)
                {
                    AddDiagnostic($"Function '{fn.Name.Text}' has a body and cannot be marked @extern.", fn.Name.Span);
                }
            }
        }
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

        // System functions (sys_*): standalone CLI support (argv, file I/O, process execution).
        AddSymbol("sys_argc", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_argv", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_read_file", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_list_dir", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_write_file", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("sys_file_exists", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
        AddSymbol("sys_file_size", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_file_mtime_ms", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_exec", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_spawn", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_spawn_async", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_sleep_ms", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_delete_file", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_time_ms", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_flush", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("sys_memcpy_u8", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("sys_memcpy_i32", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("sys_memcpy_f32", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("sys_memmove_u8", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("sys_memmove_i32", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));
        AddSymbol("sys_memmove_f32", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));

        // Legacy math functions (to be renamed to math_*)
        AddSymbol("sin", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("cos", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("sin_fast", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("cos_fast", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));

        // Type conversion functions
        AddSymbol("i32_to_f32", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        AddSymbol("f32_to_i32", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("u8_to_i32", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("u16_to_i32", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        AddSymbol("i32_to_u8_trunc", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));
        AddSymbol("i32_to_u8_checked", SymbolKind.Function, new PrimitiveTypeSymbol("u8"), new SourceSpan(0, 0));
        AddSymbol("i32_to_u16_trunc", SymbolKind.Function, new PrimitiveTypeSymbol("u16"), new SourceSpan(0, 0));
        AddSymbol("i32_to_u16_checked", SymbolKind.Function, new PrimitiveTypeSymbol("u16"), new SourceSpan(0, 0));

        if (_options.EnableGraphicsBuiltins)
        {
            // Legacy system functions (to be renamed to sys_*)
            AddSymbol("time", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("get_time_ms", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("get_time_us", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("sleep_ms", SymbolKind.Function, new VoidTypeSymbol(), new SourceSpan(0, 0));

            // Startup/asset host calls (avoid in hot paths)
            AddSymbol("init_window", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
            AddSymbol("gfx_load_sprite", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("gfx_poll_reload", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
            AddSymbol("load_font", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("measure_text", SymbolKind.Function, new PrimitiveTypeSymbol("f32"), new SourceSpan(0, 0));
        }

        if (_options.EnableAudioBuiltins)
        {
            // Legacy audio functions (external runtime)
            AddSymbol("audio_is_available", SymbolKind.Function, new PrimitiveTypeSymbol("bool"), new SourceSpan(0, 0));
            AddSymbol("audio_get_sample_rate", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("audio_get_channels", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("audio_get_queued_frames", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("audio_get_underruns", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
            AddSymbol("audio_push_f32_interleaved", SymbolKind.Function, new PrimitiveTypeSymbol("i32"), new SourceSpan(0, 0));
        }

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
                AddDiagnostic("Const initializers must be literal values for now.", decl.Initializer.Span);
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
        if (fn.IsExtern || fn.Body is null)
        {
            return;
        }

        var scope = new Dictionary<string, Symbol>(StringComparer.Ordinal);
        foreach (var param in fn.Parameters)
        {
            var type = ResolveType(param.Type);
            AddLocal(scope, param.Name.Text, SymbolKind.Parameter, type, param.Name.Span);
            EnsurePrimitiveLocal(type, param.Name.Span);
        }

        AnalyzeBlock(fn.Body, scope);
        AnalyzeDefiniteAssignment(fn.Parameters, fn.Body);
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
        AnalyzeDefiniteAssignment(test.Parameters, test.Body);
    }

    private void AnalyzeDefiniteAssignment(IReadOnlyList<ParameterSyntax> parameters, BlockStatementSyntax body)
    {
        var scopeStack = new Stack<List<string>>();
        var nameToIds = new Dictionary<string, Stack<int>>(StringComparer.Ordinal);
        var assigned = new HashSet<int>();
        var localCount = 0;

        void PushScope()
        {
            scopeStack.Push(new List<string>());
        }

        void PopScope(HashSet<int> assignedInScope)
        {
            var declared = scopeStack.Pop();
            foreach (var name in declared)
            {
                if (!nameToIds.TryGetValue(name, out var stack) || stack.Count == 0)
                {
                    continue;
                }

                var id = stack.Pop();
                assignedInScope.Remove(id);
                if (stack.Count == 0)
                {
                    nameToIds.Remove(name);
                }
            }
        }

        int DeclareLocal(string name, bool isAssigned, HashSet<int> assignedInScope)
        {
            var id = localCount;
            localCount++;

            if (!nameToIds.TryGetValue(name, out var stack))
            {
                stack = new Stack<int>();
                nameToIds[name] = stack;
            }
            stack.Push(id);

            if (scopeStack.Count > 0)
            {
                scopeStack.Peek().Add(name);
            }

            if (isAssigned)
            {
                assignedInScope.Add(id);
            }

            return id;
        }

        bool TryResolveLocalId(string name, out int id)
        {
            if (nameToIds.TryGetValue(name, out var stack) && stack.Count > 0)
            {
                id = stack.Peek();
                return true;
            }

            id = 0;
            return false;
        }

        void NoteRead(string name, SourceSpan span, HashSet<int> assignedInScope)
        {
            if (TryResolveLocalId(name, out var id) && !assignedInScope.Contains(id))
            {
                AddDiagnostic($"Local '{name}' may be uninitialized here. Assign before reading.", span);
            }
        }

        void NoteWriteTarget(ExpressionSyntax target, HashSet<int> assignedInScope)
        {
            switch (target)
            {
                case IdentifierExpressionSyntax id when TryResolveLocalId(id.Identifier.Text, out var localId):
                    assignedInScope.Add(localId);
                    return;
                case MemberAccessExpressionSyntax member:
                    NoteWriteTarget(member.Receiver, assignedInScope);
                    return;
                case ArrayAccessExpressionSyntax array:
                    NoteWriteTarget(array.Receiver, assignedInScope);
                    return;
            }
        }

        void AnalyzeLValue(ExpressionSyntax expr, HashSet<int> assignedInScope)
        {
            switch (expr)
            {
                case IdentifierExpressionSyntax:
                    return;
                case MemberAccessExpressionSyntax member:
                    // Writing through a receiver does not require the receiver local to be "definitely assigned"
                    // (e.g. `buf[i] = ...`, `s.field = ...`). Treat the receiver as an lvalue base.
                    AnalyzeLValue(member.Receiver, assignedInScope);
                    return;
                case ArrayAccessExpressionSyntax array:
                    // Same rule as member access: the receiver is an lvalue base.
                    AnalyzeLValue(array.Receiver, assignedInScope);
                    AnalyzeExpr(array.Index, assignedInScope);
                    return;
                default:
                    AnalyzeExpr(expr, assignedInScope);
                    return;
            }
        }

        void AnalyzeAssignment(AssignmentExpressionSyntax assignExpr, HashSet<int> assignedInScope)
        {
            var op = assignExpr.OperatorToken.Text;
            if (!string.Equals(op, "=", StringComparison.Ordinal))
            {
                // Compound assignment reads the current LHS value.
                AnalyzeExpr(assignExpr.Left, assignedInScope);
            }

            AnalyzeExpr(assignExpr.Right, assignedInScope);
            AnalyzeLValue(assignExpr.Left, assignedInScope);
            NoteWriteTarget(assignExpr.Left, assignedInScope);
        }

        void AnalyzeExpr(ExpressionSyntax expr, HashSet<int> assignedInScope)
        {
            switch (expr)
            {
                case IdentifierExpressionSyntax id:
                    NoteRead(id.Identifier.Text, id.Span, assignedInScope);
                    return;
                case LiteralExpressionSyntax:
                    return;
                case ParenthesizedExpressionSyntax paren:
                    AnalyzeExpr(paren.Expression, assignedInScope);
                    return;
                case UnaryExpressionSyntax unary:
                    AnalyzeExpr(unary.Operand, assignedInScope);
                    return;
                case MemberAccessExpressionSyntax member:
                    AnalyzeExpr(member.Receiver, assignedInScope);
                    return;
                case ArrayAccessExpressionSyntax array:
                    AnalyzeExpr(array.Receiver, assignedInScope);
                    AnalyzeExpr(array.Index, assignedInScope);
                    return;
                case CallExpressionSyntax call:
                    AnalyzeExpr(call.Callee, assignedInScope);
                    foreach (var arg in call.Arguments)
                    {
                        AnalyzeExpr(arg, assignedInScope);
                    }
                    return;
                case OperatorCallExpressionSyntax opCall:
                    AnalyzeExpr(opCall.Receiver, assignedInScope);
                    foreach (var arg in opCall.Arguments)
                    {
                        AnalyzeExpr(arg, assignedInScope);
                    }
                    return;
                case AssignmentExpressionSyntax assignExpr:
                    AnalyzeAssignment(assignExpr, assignedInScope);
                    return;
                case BinaryExpressionSyntax bin:
                    AnalyzeExpr(bin.Left, assignedInScope);
                    AnalyzeExpr(bin.Right, assignedInScope);
                    return;
            }
        }

        (HashSet<int> Assigned, bool Terminates) AnalyzeStmt(StatementSyntax stmt, HashSet<int> assignedInScope)
        {
            switch (stmt)
            {
                case VariableDeclarationSyntax v:
                    {
                        if (v.Type is null)
                        {
                            return (assignedInScope, false);
                        }

                        // Declaration introduces a new binding (may shadow an outer local).
                        var hasInit = v.Initializer is not null;
                        _ = DeclareLocal(v.Name.Text, isAssigned: hasInit, assignedInScope);
                        if (hasInit)
                        {
                            AnalyzeExpr(v.Initializer!, assignedInScope);
                        }
                        return (assignedInScope, false);
                    }
                case ExpressionStatementSyntax es:
                    AnalyzeExpr(es.Expression, assignedInScope);
                    return (assignedInScope, false);
                case ReturnStatementSyntax ret:
                    if (ret.Expression is not null)
                    {
                        AnalyzeExpr(ret.Expression, assignedInScope);
                    }
                    return (assignedInScope, true);
                case BlockStatementSyntax block:
                    PushScope();
                    var (outAssigned, terminates) = AnalyzeBlockDefinite(block, assignedInScope);
                    PopScope(outAssigned);
                    return (outAssigned, terminates);
                case IfStatementSyntax ifs:
                    AnalyzeExpr(ifs.Condition, assignedInScope);
                    var thenAssigned = new HashSet<int>(assignedInScope);
                    PushScope();
                    var (thenOut, thenTerminates) = AnalyzeBlockDefinite(ifs.ThenBlock, thenAssigned);
                    PopScope(thenOut);

                    if (ifs.ElseBlock is null)
                    {
                        // Then may not run.
                        return (assignedInScope, false);
                    }

                    var elseAssigned = new HashSet<int>(assignedInScope);
                    PushScope();
                    var (elseOut, elseTerminates) = AnalyzeBlockDefinite(ifs.ElseBlock, elseAssigned);
                    PopScope(elseOut);

                    thenOut.IntersectWith(elseOut);
                    return (thenOut, thenTerminates && elseTerminates);
                case ForStatementSyntax fs:
                    // Initializer always runs once.
                    if (fs.Initializer is not null)
                    {
                        AnalyzeExpr(fs.Initializer, assignedInScope);
                    }
                    if (fs.Condition is not null)
                    {
                        AnalyzeExpr(fs.Condition, assignedInScope);
                    }

                    var bodyAssigned = new HashSet<int>(assignedInScope);
                    PushScope();
                    _ = AnalyzeBlockDefinite(fs.Body, bodyAssigned);
                    PopScope(bodyAssigned);

                    if (fs.Step is not null)
                    {
                        AnalyzeExpr(fs.Step, bodyAssigned);
                    }

                    // Loop may execute zero times; do not carry assignments from body.
                    return (assignedInScope, false);
                case ForeachStatementSyntax fes:
                    AnalyzeExpr(fes.Iterable, assignedInScope);

                    var foreachAssigned = new HashSet<int>(assignedInScope);
                    PushScope();
                    _ = DeclareLocal(fes.Iterator.Text, isAssigned: true, foreachAssigned);
                    if (fes.IndexVariable is not null)
                    {
                        _ = DeclareLocal(fes.IndexVariable.Text, isAssigned: true, foreachAssigned);
                    }
                    _ = AnalyzeBlockDefinite(fes.Body, foreachAssigned);
                    PopScope(foreachAssigned);

                    // Loop may execute zero times; do not carry assignments from body.
                    return (assignedInScope, false);
            }

            return (assignedInScope, false);
        }

        (HashSet<int> Assigned, bool Terminates) AnalyzeBlockDefinite(BlockStatementSyntax block, HashSet<int> assignedInScope)
        {
            var current = assignedInScope;
            foreach (var stmt in block.Statements)
            {
                var (next, terminates) = AnalyzeStmt(stmt, current);
                current = next;
                if (terminates)
                {
                    return (current, true);
                }
            }

            return (current, false);
        }

        // Function scope.
        PushScope();
        foreach (var param in parameters)
        {
            _ = DeclareLocal(param.Name.Text, isAssigned: true, assigned);
        }

        _ = AnalyzeBlockDefinite(body, assigned);
        PopScope(assigned);
    }

    private void AnalyzeBlock(BlockStatementSyntax block, Dictionary<string, Symbol> scope)
    {
        if (AtDiagnosticLimit)
        {
            return;
        }

        foreach (var stmt in block.Statements)
        {
            if (AtDiagnosticLimit)
            {
                return;
            }

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
        if (AtDiagnosticLimit)
        {
            return;
        }

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
                        AddDiagnostic("foreach target must be an array.", fes.Iterable.Span);
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
            AddDiagnostic("Local variables must declare a type; use 'let name: type = value;' to initialize.", v.Name.Span);
        }
        else
        {
            varType = ResolveType(v.Type);
            AddLocal(scope, v.Name.Text, SymbolKind.Local, varType, v.Name.Span);
            EnsurePrimitiveLocal(varType, v.Name.Span);
        }

        if (v.Initializer is not null)
        {
            AnalyzeExpression(v.Initializer, scope);
            // Type check: ensure initializer type matches variable type
            var initType = ResolveExpressionType(v.Initializer, scope);
            if (varType is not null && initType is not null && !AreTypesCompatible(varType, initType))
            {
                if (!TryAllowNumericLiteralAssignment(varType, v.Initializer, out var literalError))
                {
                    AddDiagnostic($"Cannot assign value of type '{FormatType(initType)}' to variable of type '{FormatType(varType)}'.", v.Initializer.Span);
                }
                else if (literalError is not null)
                {
                    AddDiagnostic(literalError, v.Initializer.Span);
                }
            }
        }
    }

    private void AnalyzeExpression(ExpressionSyntax expr, Dictionary<string, Symbol> scope)
    {
        if (AtDiagnosticLimit)
        {
            return;
        }

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
                            AddDiagnostic($"Enum '{enumName}' does not have a member named '{m.Member.Text}'.", m.Member.Span);
                        }
                        // Don't recursively analyze the receiver since it's just the enum type name
                        return;
                    }
                }
                // Not an enum member access - regular struct field access
                AnalyzeExpression(m.Receiver, scope);
                // Resolve the full member access chain now so missing/invalid fields show up even when used in a return/expression context.
                _ = ResolveExpressionType(m, scope);
                break;
            case ArrayAccessExpressionSyntax a:
                AnalyzeExpression(a.Receiver, scope);
                AnalyzeExpression(a.Index, scope);
                var receiverType = ResolveExpressionType(a.Receiver, scope);
                if (receiverType is not null && receiverType is not ArrayTypeSymbol)
                {
                    AddDiagnostic($"Array access requires an array receiver; got '{FormatType(receiverType)}'.", a.Receiver.Span);
                }

                var indexType = ResolveExpressionType(a.Index, scope);
                if (indexType is not null && (indexType is not PrimitiveTypeSymbol ip || !IsIntegerType(ip.PrimitiveName)))
                {
                    AddDiagnostic($"Array index must be an integer type; got '{FormatType(indexType)}'.", a.Index.Span);
                }
                break;
            case CallExpressionSyntax c:
                if (c.Callee is MemberAccessExpressionSyntax member &&
                    string.Equals(member.Member.Text, "clear", StringComparison.Ordinal))
                {
                    // Special-form call: don't treat `.clear` as a regular member access (it's not a struct field).
                    AnalyzeExpression(member.Receiver, scope);
                    foreach (var arg in c.Arguments)
                    {
                        AnalyzeExpression(arg, scope);
                    }
                    ValidateClearCall(c, member, scope);
                    break;
                }

                AnalyzeExpression(c.Callee, scope);
                foreach (var arg in c.Arguments)
                {
                    AnalyzeExpression(arg, scope);
                }

                if (c.Callee is IdentifierExpressionSyntax idCallee)
                {
                    // Functions/tests are global-only in Stasis (no first-class function values).
                    if (scope.TryGetValue(idCallee.Identifier.Text, out var localCallee))
                    {
                        AddDiagnostic($"'{idCallee.Identifier.Text}' is not callable.", idCallee.Identifier.Span);
                    }
                    else if (_symbols.TryGetValue(idCallee.Identifier.Text, out var calleeSym))
                    {
                        if (calleeSym.Kind is not (SymbolKind.Function or SymbolKind.Test))
                        {
                            AddDiagnostic($"'{idCallee.Identifier.Text}' is not callable.", idCallee.Identifier.Span);
                        }
                    }
                    else
                    {
                        // Prefer a function-specific message over a generic undefined identifier error.
                        AddDiagnostic($"Unknown function '{idCallee.Identifier.Text}'.", idCallee.Identifier.Span);
                    }
                }
                else
                {
                    AddDiagnostic("Only simple function calls are supported.", c.Span);
                }
                break;
            case OperatorCallExpressionSyntax op:
                AnalyzeExpression(op.Receiver, scope);
                foreach (var arg in op.Arguments)
                {
                    AnalyzeExpression(arg, scope);
                }

                ValidateOperatorCall(op, scope);
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
                    if (!TryAllowNumericLiteralAssignment(leftType, assign.Right, out var literalError))
                    {
                        AddDiagnostic($"Cannot assign value of type '{FormatType(rightType)}' to target of type '{FormatType(leftType)}'.", assign.Right.Span);
                    }
                    else if (literalError is not null)
                    {
                        AddDiagnostic(literalError, assign.Right.Span);
                    }
                }
                break;
            case BinaryExpressionSyntax bin:
                AnalyzeExpression(bin.Left, scope);
                AnalyzeExpression(bin.Right, scope);
                ValidateBinary(bin, scope);
                break;
        }
    }

    private void ValidateClearCall(CallExpressionSyntax call, MemberAccessExpressionSyntax member, Dictionary<string, Symbol> scope)
    {
        if (call.Arguments.Count != 0)
        {
            AddDiagnostic("clear() takes no arguments.", call.Span);
            return;
        }

        // Require the root receiver to be a global (globals + global struct fields). Avoids ambiguous semantics for locals.
        var root = member.Receiver;
        while (root is MemberAccessExpressionSyntax m)
        {
            root = m.Receiver;
        }

        if (root is not IdentifierExpressionSyntax id)
        {
            AddDiagnostic("clear() receiver must be a global or global field.", member.Receiver.Span);
            return;
        }

        if (scope.TryGetValue(id.Identifier.Text, out var localSym) && localSym.Kind == SymbolKind.Local)
        {
            AddDiagnostic("clear() is only supported on globals and global struct fields.", member.Receiver.Span);
            return;
        }

        if (!_symbols.TryGetValue(id.Identifier.Text, out var globalSym) || globalSym.Kind != SymbolKind.Global)
        {
            AddDiagnostic("clear() receiver must be a global or global field.", member.Receiver.Span);
            return;
        }

        var recvType = ResolveExpressionType(member.Receiver, scope);
        if (recvType is null)
        {
            AddDiagnostic("clear() receiver type could not be resolved.", member.Receiver.Span);
            return;
        }

        var ok = recvType switch
        {
            ArrayTypeSymbol arr => arr.Size > 0 && IsZeroableForClear(arr),
            NamedTypeSymbol named when _structs.ContainsKey(named.TypeName) => IsZeroableForClear(named),
            _ => false
        };

        if (!ok)
        {
            AddDiagnostic($"clear() is only supported for zeroable fixed arrays and structs; got '{FormatType(recvType)}'.", member.Receiver.Span);
        }
    }

    private bool IsZeroableForClear(TypeSymbol type)
    {
        switch (type)
        {
            case PrimitiveTypeSymbol prim:
                return prim.PrimitiveName is "bool" or "u8" or "u16" or "u32" or "i32" or "f32" or "f64";
            case ArrayTypeSymbol arr:
                return arr.Size > 0 && IsZeroableForClear(arr.ElementType);
            case NamedTypeSymbol named when _structs.TryGetValue(named.TypeName, out var structDecl):
                foreach (var field in structDecl.Fields)
                {
                    var fieldType = ResolveType(field.Type);
                    if (fieldType is null || !IsZeroableForClear(fieldType))
                    {
                        return false;
                    }
                }
                return true;
            default:
                return false;
        }
    }

    private void ValidateOperatorCall(OperatorCallExpressionSyntax op, IReadOnlyDictionary<string, Symbol> scope)
    {
        var opText = op.OperatorToken.Text;
        if (op.Arguments.Count != 1)
        {
            AddDiagnostic($"Operator '.{opText}()' requires exactly one argument.", op.Span);
        }

        if (opText == "=")
        {
            AddDiagnostic("Use infix '=' for assignment.", op.Span);
            if (!IsAssignableReceiver(op.Receiver))
            {
                AddDiagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", op.Receiver.Span);
            }
            return;
        }

        if (op.Arguments.Count != 1)
        {
            return;
        }

        var receiverType = ResolveExpressionType(op.Receiver, scope);
        var argType = ResolveExpressionType(op.Arguments[0], scope);
        if (receiverType is PrimitiveTypeSymbol recvPrim && argType is PrimitiveTypeSymbol argPrim)
        {
            if (IsIntegerType(recvPrim.PrimitiveName) && IsIntegerType(argPrim.PrimitiveName) &&
                !string.Equals(recvPrim.PrimitiveName, argPrim.PrimitiveName, StringComparison.Ordinal))
            {
                if (!TryAllowNumericLiteralCompatibility(recvPrim.PrimitiveName, op.Arguments[0], out var literalError))
                {
                    AddDiagnostic($"Cannot mix integer type '{recvPrim.PrimitiveName}' with integer type '{argPrim.PrimitiveName}' in operator call. Use an explicit conversion.", op.Span);
                }
                else if (literalError is not null)
                {
                    AddDiagnostic(literalError, op.Arguments[0].Span);
                }
            }
            else if (IsFloatType(recvPrim.PrimitiveName) && IsFloatType(argPrim.PrimitiveName) &&
                     !string.Equals(recvPrim.PrimitiveName, argPrim.PrimitiveName, StringComparison.Ordinal))
            {
                AddDiagnostic($"Cannot mix float type '{recvPrim.PrimitiveName}' with float type '{argPrim.PrimitiveName}' in operator call. Use an explicit conversion.", op.Span);
            }
            else if (IsIntegerType(recvPrim.PrimitiveName) && IsFloatType(argPrim.PrimitiveName))
            {
                AddDiagnostic($"Cannot mix integer type '{recvPrim.PrimitiveName}' with float type '{argPrim.PrimitiveName}' in operator call. Use i32_to_f32() or f32_to_i32() for explicit conversion.", op.Span);
            }
            else if (IsFloatType(recvPrim.PrimitiveName) && IsIntegerType(argPrim.PrimitiveName))
            {
                AddDiagnostic($"Cannot mix float type '{recvPrim.PrimitiveName}' with integer type '{argPrim.PrimitiveName}' in operator call. Use i32_to_f32() or f32_to_i32() for explicit conversion.", op.Span);
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
            AddDiagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", target.Span);
            return;
        }

        // Check if trying to assign to a constant
        if (target is IdentifierExpressionSyntax id && _symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Const)
        {
            AddDiagnostic($"Cannot assign to constant '{id.Identifier.Text}'. Constants are immutable.", target.Span);
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

        AddDiagnostic($"Unsupported assignment operator '{opToken.Text}'.", opToken.Span);
    }

    private void ValidateSingleAssignment(AssignmentExpressionSyntax assign)
    {
        if (assign.Left is AssignmentExpressionSyntax or BinaryExpressionSyntax { OperatorToken.Kind: TokenKind.Equal or TokenKind.PlusEqual or TokenKind.MinusEqual or TokenKind.StarEqual or TokenKind.SlashEqual or TokenKind.PercentEqual })
        {
            AddDiagnostic("Only one assignment is permitted per expression.", assign.Left.Span);
        }

        if (assign.Right is AssignmentExpressionSyntax rightAssign)
        {
            AddDiagnostic("Only one assignment is permitted per expression.", rightAssign.Span);
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
            var leftType = ResolveExpressionType(bin.Left, scope);
            var rightType = ResolveExpressionType(bin.Right, scope);

            // For arithmetic operators, check for type mismatches (no implicit conversions)
            if (kind is TokenKind.Plus or TokenKind.Minus or TokenKind.Star or TokenKind.Slash or TokenKind.Percent)
            {
                if (leftType is PrimitiveTypeSymbol leftPrim && rightType is PrimitiveTypeSymbol rightPrim)
                {
                    // Check for mixed integer/float operations - require explicit conversion
                    if (IsIntegerType(leftPrim.PrimitiveName) && IsFloatType(rightPrim.PrimitiveName))
                    {
                        AddDiagnostic($"Cannot mix integer type '{leftPrim.PrimitiveName}' with float type '{rightPrim.PrimitiveName}' in arithmetic. Use i32_to_f32() or f32_to_i32() for explicit conversion.", bin.OperatorToken.Span);
                    }
                    else if (IsFloatType(leftPrim.PrimitiveName) && IsIntegerType(rightPrim.PrimitiveName))
                    {
                        AddDiagnostic($"Cannot mix float type '{leftPrim.PrimitiveName}' with integer type '{rightPrim.PrimitiveName}' in arithmetic. Use i32_to_f32() or f32_to_i32() for explicit conversion.", bin.OperatorToken.Span);
                    }
                    else if (IsIntegerType(leftPrim.PrimitiveName) && IsIntegerType(rightPrim.PrimitiveName) &&
                             !string.Equals(leftPrim.PrimitiveName, rightPrim.PrimitiveName, StringComparison.Ordinal))
                    {
                        // Allow integer literals to match the other operand type when the literal fits (e.g., u8 == 72).
                        var rightLiteralOk = TryAllowNumericLiteralCompatibility(leftPrim.PrimitiveName, bin.Right, out var rightLiteralError);
                        var leftLiteralOk = TryAllowNumericLiteralCompatibility(rightPrim.PrimitiveName, bin.Left, out var leftLiteralError);
                        if (!rightLiteralOk && !leftLiteralOk)
                        {
                            AddDiagnostic($"Cannot mix integer type '{leftPrim.PrimitiveName}' with integer type '{rightPrim.PrimitiveName}' in arithmetic. Use an explicit conversion.", bin.OperatorToken.Span);
                        }
                        else if (rightLiteralError is not null)
                        {
                            AddDiagnostic(rightLiteralError, bin.Right.Span);
                        }
                        else if (leftLiteralError is not null)
                        {
                            AddDiagnostic(leftLiteralError, bin.Left.Span);
                        }
                    }
                    else if (IsFloatType(leftPrim.PrimitiveName) && IsFloatType(rightPrim.PrimitiveName) &&
                             !string.Equals(leftPrim.PrimitiveName, rightPrim.PrimitiveName, StringComparison.Ordinal))
                    {
                        AddDiagnostic($"Cannot mix float type '{leftPrim.PrimitiveName}' with float type '{rightPrim.PrimitiveName}' in arithmetic. Use an explicit conversion.", bin.OperatorToken.Span);
                    }
                }
            }

            // For comparison operators, check type compatibility
            if (kind is TokenKind.EqualEqual or TokenKind.BangEqual
                or TokenKind.Less or TokenKind.LessEqual
                or TokenKind.Greater or TokenKind.GreaterEqual)
            {
                // Check for mixed integer/float comparisons
                if (leftType is PrimitiveTypeSymbol leftPrim && rightType is PrimitiveTypeSymbol rightPrim)
                {
                    if (IsIntegerType(leftPrim.PrimitiveName) && IsFloatType(rightPrim.PrimitiveName))
                    {
                        AddDiagnostic($"Cannot compare integer type '{leftPrim.PrimitiveName}' with float type '{rightPrim.PrimitiveName}'. Use i32_to_f32() or f32_to_i32() for explicit conversion.", bin.OperatorToken.Span);
                    }
                    else if (IsFloatType(leftPrim.PrimitiveName) && IsIntegerType(rightPrim.PrimitiveName))
                    {
                        AddDiagnostic($"Cannot compare float type '{leftPrim.PrimitiveName}' with integer type '{rightPrim.PrimitiveName}'. Use i32_to_f32() or f32_to_i32() for explicit conversion.", bin.OperatorToken.Span);
                    }
                    else if (IsIntegerType(leftPrim.PrimitiveName) && IsIntegerType(rightPrim.PrimitiveName) &&
                             !string.Equals(leftPrim.PrimitiveName, rightPrim.PrimitiveName, StringComparison.Ordinal))
                    {
                        var rightLiteralOk = TryAllowNumericLiteralCompatibility(leftPrim.PrimitiveName, bin.Right, out var rightLiteralError);
                        var leftLiteralOk = TryAllowNumericLiteralCompatibility(rightPrim.PrimitiveName, bin.Left, out var leftLiteralError);
                        if (!rightLiteralOk && !leftLiteralOk)
                        {
                            AddDiagnostic($"Cannot compare integer type '{leftPrim.PrimitiveName}' with integer type '{rightPrim.PrimitiveName}'. Use an explicit conversion.", bin.OperatorToken.Span);
                        }
                        else if (rightLiteralError is not null)
                        {
                            AddDiagnostic(rightLiteralError, bin.Right.Span);
                        }
                        else if (leftLiteralError is not null)
                        {
                            AddDiagnostic(leftLiteralError, bin.Left.Span);
                        }
                    }
                    else if (IsFloatType(leftPrim.PrimitiveName) && IsFloatType(rightPrim.PrimitiveName) &&
                             !string.Equals(leftPrim.PrimitiveName, rightPrim.PrimitiveName, StringComparison.Ordinal))
                    {
                        AddDiagnostic($"Cannot compare float type '{leftPrim.PrimitiveName}' with float type '{rightPrim.PrimitiveName}'. Use an explicit conversion.", bin.OperatorToken.Span);
                    }
                }

                // Check if either side is an enum type
                if (leftType is NamedTypeSymbol leftNamed && _symbols.TryGetValue(leftNamed.TypeName, out var leftSymbol) && leftSymbol.Kind == SymbolKind.Enum)
                {
                    // Left is an enum - right must be the same enum type
                    if (rightType is not NamedTypeSymbol rightNamed || !string.Equals(leftNamed.TypeName, rightNamed.TypeName, StringComparison.Ordinal))
                    {
                        AddDiagnostic($"Cannot compare enum '{leftNamed.TypeName}' with type '{FormatType(rightType ?? new PrimitiveTypeSymbol("unknown"))}'.", bin.Right.Span);
                    }
                }
                else if (rightType is NamedTypeSymbol rightNamed && _symbols.TryGetValue(rightNamed.TypeName, out var rightSymbol) && rightSymbol.Kind == SymbolKind.Enum)
                {
                    // Right is an enum - left must be the same enum type
                    if (leftType is not NamedTypeSymbol leftNamed2 || !string.Equals(rightNamed.TypeName, leftNamed2.TypeName, StringComparison.Ordinal))
                    {
                        AddDiagnostic($"Cannot compare type '{FormatType(leftType ?? new PrimitiveTypeSymbol("unknown"))}' with enum '{rightNamed.TypeName}'.", bin.Left.Span);
                    }
                }
            }
            return;
        }

        AddDiagnostic($"Unsupported infix operator '{bin.OperatorToken.Text}'.", bin.OperatorToken.Span);
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

                AddDiagnostic($"Unknown type '{named.Name}'.", named.Span);
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
                AddDiagnostic("Array size must be a positive integer literal.", span);
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

        AddDiagnostic($"Undefined identifier '{name}'.", span);
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

        AddDiagnostic("Locals and parameters must be primitive types, struct references, or arrays.", span);
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

            AddDiagnostic("Global arrays must declare a positive length.", span);
            return;
        }

        if (type is PrimitiveTypeSymbol or NamedTypeSymbol)
        {
            return;
        }

        AddDiagnostic("Globals must be primitive, struct, or array types.", span);
    }

    private void ValidateStructFields(StructDeclarationSyntax structDecl)
    {
        foreach (var field in structDecl.Fields)
        {
            if (field.Type is ArrayTypeSyntax array && string.IsNullOrEmpty(array.SizeText))
            {
                AddDiagnostic("Struct array fields must declare a positive length.", field.Type.Span);
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
            AddDiagnostic($"Duplicate symbol '{name}'.", span);
            return;
        }

        _symbols[name] = new Symbol(name, kind, type);
    }

    private void AddLocal(Dictionary<string, Symbol> scope, string name, SymbolKind kind, TypeSymbol? type, SourceSpan span)
    {
        if (scope.ContainsKey(name))
        {
            AddDiagnostic($"Duplicate local '{name}'.", span);
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
                    TokenKind.U8Literal => new PrimitiveTypeSymbol("u8"),
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
            case UnaryExpressionSyntax unary when unary.OperatorToken.Kind == TokenKind.Bang:
                return new PrimitiveTypeSymbol("bool");
            case UnaryExpressionSyntax unary:
                return ResolveExpressionType(unary.Operand, scope);
            case ParenthesizedExpressionSyntax paren:
                return ResolveExpressionType(paren.Expression, scope);
            case AssignmentExpressionSyntax assign:
                return ResolveExpressionType(assign.Left, scope);
            case MemberAccessExpressionSyntax member:
                // Array length property: `arr.length` yields the compile-time fixed capacity.
                if (string.Equals(member.Member.Text, "length", StringComparison.Ordinal))
                {
                    var receiverType = ResolveExpressionType(member.Receiver, scope);
                    if (receiverType is ArrayTypeSymbol)
                    {
                        return new PrimitiveTypeSymbol("i32");
                    }
                }

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
            case ArrayAccessExpressionSyntax array:
                {
                    var receiverType = ResolveExpressionType(array.Receiver, scope);
                    if (receiverType is ArrayTypeSymbol arrType)
                    {
                        if (arrType.ElementType is PrimitiveTypeSymbol prim &&
                            (prim.PrimitiveName == "ascii" || prim.PrimitiveName == "utf8"))
                        {
                            // String buffers expose byte elements when indexed.
                            return new PrimitiveTypeSymbol("u8");
                        }
                        return arrType.ElementType;
                    }
                    return null;
                }
            case BinaryExpressionSyntax bin when bin.OperatorToken.Kind is TokenKind.EqualEqual or TokenKind.BangEqual
                or TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual:
                // Comparison operators return bool
                return new PrimitiveTypeSymbol("bool");
            case BinaryExpressionSyntax bin:
                {
                    // Infer arithmetic result type from operands
                    var leftType = ResolveExpressionType(bin.Left, scope);
                    var rightType = ResolveExpressionType(bin.Right, scope);

                    // If either operand is f32/f64, result is float
                    if (leftType is PrimitiveTypeSymbol leftPrim && IsFloatType(leftPrim.PrimitiveName))
                    {
                        return leftType;
                    }
                    if (rightType is PrimitiveTypeSymbol rightPrim && IsFloatType(rightPrim.PrimitiveName))
                    {
                        return rightType;
                    }

                    // Default to left operand type, or i32 if unknown
                    return leftType ?? new PrimitiveTypeSymbol("i32");
                }
            case CallExpressionSyntax call when call.Callee is IdentifierExpressionSyntax id &&
                                               _symbols.TryGetValue(id.Identifier.Text, out var sym):
                return sym.Type;
            case OperatorCallExpressionSyntax op:
                {
                    // Operator calls are expressions; the result is either the receiver type or bool for comparisons.
                    var receiverType = ResolveExpressionType(op.Receiver, scope);
                    return op.OperatorToken.Kind switch
                    {
                        TokenKind.EqualEqual or TokenKind.BangEqual or TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual =>
                            new PrimitiveTypeSymbol("bool"),
                        _ => receiverType
                    };
                }
            default:
                return null;
        }
    }

    private TypeSymbol? ResolveMemberAccessType(MemberAccessExpressionSyntax member, IReadOnlyDictionary<string, Symbol> scope)
    {
        var chain = new List<(string Name, SourceSpan Span)>();
        ExpressionSyntax current = member;
        while (current is MemberAccessExpressionSyntax m)
        {
            chain.Add((m.Member.Text, m.Member.Span));
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

        foreach (var (memberName, memberSpan) in chain)
        {
            if (currentType is not NamedTypeSymbol named)
            {
                var got = currentType is null ? "unknown" : FormatType(currentType);
                AddDiagnostic($"Member access '.{memberName}' requires a struct type; got '{got}'.", memberSpan);
                return new PrimitiveTypeSymbol("i32");
            }

            if (!_structs.TryGetValue(named.TypeName, out var structDecl))
            {
                // Not a struct type (could be an enum or unknown)
                AddDiagnostic($"Type '{named.TypeName}' is not a struct; cannot access field '{memberName}'.", memberSpan);
                return new PrimitiveTypeSymbol("i32");
            }

            var field = structDecl.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, memberName, StringComparison.Ordinal));
            if (field is null)
            {
                AddDiagnostic($"Unknown field '{memberName}' on struct '{named.TypeName}'.", memberSpan);
                return new PrimitiveTypeSymbol("i32");
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

        // Exact type match required - no implicit numeric conversions
        if (target.GetType() == source.GetType())
        {
            if (target is PrimitiveTypeSymbol targetPrim && source is PrimitiveTypeSymbol sourcePrim)
            {
                // Require exact type match for primitives - use explicit conversions (i32_to_f32, f32_to_i32)
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
        return typeName is "i32" or "u8" or "u16" or "u32" or "f32" or "f64";
    }

    private bool IsIntegerType(string typeName)
    {
        // Note: utf8/ascii behave like u8 for indexing/byte-oriented APIs.
        return typeName is "i32" or "u8" or "u16" or "u32" or "utf8" or "ascii";
    }

    private bool IsFloatType(string typeName)
    {
        return typeName is "f32" or "f64";
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

    private static bool TryGetIntegerLiteralValue(ExpressionSyntax expr, out long value)
    {
        if (expr is LiteralExpressionSyntax lit &&
            (lit.Literal.Kind == TokenKind.IntegerLiteral || lit.Literal.Kind == TokenKind.U8Literal) &&
            long.TryParse(lit.Literal.Text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var parsed))
        {
            value = parsed;
            return true;
        }

        if (expr is UnaryExpressionSyntax unary &&
            unary.OperatorToken.Kind == TokenKind.Minus &&
            unary.Operand is LiteralExpressionSyntax innerLit &&
            (innerLit.Literal.Kind == TokenKind.IntegerLiteral || innerLit.Literal.Kind == TokenKind.U8Literal) &&
            long.TryParse(innerLit.Literal.Text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var innerParsed))
        {
            value = -innerParsed;
            return true;
        }

        value = 0;
        return false;
    }

    private static bool TryAllowNumericLiteralCompatibility(string targetPrimitiveName, ExpressionSyntax expr, out string? error)
    {
        error = null;
        if (!TryGetIntegerLiteralValue(expr, out var literal))
        {
            return false;
        }

        switch (targetPrimitiveName)
        {
            case "u8":
            case "utf8":
            case "ascii":
                if (literal < 0 || literal > byte.MaxValue)
                {
                    error = $"Integer literal {literal} does not fit in '{targetPrimitiveName}'.";
                }
                return true;
            case "u16":
                if (literal < 0 || literal > ushort.MaxValue)
                {
                    error = $"Integer literal {literal} does not fit in '{targetPrimitiveName}'.";
                }
                return true;
            case "u32":
                if (literal < 0 || literal > uint.MaxValue)
                {
                    error = $"Integer literal {literal} does not fit in '{targetPrimitiveName}'.";
                }
                return true;
            case "i32":
                if (literal < int.MinValue || literal > int.MaxValue)
                {
                    error = $"Integer literal {literal} does not fit in '{targetPrimitiveName}'.";
                }
                return true;
            default:
                return false;
        }
    }

    private static bool TryAllowNumericLiteralAssignment(TypeSymbol targetType, ExpressionSyntax expr, out string? error)
    {
        error = null;
        if (targetType is not PrimitiveTypeSymbol prim)
        {
            return false;
        }

        if (!IsIntegerLikePrimitive(prim.PrimitiveName))
        {
            return false;
        }

        return TryAllowNumericLiteralCompatibility(prim.PrimitiveName, expr, out error);
    }

    private static bool IsIntegerLikePrimitive(string primitiveName) =>
        primitiveName is "i32" or "u8" or "u16" or "u32" or "utf8" or "ascii";
}
