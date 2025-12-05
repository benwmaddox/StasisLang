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

    public SemanticResult Analyze(CompilationUnitSyntax compilationUnit)
    {
        DeclareBuiltIns();
        DeclareTypes(compilationUnit);
        DeclareGlobals(compilationUnit);
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
    }

    private void DeclareTypes(CompilationUnitSyntax compilationUnit)
    {
        foreach (var decl in compilationUnit.Declarations)
        {
            switch (decl)
            {
                case StructDeclarationSyntax s:
                    AddSymbol(s.Name.Text, SymbolKind.Struct, new NamedTypeSymbol(s.Name.Text), s.Name.Span);
                    break;
                case EnumDeclarationSyntax e:
                    AddSymbol(e.Name.Text, SymbolKind.Enum, new NamedTypeSymbol(e.Name.Text), e.Name.Span);
                    break;
            }
        }
    }

    private void DeclareGlobals(CompilationUnitSyntax compilationUnit)
    {
        foreach (var decl in compilationUnit.Declarations.OfType<GlobalDeclarationSyntax>())
        {
            var type = ResolveType(decl.Type);
            AddSymbol(decl.Name.Text, SymbolKind.Global, type, decl.Name.Span);
            EnsureGlobalType(type, decl.Type.Span);
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
                var iteratorType = BuiltInTypes["i32"];
                AddLocal(foreachScope, fes.Iterator.Text, SymbolKind.Local, iteratorType, fes.Iterator.Span);
                EnsurePrimitiveLocal(iteratorType, fes.Iterator.Span);
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
            _diagnostics.Add(new Diagnostic("Local variables must declare a type; initialization uses .=( ).", v.Name.Span));
        }
        else
        {
            var type = ResolveType(v.Type);
            AddLocal(scope, v.Name.Text, SymbolKind.Local, type, v.Name.Span);
            EnsurePrimitiveLocal(type, v.Name.Span);
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
        }
    }

    private void ValidateOperatorCall(OperatorCallExpressionSyntax op)
    {
        var opText = op.OperatorToken.Text;
        if (opText == "=")
        {
            if (op.Arguments.Count != 1)
            {
                _diagnostics.Add(new Diagnostic("Assignment operator .=( ) requires exactly one argument.", op.Span));
            }

            if (!IsAssignableReceiver(op.Receiver))
            {
                _diagnostics.Add(new Diagnostic("Left side of .=( ) must be an assignable location (identifier, field, or array element).", op.Receiver.Span));
            }
        }
        else
        {
            if (op.Arguments.Count != 1)
            {
                _diagnostics.Add(new Diagnostic($"Operator '.{opText}()' requires exactly one argument.", op.Span));
            }
        }
    }

    private bool IsAssignableReceiver(ExpressionSyntax receiver) =>
        receiver is IdentifierExpressionSyntax
        or MemberAccessExpressionSyntax
        or ArrayAccessExpressionSyntax;

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
                if (int.TryParse(array.SizeText, out var size) && size > 0)
                {
                    return elementType is null ? null : new ArrayTypeSymbol(elementType, size);
                }

                _diagnostics.Add(new Diagnostic("Array size must be a positive integer literal.", array.SizeToken.Span));
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

        _diagnostics.Add(new Diagnostic("Locals and parameters must be primitive types; structs/arrays live in static memory.", span));
    }

    private void EnsureGlobalType(TypeSymbol? type, SourceSpan span)
    {
        if (type is null)
        {
            return;
        }

        if (type is PrimitiveTypeSymbol or NamedTypeSymbol or ArrayTypeSymbol)
        {
            return;
        }

        _diagnostics.Add(new Diagnostic("Globals must be primitive, struct, or array types.", span));
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
