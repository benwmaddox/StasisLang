using System.Text;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR.Cranelift;

/// <summary>
/// Builds CLIF (Cranelift IR) function bodies.
/// Handles lowering of Stasis expressions and statements to Cranelift instructions.
/// </summary>
public sealed class CraneliftFunctionBuilder
{
    private readonly StringBuilder _instructions = new();
    private readonly CraneliftTypeMapper _typeMapper;
    private readonly IReadOnlyDictionary<string, Symbol> _symbols;
    private readonly Dictionary<string, int> _locals = new();
    private readonly Dictionary<string, int> _parameters = new();
    private int _valueCounter;
    private int _blockCounter;
    private readonly List<Diagnostic> _diagnostics;

    public CraneliftFunctionBuilder(
        CraneliftTypeMapper typeMapper,
        IReadOnlyDictionary<string, Symbol> symbols,
        List<Diagnostic> diagnostics)
    {
        _typeMapper = typeMapper;
        _symbols = symbols;
        _diagnostics = diagnostics;
    }

    /// <summary>
    /// Generates the CLIF body for a function.
    /// </summary>
    public string BuildFunctionBody(
        FunctionDeclarationSyntax function,
        Symbol functionSymbol)
    {
        _instructions.Clear();
        _locals.Clear();
        _parameters.Clear();
        _blockCounter = 0;

        // Set up parameters - value counter starts after parameters
        for (int i = 0; i < function.Parameters.Count; i++)
        {
            var param = function.Parameters[i];
            _parameters[param.Name.Text] = i;
        }
        _valueCounter = function.Parameters.Count;

        // Create entry block with parameters
        var entryBlock = NewBlock();
        _instructions.AppendLine($"{entryBlock}({FormatBlockParams(function.Parameters)}):");

        // Lower the function body
        LowerBlock(function.Body);

        // Ensure we have a return
        if (!EndsWithReturn(function.Body))
        {
            var returnType = functionSymbol.Type;
            if (returnType is VoidTypeSymbol)
            {
                _instructions.AppendLine("    return");
            }
            else
            {
                var defaultVal = NewValue();
                _instructions.AppendLine($"    {defaultVal} = iconst.i32 0");
                _instructions.AppendLine($"    return {defaultVal}");
            }
        }

        return _instructions.ToString();
    }

    /// <summary>
    /// Generates the CLIF body for a test function.
    /// </summary>
    public string BuildTestBody(TestDeclarationSyntax test)
    {
        _instructions.Clear();
        _locals.Clear();
        _parameters.Clear();
        _valueCounter = 0;
        _blockCounter = 0;

        var entryBlock = NewBlock();
        _instructions.AppendLine($"{entryBlock}:");

        LowerBlock(test.Body);

        if (!EndsWithReturn(test.Body))
        {
            var defaultVal = NewValue();
            _instructions.AppendLine($"    {defaultVal} = iconst.i32 0");
            _instructions.AppendLine($"    return {defaultVal}");
        }

        return _instructions.ToString();
    }

    private void LowerBlock(BlockStatementSyntax block)
    {
        foreach (var stmt in block.Statements)
        {
            LowerStatement(stmt);
        }
    }

    private void LowerStatement(StatementSyntax stmt)
    {
        switch (stmt)
        {
            case VariableDeclarationSyntax varDecl:
                LowerVariableDeclaration(varDecl);
                break;
            case ExpressionStatementSyntax exprStmt:
                LowerExpression(exprStmt.Expression);
                break;
            case ReturnStatementSyntax returnStmt:
                LowerReturn(returnStmt);
                break;
            case IfStatementSyntax ifStmt:
                LowerIf(ifStmt);
                break;
            case ForStatementSyntax forStmt:
                LowerFor(forStmt);
                break;
            case BlockStatementSyntax blockStmt:
                LowerBlock(blockStmt);
                break;
            default:
                _instructions.AppendLine($"    ; TODO: {stmt.GetType().Name}");
                break;
        }
    }

    private void LowerVariableDeclaration(VariableDeclarationSyntax varDecl)
    {
        var varName = varDecl.Name.Text;

        if (varDecl.Initializer != null)
        {
            var initValue = LowerExpression(varDecl.Initializer);
            _locals[varName] = _valueCounter - 1;
            _instructions.AppendLine($"    ; let {varName} = {initValue}");
        }
        else
        {
            // Uninitialized variable - default to 0
            var val = NewValue();
            _instructions.AppendLine($"    {val} = iconst.i32 0 ; let {varName}");
            _locals[varName] = _valueCounter - 1;
        }
    }

    private void LowerReturn(ReturnStatementSyntax returnStmt)
    {
        if (returnStmt.Expression != null)
        {
            var retVal = LowerExpression(returnStmt.Expression);
            _instructions.AppendLine($"    return {retVal}");
        }
        else
        {
            _instructions.AppendLine("    return");
        }
    }

    private void LowerIf(IfStatementSyntax ifStmt)
    {
        var condVal = LowerExpression(ifStmt.Condition);
        var thenBlock = NewBlock();
        var elseBlock = ifStmt.ElseBlock != null ? NewBlock() : null;
        var mergeBlock = NewBlock();

        if (elseBlock != null)
        {
            _instructions.AppendLine($"    brif {condVal}, {thenBlock}, {elseBlock}");
        }
        else
        {
            _instructions.AppendLine($"    brif {condVal}, {thenBlock}, {mergeBlock}");
        }

        // Then block
        _instructions.AppendLine($"{thenBlock}:");
        LowerBlock(ifStmt.ThenBlock);
        if (!EndsWithReturn(ifStmt.ThenBlock))
        {
            _instructions.AppendLine($"    jump {mergeBlock}");
        }

        // Else block
        if (elseBlock != null && ifStmt.ElseBlock != null)
        {
            _instructions.AppendLine($"{elseBlock}:");
            LowerBlock(ifStmt.ElseBlock);
            if (!EndsWithReturn(ifStmt.ElseBlock))
            {
                _instructions.AppendLine($"    jump {mergeBlock}");
            }
        }

        // Merge block
        _instructions.AppendLine($"{mergeBlock}:");
    }

    private void LowerFor(ForStatementSyntax forStmt)
    {
        // Initialize
        if (forStmt.Initializer != null)
        {
            LowerExpression(forStmt.Initializer);
        }

        var condBlock = NewBlock();
        var bodyBlock = NewBlock();
        var endBlock = NewBlock();

        _instructions.AppendLine($"    jump {condBlock}");

        // Condition block
        _instructions.AppendLine($"{condBlock}:");
        if (forStmt.Condition != null)
        {
            var condVal = LowerExpression(forStmt.Condition);
            _instructions.AppendLine($"    brif {condVal}, {bodyBlock}, {endBlock}");
        }
        else
        {
            _instructions.AppendLine($"    jump {bodyBlock}");
        }

        // Body block
        _instructions.AppendLine($"{bodyBlock}:");
        LowerBlock(forStmt.Body);

        // Step (increment)
        if (forStmt.Step != null)
        {
            LowerExpression(forStmt.Step);
        }

        _instructions.AppendLine($"    jump {condBlock}");

        // End block
        _instructions.AppendLine($"{endBlock}:");
    }

    private string LowerExpression(ExpressionSyntax expr)
    {
        switch (expr)
        {
            case LiteralExpressionSyntax lit:
                return LowerLiteral(lit);
            case IdentifierExpressionSyntax id:
                return LowerIdentifier(id);
            case BinaryExpressionSyntax bin:
                return LowerBinary(bin);
            case UnaryExpressionSyntax unary:
                return LowerUnary(unary);
            case CallExpressionSyntax call:
                return LowerCall(call);
            case ParenthesizedExpressionSyntax paren:
                return LowerExpression(paren.Expression);
            case AssignmentExpressionSyntax assign:
                return LowerAssignment(assign);
            case MemberAccessExpressionSyntax member:
                return LowerMemberAccess(member);
            case ArrayAccessExpressionSyntax array:
                return LowerArrayAccess(array);
            default:
                var val = NewValue();
                _instructions.AppendLine($"    {val} = iconst.i32 0 ; TODO: {expr.GetType().Name}");
                return val;
        }
    }

    private string LowerLiteral(LiteralExpressionSyntax lit)
    {
        var val = NewValue();
        var text = lit.Literal.Text;

        if (lit.Literal.Kind == TokenKind.IntegerLiteral)
        {
            _instructions.AppendLine($"    {val} = iconst.i32 {text}");
        }
        else if (lit.Literal.Kind == TokenKind.FloatLiteral)
        {
            _instructions.AppendLine($"    {val} = f32const {text}");
        }
        else if (lit.Literal.Kind == TokenKind.TrueKeyword)
        {
            _instructions.AppendLine($"    {val} = iconst.i32 1");
        }
        else if (lit.Literal.Kind == TokenKind.FalseKeyword)
        {
            _instructions.AppendLine($"    {val} = iconst.i32 0");
        }
        else
        {
            // String or other literal
            _instructions.AppendLine($"    {val} = iconst.i64 0 ; TODO: string literal");
        }

        return val;
    }

    private string LowerIdentifier(IdentifierExpressionSyntax id)
    {
        var name = id.Identifier.Text;

        // Check parameters first
        if (_parameters.TryGetValue(name, out var paramIndex))
        {
            return $"v{paramIndex}";
        }

        // Check locals
        if (_locals.TryGetValue(name, out var localIndex))
        {
            return $"v{localIndex}";
        }

        // Must be a global - emit a load
        var val = NewValue();
        _instructions.AppendLine($"    {val} = iconst.i32 0 ; TODO: load global {name}");
        return val;
    }

    private string LowerBinary(BinaryExpressionSyntax bin)
    {
        var left = LowerExpression(bin.Left);
        var right = LowerExpression(bin.Right);
        var result = NewValue();

        var op = bin.OperatorToken.Kind switch
        {
            TokenKind.Plus => "iadd",
            TokenKind.Minus => "isub",
            TokenKind.Star => "imul",
            TokenKind.Slash => "sdiv",
            TokenKind.Percent => "srem",
            TokenKind.Less => "icmp slt",
            TokenKind.LessEqual => "icmp sle",
            TokenKind.Greater => "icmp sgt",
            TokenKind.GreaterEqual => "icmp sge",
            TokenKind.EqualEqual => "icmp eq",
            TokenKind.BangEqual => "icmp ne",
            TokenKind.AmpAmp => "band",
            TokenKind.PipePipe => "bor",
            _ => "iadd"
        };

        if (op.StartsWith("icmp"))
        {
            var cmpOp = op.Replace("icmp ", "");
            _instructions.AppendLine($"    {result} = icmp {cmpOp} {left}, {right}");
        }
        else
        {
            _instructions.AppendLine($"    {result} = {op} {left}, {right}");
        }

        return result;
    }

    private string LowerUnary(UnaryExpressionSyntax unary)
    {
        var operand = LowerExpression(unary.Operand);
        var result = NewValue();

        if (unary.OperatorToken.Kind == TokenKind.Minus)
        {
            var zero = NewValue();
            _instructions.AppendLine($"    {zero} = iconst.i32 0");
            _instructions.AppendLine($"    {result} = isub {zero}, {operand}");
        }
        else if (unary.OperatorToken.Kind == TokenKind.Bang)
        {
            var zero = NewValue();
            _instructions.AppendLine($"    {zero} = iconst.i32 0");
            _instructions.AppendLine($"    {result} = icmp eq {operand}, {zero}");
        }
        else
        {
            _instructions.AppendLine($"    {result} = {operand} ; TODO: unary {unary.OperatorToken.Kind}");
        }

        return result;
    }

    private string LowerCall(CallExpressionSyntax call)
    {
        // Get function name
        string funcName;
        if (call.Callee is IdentifierExpressionSyntax id)
        {
            funcName = id.Identifier.Text;
        }
        else
        {
            funcName = "unknown";
        }

        // Lower arguments first
        var args = new List<string>();
        foreach (var arg in call.Arguments)
        {
            args.Add(LowerExpression(arg));
        }

        // Then create result value
        var result = NewValue();
        var argList = string.Join(", ", args);
        _instructions.AppendLine($"    {result} = call %{funcName}({argList})");

        return result;
    }

    private string LowerAssignment(AssignmentExpressionSyntax assign)
    {
        var value = LowerExpression(assign.Right);

        if (assign.Left is IdentifierExpressionSyntax id)
        {
            var name = id.Identifier.Text;
            if (_locals.ContainsKey(name))
            {
                // Update local reference
                _locals[name] = _valueCounter - 1;
            }
            else
            {
                // Store to global
                _instructions.AppendLine($"    ; TODO: store to global {name}");
            }
        }
        else
        {
            _instructions.AppendLine($"    ; TODO: complex assignment");
        }

        return value;
    }

    private string LowerMemberAccess(MemberAccessExpressionSyntax member)
    {
        var result = NewValue();

        if (member.Member.Text == "length")
        {
            // Array length - try to resolve statically
            _instructions.AppendLine($"    {result} = iconst.i32 0 ; TODO: array length");
        }
        else
        {
            // Struct field access
            _instructions.AppendLine($"    {result} = iconst.i32 0 ; TODO: member access .{member.Member.Text}");
        }

        return result;
    }

    private string LowerArrayAccess(ArrayAccessExpressionSyntax array)
    {
        var result = NewValue();
        var index = LowerExpression(array.Index);

        _instructions.AppendLine($"    {result} = iconst.i32 0 ; TODO: array access [{index}]");

        return result;
    }

    private string NewValue() => $"v{_valueCounter++}";
    private string NewBlock() => $"block{_blockCounter++}";

    private string FormatBlockParams(IReadOnlyList<ParameterSyntax> parameters)
    {
        var parts = new List<string>();
        for (int i = 0; i < parameters.Count; i++)
        {
            var param = parameters[i];
            var type = "i32"; // Default to i32
            if (_symbols.TryGetValue(param.Name.Text, out var symbol) && symbol.Type != null)
            {
                type = FormatType(_typeMapper.Map(symbol.Type));
            }
            parts.Add($"v{i}: {type}");
        }
        return string.Join(", ", parts);
    }

    private static string FormatType(CraneliftTypeMapper.ClifType type) =>
        type switch
        {
            CraneliftTypeMapper.ClifType.I8 => "i8",
            CraneliftTypeMapper.ClifType.I16 => "i16",
            CraneliftTypeMapper.ClifType.I32 => "i32",
            CraneliftTypeMapper.ClifType.I64 => "i64",
            CraneliftTypeMapper.ClifType.F32 => "f32",
            CraneliftTypeMapper.ClifType.F64 => "f64",
            CraneliftTypeMapper.ClifType.B1 => "b1",
            CraneliftTypeMapper.ClifType.R64 => "r64",
            _ => "i32"
        };

    private static bool EndsWithReturn(BlockStatementSyntax block)
    {
        if (block.Statements.Count == 0)
            return false;

        var lastStmt = block.Statements[^1];
        return lastStmt is ReturnStatementSyntax;
    }
}
