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

        var callGraph = new Dictionary<string, HashSet<string>>(StringComparer.Ordinal);
        foreach (var func in functionList)
        {
            var key = CallableIdentity.GetCallableKey(func);
            if (func.Body is null)
            {
                callGraph[key] = new HashSet<string>(StringComparer.Ordinal);
                continue;
            }

            callGraph[key] = CollectCalledFunctions(func.Body, functionsByName);
        }
        foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
        {
            callGraph[test.Name.Text] = CollectCalledFunctions(test.Body, functionsByName);
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

    private static HashSet<string> CollectCalledFunctions(BlockStatementSyntax block, IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions)
    {
        var results = new HashSet<string>(StringComparer.Ordinal);
        CollectFromBlock(block, functions, results);
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
        HashSet<string> results)
    {
        foreach (var stmt in block.Statements)
        {
            CollectFromStatement(stmt, functions, results);
        }
    }

    private static void CollectFromStatement(
        StatementSyntax stmt,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        HashSet<string> results)
    {
        switch (stmt)
        {
            case VariableDeclarationSyntax decl:
                if (decl.Initializer != null)
                {
                    CollectFromExpression(decl.Initializer, functions, results);
                }
                break;
            case ExpressionStatementSyntax exprStmt:
                CollectFromExpression(exprStmt.Expression, functions, results);
                break;
            case ReturnStatementSyntax ret:
                if (ret.Expression != null)
                {
                    CollectFromExpression(ret.Expression, functions, results);
                }
                break;
            case IfStatementSyntax ifStmt:
                CollectFromExpression(ifStmt.Condition, functions, results);
                CollectFromBlock(ifStmt.ThenBlock, functions, results);
                if (ifStmt.ElseBlock != null)
                {
                    CollectFromBlock(ifStmt.ElseBlock, functions, results);
                }
                break;
            case ForStatementSyntax forStmt:
                if (forStmt.Initializer != null)
                {
                    CollectFromExpression(forStmt.Initializer, functions, results);
                }
                if (forStmt.Condition != null)
                {
                    CollectFromExpression(forStmt.Condition, functions, results);
                }
                if (forStmt.Step != null)
                {
                    CollectFromExpression(forStmt.Step, functions, results);
                }
                CollectFromBlock(forStmt.Body, functions, results);
                break;
            case ForeachStatementSyntax foreachStmt:
                CollectFromExpression(foreachStmt.Iterable, functions, results);
                CollectFromBlock(foreachStmt.Body, functions, results);
                break;
            case BlockStatementSyntax inner:
                CollectFromBlock(inner, functions, results);
                break;
        }
    }

    private static void CollectFromExpression(
        ExpressionSyntax expr,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax[]> functions,
        HashSet<string> results)
    {
        switch (expr)
        {
            case ParenthesizedExpressionSyntax paren:
                CollectFromExpression(paren.Expression, functions, results);
                break;
            case UnaryExpressionSyntax unary:
                CollectFromExpression(unary.Operand, functions, results);
                break;
            case MemberAccessExpressionSyntax member:
                CollectFromExpression(member.Receiver, functions, results);
                break;
            case ArrayAccessExpressionSyntax array:
                CollectFromExpression(array.Receiver, functions, results);
                CollectFromExpression(array.Index, functions, results);
                break;
            case AssignmentExpressionSyntax assign:
                CollectFromExpression(assign.Left, functions, results);
                CollectFromExpression(assign.Right, functions, results);
                break;
            case BinaryExpressionSyntax bin:
                CollectFromExpression(bin.Left, functions, results);
                CollectFromExpression(bin.Right, functions, results);
                break;
            case CallExpressionSyntax call:
                if (call.Callee is IdentifierExpressionSyntax id && functions.TryGetValue(id.Identifier.Text, out var candidates))
                {
                    foreach (var candidate in candidates)
                    {
                        results.Add(CallableIdentity.GetCallableKey(candidate));
                    }
                }
                else if (call.Callee is MemberAccessExpressionSyntax member && functions.TryGetValue(member.Member.Text, out var memberCandidates))
                {
                    foreach (var candidate in memberCandidates)
                    {
                        results.Add(CallableIdentity.GetCallableKey(candidate));
                    }
                }
                CollectFromExpression(call.Callee, functions, results);
                foreach (var arg in call.Arguments)
                {
                    CollectFromExpression(arg, functions, results);
                }
                break;
            case OperatorCallExpressionSyntax opCall:
                CollectFromExpression(opCall.Receiver, functions, results);
                foreach (var arg in opCall.Arguments)
                {
                    CollectFromExpression(arg, functions, results);
                }
                break;
        }
    }
}
