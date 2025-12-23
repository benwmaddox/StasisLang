using System.Linq;
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
    private readonly IReadOnlyDictionary<string, EnumDeclarationSyntax> _enums;
    private readonly IReadOnlyDictionary<string, FunctionDeclarationSyntax> _functions;
    private readonly IReadOnlyDictionary<string, CraneliftTypeMapper.ClifType> _globalTypes;
    private readonly IReadOnlyDictionary<string, string> _stringLiterals;
    private readonly IReadOnlyDictionary<string, string> _cStringLiterals;
    private readonly Layout.LayoutPlan _layoutPlan;
    private readonly IReadOnlyDictionary<string, ConstValue> _consts;
    private readonly string _moduleName;
    private readonly Dictionary<string, LocalSlot> _locals = new();
    private readonly Dictionary<string, TypeSymbol> _localTypes = new();
    private readonly Dictionary<string, ForeachElementBinding> _elementBindings = new();
    private readonly HashSet<string> _inlineStack = new(StringComparer.Ordinal);
    private int _valueCounter;
    private int _blockCounter;
    private readonly List<Diagnostic> _diagnostics;

    public CraneliftFunctionBuilder(
        CraneliftTypeMapper typeMapper,
        IReadOnlyDictionary<string, Symbol> symbols,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        IReadOnlyDictionary<string, EnumDeclarationSyntax> enums,
        IReadOnlyDictionary<string, FunctionDeclarationSyntax> functions,
        IReadOnlyDictionary<string, CraneliftTypeMapper.ClifType> globalTypes,
        IReadOnlyDictionary<string, string> stringLiterals,
        IReadOnlyDictionary<string, string> cStringLiterals,
        Layout.LayoutPlan layoutPlan,
        IReadOnlyDictionary<string, ConstValue> consts,
        List<Diagnostic> diagnostics,
        string moduleName)
    {
        _typeMapper = typeMapper;
        _symbols = symbols;
        _structs = structs;
        _enums = enums;
        _functions = functions;
        _globalTypes = globalTypes;
        _stringLiterals = stringLiterals;
        _cStringLiterals = cStringLiterals;
        _layoutPlan = layoutPlan;
        _consts = consts;
        _diagnostics = diagnostics;
        _moduleName = moduleName;
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
        _elementBindings.Clear();
        _inlineStack.Clear();
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
        _elementBindings.Clear();
        _inlineStack.Clear();
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
            if (stmt is ReturnStatementSyntax)
            {
                break;
            }
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
            case ForeachStatementSyntax foreachStmt:
                LowerForeach(foreachStmt);
                break;
            case BlockStatementSyntax blockStmt:
                LowerBlock(blockStmt);
                break;
            default:
                _diagnostics.Add(new Diagnostic($"Statement not supported in Cranelift backend: {stmt.GetType().Name}.", stmt.Span));
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

    private void LowerForeach(ForeachStatementSyntax foreachStmt)
    {
        if (!TryResolveForeachIterable(foreachStmt.Iterable, out var iterable))
        {
            _diagnostics.Add(new Diagnostic("foreach target must be a fixed-size array.", foreachStmt.Iterable.Span));
            return;
        }

        var internalIndexName = $"__foreach_idx_{_blockCounter}";
        var indexSlot = NewValue();
        _instructions.AppendLine($"    {indexSlot} = stack_slot.i32");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        _instructions.AppendLine($"    store {zero}, {indexSlot}");

        var previousIndexSlot = _locals.ContainsKey(internalIndexName)
            ? _locals[internalIndexName]
            : null;
        var previousIndexType = _localTypes.ContainsKey(internalIndexName)
            ? _localTypes[internalIndexName]
            : null;
        _locals[internalIndexName] = new LocalSlot(indexSlot, CraneliftTypeMapper.ClifType.I32);
        _localTypes[internalIndexName] = new PrimitiveTypeSymbol("i32");

        var iteratorName = foreachStmt.Iterator.Text;
        var indexName = foreachStmt.IndexVariable?.Text;
        var previousIteratorBinding = _elementBindings.ContainsKey(iteratorName)
            ? _elementBindings[iteratorName]
            : null;
        var previousIteratorLocal = _locals.ContainsKey(iteratorName) ? _locals[iteratorName] : null;
        var previousIteratorType = _localTypes.ContainsKey(iteratorName) ? _localTypes[iteratorName] : null;
        var previousIndexLocal = indexName is not null && _locals.ContainsKey(indexName) ? _locals[indexName] : null;
        var previousIndexTypeEntry = indexName is not null && _localTypes.ContainsKey(indexName) ? _localTypes[indexName] : null;

        if (foreachStmt.BindByElement)
        {
            _elementBindings[iteratorName] = new ForeachElementBinding(foreachStmt.Iterable, iterable.ElementType, internalIndexName);
            if (indexName is not null)
            {
                _locals[indexName] = new LocalSlot(indexSlot, CraneliftTypeMapper.ClifType.I32);
                _localTypes[indexName] = new PrimitiveTypeSymbol("i32");
            }
        }
        else
        {
            _locals[iteratorName] = new LocalSlot(indexSlot, CraneliftTypeMapper.ClifType.I32);
            _localTypes[iteratorName] = new PrimitiveTypeSymbol("i32");
            if (indexName is not null)
            {
                _locals[indexName] = new LocalSlot(indexSlot, CraneliftTypeMapper.ClifType.I32);
                _localTypes[indexName] = new PrimitiveTypeSymbol("i32");
            }
        }

        var condBlock = NewBlock();
        var bodyBlock = NewBlock();
        var latchBlock = NewBlock();
        var endBlock = NewBlock();

        _instructions.AppendLine($"    jump {condBlock}");

        _instructions.AppendLine($"{condBlock}:");
        var currentIndex = NewValue();
        _instructions.AppendLine($"    {currentIndex} = load.i32 {indexSlot}");
        var lengthVal = NewValue();
        _instructions.AppendLine($"    {lengthVal} = iconst.i32 {iterable.Length}");
        var cond = NewValue();
        _instructions.AppendLine($"    {cond} = icmp slt {currentIndex}, {lengthVal}");
        _instructions.AppendLine($"    brif {cond}, {bodyBlock}, {endBlock}");

        _instructions.AppendLine($"{bodyBlock}:");
        LowerBlock(foreachStmt.Body);
        _instructions.AppendLine($"    jump {latchBlock}");

        _instructions.AppendLine($"{latchBlock}:");
        var nextIndex = NewValue();
        var one = ConstI32(1);
        _instructions.AppendLine($"    {nextIndex} = iadd {currentIndex}, {one}");
        _instructions.AppendLine($"    store {nextIndex}, {indexSlot}");
        _instructions.AppendLine($"    jump {condBlock}");

        _instructions.AppendLine($"{endBlock}:");

        if (foreachStmt.BindByElement)
        {
            if (previousIteratorBinding is not null)
            {
                _elementBindings[iteratorName] = previousIteratorBinding;
            }
            else
            {
                _elementBindings.Remove(iteratorName);
            }
        }
        else
        {
            RestoreLocal(iteratorName, previousIteratorLocal, previousIteratorType);
        }

        if (indexName is not null)
        {
            RestoreLocal(indexName, previousIndexLocal, previousIndexTypeEntry);
        }

        RestoreLocal(internalIndexName, previousIndexSlot, previousIndexType);
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
            case OperatorCallExpressionSyntax op:
                return LowerOperatorCall(op);
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
                _diagnostics.Add(new Diagnostic($"Expression not supported in Cranelift backend: {expr.GetType().Name}.", expr.Span));
                _instructions.AppendLine($"    {val} = iconst.i32 0");
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
                var baseAddr = NewValue();
                _instructions.AppendLine($"    {baseAddr} = global_value {globalName}");
                var payload = NewValue();
                var headerOffset = ConstI64(HeaderSizeFor("string"));
                _instructions.AppendLine($"    {payload} = iadd {baseAddr}, {headerOffset}");
                return payload;
            }
            else
            {
                _diagnostics.Add(new Diagnostic($"String literal not defined: \"{text}\"", lit.Span));
                _instructions.AppendLine($"    {val} = iconst.i64 0 ; missing string literal");
            }
        }
        else
        {
            _diagnostics.Add(new Diagnostic("Unsupported literal expression.", lit.Span));
            _instructions.AppendLine($"    {val} = iconst.i32 0");
        }

        return val;
    }

    private string LowerIdentifier(IdentifierExpressionSyntax id)
    {
        var name = id.Identifier.Text;

        if (_elementBindings.TryGetValue(name, out var binding))
        {
            return LowerForeachElementValue(binding, id.Span);
        }

        // Check locals/parameters (stored in stack slots)
        if (_locals.TryGetValue(name, out var local))
        {
            var val = NewValue();
            _instructions.AppendLine($"    {val} = load.{FormatType(local.Type)} {local.Address}");
            return val;
        }

        if (_consts.TryGetValue(name, out var constValue))
        {
            return EmitConstValue(constValue);
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
        _diagnostics.Add(new Diagnostic($"Unary operator '{unary.OperatorToken.Text}' is not supported in Cranelift backend.", unary.Span));
        return operand;
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

        if (TryInlineCall(call, funcName, out var inlined))
        {
            return inlined;
        }

        // Lower arguments first
        var args = new List<string>();
        foreach (var arg in call.Arguments)
        {
            if (GetExpressionType(arg) is ArrayTypeSymbol)
            {
                if (TryLowerArrayPointerForCall(arg, out var ptr))
                {
                    args.Add(ptr);
                    continue;
                }
            }

            args.Add(LowerExpression(arg));
        }

        var callName = IsBuiltinFunction(funcName) ? funcName : MangleFunctionName(funcName);
        var argList = string.Join(", ", args);
        if (IsVoidFunction(funcName))
        {
            _instructions.AppendLine($"    call %{callName}({argList})");
            return ZeroI32();
        }

        // Then create result value
        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %{callName}({argList})");

        return result;
    }

    private bool TryInlineCall(CallExpressionSyntax call, string funcName, out string result)
    {
        result = string.Empty;
        if (!_functions.TryGetValue(funcName, out var func))
        {
            return false;
        }

        if (!HasInlineAttribute(func))
        {
            return false;
        }

        if (_inlineStack.Contains(funcName))
        {
            return false;
        }

        if (func.Parameters.Count != call.Arguments.Count)
        {
            return false;
        }

        if (func.Body.Statements.Count != 1 || func.Body.Statements[0] is not ReturnStatementSyntax ret || ret.Expression is null)
        {
            return false;
        }

        var savedLocals = new List<(string Name, LocalSlot? Slot, TypeSymbol? Type)>();
        _inlineStack.Add(funcName);

        for (int i = 0; i < func.Parameters.Count; i++)
        {
            var param = func.Parameters[i];
            var argExpr = call.Arguments[i];
            var argType = GetExpressionType(argExpr);
            var paramType = ResolveType(param.Type);
            var clifType = NormalizeLocalStorageType(_typeMapper.Map(paramType));

            var argValue = LowerInlineArgument(argExpr, paramType);
            argValue = CoerceAssignmentValue(argValue, argType, paramType);

            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = stack_slot.{FormatType(clifType)}");
            _instructions.AppendLine($"    store {argValue}, {addr}");

            savedLocals.Add((param.Name.Text, _locals.TryGetValue(param.Name.Text, out var existing) ? existing : null, _localTypes.TryGetValue(param.Name.Text, out var existingType) ? existingType : null));
            _locals[param.Name.Text] = new LocalSlot(addr, clifType);
            _localTypes[param.Name.Text] = paramType;
        }

        result = LowerExpression(ret.Expression);

        for (int i = savedLocals.Count - 1; i >= 0; i--)
        {
            var saved = savedLocals[i];
            if (saved.Slot is null)
            {
                _locals.Remove(saved.Name);
            }
            else
            {
                _locals[saved.Name] = saved.Slot;
            }

            if (saved.Type is null)
            {
                _localTypes.Remove(saved.Name);
            }
            else
            {
                _localTypes[saved.Name] = saved.Type;
            }
        }

        _inlineStack.Remove(funcName);
        return true;
    }

    private string LowerInlineArgument(ExpressionSyntax argExpr, TypeSymbol paramType)
    {
        if (paramType is ArrayTypeSymbol)
        {
            if (TryLowerArrayPointerForCall(argExpr, out var ptr))
            {
                return ptr;
            }
        }

        return LowerExpression(argExpr);
    }

    private static bool HasInlineAttribute(FunctionDeclarationSyntax func) =>
        func.Attributes.Any(attr => string.Equals(attr.Text, "inline", StringComparison.Ordinal));

    private string LowerOperatorCall(OperatorCallExpressionSyntax op)
    {
        var opText = op.OperatorToken.Text;
        if (op.Arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic($"Operator '.{opText}()' requires exactly one argument.", op.Span));
            return ZeroI32();
        }

        var rhs = LowerExpression(op.Arguments[0]);
        if (opText == "=")
        {
            _diagnostics.Add(new Diagnostic("Use infix '=' for assignment.", op.Span));
            StoreOperatorAssignment(op.Receiver, rhs, GetExpressionType(op.Arguments[0]));
            return rhs;
        }

        var lhs = LowerExpression(op.Receiver);
        return LowerBinaryOperator(opText, lhs, rhs, GetExpressionType(op.Receiver), GetExpressionType(op.Arguments[0]), op.Span);
    }

    private void StoreOperatorAssignment(ExpressionSyntax target, string value, TypeSymbol? valueType)
    {
        switch (target)
        {
            case IdentifierExpressionSyntax id:
                {
                    var name = id.Identifier.Text;
                    if (_elementBindings.TryGetValue(name, out var elementBinding))
                    {
                        LowerForeachElementStore(elementBinding, value, valueType, target.Span);
                        return;
                    }
                    if (_locals.TryGetValue(name, out var local))
                    {
                        if (_localTypes.TryGetValue(name, out var localType))
                        {
                            value = CoerceAssignmentValue(value, valueType, localType);
                        }
                        _instructions.AppendLine($"    store {value}, {local.Address}");
                        return;
                    }
                    if (_globalTypes.TryGetValue(name, out _))
                    {
                        var addr = NewValue();
                        _instructions.AppendLine($"    {addr} = global_value {name}");
                        if (_symbols.TryGetValue(name, out var symbol))
                        {
                            value = CoerceAssignmentValue(value, valueType, symbol.Type);
                        }
                        _instructions.AppendLine($"    store {value}, {addr}");
                        return;
                    }
                    _instructions.AppendLine($"    ; unknown assignment target {name}");
                    return;
                }
            case ArrayAccessExpressionSyntax arrayAccess:
                LowerArrayStore(arrayAccess, value, valueType);
                return;
            case MemberAccessExpressionSyntax memberAccess:
                if (memberAccess.Receiver is IdentifierExpressionSyntax receiverId &&
                    _elementBindings.TryGetValue(receiverId.Identifier.Text, out var boundElement))
                {
                    LowerForeachElementFieldStore(boundElement, memberAccess.Member.Text, value, valueType, target.Span);
                    return;
                }
                LowerMemberStore(memberAccess, value, valueType);
                return;
            default:
                _diagnostics.Add(new Diagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", target.Span));
                return;
        }
    }

    private string LowerBinaryOperator(string opText, string left, string right, TypeSymbol? leftType, TypeSymbol? rightType, SourceSpan span)
    {
        var isFloat = IsFloatType(leftType) || IsFloatType(rightType);
        if (isFloat)
        {
            var useF64 = IsF64Type(leftType) || IsF64Type(rightType);
            left = CoerceFloatOperand(left, leftType, useF64);
            right = CoerceFloatOperand(right, rightType, useF64);
        }

        var op = opText switch
        {
            "+" => isFloat ? "fadd" : "iadd",
            "-" => isFloat ? "fsub" : "isub",
            "*" => isFloat ? "fmul" : "imul",
            "/" => isFloat ? "fdiv" : "sdiv",
            "%" => "srem",
            "<" => isFloat ? "fcmp lt" : "icmp slt",
            "<=" => isFloat ? "fcmp le" : "icmp sle",
            ">" => isFloat ? "fcmp gt" : "icmp sgt",
            ">=" => isFloat ? "fcmp ge" : "icmp sge",
            "==" => isFloat ? "fcmp eq" : "icmp eq",
            "!=" => isFloat ? "fcmp ne" : "icmp ne",
            "&&" => "band",
            "||" => "bor",
            _ => string.Empty
        };

        if (string.IsNullOrEmpty(op))
        {
            _diagnostics.Add(new Diagnostic($"Unsupported operator '{opText}'.", span));
            var fallback = NewValue();
            _instructions.AppendLine($"    {fallback} = iconst.i32 0");
            return fallback;
        }

        if (op.StartsWith("icmp") || op.StartsWith("fcmp"))
        {
            var cmpOp = op.Replace("icmp ", "").Replace("fcmp ", "");
            var cmp = NewValue();
            var cmpPrefix = op.StartsWith("fcmp") ? "fcmp" : "icmp";
            _instructions.AppendLine($"    {cmp} = {cmpPrefix} {cmpOp} {left}, {right}");
            var result = NewValue();
            _instructions.AppendLine($"    {result} = bint.i32 {cmp}");
            return result;
        }

        var nonCmp = NewValue();
        _instructions.AppendLine($"    {nonCmp} = {op} {left}, {right}");
        return nonCmp;
    }

    private bool IsVoidFunction(string name) =>
        _symbols.TryGetValue(name, out var sym) && sym.Type is VoidTypeSymbol;

    private bool IsBuiltinFunction(string name)
    {
        return name switch
        {
            "print_int" => true,
            "print_string" => true,
            "print_char" => true,
            "print_prompt" => true,
            "print_invalid" => true,
            "print_clue_error" => true,
            "print_solved" => true,
            "print_cell" => true,
            "read_int" => true,
            "read_char" => true,
            "time" => true,
            "get_time_ms" => true,
            "sleep_ms" => true,
            "sin" => true,
            "cos" => true,
            "sin_fast" => true,
            "cos_fast" => true,
            "init_window" => true,
            "begin_frame" => true,
            "end_frame" => true,
            "clear" => true,
            "draw_line" => true,
            "gfx_load_sprite" => true,
            "gfx_draw_sprite" => true,
            "gfx_poll_reload" => true,
            "gfx_debug_bake_hash" => true,
            "is_key_down" => true,
            "should_quit" => true,
            "get_window_size" => true,
            "set_fullscreen" => true,
            "set_postfx" => true,
            "load_font" => true,
            "draw_text" => true,
            "measure_text" => true,
            "list_directory" => true,
            "dir_list_entry_is_dir" => true,
            "dir_list_entry_copy_name" => true,
            "char_is_digit" => true,
            "char_is_alpha" => true,
            "char_is_alnum" => true,
            "char_is_space" => true,
            "char_is_upper" => true,
            "char_is_lower" => true,
            "char_is_hex" => true,
            "char_is_print" => true,
            "char_to_upper" => true,
            "char_to_lower" => true,
            "char_to_digit" => true,
            "char_from_digit" => true,
            "char_to_hex" => true,
            "char_from_hex" => true,
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

    private string MangleFunctionName(string name) => $"{_moduleName}__{name}";

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
            case "print_prompt":
                return LowerPrintPrompt(arguments);
            case "print_invalid":
                return LowerPrintInvalid(arguments);
            case "print_clue_error":
                return LowerPrintClueError(arguments);
            case "print_solved":
                return LowerPrintSolved(arguments);
            case "print_cell":
                return LowerPrintCell(arguments);
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
            case "sin":
                return LowerSinCos(arguments, isSin: true);
            case "cos":
                return LowerSinCos(arguments, isSin: false);
            case "sin_fast":
                return LowerSinCos(arguments, isSin: true);
            case "cos_fast":
                return LowerSinCos(arguments, isSin: false);
            case "init_window":
                return LowerInitWindow(arguments);
            case "begin_frame":
                return LowerBeginFrame(arguments);
            case "end_frame":
                return LowerEndFrame(arguments);
            case "clear":
                return LowerClear(arguments);
            case "draw_line":
                return LowerDrawLine(arguments);
            case "gfx_load_sprite":
                return LowerGfxLoadSprite(arguments);
            case "gfx_draw_sprite":
                return LowerGfxDrawSprite(arguments);
            case "gfx_poll_reload":
                return LowerGfxPollReload(arguments);
            case "gfx_debug_bake_hash":
                return LowerGfxDebugBakeHash(arguments);
            case "is_key_down":
                return LowerIsKeyDown(arguments);
            case "should_quit":
                return LowerShouldQuit(arguments);
            case "get_window_size":
                return LowerGetWindowSize(arguments);
            case "set_fullscreen":
                return LowerSetFullscreen(arguments);
            case "set_postfx":
                return LowerSetPostfx(arguments);
            case "load_font":
                return LowerLoadFont(arguments);
            case "draw_text":
                return LowerDrawText(arguments);
            case "measure_text":
                return LowerMeasureText(arguments);
            case "list_directory":
                return LowerListDirectory(arguments);
            case "dir_list_entry_is_dir":
                return LowerDirListEntryIsDir(arguments);
            case "dir_list_entry_copy_name":
                return LowerDirListEntryCopyName(arguments);
            case "char_is_digit":
                return LowerCharIsDigit(arguments);
            case "char_is_alpha":
                return LowerCharIsAlpha(arguments);
            case "char_is_alnum":
                return LowerCharIsAlnum(arguments);
            case "char_is_space":
                return LowerCharIsSpace(arguments);
            case "char_is_upper":
                return LowerCharIsUpper(arguments);
            case "char_is_lower":
                return LowerCharIsLower(arguments);
            case "char_is_hex":
                return LowerCharIsHex(arguments);
            case "char_is_print":
                return LowerCharIsPrint(arguments);
            case "char_to_upper":
                return LowerCharToUpper(arguments);
            case "char_to_lower":
                return LowerCharToLower(arguments);
            case "char_to_digit":
                return LowerCharToDigit(arguments);
            case "char_from_digit":
                return LowerCharFromDigit(arguments);
            case "char_to_hex":
                return LowerCharToHex(arguments);
            case "char_from_hex":
                return LowerCharFromHex(arguments);
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
                  _diagnostics.Add(new Diagnostic($"Built-in '{funcName}' is not supported in Cranelift backend.", new SourceSpan(0, 0)));
                  return ZeroI32();
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

    private string LowerPrintPrompt(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("print_prompt expects no arguments", new SourceSpan(0, 0)));
        }

        return EmitPrintfLiteral("Enter row col val (1-9, 0 clears), or q to quit:\n");
    }

    private string LowerPrintInvalid(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("print_invalid expects no arguments", new SourceSpan(0, 0)));
        }

        return EmitPrintfLiteral("\u001b[31mInvalid move.\u001b[0m\n");
    }

    private string LowerPrintClueError(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("print_clue_error expects no arguments", new SourceSpan(0, 0)));
        }

        return EmitPrintfLiteral("\u001b[31mCannot change a clue.\u001b[0m\n");
    }

    private string LowerPrintSolved(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 0)
        {
            _diagnostics.Add(new Diagnostic("print_solved expects no arguments", new SourceSpan(0, 0)));
        }

        return EmitPrintfLiteral("\u001b[32mSolved!\u001b[0m\n");
    }

    private string LowerPrintCell(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2)
        {
            _diagnostics.Add(new Diagnostic("print_cell expects 2 arguments (value, is_clue).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        var value = LowerExpression(arguments[0]);
        var isClue = CoerceI32ToB1(LowerExpression(arguments[1]));

        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var isEmpty = NewValue();
        _instructions.AppendLine($"    {isEmpty} = icmp eq {value}, {zero}");

        var emptyBlock = NewBlock();
        var printBlock = NewBlock();
        var mergeBlock = NewBlock();

        _instructions.AppendLine($"    brif {isEmpty}, {emptyBlock}, {printBlock}");

        _instructions.AppendLine($"{emptyBlock}:");
        EmitPrintfLiteral(". ");
        _instructions.AppendLine($"    jump {mergeBlock}");

        _instructions.AppendLine($"{printBlock}:");
        var cluePrefix = GetOrCreateFormatString("\u001b[36m");
        var userPrefix = GetOrCreateFormatString("\u001b[32m");
        var reset = GetOrCreateFormatString("\u001b[0m ");
        var fmtString = GetOrCreateFormatString("%s");
        var fmtInt = GetOrCreateFormatString("%d");

        var clueAddr = NewValue();
        _instructions.AppendLine($"    {clueAddr} = global_value {cluePrefix}");
        var userAddr = NewValue();
        _instructions.AppendLine($"    {userAddr} = global_value {userPrefix}");
        var prefixAddr = NewValue();
        _instructions.AppendLine($"    {prefixAddr} = select {isClue}, {clueAddr}, {userAddr}");

        var fmtStrAddr = NewValue();
        _instructions.AppendLine($"    {fmtStrAddr} = global_value {fmtString}");
        var zero64 = NewValue();
        _instructions.AppendLine($"    {zero64} = iconst.i64 0");
        var prefixPrint = NewValue();
        _instructions.AppendLine($"    {prefixPrint} = call %printf3({fmtStrAddr}, {prefixAddr}, {zero64})");

        var fmtIntAddr = NewValue();
        _instructions.AppendLine($"    {fmtIntAddr} = global_value {fmtInt}");
        var value64 = NewValue();
        _instructions.AppendLine($"    {value64} = sextend.i64 {value}");
        var valuePrint = NewValue();
        _instructions.AppendLine($"    {valuePrint} = call %printf3({fmtIntAddr}, {value64}, {zero64})");

        var resetAddr = NewValue();
        _instructions.AppendLine($"    {resetAddr} = global_value {reset}");
        var resetPrint = NewValue();
        _instructions.AppendLine($"    {resetPrint} = call %printf3({fmtStrAddr}, {resetAddr}, {zero64})");

        _instructions.AppendLine($"    jump {mergeBlock}");

        _instructions.AppendLine($"{mergeBlock}:");
        return ZeroI32();
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

    private string EmitPrintfLiteral(string literal)
    {
        var formatGlobalName = GetOrCreateFormatString(literal);
        var fmtAddr = NewValue();
        _instructions.AppendLine($"    {fmtAddr} = global_value {formatGlobalName}");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i64 0");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %printf3({fmtAddr}, {zero}, {zero})");
        return result;
    }

    private bool TryGetCharArg(IReadOnlyList<ExpressionSyntax> arguments, string name, out string value)
    {
        value = string.Empty;
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic($"{name} expects 1 argument (c: u8).", new SourceSpan(0, 0)));
            return false;
        }

        var arg = arguments[0];
        var raw = LowerExpression(arg);
        var argType = GetExpressionType(arg);
        if (argType is PrimitiveTypeSymbol p && p.PrimitiveName == "u8")
        {
            var extended = NewValue();
            _instructions.AppendLine($"    {extended} = uextend.i32 {raw}");
            value = extended;
            return true;
        }

        value = raw;
        return true;
    }

    private string LowerCharPredicate(IReadOnlyList<ExpressionSyntax> arguments, string name, char min, char max)
    {
        if (!TryGetCharArg(arguments, name, out var c))
        {
            return ZeroI32();
        }

        var result = EmitCharRange(c, min, max);
        var asI32 = NewValue();
        _instructions.AppendLine($"    {asI32} = bint.i32 {result}");
        return asI32;
    }

    private string EmitCharRange(string value, char min, char max)
    {
        var ge = EmitI32Compare(value, "icmp uge", min);
        var le = EmitI32Compare(value, "icmp ule", max);
        var result = NewValue();
        _instructions.AppendLine($"    {result} = band {ge}, {le}");
        return result;
    }

    private string EmitCharEq(string value, char match)
    {
        var eq = EmitI32Compare(value, "icmp eq", match);
        return eq;
    }

    private string EmitI32Compare(string value, string op, int constant)
    {
        var cmpConst = ConstI32(constant);
        var result = NewValue();
        _instructions.AppendLine($"    {result} = {op} {value}, {cmpConst}");
        return result;
    }

    private string ConstI32(int value)
    {
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iconst.i32 {value}");
        return result;
    }

    private string ConstI64(int value)
    {
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iconst.i64 {value}");
        return result;
    }

    private bool TryLowerDirListArgument(ExpressionSyntax expr, out string namesPtr, out string flagsPtr, out string countPtr)
    {
        namesPtr = string.Empty;
        flagsPtr = string.Empty;
        countPtr = string.Empty;

        if (!TryResolveDirListBase(expr, out var baseName))
        {
            return false;
        }

        if (!_structs.TryGetValue("DirList", out var dirListStruct) || !_structs.TryGetValue("DirEntry", out var dirEntryStruct))
        {
            return false;
        }

        var nameField = dirEntryStruct.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, "name", StringComparison.Ordinal));
        var isDirField = dirEntryStruct.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, "is_dir", StringComparison.Ordinal));
        var countField = dirListStruct.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, "count", StringComparison.Ordinal));
        if (nameField is null || isDirField is null || countField is null)
        {
            return false;
        }

        var namesGlobal = $"{baseName}_entries_name";
        var flagsGlobal = $"{baseName}_entries_is_dir";
        var countGlobal = $"{baseName}_count";

        if (!_globalTypes.ContainsKey(namesGlobal) || !_globalTypes.ContainsKey(flagsGlobal) || !_globalTypes.ContainsKey(countGlobal))
        {
            return false;
        }

        namesPtr = NewValue();
        _instructions.AppendLine($"    {namesPtr} = global_value {namesGlobal}");
        flagsPtr = NewValue();
        _instructions.AppendLine($"    {flagsPtr} = global_value {flagsGlobal}");
        countPtr = NewValue();
        _instructions.AppendLine($"    {countPtr} = global_value {countGlobal}");
        return true;
    }

    private bool TryResolveDirListBase(ExpressionSyntax expr, out string baseName)
    {
        baseName = string.Empty;
        if (expr is IdentifierExpressionSyntax id &&
            _symbols.TryGetValue(id.Identifier.Text, out var symbol) &&
            symbol.Type is NamedTypeSymbol named &&
            string.Equals(named.TypeName, "DirList", StringComparison.Ordinal))
        {
            baseName = id.Identifier.Text;
            return true;
        }

        if (expr is MemberAccessExpressionSyntax member &&
            TryResolveFlattenedMember(member, out var flattened, out var memberType) &&
            memberType is NamedTypeSymbol memberNamed &&
            string.Equals(memberNamed.TypeName, "DirList", StringComparison.Ordinal))
        {
            baseName = flattened;
            return true;
        }

        return false;
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

    private string LowerSinCos(IReadOnlyList<ExpressionSyntax> arguments, bool isSin)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("sin/cos expects a single f32 argument.", new SourceSpan(0, 0)));
            var zero = NewValue();
            _instructions.AppendLine($"    {zero} = f32const 0.0");
            return zero;
        }

        var arg = LowerExpression(arguments[0]);
        var result = NewValue();
        var callee = isSin ? "sinf" : "cosf";
        _instructions.AppendLine($"    {result} = call %{callee}({arg})");
        return result;
    }

    private string LowerInitWindow(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_init_window", "init_window expects (width: i32, height: i32, title: string).", arguments, 3);

    private string LowerBeginFrame(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_begin_frame", "begin_frame expects no arguments.", arguments, 0);

    private string LowerEndFrame(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_end_frame", "end_frame expects no arguments.", arguments, 0);

    private string LowerClear(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_clear", "clear expects (r: f32, g: f32, b: f32, a: f32).", arguments, 4);

    private string LowerDrawLine(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_draw_line", "draw_line expects 8 arguments (x1,y1,x2,y2,r,g,b,a).", arguments, 8);

    private string LowerGfxLoadSprite(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_gfx_load_sprite", "gfx_load_sprite expects (path: string).", arguments, 1);

    private string LowerGfxDrawSprite(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_gfx_draw_sprite", "gfx_draw_sprite expects (handle,x,y,sx,sy,rot,r,g,b,a).", arguments, 10);

    private string LowerGfxPollReload(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_gfx_poll_reload", "gfx_poll_reload expects (handle: i32).", arguments, 1);

    private string LowerGfxDebugBakeHash(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_gfx_debug_bake_hash", "gfx_debug_bake_hash expects (path: string).", arguments, 1);

    private string LowerIsKeyDown(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_is_key_down", "is_key_down expects (scancode: i32).", arguments, 1);

    private string LowerShouldQuit(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_should_quit", "should_quit expects no arguments.", arguments, 0);

    private string LowerGetWindowSize(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2)
        {
            _diagnostics.Add(new Diagnostic("get_window_size expects (width: i32, height: i32).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        var widthPtr = NewValue();
        var heightPtr = NewValue();
        _instructions.AppendLine($"    {widthPtr} = stack_slot.i32");
        _instructions.AppendLine($"    {heightPtr} = stack_slot.i32");
        _instructions.AppendLine($"    call %stasis_get_window_size({widthPtr}, {heightPtr})");

        var widthVal = NewValue();
        var heightVal = NewValue();
        _instructions.AppendLine($"    {widthVal} = load.i32 {widthPtr}");
        _instructions.AppendLine($"    {heightVal} = load.i32 {heightPtr}");

        StoreOutParam(arguments[0], widthVal, "get_window_size expects (width: i32, height: i32).");
        StoreOutParam(arguments[1], heightVal, "get_window_size expects (width: i32, height: i32).");

        return ZeroI32();
    }

    private void StoreOutParam(ExpressionSyntax arg, string value, string errorMessage)
    {
        var valueType = new PrimitiveTypeSymbol("i32");

        if (arg is IdentifierExpressionSyntax id)
        {
            var name = id.Identifier.Text;
            if (_locals.TryGetValue(name, out var local))
            {
                if (_localTypes.TryGetValue(name, out var localType))
                {
                    value = CoerceAssignmentValue(value, valueType, localType);
                }
                _instructions.AppendLine($"    store {value}, {local.Address}");
                return;
            }

            if (_symbols.TryGetValue(name, out var symbol) && symbol.Type is not null)
            {
                var addr = NewValue();
                _instructions.AppendLine($"    {addr} = global_value {name}");
                value = CoerceAssignmentValue(value, valueType, symbol.Type);
                _instructions.AppendLine($"    store {value}, {addr}");
                return;
            }
        }
        else if (arg is MemberAccessExpressionSyntax member &&
                 TryResolveFlattenedMember(member, out var flattenedName, out var memberType))
        {
            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = global_value {flattenedName}");
            value = CoerceAssignmentValue(value, valueType, memberType);
            _instructions.AppendLine($"    store {value}, {addr}");
            return;
        }

        _diagnostics.Add(new Diagnostic(errorMessage, new SourceSpan(0, 0)));
    }

    private string LowerSetFullscreen(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_set_fullscreen", "set_fullscreen expects (enabled: i32).", arguments, 1);

    private string LowerSetPostfx(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_set_postfx", "set_postfx expects (strength, phase, speed, r, g, b).", arguments, 6);

    private string LowerLoadFont(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_load_font", "load_font expects (path: string, size: i32).", arguments, 2);

    private string LowerDrawText(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallVoid("stasis_draw_text", "draw_text expects (font_handle, text, x, y, r, g, b, a).", arguments, 8);

    private string LowerMeasureText(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerExternalCallValue("stasis_measure_text", "measure_text expects (font_handle, text).", arguments, 2);

    private string LowerListDirectory(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2)
        {
            _diagnostics.Add(new Diagnostic("list_directory expects (path: string, dir_list: DirList).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        if (!TryGetStringArg(arguments[0], out var path))
        {
            return EmitInvalidBuiltin("list_directory", "list_directory expects (path: string, dir_list: DirList).");
        }

        if (!TryLowerDirListArgument(arguments[1], out var namesPtr, out var flagsPtr, out var countPtr))
        {
            return EmitInvalidBuiltin("list_directory", "list_directory requires a DirList with entries, is_dir, and count fields.");
        }

        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %stasis_list_directory_struct({path}, {namesPtr}, {flagsPtr}, {countPtr})");
        return result;
    }

    private string LowerDirListEntryIsDir(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 2)
        {
            _diagnostics.Add(new Diagnostic("dir_list_entry_is_dir expects (dir_list: DirList, idx: i32).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        if (!TryLowerDirListArgument(arguments[0], out _, out var flagsPtr, out _))
        {
            return EmitInvalidBuiltin("dir_list_entry_is_dir", "dir_list_entry_is_dir requires a DirList with entries, is_dir, and count fields.");
        }

        var idx = LowerExpression(arguments[1]);
        var indexI64 = NewValue();
        _instructions.AppendLine($"    {indexI64} = sextend.i64 {idx}");
        var elemSize = NewValue();
        _instructions.AppendLine($"    {elemSize} = iconst.i64 4");
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = imul {indexI64}, {elemSize}");
        var elemPtr = NewValue();
        _instructions.AppendLine($"    {elemPtr} = iadd {flagsPtr}, {offset}");
        var flag = NewValue();
        _instructions.AppendLine($"    {flag} = load.i32 {elemPtr}");
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var isDir = NewValue();
        _instructions.AppendLine($"    {isDir} = icmp ne {flag}, {zero}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bint.i32 {isDir}");
        return result;
    }

    private string LowerDirListEntryCopyName(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 3)
        {
            _diagnostics.Add(new Diagnostic("dir_list_entry_copy_name expects (dir_list: DirList, idx: i32, dst: string).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        if (!TryLowerDirListArgument(arguments[0], out var namesPtr, out _, out _))
        {
            return EmitInvalidBuiltin("dir_list_entry_copy_name", "dir_list_entry_copy_name requires a DirList with entries, is_dir, and count fields.");
        }

        if (!TryGetStringArg(arguments[2], out var dst))
        {
            return EmitInvalidBuiltin("dir_list_entry_copy_name", "dir_list_entry_copy_name expects (dir_list: DirList, idx: i32, dst: string).");
        }

        var idx = LowerExpression(arguments[1]);
        var headerPtr = EmitUtf8HeaderPointer(dst, HeaderSizeFor("string"));
        _instructions.AppendLine($"    call %stasis_copy_dir_entry_name({namesPtr}, {idx}, {headerPtr})");
        return ZeroI32();
    }

    private string LowerCharIsDigit(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerCharPredicate(arguments, "char_is_digit", '0', '9');

    private string LowerCharIsAlpha(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_is_alpha", out var c))
        {
            return ZeroI32();
        }

        var lower = EmitCharRange(c, 'a', 'z');
        var upper = EmitCharRange(c, 'A', 'Z');
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bor {lower}, {upper}");
        var asI32 = NewValue();
        _instructions.AppendLine($"    {asI32} = bint.i32 {result}");
        return asI32;
    }

    private string LowerCharIsAlnum(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_is_alnum", out var c))
        {
            return ZeroI32();
        }

        var digit = EmitCharRange(c, '0', '9');
        var lower = EmitCharRange(c, 'a', 'z');
        var upper = EmitCharRange(c, 'A', 'Z');
        var alpha = NewValue();
        _instructions.AppendLine($"    {alpha} = bor {lower}, {upper}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bor {digit}, {alpha}");
        var asI32 = NewValue();
        _instructions.AppendLine($"    {asI32} = bint.i32 {result}");
        return asI32;
    }

    private string LowerCharIsSpace(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_is_space", out var c))
        {
            return ZeroI32();
        }

        var isSpace = EmitCharEq(c, ' ');
        var isTab = EmitCharEq(c, '\t');
        var isNewline = EmitCharEq(c, '\n');
        var isCr = EmitCharEq(c, '\r');
        var r1 = NewValue();
        _instructions.AppendLine($"    {r1} = bor {isSpace}, {isTab}");
        var r2 = NewValue();
        _instructions.AppendLine($"    {r2} = bor {r1}, {isNewline}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bor {r2}, {isCr}");
        var asI32 = NewValue();
        _instructions.AppendLine($"    {asI32} = bint.i32 {result}");
        return asI32;
    }

    private string LowerCharIsUpper(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerCharPredicate(arguments, "char_is_upper", 'A', 'Z');

    private string LowerCharIsLower(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerCharPredicate(arguments, "char_is_lower", 'a', 'z');

    private string LowerCharIsHex(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_is_hex", out var c))
        {
            return ZeroI32();
        }

        var digit = EmitCharRange(c, '0', '9');
        var lower = EmitCharRange(c, 'a', 'f');
        var upper = EmitCharRange(c, 'A', 'F');
        var hex = NewValue();
        _instructions.AppendLine($"    {hex} = bor {lower}, {upper}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = bor {digit}, {hex}");
        var asI32 = NewValue();
        _instructions.AppendLine($"    {asI32} = bint.i32 {result}");
        return asI32;
    }

    private string LowerCharIsPrint(IReadOnlyList<ExpressionSyntax> arguments) =>
        LowerCharPredicate(arguments, "char_is_print", (char)32, (char)126);

    private string LowerCharToUpper(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_to_upper", out var c))
        {
            return ZeroI32();
        }

        var isLower = EmitCharRange(c, 'a', 'z');
        var delta = ConstI32(32);
        var upper = NewValue();
        _instructions.AppendLine($"    {upper} = isub {c}, {delta}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {isLower}, {upper}, {c}");
        return result;
    }

    private string LowerCharToLower(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_to_lower", out var c))
        {
            return ZeroI32();
        }

        var isUpper = EmitCharRange(c, 'A', 'Z');
        var delta = ConstI32(32);
        var lower = NewValue();
        _instructions.AppendLine($"    {lower} = iadd {c}, {delta}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {isUpper}, {lower}, {c}");
        return result;
    }

    private string LowerCharToDigit(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_to_digit", out var c))
        {
            return ZeroI32();
        }

        var isDigit = EmitCharRange(c, '0', '9');
        var zeroChar = ConstI32('0');
        var minusOne = ConstI32(-1);
        var digit = NewValue();
        _instructions.AppendLine($"    {digit} = isub {c}, {zeroChar}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {isDigit}, {digit}, {minusOne}");
        return result;
    }

    private string LowerCharFromDigit(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("char_from_digit expects 1 argument (d: i32).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        var d = LowerExpression(arguments[0]);
        var ge0 = EmitI32Compare(d, "icmp sge", 0);
        var le9 = EmitI32Compare(d, "icmp sle", 9);
        var valid = NewValue();
        _instructions.AppendLine($"    {valid} = band {ge0}, {le9}");
        var zeroChar = ConstI32('0');
        var questionChar = ConstI32('?');
        var ch = NewValue();
        _instructions.AppendLine($"    {ch} = iadd {d}, {zeroChar}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {valid}, {ch}, {questionChar}");
        return result;
    }

    private string LowerCharToHex(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (!TryGetCharArg(arguments, "char_to_hex", out var c))
        {
            return ZeroI32();
        }

        var isDigit = EmitCharRange(c, '0', '9');
        var zeroChar = ConstI32('0');
        var lowerBase = ConstI32('a' - 10);
        var upperBase = ConstI32('A' - 10);
        var minusOne = ConstI32(-1);
        var digitVal = NewValue();
        _instructions.AppendLine($"    {digitVal} = isub {c}, {zeroChar}");
        var isLower = EmitCharRange(c, 'a', 'f');
        var lowerVal = NewValue();
        _instructions.AppendLine($"    {lowerVal} = isub {c}, {lowerBase}");
        var isUpper = EmitCharRange(c, 'A', 'F');
        var upperVal = NewValue();
        _instructions.AppendLine($"    {upperVal} = isub {c}, {upperBase}");
        var temp = NewValue();
        _instructions.AppendLine($"    {temp} = select {isLower}, {lowerVal}, {minusOne}");
        var temp2 = NewValue();
        _instructions.AppendLine($"    {temp2} = select {isUpper}, {upperVal}, {temp}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {isDigit}, {digitVal}, {temp2}");
        return result;
    }

    private string LowerCharFromHex(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1)
        {
            _diagnostics.Add(new Diagnostic("char_from_hex expects 1 argument (d: i32).", new SourceSpan(0, 0)));
            return ZeroI32();
        }

        var d = LowerExpression(arguments[0]);
        var ge0 = EmitI32Compare(d, "icmp sge", 0);
        var le9 = EmitI32Compare(d, "icmp sle", 9);
        var isDigit = NewValue();
        _instructions.AppendLine($"    {isDigit} = band {ge0}, {le9}");
        var ge10 = EmitI32Compare(d, "icmp sge", 10);
        var le15 = EmitI32Compare(d, "icmp sle", 15);
        var isHex = NewValue();
        _instructions.AppendLine($"    {isHex} = band {ge10}, {le15}");
        var zeroChar = ConstI32('0');
        var tenConst = ConstI32(10);
        var aChar = ConstI32('a');
        var questionChar = ConstI32('?');
        var digitCh = NewValue();
        _instructions.AppendLine($"    {digitCh} = iadd {d}, {zeroChar}");
        var d10 = NewValue();
        _instructions.AppendLine($"    {d10} = isub {d}, {tenConst}");
        var hexCh = NewValue();
        _instructions.AppendLine($"    {hexCh} = iadd {d10}, {aChar}");
        var temp = NewValue();
        _instructions.AppendLine($"    {temp} = select {isHex}, {hexCh}, {questionChar}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = select {isDigit}, {digitCh}, {temp}");
        return result;
    }

    private string LowerExternalCallValue(string externalName, string errorMessage, IReadOnlyList<ExpressionSyntax> arguments, int expectedArgs)
    {
        if (arguments.Count != expectedArgs)
        {
            _diagnostics.Add(new Diagnostic(errorMessage, new SourceSpan(0, 0)));
            return ZeroI32();
        }

        var args = new List<string>(expectedArgs);
        for (int i = 0; i < expectedArgs; i++)
        {
            var arg = arguments[i];
            if (GetExpressionType(arg) is ArrayTypeSymbol)
            {
                if (TryLowerArrayPointerForCall(arg, out var ptr))
                {
                    args.Add(ptr);
                    continue;
                }
            }
            args.Add(LowerExpression(arg));
        }

        var result = NewValue();
        _instructions.AppendLine($"    {result} = call %{externalName}({string.Join(", ", args)})");
        return result;
    }

    private string LowerExternalCallVoid(string externalName, string errorMessage, IReadOnlyList<ExpressionSyntax> arguments, int expectedArgs)
    {
        if (arguments.Count != expectedArgs)
        {
            _diagnostics.Add(new Diagnostic(errorMessage, new SourceSpan(0, 0)));
            return ZeroI32();
        }

        var args = new List<string>(expectedArgs);
        for (int i = 0; i < expectedArgs; i++)
        {
            var arg = arguments[i];
            if (GetExpressionType(arg) is ArrayTypeSymbol)
            {
                if (TryLowerArrayPointerForCall(arg, out var ptr))
                {
                    args.Add(ptr);
                    continue;
                }
            }
            args.Add(LowerExpression(arg));
        }

        _instructions.AppendLine($"    call %{externalName}({string.Join(", ", args)})");
        return ZeroI32();
    }

    private string LowerStrLen(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_len", "str_len expects 1 argument (s: u8[]).");
        }

        var headerSize = HeaderSizeFor("string");
        return LoadUtf8ByteLength(ptr, headerSize);
    }

    private string LowerStrIsEmpty(IReadOnlyList<ExpressionSyntax> arguments)
    {
        if (arguments.Count != 1 || !TryGetStringArg(arguments[0], out var ptr))
        {
            return EmitInvalidBuiltin("str_is_empty", "str_is_empty expects 1 argument (s: u8[]).");
        }

        var headerSize = HeaderSizeFor("string");
        var len = LoadUtf8ByteLength(ptr, headerSize);
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        var cmp = NewValue();
        _instructions.AppendLine($"    {cmp} = icmp eq {len}, {zero}");
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
        var len64 = NewValue();
        _instructions.AppendLine($"    {len64} = call %strlen({ptr})");
        var len = NewValue();
        _instructions.AppendLine($"    {len} = ireduce.i32 {len64}");
        StoreUtf8Lengths(ptr, HeaderSizeFor("string"), len);
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
        StoreUtf8Lengths(dst, HeaderSizeFor("string"), result);
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
        StoreUtf8Lengths(dst, HeaderSizeFor("string"), result);
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
        var one = OneI32();
        var nextIndex = NewValue();
        _instructions.AppendLine($"    {nextIndex} = iadd {len}, {one}");
        var nextAddr = EmitByteAddress(ptr, nextIndex);
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i8 0");
        _instructions.AppendLine($"    store {zero}, {nextAddr}");
        var result = NewValue();
        _instructions.AppendLine($"    {result} = iadd {len}, {one}");
        StoreUtf8Lengths(ptr, HeaderSizeFor("string"), result);
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
        var len = NewValue();
        _instructions.AppendLine($"    {len} = iconst.i32 0");
        StoreUtf8Lengths(ptr, HeaderSizeFor("string"), len);
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
        StoreUtf8Lengths(dst, HeaderSizeFor("string"), byteLen);

        return byteLen;
    }

    private string GetOrCreateFormatString(string format)
    {
        if (_cStringLiterals.TryGetValue(format, out var existing))
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

    private string EmitUtf8HeaderPointer(string payloadPtr, int headerSize)
    {
        var headerPtr = NewValue();
        var offset = NewValue();
        _instructions.AppendLine($"    {offset} = iconst.i64 {-headerSize}");
        _instructions.AppendLine($"    {headerPtr} = iadd {payloadPtr}, {offset}");
        return headerPtr;
    }

    private string LoadUtf8ByteLength(string payloadPtr, int headerSize)
    {
        var headerPtr = EmitUtf8HeaderPointer(payloadPtr, headerSize);
        var len = NewValue();
        _instructions.AppendLine($"    {len} = load.i32 {headerPtr}");
        return len;
    }

    private void StoreUtf8Lengths(string payloadPtr, int headerSize, string byteLen)
    {
        var headerPtr = EmitUtf8HeaderPointer(payloadPtr, headerSize);
        _instructions.AppendLine($"    store {byteLen}, {headerPtr}");
        var lengthOffset = ConstI32(4);
        var charPtr = EmitByteAddress(headerPtr, lengthOffset);
        _instructions.AppendLine($"    store {byteLen}, {charPtr}");
    }

    private bool TryLowerArrayPointer(ExpressionSyntax expr, out string ptr, bool reportErrors = true)
    {
        ptr = string.Empty;
        if (expr is IdentifierExpressionSyntax id)
        {
            if (_locals.TryGetValue(id.Identifier.Text, out var local) &&
                _localTypes.TryGetValue(id.Identifier.Text, out var localType) &&
                localType is ArrayTypeSymbol)
            {
                ptr = NewValue();
                _instructions.AppendLine($"    {ptr} = load.{FormatType(local.Type)} {local.Address}");
                return true;
            }

            if (_symbols.TryGetValue(id.Identifier.Text, out var symbol) && symbol.Type is ArrayTypeSymbol)
            {
                ptr = NewValue();
                _instructions.AppendLine($"    {ptr} = global_value {id.Identifier.Text}");
                if (symbol.Type is ArrayTypeSymbol arrayType && IsStringBuffer(arrayType, out var headerSize))
                {
                    var payload = NewValue();
                    var headerOffset = ConstI64(headerSize);
                    _instructions.AppendLine($"    {payload} = iadd {ptr}, {headerOffset}");
                    ptr = payload;
                }
                return true;
            }
        }

        if (expr is MemberAccessExpressionSyntax member &&
            TryResolveArrayMember(member, out var arrayName))
        {
            ptr = NewValue();
            _instructions.AppendLine($"    {ptr} = global_value {arrayName}");
            if (_symbols.TryGetValue(arrayName, out var symbol) &&
                symbol.Type is ArrayTypeSymbol arrayType &&
                IsStringBuffer(arrayType, out var headerSize))
            {
                var payload = NewValue();
                var headerOffset = ConstI64(headerSize);
                _instructions.AppendLine($"    {payload} = iadd {ptr}, {headerOffset}");
                ptr = payload;
            }
            return true;
        }

        if (reportErrors)
        {
            _diagnostics.Add(new Diagnostic("String built-ins require array arguments.", new SourceSpan(0, 0)));
        }
        return false;
    }

    private bool TryLowerArrayPointerForCall(ExpressionSyntax expr, out string ptr)
    {
        if (TryLowerArrayPointer(expr, out ptr, reportErrors: false))
        {
            return true;
        }

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

        arrayName = $"{baseName}__{member.Member.Text}";
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

    private string ZeroI32()
    {
        var zero = NewValue();
        _instructions.AppendLine($"    {zero} = iconst.i32 0");
        return zero;
    }

    private string LowerAssignment(AssignmentExpressionSyntax assign)
    {
        var value = LowerExpression(assign.Right);
        var valueType = GetExpressionType(assign.Right);
        var isCompound = assign.OperatorToken.Kind != TokenKind.Equal;

        if (isCompound)
        {
            var current = LowerExpression(assign.Left);
            var leftType = GetExpressionType(assign.Left);
            value = LowerCompoundAssignmentValue(assign.OperatorToken.Kind, current, leftType, value, valueType);
            valueType = leftType;
        }

        if (assign.Left is IdentifierExpressionSyntax id)
        {
            var name = id.Identifier.Text;
            if (_elementBindings.TryGetValue(name, out var elementBinding))
            {
                LowerForeachElementStore(elementBinding, value, valueType, assign.Left.Span);
                return value;
            }
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
            if (memberAccess.Receiver is IdentifierExpressionSyntax receiverId &&
                _elementBindings.TryGetValue(receiverId.Identifier.Text, out var elementBinding))
            {
                LowerForeachElementFieldStore(elementBinding, memberAccess.Member.Text, value, valueType, assign.Left.Span);
                return value;
            }
            LowerMemberStore(memberAccess, value, valueType);
        }
        else
        {
            _diagnostics.Add(new Diagnostic("Assignment target not supported in Cranelift backend.", assign.Left.Span));
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
                _localTypes.TryGetValue(id.Identifier.Text, out var localType) &&
                localType is ArrayTypeSymbol localArrayType)
            {
                _instructions.AppendLine($"    {result} = iconst.i32 {localArrayType.Size}");
                return result;
            }

            if (member.Receiver is IdentifierExpressionSyntax globalId &&
                _symbols.TryGetValue(globalId.Identifier.Text, out var symbol) &&
                symbol.Type is ArrayTypeSymbol arrayType)
            {
                _instructions.AppendLine($"    {result} = iconst.i32 {arrayType.Size}");
                return result;
            }

            if (member.Receiver is MemberAccessExpressionSyntax arrayMember &&
                TryResolveMemberBase(arrayMember.Receiver, out _, out var baseType) &&
                baseType is NamedTypeSymbol named &&
                _structs.TryGetValue(named.TypeName, out var structDecl))
            {
                var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == arrayMember.Member.Text);
                if (field?.Type is ArrayTypeSyntax arraySyntax &&
                    int.TryParse(arraySyntax.SizeToken?.Text, out var size))
                {
                    _instructions.AppendLine($"    {result} = iconst.i32 {size}");
                    return result;
                }
            }

            _instructions.AppendLine($"    {result} = iconst.i32 0 ; error: could not resolve array length");
        }
        else if (member.Receiver is IdentifierExpressionSyntax enumId &&
                 _enums.TryGetValue(enumId.Identifier.Text, out var enumDecl))
        {
            var memberIndex = -1;
            for (int i = 0; i < enumDecl.Members.Count; i++)
            {
                if (string.Equals(enumDecl.Members[i].Identifier.Text, member.Member.Text, StringComparison.Ordinal))
                {
                    memberIndex = i;
                    break;
                }
            }

            if (memberIndex >= 0)
            {
                _instructions.AppendLine($"    {result} = iconst.i32 {memberIndex}");
                return result;
            }

            _instructions.AppendLine($"    {result} = iconst.i32 0 ; error: unknown enum member {member.Member.Text}");
        }
        else if (member.Receiver is IdentifierExpressionSyntax receiverId &&
                 _elementBindings.TryGetValue(receiverId.Identifier.Text, out var elementBinding))
        {
            if (elementBinding.ElementType is NamedTypeSymbol)
            {
                return LowerForeachElementFieldAccess(elementBinding, member.Member.Text, member.Span);
            }

            _diagnostics.Add(new Diagnostic("Only struct elements support field access in foreach.", member.Span));
            _instructions.AppendLine($"    {result} = iconst.i32 0");
            return result;
        }
        else if (member.Receiver is ArrayAccessExpressionSyntax arrayAccess)
        {
            return LowerArrayElementFieldAccess(arrayAccess, member.Member.Text);
        }
        else if (TryResolveArrayMember(member, out var arrayName))
        {
            var addr = NewValue();
            _instructions.AppendLine($"    {addr} = global_value {arrayName}");
            if (_symbols.TryGetValue(arrayName, out var symbol) &&
                symbol.Type is ArrayTypeSymbol arrayType &&
                IsStringBuffer(arrayType, out var headerSize))
            {
                var payload = NewValue();
                var headerOffset = ConstI64(headerSize);
                _instructions.AppendLine($"    {payload} = iadd {addr}, {headerOffset}");
                return payload;
            }
            return addr;
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
            _diagnostics.Add(new Diagnostic($"Member access not supported in Cranelift backend: .{member.Member.Text}", member.Span));
            _instructions.AppendLine($"    {result} = iconst.i32 0");
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
        if (_locals.TryGetValue(arrayName, out var local) &&
            _localTypes.TryGetValue(arrayName, out var localType) &&
            localType is ArrayTypeSymbol localArrayType)
        {
            if (IsStringBuffer(localArrayType, out var localHeaderSize))
            {
                var payloadBase = NewValue();
                _instructions.AppendLine($"    {payloadBase} = load.{FormatType(local.Type)} {local.Address}");
                var addr = EmitByteAddress(payloadBase, index);
                var value = NewValue();
                _instructions.AppendLine($"    {value} = load.i8 {addr}");
                var localResult = NewValue();
                _instructions.AppendLine($"    {localResult} = uextend.i32 {value}");
                return localResult;
            }

            var localElemType = _typeMapper.Map(localArrayType.ElementType);
            var localElemSize = GetTypeSize(localElemType);

            var localBaseAddr = NewValue();
            _instructions.AppendLine($"    {localBaseAddr} = load.{FormatType(local.Type)} {local.Address}");

            var localElemSizeVal = NewValue();
            _instructions.AppendLine($"    {localElemSizeVal} = iconst.i64 {localElemSize}");
            var localIndexI64 = NewValue();
            _instructions.AppendLine($"    {localIndexI64} = sextend.i64 {index}");
            var localOffset = NewValue();
            _instructions.AppendLine($"    {localOffset} = imul {localIndexI64}, {localElemSizeVal}");
            var localElemAddr = NewValue();
            _instructions.AppendLine($"    {localElemAddr} = iadd {localBaseAddr}, {localOffset}");

            var localResultValue = NewValue();
            _instructions.AppendLine($"    {localResultValue} = load.{FormatType(localElemType)} {localElemAddr}");
            return localResultValue;
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

        // Load array base address
        var globalBaseAddr = NewValue();
        _instructions.AppendLine($"    {globalBaseAddr} = global_value {arrayName}");

        if (TryGetStringBufferElement(arrayType.ElementType, out var payloadSize, out var elementHeaderSize))
        {
            var stride = payloadSize + elementHeaderSize;
            var strideVal = NewValue();
            _instructions.AppendLine($"    {strideVal} = iconst.i64 {stride}");
            var elemIndexI64 = NewValue();
            _instructions.AppendLine($"    {elemIndexI64} = sextend.i64 {index}");
            var elemOffset = NewValue();
            _instructions.AppendLine($"    {elemOffset} = imul {elemIndexI64}, {strideVal}");
            var elemBase = NewValue();
            _instructions.AppendLine($"    {elemBase} = iadd {globalBaseAddr}, {elemOffset}");
            var payload = NewValue();
            var headerOffset = ConstI64(elementHeaderSize);
            _instructions.AppendLine($"    {payload} = iadd {elemBase}, {headerOffset}");
            return payload;
        }

        if (IsStringBuffer(arrayType, out var globalHeaderSize))
        {
            var payloadBase = NewValue();
            var headerOffset = ConstI64(globalHeaderSize);
            _instructions.AppendLine($"    {payloadBase} = iadd {globalBaseAddr}, {headerOffset}");
            var addr = EmitByteAddress(payloadBase, index);
            var value = NewValue();
            _instructions.AppendLine($"    {value} = load.i8 {addr}");
            var globalResultValue = NewValue();
            _instructions.AppendLine($"    {globalResultValue} = uextend.i32 {value}");
            return globalResultValue;
        }

        // Calculate element size
        var globalElemType = _typeMapper.Map(arrayType.ElementType);
        var globalElemSize = GetTypeSize(globalElemType);

        // Calculate offset: index * elem_size
        var globalElemSizeVal = NewValue();
        _instructions.AppendLine($"    {globalElemSizeVal} = iconst.i64 {globalElemSize}");
        var globalIndexI64 = NewValue();
        _instructions.AppendLine($"    {globalIndexI64} = sextend.i64 {index}");
        var globalOffset = NewValue();
        _instructions.AppendLine($"    {globalOffset} = imul {globalIndexI64}, {globalElemSizeVal}");

        // Calculate element address: base + offset
        var globalElemAddr = NewValue();
        _instructions.AppendLine($"    {globalElemAddr} = iadd {globalBaseAddr}, {globalOffset}");

        // Load the element value
        var globalResult = NewValue();
        _instructions.AppendLine($"    {globalResult} = load.{FormatType(globalElemType)} {globalElemAddr}");

        return globalResult;
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
        var baseName = $"{id.Identifier.Text}__{memberAccess.Member.Text}";

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
            var baseName = $"{structDecl.Name.Text}__{fieldName}";

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
                var baseName = $"{structId.Identifier.Text}__{memberAccess.Member.Text}__{fieldName}";

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
        _diagnostics.Add(new Diagnostic("Array element field access not supported in Cranelift backend.", array.Span));
        _instructions.AppendLine($"    {fallback} = iconst.i32 0");
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
        if (_locals.TryGetValue(arrayName, out var local) &&
            _localTypes.TryGetValue(arrayName, out var localType) &&
            localType is ArrayTypeSymbol localArrayType)
        {
            if (IsStringBuffer(localArrayType, out _))
            {
                var localPayloadBase = NewValue();
                _instructions.AppendLine($"    {localPayloadBase} = load.{FormatType(local.Type)} {local.Address}");
                var addr = EmitByteAddress(localPayloadBase, index);
                var truncated = NewValue();
                _instructions.AppendLine($"    {truncated} = ireduce.i8 {value}");
                _instructions.AppendLine($"    store {truncated}, {addr}");
                return;
            }

            value = CoerceAssignmentValue(value, valueType, localArrayType.ElementType);
            var localElemType = _typeMapper.Map(localArrayType.ElementType);
            var localElemSize = GetTypeSize(localElemType);

            var localBaseAddr = NewValue();
            _instructions.AppendLine($"    {localBaseAddr} = load.{FormatType(local.Type)} {local.Address}");

            var localElemSizeVal = NewValue();
            _instructions.AppendLine($"    {localElemSizeVal} = iconst.i64 {localElemSize}");
            var localIndexI64 = NewValue();
            _instructions.AppendLine($"    {localIndexI64} = sextend.i64 {index}");
            var localOffset = NewValue();
            _instructions.AppendLine($"    {localOffset} = imul {localIndexI64}, {localElemSizeVal}");
            var localElemAddr = NewValue();
            _instructions.AppendLine($"    {localElemAddr} = iadd {localBaseAddr}, {localOffset}");
            _instructions.AppendLine($"    store {value}, {localElemAddr}");
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
        var globalElemType = _typeMapper.Map(arrayType.ElementType);
        var globalElemSize = GetTypeSize(globalElemType);
        value = CoerceAssignmentValue(value, valueType, arrayType.ElementType);

        // Load array base address
        var globalBaseAddr = NewValue();
        _instructions.AppendLine($"    {globalBaseAddr} = global_value {arrayName}");

        if (IsStringBuffer(arrayType, out var globalHeaderSize))
        {
            var payloadBase = NewValue();
            var headerOffset = ConstI64(globalHeaderSize);
            _instructions.AppendLine($"    {payloadBase} = iadd {globalBaseAddr}, {headerOffset}");
            var addr = EmitByteAddress(payloadBase, index);
            var truncated = NewValue();
            _instructions.AppendLine($"    {truncated} = ireduce.i8 {value}");
            _instructions.AppendLine($"    store {truncated}, {addr}");
            return;
        }

        // Calculate offset: index * elem_size
        var globalElemSizeVal = NewValue();
        _instructions.AppendLine($"    {globalElemSizeVal} = iconst.i64 {globalElemSize}");
        var globalIndexI64 = NewValue();
        _instructions.AppendLine($"    {globalIndexI64} = sextend.i64 {index}");
        var globalOffset = NewValue();
        _instructions.AppendLine($"    {globalOffset} = imul {globalIndexI64}, {globalElemSizeVal}");

        // Calculate element address: base + offset
        var globalElemAddr = NewValue();
        _instructions.AppendLine($"    {globalElemAddr} = iadd {globalBaseAddr}, {globalOffset}");

        // Store the value to the element address
        _instructions.AppendLine($"    store {value}, {globalElemAddr}");
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
        var baseName = $"{id.Identifier.Text}__{memberAccess.Member.Text}";

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
            var baseName = $"{structDecl.Name.Text}__{fieldName}";

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
                var baseName = $"{structId.Identifier.Text}__{memberAccess.Member.Text}__{fieldName}";

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

        _diagnostics.Add(new Diagnostic("Array element field store not supported in Cranelift backend.", array.Span));
    }

    private string LowerForeachElementValue(ForeachElementBinding binding, SourceSpan span)
    {
        if (binding.ElementType is NamedTypeSymbol)
        {
            _diagnostics.Add(new Diagnostic("Struct elements must access a field.", span));
            var fallback = NewValue();
            _instructions.AppendLine($"    {fallback} = iconst.i32 0");
            return fallback;
        }

        var arrayAccess = BuildForeachArrayAccess(binding);
        return LowerArrayAccess(arrayAccess);
    }

    private string LowerForeachElementFieldAccess(ForeachElementBinding binding, string fieldName, SourceSpan span)
    {
        var arrayAccess = BuildForeachArrayAccess(binding);
        return LowerArrayElementFieldAccess(arrayAccess, fieldName);
    }

    private void LowerForeachElementStore(ForeachElementBinding binding, string value, TypeSymbol? valueType, SourceSpan span)
    {
        if (binding.ElementType is NamedTypeSymbol)
        {
            _diagnostics.Add(new Diagnostic("Struct elements must assign individual fields.", span));
            return;
        }

        var arrayAccess = BuildForeachArrayAccess(binding);
        LowerArrayStore(arrayAccess, value, valueType);
    }

    private void LowerForeachElementFieldStore(ForeachElementBinding binding, string fieldName, string value, TypeSymbol? valueType, SourceSpan span)
    {
        if (binding.ElementType is not NamedTypeSymbol)
        {
            _diagnostics.Add(new Diagnostic("Only struct elements support field assignment.", span));
            return;
        }

        var arrayAccess = BuildForeachArrayAccess(binding);
        LowerArrayElementFieldStore(arrayAccess, fieldName, value, valueType);
    }

    private ArrayAccessExpressionSyntax BuildForeachArrayAccess(ForeachElementBinding binding)
    {
        var idxToken = new Token(TokenKind.Identifier, binding.IndexName, new SourceSpan(0, 0));
        var idxExpr = new IdentifierExpressionSyntax(idxToken);
        var lbracket = new Token(TokenKind.LBracket, "[", new SourceSpan(0, 0));
        var rbracket = new Token(TokenKind.RBracket, "]", new SourceSpan(0, 0));
        return new ArrayAccessExpressionSyntax(binding.Iterable, lbracket, idxExpr, rbracket);
    }

    private bool TryResolveForeachIterable(ExpressionSyntax iterable, out ForeachIterableInfo info)
    {
        info = new ForeachIterableInfo(iterable, new NamedTypeSymbol("unknown"), 0);

        if (iterable is IdentifierExpressionSyntax id)
        {
            if (_localTypes.TryGetValue(id.Identifier.Text, out var localType) && localType is ArrayTypeSymbol localArray)
            {
                info = new ForeachIterableInfo(iterable, localArray.ElementType, localArray.Size);
                return localArray.Size > 0;
            }

            if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Type is ArrayTypeSymbol array)
            {
                info = new ForeachIterableInfo(iterable, array.ElementType, array.Size);
                return array.Size > 0;
            }
        }

        if (iterable is MemberAccessExpressionSyntax member &&
            TryResolveMemberBase(member.Receiver, out _, out var baseType) &&
            baseType is NamedTypeSymbol named &&
            _structs.TryGetValue(named.TypeName, out var structDecl))
        {
            var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
            if (field?.Type is ArrayTypeSyntax arraySyntax)
            {
                var elementType = ResolveType(arraySyntax.ElementType);
                var length = int.TryParse(arraySyntax.SizeToken?.Text, out var size) ? size : 0;
                info = new ForeachIterableInfo(iterable, elementType, length);
                return length > 0;
            }
        }

        return false;
    }

    private void RestoreLocal(string name, LocalSlot? slot, TypeSymbol? type)
    {
        if (slot is not null && type is not null)
        {
            _locals[name] = slot;
            _localTypes[name] = type;
        }
        else
        {
            _locals.Remove(name);
            _localTypes.Remove(name);
        }
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

        flattenedName = $"{baseName}__{member.Member.Text}";
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

    private static int HeaderSizeFor(string name) =>
        name switch
        {
            "string" => 8,
            "utf8" => 8,
            "ascii" => 4,
            _ => 0
        };

    private static bool IsStringBuffer(ArrayTypeSymbol arrayType, out int headerSize)
    {
        headerSize = 0;
        if (arrayType.ElementType is PrimitiveTypeSymbol prim)
        {
            headerSize = HeaderSizeFor(prim.PrimitiveName);
            return headerSize > 0;
        }

        return false;
    }

    private static bool TryGetStringBufferElement(TypeSymbol? elementType, out int payloadSize, out int headerSize)
    {
        payloadSize = 0;
        headerSize = 0;
        if (elementType is ArrayTypeSymbol array &&
            array.ElementType is PrimitiveTypeSymbol prim)
        {
            headerSize = HeaderSizeFor(prim.PrimitiveName);
            if (headerSize <= 0)
            {
                return false;
            }

            payloadSize = array.Size;
            return true;
        }

        return false;
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

    public sealed record ConstValue(TypeSymbol Type, TokenKind LiteralKind, string LiteralText);

    private string EmitConstValue(ConstValue value)
    {
        var val = NewValue();
        var type = value.Type;
        if (type is PrimitiveTypeSymbol prim)
        {
            switch (prim.PrimitiveName)
            {
                case "f32":
                    _instructions.AppendLine($"    {val} = f32const {FormatFloatLiteral(value)}");
                    return val;
                case "f64":
                    _instructions.AppendLine($"    {val} = f64const {FormatFloatLiteral(value)}");
                    return val;
                case "bool":
                    _instructions.AppendLine($"    {val} = iconst.i32 {FormatBoolLiteral(value)}");
                    return val;
            }
        }

        var clifType = _typeMapper.Map(type);
        var iconstType = FormatType(clifType);
        _instructions.AppendLine($"    {val} = iconst.{iconstType} {FormatIntLiteral(value)}");
        return val;
    }

    private static string FormatBoolLiteral(ConstValue value) =>
        value.LiteralKind switch
        {
            TokenKind.TrueKeyword => "1",
            TokenKind.FalseKeyword => "0",
            _ => "0"
        };

    private static string FormatIntLiteral(ConstValue value)
    {
        if (int.TryParse(value.LiteralText, out var intValue))
        {
            return intValue.ToString(System.Globalization.CultureInfo.InvariantCulture);
        }

        return "0";
    }

    private static string FormatFloatLiteral(ConstValue value)
    {
        if (float.TryParse(value.LiteralText, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out var floatValue))
        {
            return floatValue.ToString(System.Globalization.CultureInfo.InvariantCulture);
        }

        if (int.TryParse(value.LiteralText, out var intValue))
        {
            return ((float)intValue).ToString(System.Globalization.CultureInfo.InvariantCulture);
        }

        return "0.0";
    }

    private string LowerCompoundAssignmentValue(TokenKind opKind, string left, TypeSymbol? leftType, string right, TypeSymbol? rightType)
    {
        var isFloat = IsFloatType(leftType) || IsFloatType(rightType);
        if (isFloat)
        {
            var useF64 = IsF64Type(leftType) || IsF64Type(rightType);
            left = CoerceFloatOperand(left, leftType, useF64);
            right = CoerceFloatOperand(right, rightType, useF64);
        }

        var op = opKind switch
        {
            TokenKind.PlusEqual => isFloat ? "fadd" : "iadd",
            TokenKind.MinusEqual => isFloat ? "fsub" : "isub",
            TokenKind.StarEqual => isFloat ? "fmul" : "imul",
            TokenKind.SlashEqual => isFloat ? "fdiv" : "sdiv",
            TokenKind.PercentEqual => "srem",
            _ => "iadd"
        };

        var result = NewValue();
        _instructions.AppendLine($"    {result} = {op} {left}, {right}");
        return result;
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
                TokenKind.StringLiteral => new PrimitiveTypeSymbol("string_literal"),
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
            MemberAccessExpressionSyntax member when TryGetMemberType(member, out var memberType) => memberType,
            ArrayAccessExpressionSyntax array when GetExpressionType(array.Receiver) is ArrayTypeSymbol arr => arr.ElementType,
            BinaryExpressionSyntax bin => GetBinaryResultType(bin),
            OperatorCallExpressionSyntax op => GetOperatorCallResultType(op),
            _ => null
        };
    }

    private bool TryGetMemberType(MemberAccessExpressionSyntax member, out TypeSymbol? memberType)
    {
        // Array length property returns i32.
        var receiverType = GetExpressionType(member.Receiver);
        if (member.Member.Text == "length" && receiverType is ArrayTypeSymbol)
        {
            memberType = new PrimitiveTypeSymbol("i32");
            return true;
        }

        // Enum members are recorded in the symbol table (e.g., Color.Red).
        if (member.Receiver is IdentifierExpressionSyntax enumId &&
            _symbols.TryGetValue($"{enumId.Identifier.Text}.{member.Member.Text}", out var memberSym))
        {
            memberType = memberSym.Type;
            return memberType is not null;
        }

        // Foreach element bindings know their struct element type.
        if (member.Receiver is IdentifierExpressionSyntax iterId &&
            _elementBindings.TryGetValue(iterId.Identifier.Text, out var binding) &&
            binding.ElementType is NamedTypeSymbol iterStruct &&
            _structs.TryGetValue(iterStruct.TypeName, out var iterStructDecl))
        {
            var field = iterStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
            if (field is not null)
            {
                memberType = ResolveType(field.Type);
                return true;
            }
        }

        // Globals/flattened structs.
        if (TryResolveMemberBase(member.Receiver, out _, out var baseType) &&
            baseType is NamedTypeSymbol named &&
            _structs.TryGetValue(named.TypeName, out var structDecl))
        {
            var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
            if (field is not null)
            {
                memberType = ResolveType(field.Type);
                return true;
            }
        }

        // Already-flattened member (struct field lowered to separate global).
        if (TryResolveFlattenedMember(member, out _, out var flattenedType))
        {
            memberType = flattenedType;
            return true;
        }

        memberType = null;
        return false;
    }

    private TypeSymbol? GetOperatorCallResultType(OperatorCallExpressionSyntax op)
    {
        var opText = op.OperatorToken.Text;
        if (opText is "==" or "!=" or "<" or "<=" or ">" or ">=" or "&&" or "||")
        {
            return new PrimitiveTypeSymbol("bool");
        }

        var leftType = GetExpressionType(op.Receiver);
        var rightType = op.Arguments.Count > 0 ? GetExpressionType(op.Arguments[0]) : null;
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

    private sealed record ForeachIterableInfo(ExpressionSyntax Iterable, TypeSymbol ElementType, int Length);

    private sealed record ForeachElementBinding(ExpressionSyntax Iterable, TypeSymbol ElementType, string IndexName);

    private sealed record LocalSlot(string Address, CraneliftTypeMapper.ClifType Type);
}
