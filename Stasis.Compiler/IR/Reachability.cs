using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR;

public static class Reachability
{
    public static HashSet<string> CollectReachableFunctions(CompilationUnitSyntax compilationUnit, bool includeTests, bool allowFallback)
    {
        var functions = compilationUnit.Declarations
            .OfType<FunctionDeclarationSyntax>()
            .ToDictionary(fn => fn.Name.Text, fn => fn, StringComparer.Ordinal);

        var callGraph = new Dictionary<string, HashSet<string>>(StringComparer.Ordinal);
        foreach (var (name, func) in functions)
        {
            if (func.Body is null)
            {
                callGraph[name] = new HashSet<string>(StringComparer.Ordinal);
                continue;
            }

            callGraph[name] = CollectCalledFunctions(func.Body, functions);
        }
        foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
        {
            callGraph[test.Name.Text] = CollectCalledFunctions(test.Body, functions);
        }

        var reachable = new HashSet<string>(StringComparer.Ordinal);
        var queue = new Queue<string>();

        if (!includeTests)
        {
            if (functions.ContainsKey("main"))
            {
                queue.Enqueue("main");
            }

            // Tick hosting: if a program defines `tick`, treat it as an entrypoint alongside `main`.
            if (functions.ContainsKey("tick"))
            {
                queue.Enqueue("tick");
            }

            foreach (var export in functions.Values.Where(fn => fn.IsExported))
            {
                queue.Enqueue(export.Name.Text);
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
            return new HashSet<string>(functions.Keys, StringComparer.Ordinal);
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

    private static HashSet<string> CollectCalledFunctions(BlockStatementSyntax block, IReadOnlyDictionary<string, FunctionDeclarationSyntax> functions)
    {
        var results = new HashSet<string>(StringComparer.Ordinal);
        CollectFromBlock(block, functions, results);
        return results;
    }

    private static void CollectFromBlock(
        BlockStatementSyntax block,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax> functions,
        HashSet<string> results)
    {
        foreach (var stmt in block.Statements)
        {
            CollectFromStatement(stmt, functions, results);
        }
    }

    private static void CollectFromStatement(
        StatementSyntax stmt,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax> functions,
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
        IReadOnlyDictionary<string, FunctionDeclarationSyntax> functions,
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
                if (call.Callee is IdentifierExpressionSyntax id && functions.ContainsKey(id.Identifier.Text))
                {
                    results.Add(id.Identifier.Text);
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
