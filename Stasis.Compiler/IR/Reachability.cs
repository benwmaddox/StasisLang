using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR;

public static class Reachability
{
    public static HashSet<string> CollectReachableFunctions(CompilationUnitSyntax compilationUnit, bool includeTests, bool allowFallback)
    {
        var functionList = compilationUnit.Declarations
            .OfType<FunctionDeclarationSyntax>()
            .ToList();
        var functionsByKey = functionList
            .ToDictionary(CallableIdentity.GetCallableKey, fn => fn, StringComparer.Ordinal);
        var functionsByName = functionList
            .GroupBy(fn => fn.Name.Text, StringComparer.Ordinal)
            .ToDictionary(g => g.Key, g => g.ToArray(), StringComparer.Ordinal);
        var globalsByName = compilationUnit.Declarations
            .OfType<GlobalDeclarationSyntax>()
            .ToDictionary(g => g.Name.Text, g => g.Type, StringComparer.Ordinal);
        var structsByName = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);

        var callGraph = new Dictionary<string, HashSet<string>>(StringComparer.Ordinal);
        foreach (var func in functionList)
        {
            var key = CallableIdentity.GetCallableKey(func);
            if (func.Body is null)
            {
                callGraph[key] = new HashSet<string>(StringComparer.Ordinal);
                continue;
            }

            var locals = func.Parameters.ToDictionary(p => p.Name.Text, p => p.Type, StringComparer.Ordinal);
            callGraph[key] = CollectCalledFunctions(func.Body, functionsByName, globalsByName, structsByName, locals);
        }

        foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
        {
            var locals = test.Parameters.ToDictionary(p => p.Name.Text, p => p.Type, StringComparer.Ordinal);
            callGraph[test.Name.Text] = CollectCalledFunctions(test.Body, functionsByName, globalsByName, structsByName, locals);
        }

        var reachable = new HashSet<string>(StringComparer.Ordinal);
        var queue = new Queue<string>();

        if (!includeTests)
        {
            if (TryGetReceiverlessCallable(functionsByName, "main", out var mainKey))
            {
                queue.Enqueue(mainKey);
            }

            // Tick hosting: if a program defines `tick`, treat it as an entrypoint alongside `main`.
            if (TryGetReceiverlessCallable(functionsByName, "tick", out var tickKey))
            {
                queue.Enqueue(tickKey);
            }

            foreach (var export in functionList.Where(fn => fn.IsExported))
            {
                queue.Enqueue(CallableIdentity.GetCallableKey(export));
            }
        }

        if (includeTests)
        {
            foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
            {
                queue.Enqueue(test.Name.Text);
            }
        }

        if (queue.Count == 0 && allowFallback)
        {
            return new HashSet<string>(functionsByKey.Keys, StringComparer.Ordinal);
        }

        while (queue.Count > 0)
        {
            var name = queue.Dequeue();
            if (!reachable.Add(name))
            {
                continue;
            }

            if (callGraph.TryGetValue(name, out var callees))
            {
                foreach (var callee in callees)
                {
                    queue.Enqueue(callee);
                }
            }
        }

        return reachable;
    }

    private static HashSet<string> CollectCalledFunctions(
        BlockStatementSyntax block,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        IReadOnlyDictionary<string, TypeSyntax> initialLocals)
    {
        var results = new HashSet<string>(StringComparer.Ordinal);
        var locals = new Dictionary<string, TypeSyntax>(initialLocals, StringComparer.Ordinal);
        CollectFromBlock(block, functions, globals, structs, locals, results);
        return results;
    }

    private static bool TryGetReceiverlessCallable(
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functionsByName,
        string name,
        out string callableKey)
    {
        callableKey = string.Empty;
        if (!functionsByName.TryGetValue(name, out var funcs))
        {
            return false;
        }

        var receiverless = funcs.FirstOrDefault(f => !CallableIdentity.HasReceiver(f));
        if (receiverless is null)
        {
            return false;
        }

        callableKey = CallableIdentity.GetCallableKey(receiverless);
        return true;
    }

    private static void CollectFromBlock(
        BlockStatementSyntax block,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        Dictionary<string, TypeSyntax> locals,
        HashSet<string> results)
    {
        foreach (var stmt in block.Statements)
        {
            CollectFromStatement(stmt, functions, globals, structs, locals, results);
        }
    }

    private static void CollectFromStatement(
        StatementSyntax stmt,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        Dictionary<string, TypeSyntax> locals,
        HashSet<string> results)
    {
        switch (stmt)
        {
            case VariableDeclarationSyntax decl:
                if (decl.Initializer != null)
                {
                    CollectFromExpression(decl.Initializer, functions, globals, structs, locals, results);
                }

                if (decl.Type is not null)
                {
                    locals[decl.Name.Text] = decl.Type;
                }
                break;
            case ExpressionStatementSyntax exprStmt:
                CollectFromExpression(exprStmt.Expression, functions, globals, structs, locals, results);
                break;
            case ReturnStatementSyntax ret:
                if (ret.Expression != null)
                {
                    CollectFromExpression(ret.Expression, functions, globals, structs, locals, results);
                }
                break;
            case IfStatementSyntax ifStmt:
                CollectFromExpression(ifStmt.Condition, functions, globals, structs, locals, results);
                CollectFromBlock(ifStmt.ThenBlock, functions, globals, structs, new Dictionary<string, TypeSyntax>(locals, StringComparer.Ordinal), results);
                if (ifStmt.ElseBlock != null)
                {
                    CollectFromBlock(ifStmt.ElseBlock, functions, globals, structs, new Dictionary<string, TypeSyntax>(locals, StringComparer.Ordinal), results);
                }
                break;
            case ForStatementSyntax forStmt:
                var forLocals = new Dictionary<string, TypeSyntax>(locals, StringComparer.Ordinal);
                if (forStmt.Initializer != null)
                {
                    CollectFromExpression(forStmt.Initializer, functions, globals, structs, forLocals, results);
                }
                if (forStmt.Condition != null)
                {
                    CollectFromExpression(forStmt.Condition, functions, globals, structs, forLocals, results);
                }
                if (forStmt.Step != null)
                {
                    CollectFromExpression(forStmt.Step, functions, globals, structs, forLocals, results);
                }
                CollectFromBlock(forStmt.Body, functions, globals, structs, new Dictionary<string, TypeSyntax>(forLocals, StringComparer.Ordinal), results);
                break;
            case ForeachStatementSyntax foreachStmt:
                CollectFromExpression(foreachStmt.Iterable, functions, globals, structs, locals, results);
                var foreachLocals = new Dictionary<string, TypeSyntax>(locals, StringComparer.Ordinal);
                if (TryResolveExpressionType(foreachStmt.Iterable, functions, foreachLocals, globals, structs, out var iterableType) &&
                    iterableType is ArrayTypeSyntax arrayType)
                {
                    foreachLocals[foreachStmt.Iterator.Text] = arrayType.ElementType;
                    if (foreachStmt.IndexVariable is not null)
                    {
                        foreachLocals[foreachStmt.IndexVariable.Text] = CreateNamedType("i32");
                    }
                }
                CollectFromBlock(foreachStmt.Body, functions, globals, structs, foreachLocals, results);
                break;
            case BlockStatementSyntax inner:
                CollectFromBlock(inner, functions, globals, structs, new Dictionary<string, TypeSyntax>(locals, StringComparer.Ordinal), results);
                break;
        }
    }

    private static void CollectFromExpression(
        ExpressionSyntax expr,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        Dictionary<string, TypeSyntax> locals,
        HashSet<string> results)
    {
        switch (expr)
        {
            case ParenthesizedExpressionSyntax paren:
                CollectFromExpression(paren.Expression, functions, globals, structs, locals, results);
                break;
            case UnaryExpressionSyntax unary:
                CollectFromExpression(unary.Operand, functions, globals, structs, locals, results);
                break;
            case MemberAccessExpressionSyntax member:
                CollectFromExpression(member.Receiver, functions, globals, structs, locals, results);
                break;
            case ArrayAccessExpressionSyntax array:
                CollectFromExpression(array.Receiver, functions, globals, structs, locals, results);
                CollectFromExpression(array.Index, functions, globals, structs, locals, results);
                break;
            case AssignmentExpressionSyntax assign:
                CollectFromExpression(assign.Left, functions, globals, structs, locals, results);
                CollectFromExpression(assign.Right, functions, globals, structs, locals, results);
                break;
            case BinaryExpressionSyntax bin:
                CollectFromExpression(bin.Left, functions, globals, structs, locals, results);
                CollectFromExpression(bin.Right, functions, globals, structs, locals, results);
                break;
            case CallExpressionSyntax call:
                foreach (var candidate in ResolveCallCandidates(call, functions, globals, structs, locals))
                {
                    results.Add(CallableIdentity.GetCallableKey(candidate));
                }

                CollectFromExpression(call.Callee, functions, globals, structs, locals, results);
                foreach (var arg in call.Arguments)
                {
                    CollectFromExpression(arg, functions, globals, structs, locals, results);
                }
                break;
            case OperatorCallExpressionSyntax opCall:
                CollectFromExpression(opCall.Receiver, functions, globals, structs, locals, results);
                foreach (var arg in opCall.Arguments)
                {
                    CollectFromExpression(arg, functions, globals, structs, locals, results);
                }
                break;
        }
    }

    private static FunctionDeclarationSyntax[] ResolveCallCandidates(
        CallExpressionSyntax call,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        IReadOnlyDictionary<string, TypeSyntax> locals)
    {
        switch (call.Callee)
        {
            case IdentifierExpressionSyntax id when locals.ContainsKey(id.Identifier.Text):
                return Array.Empty<FunctionDeclarationSyntax>();
            case IdentifierExpressionSyntax id when functions.TryGetValue(id.Identifier.Text, out var candidates):
                return ResolveFunctionFormCandidates(candidates, call.Arguments, functions, globals, structs, locals);
            case MemberAccessExpressionSyntax member when string.Equals(member.Member.Text, "clear", StringComparison.Ordinal):
                return Array.Empty<FunctionDeclarationSyntax>();
            case MemberAccessExpressionSyntax member when functions.TryGetValue(member.Member.Text, out var memberCandidates):
                return ResolveReceiverFormCandidates(member, call.Arguments.Count, memberCandidates, functions, globals, structs, locals);
            default:
                return Array.Empty<FunctionDeclarationSyntax>();
        }
    }

    private static FunctionDeclarationSyntax[] ResolveFunctionFormCandidates(
        FunctionDeclarationSyntax[] candidates,
        IReadOnlyList<ExpressionSyntax> arguments,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        IReadOnlyDictionary<string, TypeSyntax> locals)
    {
        var arityMatches = candidates.Where(fn => fn.Parameters.Count == arguments.Count).ToArray();
        if (arityMatches.Length == 0)
        {
            return Array.Empty<FunctionDeclarationSyntax>();
        }

        if (arguments.Count > 0)
        {
            if (TryResolveExpressionType(arguments[0], functions, locals, globals, structs, out var firstArgType))
            {
                var firstArgKey = CallableIdentity.TypeKey(firstArgType);
                var receiverMatches = arityMatches
                    .Where(fn => fn.Parameters.Count > 0 &&
                                 string.Equals(CallableIdentity.TypeKey(fn.Parameters[0].Type), firstArgKey, StringComparison.Ordinal))
                    .ToArray();
                if (receiverMatches.Length > 0)
                {
                    return receiverMatches;
                }
            }

            // Keep same-arity candidates when first-arg typing is unavailable or inconclusive.
            // Reachability must stay conservative to avoid dropping required overloads.
            return arityMatches;
        }

        var receiverlessMatches = arityMatches.Where(fn => !CallableIdentity.HasReceiver(fn)).ToArray();
        if (receiverlessMatches.Length > 0)
        {
            return receiverlessMatches;
        }

        return arityMatches;
    }

    private static FunctionDeclarationSyntax[] ResolveReceiverFormCandidates(
        MemberAccessExpressionSyntax member,
        int argumentCount,
        FunctionDeclarationSyntax[] candidates,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        IReadOnlyDictionary<string, TypeSyntax> locals)
    {
        var arityMatches = candidates.Where(fn => fn.Parameters.Count == argumentCount + 1).ToArray();
        if (arityMatches.Length == 0)
        {
            return Array.Empty<FunctionDeclarationSyntax>();
        }

        if (TryResolveExpressionType(member.Receiver, functions, locals, globals, structs, out var receiverType))
        {
            var receiverKey = CallableIdentity.TypeKey(receiverType);
            var receiverMatches = arityMatches
                .Where(fn => fn.Parameters.Count > 0 &&
                             string.Equals(CallableIdentity.TypeKey(fn.Parameters[0].Type), receiverKey, StringComparison.Ordinal))
                .ToArray();
            if (receiverMatches.Length > 0)
            {
                return receiverMatches;
            }
        }

        return arityMatches;
    }

    private static bool TryResolveExpressionType(
        ExpressionSyntax expr,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        IReadOnlyDictionary<string, TypeSyntax> locals,
        IReadOnlyDictionary<string, TypeSyntax> globals,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        out TypeSyntax type)
    {
        switch (expr)
        {
            case ParenthesizedExpressionSyntax paren:
                return TryResolveExpressionType(paren.Expression, functions, locals, globals, structs, out type);
            case UnaryExpressionSyntax unary:
                return TryResolveExpressionType(unary.Operand, functions, locals, globals, structs, out type);
            case AssignmentExpressionSyntax assign:
                return TryResolveExpressionType(assign.Right, functions, locals, globals, structs, out type);
            case IdentifierExpressionSyntax id:
                if (locals.TryGetValue(id.Identifier.Text, out var localType))
                {
                    type = localType;
                    return true;
                }

                if (globals.TryGetValue(id.Identifier.Text, out var globalType))
                {
                    type = globalType;
                    return true;
                }

                break;
            case LiteralExpressionSyntax lit:
                switch (lit.Literal.Kind)
                {
                    case TokenKind.IntegerLiteral:
                        type = CreateNamedType("i32");
                        return true;
                    case TokenKind.U8Literal:
                        type = CreateNamedType("u8");
                        return true;
                    case TokenKind.FloatLiteral:
                        type = CreateNamedType("f32");
                        return true;
                    case TokenKind.StringLiteral:
                    case TokenKind.BacktickLiteral:
                        type = CreateNamedType("string");
                        return true;
                    case TokenKind.TrueKeyword:
                    case TokenKind.FalseKeyword:
                        type = CreateNamedType("bool");
                        return true;
                }
                break;
            case BinaryExpressionSyntax bin:
                {
                    if (bin.OperatorToken.Kind is TokenKind.EqualEqual or TokenKind.BangEqual or
                        TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual or
                        TokenKind.AmpAmp or TokenKind.PipePipe)
                    {
                        type = CreateNamedType("bool");
                        return true;
                    }

                    if (TryResolveExpressionType(bin.Left, functions, locals, globals, structs, out var leftType) &&
                        TryResolveExpressionType(bin.Right, functions, locals, globals, structs, out var rightType))
                    {
                        var leftKey = CallableIdentity.TypeKey(leftType);
                        var rightKey = CallableIdentity.TypeKey(rightType);
                        if (leftKey == "f32" || rightKey == "f32")
                        {
                            type = CreateNamedType("f32");
                            return true;
                        }

                        type = leftType;
                        return true;
                    }

                    break;
                }
            case OperatorCallExpressionSyntax op:
                {
                    var opText = op.OperatorToken.Text;
                    if (opText is "==" or "!=" or "<" or "<=" or ">" or ">=" or "&&" or "||")
                    {
                        type = CreateNamedType("bool");
                        return true;
                    }

                    return TryResolveExpressionType(op.Receiver, functions, locals, globals, structs, out type);
                }
            case ArrayAccessExpressionSyntax array:
                if (TryResolveExpressionType(array.Receiver, functions, locals, globals, structs, out var arrayType) &&
                    arrayType is ArrayTypeSyntax arraySyntax)
                {
                    type = arraySyntax.ElementType;
                    return true;
                }
                break;
            case MemberAccessExpressionSyntax member:
                if (TryResolveExpressionType(member.Receiver, functions, locals, globals, structs, out var receiverType))
                {
                    if (receiverType is NamedTypeSyntax named &&
                        structs.TryGetValue(named.Name, out var structDecl))
                    {
                        var field = structDecl.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, member.Member.Text, StringComparison.Ordinal));
                        if (field is not null)
                        {
                            type = field.Type;
                            return true;
                        }
                    }

                    if (receiverType is ArrayTypeSyntax && string.Equals(member.Member.Text, "length", StringComparison.Ordinal))
                    {
                        type = CreateNamedType("i32");
                        return true;
                    }
                }
                break;
            case CallExpressionSyntax call:
                {
                    var callCandidates = ResolveCallCandidates(call, functions, globals, structs, locals);
                    if (callCandidates.Length == 1)
                    {
                        var returnType = callCandidates[0].ReturnType;
                        if (returnType is not null)
                        {
                            type = returnType;
                            return true;
                        }
                    }

                    break;
                }
        }

        type = null!;
        return false;
    }

    private static NamedTypeSyntax CreateNamedType(string name) =>
        new(new Token(TokenKind.Identifier, name, new SourceSpan(0, 0)));
}
