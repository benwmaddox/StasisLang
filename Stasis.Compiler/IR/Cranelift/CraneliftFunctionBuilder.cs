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
    private readonly IReadOnlyDictionary<string, StructDeclarationSyntax> _structs;
    private readonly IReadOnlyDictionary<string, CraneliftTypeMapper.ClifType> _globalTypes;
    private readonly IReadOnlyDictionary<string, string> _stringLiterals;
    private readonly Layout.LayoutPlan _layoutPlan;
    private readonly Dictionary<string, LocalSlot> _locals = new();
    private readonly Dictionary<string, TypeSymbol> _localTypes = new();
    private int _valueCounter;
    private int _blockCounter;
    private readonly List<Diagnostic> _diagnostics;

    public CraneliftFunctionBuilder(
        CraneliftTypeMapper typeMapper,
        IReadOnlyDictionary<string, Symbol> symbols,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        IReadOnlyDictionary<string, CraneliftTypeMapper.ClifType> globalTypes,
        IReadOnlyDictionary<string, string> stringLiterals,
        Layout.LayoutPlan layoutPlan,
        List<Diagnostic> diagnostics)
    {
        _typeMapper = typeMapper;
        _symbols = symbols;
        _structs = structs;
        _globalTypes = globalTypes;
        _stringLiterals = stringLiterals;
        _layoutPlan = layoutPlan;
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
        _localTypes.Clear();
        _blockCounter = 0;

        _valueCounter = function.Parameters.Count;

        // Create entry block with parameters
        var entryBlock = NewBlock();
        _instructions.AppendLine($"{entryBlock}({FormatBlockParams(function.Parameters)}):");

        // Materialize parameters into stack slots for stable references across control flow.
        for (int i = 0; i < function.Parameters.Count; i++)
        {
            var param = function.Parameters[i];
            var paramType = ResolveType(param.Type);
            var clifType = NormalizeLocalStorageType(_typeMapper.Map(paramType));
            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = stack_slot.{FormatType(clifType)}");
            _instructions.AppendLine($"    store v{i}, {addr}");
            _locals[param.Name.Text] = new LocalSlot(addr, clifType);
            _localTypes[param.Name.Text] = paramType;
        }

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
        _localTypes.Clear();
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
        var varType = varDecl.Type is not null
            ? ResolveType(varDecl.Type)
            : new PrimitiveTypeSymbol("i32");
        var clifType = NormalizeLocalStorageType(_typeMapper.Map(varType));

        var addr = NewValue();
        _instructions.AppendLine($"    {addr} = stack_slot.{FormatType(clifType)}");
        _locals[varName] = new LocalSlot(addr, clifType);
        _localTypes[varName] = varType;

        if (varDecl.Initializer != null)
        {
            var initValue = LowerExpression(varDecl.Initializer);
            var initType = GetExpressionType(varDecl.Initializer);
            initValue = CoerceAssignmentValue(initValue, initType, varType);
            _instructions.AppendLine($"    store {initValue}, {addr}");
            _instructions.AppendLine($"    ; let {varName} = {initValue}");
        }
        else
        {
            // Uninitialized variable - default to 0
            var val = NewValue();
            _instructions.AppendLine($"    {val} = iconst.i32 0 ; let {varName}");
            _instructions.AppendLine($"    store {val}, {addr}");
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
        var condBool = CoerceI32ToB1(condVal);
        var thenBlock = NewBlock();
        var elseBlock = ifStmt.ElseBlock != null ? NewBlock() : null;
        var mergeBlock = NewBlock();

        if (elseBlock != null)
        {
            _instructions.AppendLine($"    brif {condBool}, {thenBlock}, {elseBlock}");
        }
        else
        {
            _instructions.AppendLine($"    brif {condBool}, {thenBlock}, {mergeBlock}");
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
            var condBool = CoerceI32ToB1(condVal);
            _instructions.AppendLine($"    brif {condBool}, {bodyBlock}, {endBlock}");
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
        var literalText = lit.Literal.Text;

        if (lit.Literal.Kind == TokenKind.IntegerLiteral)
        {
            _instructions.AppendLine($"    {val} = iconst.i32 {literalText}");
        }
        else if (lit.Literal.Kind == TokenKind.FloatLiteral)
        {
            _instructions.AppendLine($"    {val} = f32const {literalText}");
        }
        else if (lit.Literal.Kind == TokenKind.TrueKeyword)
        {
            _instructions.AppendLine($"    {val} = iconst.i32 1");
        }
        else if (lit.Literal.Kind == TokenKind.FalseKeyword)
        {
            _instructions.AppendLine($"    {val} = iconst.i32 0");
        }
        else if (lit.Literal.Kind == TokenKind.StringLiteral)
        {
            var text = UnescapeString(lit.Literal.Text);
            if (_stringLiterals.TryGetValue(text, out var globalName))
            {
                _instructions.AppendLine($"    {val} = global_value {globalName}");
            }
            else
            {
                _diagnostics.Add(new Diagnostic($"String literal not defined: \"{text}\"", lit.Span));
                _instructions.AppendLine($"    {val} = iconst.i64 0 ; missing string literal");
            }
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

        // Check locals/parameters (stored in stack slots)
        if (_locals.TryGetValue(name, out var local))
        {
            var val = NewValue();
            _instructions.AppendLine($"    {val} = load.{FormatType(local.Type)} {local.Address}");
            return val;
        }

        // Must be a global - emit a load
        if (_globalTypes.TryGetValue(name, out var globalType))
        {
            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = global_value {name}");
            var val = NewValue();
            _instructions.AppendLine($"    {val} = load.{FormatType(globalType)} {addr}");
            return val;
        }

        // Unknown identifier - emit placeholder
        var fallback = NewValue();
        _instructions.AppendLine($"    {fallback} = iconst.i32 0 ; unknown identifier {name}");
        return fallback;
    }

    private string LowerBinary(BinaryExpressionSyntax bin)
    {
        var left = LowerExpression(bin.Left);
        var right = LowerExpression(bin.Right);
        var leftType = GetExpressionType(bin.Left);
        var rightType = GetExpressionType(bin.Right);
        var isFloat = IsFloatType(leftType) || IsFloatType(rightType);
        if (isFloat)
        {
            var useF64 = IsF64Type(leftType) || IsF64Type(rightType);
            left = CoerceFloatOperand(left, leftType, useF64);
            right = CoerceFloatOperand(right, rightType, useF64);
        }

        var op = bin.OperatorToken.Kind switch
        {
            TokenKind.Plus => isFloat ? "fadd" : "iadd",
            TokenKind.Minus => isFloat ? "fsub" : "isub",
            TokenKind.Star => isFloat ? "fmul" : "imul",
            TokenKind.Slash => isFloat ? "fdiv" : "sdiv",
            TokenKind.Percent => "srem",
            TokenKind.Less => isFloat ? "fcmp lt" : "icmp slt",
            TokenKind.LessEqual => isFloat ? "fcmp le" : "icmp sle",
            TokenKind.Greater => isFloat ? "fcmp gt" : "icmp sgt",
            TokenKind.GreaterEqual => isFloat ? "fcmp ge" : "icmp sge",
            TokenKind.EqualEqual => isFloat ? "fcmp eq" : "icmp eq",
            TokenKind.BangEqual => isFloat ? "fcmp ne" : "icmp ne",
            TokenKind.AmpAmp => "band",
            TokenKind.PipePipe => "bor",
            _ => "iadd"
        };

        if (op.StartsWith("icmp") || op.StartsWith("fcmp"))
        {
            var cmpOp = op.Replace("icmp ", "").Replace("fcmp ", "");
            var cmp = NewValue();
            var cmpPrefix = op.StartsWith("fcmp") ? "fcmp" : "icmp";
            _instructions.AppendLine($"    {cmp} = {cmpPrefix} {cmpOp} {left}, {right}");

            // Stasis represents bool as i32, so convert b1 -> i32.
            var result = NewValue();
            _instructions.AppendLine($"    {result} = bint.i32 {cmp}");
            return result;
        }

        var nonCmp = NewValue();
        _instructions.AppendLine($"    {nonCmp} = {op} {left}, {right}");
        return nonCmp;
    }

    private string LowerUnary(UnaryExpressionSyntax unary)
    {
        var operand = LowerExpression(unary.Operand);

        if (unary.OperatorToken.Kind == TokenKind.Minus)
        {
            var operandType = GetExpressionType(unary.Operand);
            var result = NewValue();
            var zero = NewValue();
            if (IsFloatType(operandType))
            {
                var zeroConst = IsF64Type(operandType) ? "f64const 0.0" : "f32const 0.0";
                _instructions.AppendLine($"    {zero} = {zeroConst}");
                _instructions.AppendLine($"    {result} = fsub {zero}, {operand}");
            }
            else
            {
                _instructions.AppendLine($"    {zero} = iconst.i32 0");
                _instructions.AppendLine($"    {result} = isub {zero}, {operand}");
            }
            return result;
        }
        if (unary.OperatorToken.Kind == TokenKind.Bang)
        {
            var cmp = NewValue();
            var zero = NewValue();
            _instructions.AppendLine($"    {zero} = iconst.i32 0");
            _instructions.AppendLine($"    {cmp} = icmp eq {operand}, {zero}");

            // Stasis represents bool as i32, so convert b1 -> i32.
            var result = NewValue();
            _instructions.AppendLine($"    {result} = bint.i32 {cmp}");
            return result;
        }

        var fallback = NewValue();
        _instructions.AppendLine($"    {fallback} = {operand} ; TODO: unary {unary.OperatorToken.Kind}");
        return fallback;
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

        // Check if this is a built-in function
        if (IsBuiltinFunction(funcName))
        {
            return LowerBuiltinCall(funcName, call.Arguments);
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

    private bool IsBuiltinFunction(string name)
    {
        return name switch
        {
            "print_int" => true,
            "print_string" => true,
            "print_char" => true,
            "read_int" => true,
            "read_char" => true,
            "time" => true,
            "get_time_ms" => true,
            "sleep_ms" => true,
            "str_len" => true,
            "str_is_empty" => true,
            "str_get" => true,
            "str_set" => true,
            "str_eq" => true,
            "str_cmp" => true,
            "str_copy" => true,
            "str_append" => true,
            "str_append_char" => true,
            "str_clear" => true,
            "str_contains" => true,
            "str_find" => true,
            "str_find_char" => true,
            "str_find_last_char" => true,
            "str_starts_with" => true,
            "str_ends_with" => true,
            "str_substr" => true,
            _ => false
        };
    }

    private string LowerBuiltinCall(string funcName, IReadOnlyList<ExpressionSyntax> arguments)
    {
        switch (funcName)
        {
            case "print_int":
                return LowerPrintInt(arguments);
            case "print_char":
                return LowerPrintChar(arguments);
            case "print_string":
                return LowerPrintString(arguments);
            case "read_int":
                return LowerReadInt(arguments);
            case "read_char":
                return LowerReadChar(arguments);
            case "time":
                return LowerTime(arguments);
            case "get_time_ms":
                return LowerGetTimeMs(arguments);
            case "sleep_ms":
                return LowerSleepMs(arguments);
            case "str_len":
                return LowerStrLen(arguments);
            case "str_is_empty":
                return LowerStrIsEmpty(arguments);
            case "str_get":
                return LowerStrGet(arguments);
            case "str_set":
                return LowerStrSet(arguments);
            case "str_eq":
                return LowerStrEq(arguments);
            case "str_cmp":
                return LowerStrCmp(arguments);
            case "str_copy":
                return LowerStrCopy(arguments);
            case "str_append":
                return LowerStrAppend(arguments);
            case "str_append_char":
                return LowerStrAppendChar(arguments);
            case "str_clear":
                return LowerStrClear(arguments);
            case "str_contains":
                return LowerStrContains(arguments);
            case "str_find":
                return LowerStrFind(arguments);
            case "str_find_char":
                return LowerStrFindChar(arguments);
            case "str_find_last_char":
                return LowerStrFindLastChar(arguments);
            case "str_starts_with":
                return LowerStrStartsWith(arguments);
            case "str_ends_with":
                return LowerStrEndsWith(arguments);
            case "str_substr":
                return LowerStrSubstr(arguments);

            default:
                // Unsupported built-in - emit placeholder
                var fallback = NewValue();
                _instructions.AppendLine($"    {fallback} = iconst.i32 0 ; TODO: built-in {funcName}");
                return fallback;
        }
    }

    private string LowerPrintInt(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("print_int expects 1 argument", new SourceSpan(0, 0)));
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: wrong arg count");
            return err;
        }

        // Get the integer value to print
        var value = LowerExpression(arguments[0]);

        // Get format string " %d"
        var formatGlobalName = GetOrCreateFormatString(" %d");

        // Load format string address
        var fmtAddr = NewValue();
        _instructions.AppendLine($"    {fmtAddr} = global_value {formatGlobalName}");

        var value64 = NewValue();
        _instructions.AppendLine($"    {value64} = sextend.i64 {value}");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i64 0");
        // Call printf3(format, value, 0)
        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %printf3({fmtAddr}, {value64}, {zero})");

        return result;
    }

    private string LowerPrintChar(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("print_char expects 1 argument", new SourceSpan(0, 0)));
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: wrong arg count");
            return err;
        }

        // Get the char value to print (as i32)
        var value = LowerExpression(arguments[0]);

        // Get format string "%c"
        var formatGlobalName = GetOrCreateFormatString("%c");

        // Load format string address
        var fmtAddr = NewValue();
        _instructions.AppendLine($"    {fmtAddr} = global_value {formatGlobalName}");

        var value64 = NewValue();
        _instructions.AppendLine($"    {value64} = sextend.i64 {value}");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i64 0");
        // Call printf3(format, value, 0)
        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %printf3({fmtAddr}, {value64}, {zero})");

        return result;
    }

    private string LowerPrintString(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("print_string expects 1 argument", new SourceSpan(0, 0)));
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: wrong arg count");
            return err;
        }

        // Get the string value to print (pointer)
        var value = LowerExpression(arguments[0]);

        // Get format string "%s"
        var formatGlobalName = GetOrCreateFormatString("%s");

        // Load format string address
        var fmtAddr = NewValue();
        _instructions.AppendLine($"    {fmtAddr} = global_value {formatGlobalName}");

        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i64 0");
        // Call printf3(format, string, 0)
        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %printf3({fmtAddr}, {value}, {zero})");

        return result;
    }

    private string LowerReadInt(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("read_int expects no arguments", new SourceSpan(0, 0)));
        }

        // Get format string "%d"
        var formatGlobalName = GetOrCreateFormatString("%d");

        // Load format string address
        var fmtAddr = NewValue();
        _instructions.AppendLine($"    {fmtAddr} = global_value {formatGlobalName}");

        var tmpAddr = NewValue();
        _instructions.AppendLine($"    {tmpAddr} = stack_slot.i32");

        var callResult = NewValue();
        _instructions.AppendLine($"    {callResult} = call %scanf({fmtAddr}, {tmpAddr})");

        var result = NewValue();
        _instructions.AppendLine($"    {result} = load.i32 {tmpAddr}");
        return result;
    }

    private string LowerReadChar(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("read_char expects no arguments", new SourceSpan(0, 0)));
        }

        // Get format string " %c"
        var formatGlobalName = GetOrCreateFormatString(" %c");

        // Load format string address
        var fmtAddr = NewValue();
        _instructions.AppendLine($"    {fmtAddr} = global_value {formatGlobalName}");

        var tmpAddr = NewValue();
        _instructions.AppendLine($"    {tmpAddr} = stack_slot.i32");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        _instructions.AppendLine($"    store {zero}, {tmpAddr} ; init read_char slot to 0");

        var callResult = NewValue();
        _instructions.AppendLine($"    {callResult} = call %scanf({fmtAddr}, {tmpAddr})");

        var result = NewValue();
        _instructions.AppendLine($"    {result} = load.i32 {tmpAddr}");
        return result;
    }

    private string LowerTime(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("time expects no arguments", new SourceSpan(0, 0)));
        }

        var nullPtr = NewValue();
        _instructions.AppendLine($"    {nullPtr} = iconst.i64 0");
        var callResult = NewValue();
        _instructions.AppendLine($"    {callResult} = call %time({nullPtr})");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = ireduce.i32 {callResult}");
        return result;
    }

    private string LowerGetTimeMs(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("get_time_ms expects no arguments", new SourceSpan(0, 0)));
        }

        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %stasis_get_time_ms()");
        return result;
    }

    private string LowerSleepMs(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("sleep_ms expects the duration in milliseconds", new SourceSpan(0, 0)));
        }

        var arg = arguments.Count > 0 ? LowerExpression(arguments[0]) : NewValue();
        if (arguments.Count == 0)
        {
            _instructions.AppendLine($"    {arg} = iconst.i32 0");
        }
        _instructions.AppendLine($"    call %stasis_sleep_ms({arg})");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iconst.i32 0");
        return result;
    }

    private string LowerStrLen(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_len", "str_len expects 1 argument (s: u8[]).");
        }

        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({ptr})");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = ireduce.i32 {len64}");
        return result;
    }

    private string LowerStrIsEmpty(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_is_empty", "str_is_empty expects 1 argument (s: u8[]).");
        }

        var first = NewValue();
        _instructions.AppendLine($"    {first} = load.i8 {ptr}");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i8 0");
        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = icmp eq {first}, {zero}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bint.i32 {cmp}");
        return result;
    }

    private string LowerStrGet(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_get", "str_get expects 2 arguments (s: u8[], index: i32).");
        }

        var index = LowerExpression(arguments[1]);
        var addr = EmitByteAddress(ptr, index);
        var value = NewValue();
        _instructions.AppendLine($"    {value} = load.i8 {addr}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = uextend.i32 {value}");
        return result;
    }

    private string LowerStrSet(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 3 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_set", "str_set expects 3 arguments (s: u8[], index: i32, byte: u8).");
        }

        var index = LowerExpression(arguments[1]);
        var addr = EmitByteAddress(ptr, index);
        var byteVal = LowerExpression(arguments[2]);
        var truncated = NewValue();
        _instructions.AppendLine($"    {truncated} = ireduce.i8 {byteVal}");
        _instructions.AppendLine($"    store {truncated}, {addr}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iconst.i32 0");
        return result;
    }

    private string LowerStrEq(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_eq", out var ptrA, out var ptrB))
        {
            return EmitInvalidBuiltin("str_eq", "str_eq expects 2 arguments (a: u8[], b: u8[]).");
        }

        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = call %strcmp({ptrA}, {ptrB})");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var isEq = NewValue();
        _instructions.AppendLine($"    {isEq} = icmp eq {cmp}, {zero}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bint.i32 {isEq}");
        return result;
    }

    private string LowerStrCmp(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_cmp", out var ptrA, out var ptrB))
        {
            return EmitInvalidBuiltin("str_cmp", "str_cmp expects 2 arguments (a: u8[], b: u8[]).");
        }

        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %strcmp({ptrA}, {ptrB})");
        return result;
    }

    private string LowerStrCopy(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_copy", out var dst, out var src))
        {
            return EmitInvalidBuiltin("str_copy", "str_copy expects 2 arguments (dst: u8[], src: u8[]).");
        }

        _instructions.AppendLine($"    call %strcpy({dst}, {src})");
        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({dst})");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = ireduce.i32 {len64}");
        return result;
    }

    private string LowerStrAppend(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_append", out var dst, out var src))
        {
            return EmitInvalidBuiltin("str_append", "str_append expects 2 arguments (dst: u8[], src: u8[]).");
        }

        _instructions.AppendLine($"    call %strcat({dst}, {src})");
        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({dst})");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = ireduce.i32 {len64}");
        return result;
    }

    private string LowerStrAppendChar(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_append_char", "str_append_char expects 2 arguments (dst: u8[], byte: u8).");
        }

        var byteVal = LowerExpression(arguments[1]);
        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({ptr})");
        var len = NewValue();
        _instructions.AppendLine($"    {len} = ireduce.i32 {len64}");
        var addr = EmitByteAddress(ptr, len);
        var truncated = NewValue();
        _instructions.AppendLine($"    {truncated} = ireduce.i8 {byteVal}");
        _instructions.AppendLine($"    store {truncated}, {addr}");
        var nextIndex = NewValue();
        _instructions.AppendLine($"    {nextIndex} = iadd {len}, {OneI32()}");
        var nextAddr = EmitByteAddress(ptr, nextIndex);
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i8 0");
        _instructions.AppendLine($"    store {zero}, {nextAddr}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iadd {len}, {OneI32()}");
        return result;
    }

    private string LowerStrClear(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_clear", "str_clear expects 1 argument (s: u8[]).");
        }

        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i8 0");
        _instructions.AppendLine($"    store {zero}, {ptr}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iconst.i32 0");
        return result;
    }

    private string LowerStrContains(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_contains", out var ptrA, out var ptrB))
        {
            return EmitInvalidBuiltin("str_contains", "str_contains expects 2 arguments (s: u8[], needle: u8[]).");
        }

        var found = NewValue();
        _instructions.AppendLine($"    {found} = call %strstr({ptrA}, {ptrB})");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i64 0");
        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = icmp ne {found}, {zero}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bint.i32 {cmp}");
        return result;
    }

    private string LowerStrFind(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_find", out var ptrA, out var ptrB))
        {
            return EmitInvalidBuiltin("str_find", "str_find expects 2 arguments (s: u8[], needle: u8[]).");
        }

        var found = NewValue();
        _instructions.AppendLine($"    {found} = call %strstr({ptrA}, {ptrB})");
        return EmitPtrDiffIndex(ptrA, found);
    }

    private string LowerStrFindChar(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_find_char", "str_find_char expects 2 arguments (s: u8[], byte: u8).");
        }

        var byteVal = LowerExpression(arguments[1]);
        var found = NewValue();
        _instructions.AppendLine($"    {found} = call %strchr({ptr}, {byteVal})");
        return EmitPtrDiffIndex(ptr, found);
    }

    private string LowerStrFindLastChar(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_find_last_char", "str_find_last_char expects 2 arguments (s: u8[], byte: u8).");
        }

        var byteVal = LowerExpression(arguments[1]);
        var found = NewValue();
        _instructions.AppendLine($"    {found} = call %strrchr({ptr}, {byteVal})");
        return EmitPtrDiffIndex(ptr, found);
    }

    private string LowerStrStartsWith(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_starts_with", out var ptrA, out var ptrB))
        {
            return EmitInvalidBuiltin("str_starts_with", "str_starts_with expects 2 arguments (s: u8[], prefix: u8[]).");
        }

        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({ptrB})");
        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = call %strncmp({ptrA}, {ptrB}, {len64})");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var eq = NewValue();
        _instructions.AppendLine($"    {eq} = icmp eq {cmp}, {zero}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bint.i32 {eq}");
        return result;
    }

    private string LowerStrEndsWith(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetStringPair(arguments, "str_ends_with", out var ptrA, out var ptrB))
        {
            return EmitInvalidBuiltin("str_ends_with", "str_ends_with expects 2 arguments (s: u8[], suffix: u8[]).");
        }

        var lenA = NewValue();
        _instructions.AppendLine($"    {lenA} = call %strlen({ptrA})");
        var lenB = NewValue();
        _instructions.AppendLine($"    {lenB} = call %strlen({ptrB})");
        var lenOk = NewValue();
        _instructions.AppendLine($"    {lenOk} = icmp sge {lenA}, {lenB}");
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = isub {lenA}, {lenB}");
        var endPtr = NewValue();
        _instructions.AppendLine($"    {endPtr} = iadd {ptrA}, {offset}");
        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = call %strncmp({endPtr}, {ptrB}, {lenB})");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var eq = NewValue();
        _instructions.AppendLine($"    {eq} = icmp eq {cmp}, {zero}");
        var falseVal = NewValue();
        _instructions.AppendLine($"    {falseVal} = icmp ne {zero}, {zero}");
        var selected = NewValue();
        _instructions.AppendLine($"    {selected} = select {lenOk}, {eq}, {falseVal}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bint.i32 {selected}");
        return result;
    }

    private string LowerStrSubstr(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 4 ||
            !TryGetStringArg(arguments[0], out var dst) ||
            !TryGetStringArg(arguments[1], out var src))
        {
            return EmitInvalidBuiltin("str_substr", "str_substr expects 4 arguments (dst: u8[], src: u8[], start: i32, byte_len: i32).");
        }

        var start = LowerExpression(arguments[2]);
        var byteLen = LowerExpression(arguments[3]);

        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({src})");
        var srcLen = NewValue();
        _instructions.AppendLine($"    {srcLen} = ireduce.i32 {len64}");

        var end = NewValue();
        _instructions.AppendLine($"    {end} = iadd {start}, {byteLen}");

        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var startNeg = NewValue();
        _instructions.AppendLine($"    {startNeg} = icmp slt {start}, {zero}");

        var lenNeg = NewValue();
        _instructions.AppendLine($"    {lenNeg} = icmp slt {byteLen}, {zero}");

        var endGt = NewValue();
        _instructions.AppendLine($"    {endGt} = icmp sgt {end}, {srcLen}");

        var abortBlock = NewBlock();
        var checkLenBlock = NewBlock();
        var checkEndBlock = NewBlock();
        var okBlock = NewBlock();

        _instructions.AppendLine($"    brif {startNeg}, {abortBlock}, {checkLenBlock}");

        _instructions.AppendLine($"{checkLenBlock}:");
        _instructions.AppendLine($"    brif {lenNeg}, {abortBlock}, {checkEndBlock}");

        _instructions.AppendLine($"{checkEndBlock}:");
        _instructions.AppendLine($"    brif {endGt}, {abortBlock}, {okBlock}");

        _instructions.AppendLine($"{abortBlock}:");
        _instructions.AppendLine($"    call %abort()");
        _instructions.AppendLine($"    jump {okBlock}");

        _instructions.AppendLine($"{okBlock}:");
        var startPtr = EmitByteAddress(src, start);
        var lenBytes64 = NewValue();
        _instructions.AppendLine($"    {lenBytes64} = sextend.i64 {byteLen}");
        var copyResult = NewValue();
        _instructions.AppendLine($"    {copyResult} = call %memcpy({dst}, {startPtr}, {lenBytes64})");

        var termAddr = EmitByteAddress(dst, byteLen);
        var zeroByte = NewValue();
        _instructions.AppendLine($"    {zeroByte} = iconst.i8 0");
        _instructions.AppendLine($"    store {zeroByte}, {termAddr}");

        return byteLen;
    }

    private string GetOrCreateFormatString(string format)
    {
        if (_stringLiterals.TryGetValue(format, out var existing))
        {
            return existing;
        }

        _diagnostics.Add(new Diagnostic($"Missing format string literal: \"{format}\"", new SourceSpan(0, 0)));
        return "str_missing";
    }

    private bool TryGetStringArg(ExpressionSyntax argument, out string ptr)
    {
        return TryLowerArrayPointer(argument, out ptr);
    }

    private bool TryGetStringPair(IReadOnlyList<ExpressionSyntax> arguments, string name, out string ptrA, out string ptrB)
    {
        ptrA = string.Empty;
        ptrB = string.Empty;
        if (arguments.Count != 2)
        {
            return false;
        }

        if (!TryLowerArrayPointer(arguments[0], out ptrA) || !TryLowerArrayPointer(arguments[1], out ptrB))
        {
            _diagnostics.Add(new Diagnostic($"{name} requires array arguments.", new SourceSpan(0, 0)));
            return false;
        }

        return true;
    }

    private string EmitInvalidBuiltin(string name, string message)
    {
        _diagnostics.Add(new Diagnostic(message, new SourceSpan(0, 0)));
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iconst.i32 0 ; invalid {name}");
        return result;
    }

    private bool TryLowerArrayPointer(ExpressionSyntax expr, out string ptr)
    {
        ptr = string.Empty;
        if (expr is IdentifierExpressionSyntax id)
        {
            if (_symbols.TryGetValue(id.Identifier.Text, out var symbol) && symbol.Type is ArrayTypeSymbol)
            {
                ptr = NewValue();
                _instructions.AppendLine($"    {ptr} = global_value {id.Identifier.Text}");
                return true;
            }
        }

        if (expr is MemberAccessExpressionSyntax member &&
            TryResolveArrayMember(member, out var arrayName))
        {
            ptr = NewValue();
            _instructions.AppendLine($"    {ptr} = global_value {arrayName}");
            return true;
        }

        _diagnostics.Add(new Diagnostic("String built-ins require global array arguments.", new SourceSpan(0, 0)));
        return false;
    }

    private bool TryResolveArrayMember(MemberAccessExpressionSyntax member, out string arrayName)
    {
        arrayName = string.Empty;
        if (!TryResolveMemberBase(member.Receiver, out var baseName, out var baseType))
        {
            return false;
        }

        if (baseType is not NamedTypeSymbol named || !_structs.TryGetValue(named.TypeName, out var structDecl))
        {
            return false;
        }

        var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
        if (field?.Type is not ArrayTypeSyntax)
        {
            return false;
        }

        arrayName = $"{baseName}_{member.Member.Text}";
        return true;
    }

    private string EmitByteAddress(string basePtr, string index)
    {
        var indexI64 = NewValue();
        _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
        var addr = NewValue();
        _instructions.AppendLine($"    {addr} = iadd {basePtr}, {indexI64}");
        return addr;
    }

    private string EmitPtrDiffIndex(string basePtr, string foundPtr)
    {
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i64 0");
        var isNull = NewValue();
        _instructions.AppendLine($"    {isNull} = icmp eq {foundPtr}, {zero}");
        var diff = NewValue();
        _instructions.AppendLine($"    {diff} = isub {foundPtr}, {basePtr}");
        var idx = NewValue();
        _instructions.AppendLine($"    {idx} = ireduce.i32 {diff}");
        var minusOne = NewValue();
        _instructions.AppendLine($"    {minusOne} = iconst.i32 -1");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {isNull}, {minusOne}, {idx}");
        return result;
    }

    private string OneI32()
    {
        var one = NewValue();
        _instructions.AppendLine($"    {one} = iconst.i32 1");
        return one;
    }

    private string LowerAssignment(AssignmentExpressionSyntax assign)
    {
        var value = LowerExpression(assign.Right);
        var valueType = GetExpressionType(assign.Right);

        if (assign.Left is IdentifierExpressionSyntax id)
        {
            var name = id.Identifier.Text;
            if (_locals.TryGetValue(name, out var local))
            {
                if (_localTypes.TryGetValue(name, out var localType))
                {
                    value = CoerceAssignmentValue(value, valueType, localType);
                }
                _instructions.AppendLine($"    store {value}, {local.Address}");
            }
            else if (_globalTypes.TryGetValue(name, out _))
            {
                // Store to global
                var addr = NewValue();
                _instructions.AppendLine($"    {addr} = global_value {name}");
                if (_symbols.TryGetValue(name, out var symbol))
                {
                    value = CoerceAssignmentValue(value, valueType, symbol.Type);
                }
                _instructions.AppendLine($"    store {value}, {addr}");
            }
            else
            {
                _instructions.AppendLine($"    ; unknown assignment target {name}");
            }
        }
        else if (assign.Left is ArrayAccessExpressionSyntax arrayAccess)
        {
            // Store to array element
            LowerArrayStore(arrayAccess, value, valueType);
        }
        else if (assign.Left is MemberAccessExpressionSyntax memberAccess)
        {
            LowerMemberStore(memberAccess, value, valueType);
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
            // Array length - resolve statically from symbol type
            if (member.Receiver is IdentifierExpressionSyntax id &&
                _symbols.TryGetValue(id.Identifier.Text, out var symbol) &&
                symbol.Type is ArrayTypeSymbol arrayType)
            {
                _instructions.AppendLine($"    {result} = iconst.i32 {arrayType.Size}");
                return result;
            }

            _instructions.AppendLine($"    {result} = iconst.i32 0 ; error: could not resolve array length");
        }
        else if (member.Receiver is ArrayAccessExpressionSyntax arrayAccess)
        {
            return LowerArrayElementFieldAccess(arrayAccess, member.Member.Text);
        }
        else if (TryResolveFlattenedMember(member, out var flattenedName, out var memberType))
        {
            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = global_value {flattenedName}");
            var clifType = _typeMapper.Map(memberType);
            _instructions.AppendLine($"    {result} = load.{FormatType(clifType)} {addr}");
            return result;
        }
        else
        {
            // Struct field access - TODO: SoA transformation
            _instructions.AppendLine($"    {result} = iconst.i32 0 ; TODO: member access .{member.Member.Text}");
        }

        return result;
    }

    private string LowerArrayAccess(ArrayAccessExpressionSyntax array)
    {
        // Get the array name and index
        if (array.Receiver is MemberAccessExpressionSyntax memberAccess)
        {
            return LowerArrayFieldAccess(memberAccess, array.Index);
        }

        if (array.Receiver is not IdentifierExpressionSyntax id)
        {
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: complex array expression");
            return err;
        }

        var arrayName = id.Identifier.Text;
        var index = LowerExpression(array.Index);

        // Check if it's a global, parameter, or local
        if (_locals.ContainsKey(arrayName))
        {
            // Local/parameter array - TODO: need local array support
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; TODO: local array access");
            return err;
        }

        // Global array - get base address and calculate element address
        if (!_globalTypes.TryGetValue(arrayName, out var globalType))
        {
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: unknown array {arrayName}");
            return err;
        }

        // Get symbol to determine element size
        if (!_symbols.TryGetValue(arrayName, out var symbol) || symbol.Type is not ArrayTypeSymbol arrayType)
        {
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: not an array type");
            return err;
        }

        // Calculate element size
        var elemType = _typeMapper.Map(arrayType.ElementType);
        var elemSize = GetTypeSize(elemType);

        // Load array base address
        var baseAddr = NewValue();
        _instructions.AppendLine($"    {baseAddr} = global_value {arrayName}");

        // Calculate offset: index * elem_size
        var elemSizeVal = NewValue();
        _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
        var indexI64 = NewValue();
        _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

        // Calculate element address: base + offset
        var elemAddr = NewValue();
        _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");

        // Load the element value
        var result = NewValue();
        _instructions.AppendLine($"    {result} = load.{FormatType(elemType)} {elemAddr}");

        return result;
    }

    private string LowerArrayFieldAccess(MemberAccessExpressionSyntax memberAccess, ExpressionSyntax indexExpr)
    {
        if (memberAccess.Receiver is not IdentifierExpressionSyntax id ||
            !_symbols.TryGetValue(id.Identifier.Text, out var symbol) ||
            symbol.Type is not NamedTypeSymbol named ||
            !_structs.TryGetValue(named.TypeName, out var structDecl))
        {
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: complex array access");
            return err;
        }

        var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == memberAccess.Member.Text);
        if (field?.Type is not ArrayTypeSyntax arrayType)
        {
            var err = NewValue();
            _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: not an array field");
            return err;
        }

        var elemType = ResolveType(arrayType.ElementType);
        var clifElemType = _typeMapper.Map(elemType);
        var index = LowerExpression(indexExpr);
        var baseName = $"{id.Identifier.Text}_{memberAccess.Member.Text}";

        var baseAddr = NewValue();
        _instructions.AppendLine($"    {baseAddr} = global_value {baseName}");

        var elemSize = GetTypeSize(clifElemType);
        var elemSizeVal = NewValue();
        _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
        var indexI64 = NewValue();
        _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

        var elemAddr = NewValue();
        _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");

        var result = NewValue();
        _instructions.AppendLine($"    {result} = load.{FormatType(clifElemType)} {elemAddr}");
        return result;
    }

    private string LowerArrayElementFieldAccess(ArrayAccessExpressionSyntax array, string fieldName)
    {
        if (array.Receiver is IdentifierExpressionSyntax id &&
            _symbols.TryGetValue(id.Identifier.Text, out var symbol) &&
            symbol.Type is ArrayTypeSymbol arrayType &&
            arrayType.ElementType is NamedTypeSymbol namedElem &&
            _structs.TryGetValue(namedElem.TypeName, out var structDecl))
        {
            var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == fieldName);
            if (field is null)
            {
                var err = NewValue();
                _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: unknown field {fieldName}");
                return err;
            }

            var elemType = ResolveType(field.Type);
            var clifElemType = _typeMapper.Map(elemType);
            var index = LowerExpression(array.Index);
            var baseName = $"{structDecl.Name.Text}_{fieldName}";

            var baseAddr = NewValue();
            _instructions.AppendLine($"    {baseAddr} = global_value {baseName}");

            var elemSize = GetTypeSize(clifElemType);
            var elemSizeVal = NewValue();
            _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
            var indexI64 = NewValue();
            _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
            var offset = NewValue();
            _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

            var elemAddr = NewValue();
            _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");

            var result = NewValue();
            _instructions.AppendLine($"    {result} = load.{FormatType(clifElemType)} {elemAddr}");
            return result;
        }

        if (array.Receiver is MemberAccessExpressionSyntax memberAccess &&
            memberAccess.Receiver is IdentifierExpressionSyntax structId &&
            _symbols.TryGetValue(structId.Identifier.Text, out var structSymbol) &&
            structSymbol.Type is NamedTypeSymbol structType &&
            _structs.TryGetValue(structType.TypeName, out var parentStructDecl))
        {
            var arrayField = parentStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == memberAccess.Member.Text);
            if (arrayField?.Type is ArrayTypeSyntax arrayTypeSyntax &&
                arrayTypeSyntax.ElementType is NamedTypeSyntax elementNamed &&
                _structs.TryGetValue(elementNamed.Name, out var elemStructDecl))
            {
                var field = elemStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == fieldName);
                if (field is null)
                {
                    var err = NewValue();
                    _instructions.AppendLine($"    {err} = iconst.i32 0 ; error: unknown field {fieldName}");
                    return err;
                }

                var elemType = ResolveType(field.Type);
                var clifElemType = _typeMapper.Map(elemType);
                var index = LowerExpression(array.Index);
                var baseName = $"{structId.Identifier.Text}_{memberAccess.Member.Text}_{fieldName}";

                var baseAddr = NewValue();
                _instructions.AppendLine($"    {baseAddr} = global_value {baseName}");

                var elemSize = GetTypeSize(clifElemType);
                var elemSizeVal = NewValue();
                _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
                var indexI64 = NewValue();
                _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
                var offset = NewValue();
                _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

                var elemAddr = NewValue();
                _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");

                var result = NewValue();
                _instructions.AppendLine($"    {result} = load.{FormatType(clifElemType)} {elemAddr}");
                return result;
            }
        }

        var fallback = NewValue();
        _instructions.AppendLine($"    {fallback} = iconst.i32 0 ; error: unsupported array element field access");
        return fallback;
    }

    private void LowerArrayStore(ArrayAccessExpressionSyntax array, string value, TypeSymbol? valueType)
    {
        // Get the array name and index
        if (array.Receiver is MemberAccessExpressionSyntax memberAccess)
        {
            LowerArrayFieldStore(memberAccess, array.Index, value, valueType);
            return;
        }

        if (array.Receiver is not IdentifierExpressionSyntax id)
        {
            _instructions.AppendLine($"    ; error: complex array expression in store");
            return;
        }

        var arrayName = id.Identifier.Text;
        var index = LowerExpression(array.Index);

        // Check if it's a global, parameter, or local
        if (_locals.ContainsKey(arrayName))
        {
            // Local/parameter array - TODO: need local array support
            _instructions.AppendLine($"    ; TODO: local array store");
            return;
        }

        // Global array - get base address and calculate element address
        if (!_globalTypes.TryGetValue(arrayName, out var globalType))
        {
            _instructions.AppendLine($"    ; error: unknown array {arrayName}");
            return;
        }

        // Get symbol to determine element size
        if (!_symbols.TryGetValue(arrayName, out var symbol) || symbol.Type is not ArrayTypeSymbol arrayType)
        {
            _instructions.AppendLine($"    ; error: not an array type");
            return;
        }

        // Calculate element size
        var elemType = _typeMapper.Map(arrayType.ElementType);
        var elemSize = GetTypeSize(elemType);
        value = CoerceAssignmentValue(value, valueType, arrayType.ElementType);

        // Load array base address
        var baseAddr = NewValue();
        _instructions.AppendLine($"    {baseAddr} = global_value {arrayName}");

        // Calculate offset: index * elem_size
        var elemSizeVal = NewValue();
        _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
        var indexI64 = NewValue();
        _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

        // Calculate element address: base + offset
        var elemAddr = NewValue();
        _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");

        // Store the value to the element address
        _instructions.AppendLine($"    store {value}, {elemAddr}");
    }

    private void LowerArrayFieldStore(MemberAccessExpressionSyntax memberAccess, ExpressionSyntax indexExpr, string value, TypeSymbol? valueType)
    {
        if (memberAccess.Receiver is not IdentifierExpressionSyntax id ||
            !_symbols.TryGetValue(id.Identifier.Text, out var symbol) ||
            symbol.Type is not NamedTypeSymbol named ||
            !_structs.TryGetValue(named.TypeName, out var structDecl))
        {
            _instructions.AppendLine($"    ; error: complex array store");
            return;
        }

        var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == memberAccess.Member.Text);
        if (field?.Type is not ArrayTypeSyntax arrayType)
        {
            _instructions.AppendLine($"    ; error: not an array field");
            return;
        }

        var elemType = ResolveType(arrayType.ElementType);
        var clifElemType = _typeMapper.Map(elemType);
        value = CoerceAssignmentValue(value, valueType, elemType);
        var index = LowerExpression(indexExpr);
        var baseName = $"{id.Identifier.Text}_{memberAccess.Member.Text}";

        var baseAddr = NewValue();
        _instructions.AppendLine($"    {baseAddr} = global_value {baseName}");

        var elemSize = GetTypeSize(clifElemType);
        var elemSizeVal = NewValue();
        _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
        var indexI64 = NewValue();
        _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

        var elemAddr = NewValue();
        _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");
        _instructions.AppendLine($"    store {value}, {elemAddr}");
    }

    private void LowerMemberStore(MemberAccessExpressionSyntax member, string value, TypeSymbol? valueType)
    {
        if (member.Member.Text == "length")
        {
            _instructions.AppendLine("    ; error: cannot assign to length");
            return;
        }

        if (member.Receiver is ArrayAccessExpressionSyntax arrayAccess)
        {
            LowerArrayElementFieldStore(arrayAccess, member.Member.Text, value, valueType);
            return;
        }

        if (TryResolveFlattenedMember(member, out var flattenedName, out var memberType))
        {
            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = global_value {flattenedName}");
            value = CoerceAssignmentValue(value, valueType, memberType);
            _instructions.AppendLine($"    store {value}, {addr}");
            return;
        }
        _instructions.AppendLine($"    ; error: complex member store");
    }

    private void LowerArrayElementFieldStore(ArrayAccessExpressionSyntax array, string fieldName, string value, TypeSymbol? valueType)
    {
        if (array.Receiver is IdentifierExpressionSyntax id &&
            _symbols.TryGetValue(id.Identifier.Text, out var symbol) &&
            symbol.Type is ArrayTypeSymbol arrayType &&
            arrayType.ElementType is NamedTypeSymbol namedElem &&
            _structs.TryGetValue(namedElem.TypeName, out var structDecl))
        {
            var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == fieldName);
            if (field is null)
            {
                _instructions.AppendLine($"    ; error: unknown field {fieldName}");
                return;
            }

            var elemType = ResolveType(field.Type);
            var clifElemType = _typeMapper.Map(elemType);
            value = CoerceAssignmentValue(value, valueType, elemType);
            var index = LowerExpression(array.Index);
            var baseName = $"{structDecl.Name.Text}_{fieldName}";

            var baseAddr = NewValue();
            _instructions.AppendLine($"    {baseAddr} = global_value {baseName}");

            var elemSize = GetTypeSize(clifElemType);
            var elemSizeVal = NewValue();
            _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
            var indexI64 = NewValue();
            _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
            var offset = NewValue();
            _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

            var elemAddr = NewValue();
            _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");
            _instructions.AppendLine($"    store {value}, {elemAddr}");
            return;
        }

        if (array.Receiver is MemberAccessExpressionSyntax memberAccess &&
            memberAccess.Receiver is IdentifierExpressionSyntax structId &&
            _symbols.TryGetValue(structId.Identifier.Text, out var structSymbol) &&
            structSymbol.Type is NamedTypeSymbol structType &&
            _structs.TryGetValue(structType.TypeName, out var parentStructDecl))
        {
            var arrayField = parentStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == memberAccess.Member.Text);
            if (arrayField?.Type is ArrayTypeSyntax arrayTypeSyntax &&
                arrayTypeSyntax.ElementType is NamedTypeSyntax elementNamed &&
                _structs.TryGetValue(elementNamed.Name, out var elemStructDecl))
            {
                var field = elemStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == fieldName);
                if (field is null)
                {
                    _instructions.AppendLine($"    ; error: unknown field {fieldName}");
                    return;
                }

                var elemType = ResolveType(field.Type);
                var clifElemType = _typeMapper.Map(elemType);
                value = CoerceAssignmentValue(value, valueType, elemType);
                var index = LowerExpression(array.Index);
                var baseName = $"{structId.Identifier.Text}_{memberAccess.Member.Text}_{fieldName}";

                var baseAddr = NewValue();
                _instructions.AppendLine($"    {baseAddr} = global_value {baseName}");

                var elemSize = GetTypeSize(clifElemType);
                var elemSizeVal = NewValue();
                _instructions.AppendLine($"    {elemSizeVal} = iconst.i64 {elemSize}");
                var indexI64 = NewValue();
                _instructions.AppendLine($"    {indexI64} = sextend.i64 {index}");
                var offset = NewValue();
                _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSizeVal}");

                var elemAddr = NewValue();
                _instructions.AppendLine($"    {elemAddr} = iadd {baseAddr}, {offset}");
                _instructions.AppendLine($"    store {value}, {elemAddr}");
                return;
            }
        }

        _instructions.AppendLine($"    ; error: unsupported array element field store");
    }

    private TypeSymbol ResolveType(TypeSyntax syntax)
    {
        return syntax switch
        {
            NamedTypeSyntax named when _symbols.TryGetValue(named.Name, out var sym) && sym.Type is not null => sym.Type,
            NamedTypeSyntax named when string.Equals(named.Name, "void", StringComparison.Ordinal) => new VoidTypeSymbol(),
            NamedTypeSyntax named => new NamedTypeSymbol(named.Name),
            ArrayTypeSyntax array => new ArrayTypeSymbol(
                ResolveType(array.ElementType),
                int.TryParse(array.SizeToken?.Text, out var parsed) ? parsed : -1),
            _ => new NamedTypeSymbol("unknown")
        };
    }

    private bool TryResolveFlattenedMember(
        MemberAccessExpressionSyntax member,
        out string flattenedName,
        out TypeSymbol memberType)
    {
        flattenedName = string.Empty;
        memberType = new NamedTypeSymbol("unknown");

        if (!TryResolveMemberBase(member.Receiver, out var baseName, out var baseType))
        {
            return false;
        }

        if (baseType is not NamedTypeSymbol named || !_structs.TryGetValue(named.TypeName, out var structDecl))
        {
            return false;
        }

        var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
        if (field is null)
        {
            return false;
        }

        memberType = ResolveType(field.Type);
        if (memberType is ArrayTypeSymbol)
        {
            return false;
        }

        flattenedName = $"{baseName}_{member.Member.Text}";
        return true;
    }

    private bool TryResolveMemberBase(ExpressionSyntax receiver, out string baseName, out TypeSymbol baseType)
    {
        baseName = string.Empty;
        baseType = new NamedTypeSymbol("unknown");

        if (receiver is IdentifierExpressionSyntax id)
        {
            if (_locals.ContainsKey(id.Identifier.Text))
            {
                return false;
            }

            if (_symbols.TryGetValue(id.Identifier.Text, out var symbol) && symbol.Type is not null)
            {
                baseName = id.Identifier.Text;
                baseType = symbol.Type;
                return true;
            }

            return false;
        }

        if (receiver is MemberAccessExpressionSyntax member)
        {
            if (TryResolveFlattenedMember(member, out var flattened, out var type))
            {
                baseName = flattened;
                baseType = type;
                return true;
            }
        }

        return false;
    }

    private int GetTypeSize(CraneliftTypeMapper.ClifType type)
    {
        return type switch
        {
            CraneliftTypeMapper.ClifType.I8 => 1,
            CraneliftTypeMapper.ClifType.I16 => 2,
            CraneliftTypeMapper.ClifType.I32 => 4,
            CraneliftTypeMapper.ClifType.I64 => 8,
            CraneliftTypeMapper.ClifType.F32 => 4,
            CraneliftTypeMapper.ClifType.F64 => 8,
            CraneliftTypeMapper.ClifType.R64 => 8,
            _ => 4
        };
    }

    private string NewValue() => $"v{_valueCounter++}";
    private string NewBlock() => $"block{_blockCounter++}";
    private string CoerceI32ToB1(string value)
    {
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = icmp ne {value}, {zero}");
        return cmp;
    }

    private string FormatBlockParams(IReadOnlyList<ParameterSyntax> parameters)
    {
        var parts = new List<string>();
        for (int i = 0; i < parameters.Count; i++)
        {
            var param = parameters[i];
            var typeSymbol = ResolveType(param.Type);
            var type = FormatType(NormalizeLocalStorageType(_typeMapper.Map(typeSymbol)));
            parts.Add($"v{i}: {type}");
        }
        return string.Join(", ", parts);
    }

    private static CraneliftTypeMapper.ClifType NormalizeLocalStorageType(CraneliftTypeMapper.ClifType type) =>
        type switch
        {
            CraneliftTypeMapper.ClifType.I8 => CraneliftTypeMapper.ClifType.I32,
            CraneliftTypeMapper.ClifType.I16 => CraneliftTypeMapper.ClifType.I32,
            CraneliftTypeMapper.ClifType.B1 => CraneliftTypeMapper.ClifType.I32,
            CraneliftTypeMapper.ClifType.R64 => CraneliftTypeMapper.ClifType.I64,
            _ => type
        };

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

    private sealed record LocalSlot(string Address, CraneliftTypeMapper.ClifType Type);

    private static string UnescapeString(string text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return string.Empty;
        }

        var inner = text.Length >= 2 ? text.Substring(1, text.Length - 2) : text;
        var sb = new StringBuilder(inner.Length);
        for (int i = 0; i < inner.Length; i++)
        {
            var ch = inner[i];
            if (ch == '\\' && i + 1 < inner.Length)
            {
                i++;
                var esc = inner[i];
                sb.Append(esc switch
                {
                    '\\' => '\\',
                    '"' => '"',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => esc
                });
            }
            else
            {
                sb.Append(ch);
            }
        }

        return sb.ToString();
    }

    private string CoerceAssignmentValue(string value, TypeSymbol? fromType, TypeSymbol? toType)
    {
        if (fromType is null || toType is null)
        {
            return value;
        }

        if (IsFloatType(toType) && !IsFloatType(fromType))
        {
            var result = NewValue();
            var op = IsF64Type(toType) ? "fcvt_from_sint.f64" : "fcvt_from_sint.f32";
            _instructions.AppendLine($"    {result} = {op} {value}");
            return result;
        }

        return value;
    }

    private string CoerceFloatOperand(string value, TypeSymbol? fromType, bool useF64)
    {
        if (fromType is null || IsFloatType(fromType))
        {
            return value;
        }

        var result = NewValue();
        var op = useF64 ? "fcvt_from_sint.f64" : "fcvt_from_sint.f32";
        _instructions.AppendLine($"    {result} = {op} {value}");
        return result;
    }

    private TypeSymbol? GetExpressionType(ExpressionSyntax expr)
    {
        return expr switch
        {
            LiteralExpressionSyntax lit => lit.Literal.Kind switch
            {
                TokenKind.IntegerLiteral => new PrimitiveTypeSymbol("i32"),
                TokenKind.FloatLiteral => new PrimitiveTypeSymbol("f32"),
                TokenKind.TrueKeyword or TokenKind.FalseKeyword => new PrimitiveTypeSymbol("bool"),
                TokenKind.StringLiteral => new PrimitiveTypeSymbol("string"),
                _ => null
            },
            IdentifierExpressionSyntax id when _localTypes.TryGetValue(id.Identifier.Text, out var localType) => localType,
            IdentifierExpressionSyntax id when _symbols.TryGetValue(id.Identifier.Text, out var sym) => sym.Type,
            ParenthesizedExpressionSyntax paren => GetExpressionType(paren.Expression),
            UnaryExpressionSyntax unary when unary.OperatorToken.Kind == TokenKind.Bang => new PrimitiveTypeSymbol("bool"),
            UnaryExpressionSyntax unary => GetExpressionType(unary.Operand),
            AssignmentExpressionSyntax assign => GetExpressionType(assign.Right),
            CallExpressionSyntax call when call.Callee is IdentifierExpressionSyntax id &&
                                            _symbols.TryGetValue(id.Identifier.Text, out var funcSym) => funcSym.Type,
            MemberAccessExpressionSyntax member when member.Receiver is IdentifierExpressionSyntax enumId =>
                _symbols.TryGetValue($"{enumId.Identifier.Text}.{member.Member.Text}", out var memberSym) ? memberSym.Type : null,
            ArrayAccessExpressionSyntax array when GetExpressionType(array.Receiver) is ArrayTypeSymbol arr => arr.ElementType,
            BinaryExpressionSyntax bin => GetBinaryResultType(bin),
            _ => null
        };
    }

    private TypeSymbol? GetBinaryResultType(BinaryExpressionSyntax bin)
    {
        if (bin.OperatorToken.Kind is TokenKind.EqualEqual or TokenKind.BangEqual
            or TokenKind.Less or TokenKind.LessEqual or TokenKind.Greater or TokenKind.GreaterEqual
            or TokenKind.AmpAmp or TokenKind.PipePipe)
        {
            return new PrimitiveTypeSymbol("bool");
        }

        var leftType = GetExpressionType(bin.Left);
        var rightType = GetExpressionType(bin.Right);
        if (IsF64Type(leftType) || IsF64Type(rightType))
        {
            return new PrimitiveTypeSymbol("f64");
        }
        if (IsFloatType(leftType) || IsFloatType(rightType))
        {
            return new PrimitiveTypeSymbol("f32");
        }

        return leftType ?? rightType;
    }

    private static bool IsFloatType(TypeSymbol? type) =>
        type is PrimitiveTypeSymbol p && (p.PrimitiveName == "f32" || p.PrimitiveName == "f64");

    private static bool IsF64Type(TypeSymbol? type) =>
        type is PrimitiveTypeSymbol p && p.PrimitiveName == "f64";
}
