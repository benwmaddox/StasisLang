using LLVMSharp.Interop;
using Stasis.Compiler;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
using System.Globalization;
using System;

namespace Stasis.Compiler.IR;

/// <summary>
/// End-to-end lowering of parsed and analyzed Stasis code into an LLVM module.
/// Emits SoA globals, function prototypes, and basic bodies for simple expressions/returns.
/// </summary>
public sealed class ModuleLowerer
{
    public LowerResult LowerToIr(CompilationUnitSyntax compilationUnit, SemanticResult semantic, LayoutPlan layout, string moduleName = "module", LowerOptions? options = null)
    {
        var opts = options ?? LowerOptions.Default;
        using var builder = new LlvmModuleBuilder(moduleName);
        EmitGlobals(compilationUnit, semantic.Symbols, layout, builder);
        EmitFunctionSignatures(compilationUnit, semantic.Symbols, builder, opts.IncludeTests);

        var diagnostics = new List<Diagnostic>();
        var lowerer = new FunctionLowerer(builder, semantic.Symbols, layout, diagnostics, opts.IncludeTests);
        lowerer.Lower(compilationUnit, opts.IncludeTests);

        if (opts.IncludeTests && opts.EmitTestHarness)
        {
            EmitTestHarness(compilationUnit, builder, semantic.Symbols, diagnostics);
        }

        return new LowerResult(builder.EmitToString(), diagnostics);
    }

    private static void EmitGlobals(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols, LayoutPlan layout, LlvmModuleBuilder builder)
    {
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
        var layoutMap = layout.Globals.ToDictionary(g => g.Name, g => g, StringComparer.Ordinal);

        foreach (var global in compilationUnit.Declarations.OfType<GlobalDeclarationSyntax>())
        {
            layoutMap.TryGetValue(global.Name.Text, out var globalLayout);
            switch (global.Type)
            {
                case ArrayTypeSyntax array when array.ElementType is NamedTypeSyntax named && structs.TryGetValue(named.Name, out var structDecl):
                    {
                        foreach (var field in structDecl.Fields)
                        {
                            var fieldType = ResolveType(field.Type, symbols);
                            var llvmElem = builder.TypeMapper.Map(fieldType);
                            var fieldLayout = globalLayout?.Fields.FirstOrDefault(f => string.Equals(f.Name, $"{structDecl.Name.Text}_{field.Identifier.Text}", StringComparison.Ordinal));
                            var length = fieldLayout is null
                                ? ParseArrayLength(array.SizeToken.Text)
                                : (uint)Math.Max(1, fieldLayout.Size / SizeOf(fieldType));
                            builder.DefineGlobalArray($"{structDecl.Name.Text}_{field.Identifier.Text}", llvmElem, length);
                        }

                        break;
                    }
                case ArrayTypeSyntax array:
                    {
                        var elementType = ResolveType(array.ElementType, symbols);
                        var llvmElem = builder.TypeMapper.Map(elementType);
                        var length = globalLayout is null
                            ? ParseArrayLength(array.SizeToken.Text)
                            : (uint)Math.Max(1, globalLayout.Size / SizeOf(elementType));
                        builder.DefineGlobalArray(global.Name.Text, llvmElem, length);
                        break;
                    }
                case NamedTypeSyntax named:
                    {
                        var type = ResolveType(named, symbols);
                        var llvmType = builder.TypeMapper.Map(type);
                        builder.DefineGlobalScalar(global.Name.Text, llvmType);
                        break;
                    }
            }
        }
    }

    private static uint ParseArrayLength(string text) =>
        uint.TryParse(text, out var n) ? n : 0;

    private static int SizeOf(TypeSymbol type) =>
        type switch
        {
            PrimitiveTypeSymbol p => SizeOfPrimitive(p.PrimitiveName),
            NamedTypeSymbol => 4, // indices or placeholders
            ArrayTypeSymbol a => SizeOf(a.ElementType) * a.Size,
            _ => 4
        };

    private static int SizeOfPrimitive(string name) =>
        name switch
        {
            "bool" or "u8" => 1,
            "u16" => 2,
            "u32" or "i32" or "f32" => 4,
            "f64" => 8,
            _ => 4
        };

    private void EmitTestHarness(CompilationUnitSyntax compilationUnit, LlvmModuleBuilder builder, IReadOnlyDictionary<string, Symbol> symbols, List<Diagnostic> diagnostics)
    {
        var tests = compilationUnit.Declarations.OfType<TestDeclarationSyntax>().ToList();
        var int32 = LLVMTypeRef.Int32;
        var harness = builder.DefineFunction("run_tests", int32);
        using var llvmBuilder = builder.Context.CreateBuilder();
        var entry = harness.AppendBasicBlock("entry");
        llvmBuilder.PositionAtEnd(entry);
        var failures = llvmBuilder.BuildAlloca(int32, "failures");
        llvmBuilder.BuildStore(ConstInt(int32, 0), failures);

        var totalTests = tests.Count;
        var (putsFn, putsType) = GetOrDeclarePuts(builder);

        foreach (var testDecl in tests)
        {
            if (testDecl.Parameters.Count > 0)
            {
                diagnostics.Add(new Diagnostic("Test harness supports parameterless tests only.", testDecl.Name.Span));
                continue;
            }

            var testFn = builder.Module.GetNamedFunction(testDecl.Name.Text);
            if (testFn.Handle == IntPtr.Zero)
            {
                continue;
            }

            var retSymbol = testDecl.ReturnType is null
                ? new PrimitiveTypeSymbol("i32")
                : ResolveType(testDecl.ReturnType, symbols);
            var retLlvm = builder.TypeMapper.Map(retSymbol);
            var fnType = LLVMTypeRef.CreateFunction(retLlvm, Array.Empty<LLVMTypeRef>(), false);

            var call = llvmBuilder.BuildCall2(fnType, testFn, Array.Empty<LLVMValueRef>(), $"{testDecl.Name.Text}.call");
            if (retLlvm.Kind == LLVMTypeKind.LLVMVoidTypeKind)
            {
                continue;
            }

            var ok = AsBoolean(llvmBuilder, call);
            var passMsg = llvmBuilder.BuildGlobalStringPtr($"\u001b[32mPASS\u001b[0m: {testDecl.Name.Text}", $"{testDecl.Name.Text}.passmsg");
            var failMsg = llvmBuilder.BuildGlobalStringPtr($"\u001b[31mFAIL\u001b[0m: {testDecl.Name.Text}", $"{testDecl.Name.Text}.failmsg");
            var msg = llvmBuilder.BuildSelect(ok, passMsg, failMsg, $"{testDecl.Name.Text}.msg");
            llvmBuilder.BuildCall2(putsType, putsFn, new[] { msg }, $"{testDecl.Name.Text}.print");

            var fail = llvmBuilder.BuildNot(ok, $"{testDecl.Name.Text}.fail");
            var failI32 = llvmBuilder.BuildZExt(fail, int32, $"{testDecl.Name.Text}.faili32");
            var cur = llvmBuilder.BuildLoad2(int32, failures, "failcur");
            var next = llvmBuilder.BuildAdd(cur, failI32, "failnext");
            llvmBuilder.BuildStore(next, failures);
        }

        var result = llvmBuilder.BuildLoad2(int32, failures, "failures.result");

        // Print a simple summary: Tests: passed=X failed=Y
        var (printf, printfType) = GetOrDeclarePrintf(builder);
        var fmtPass = llvmBuilder.BuildGlobalStringPtr("Tests: \u001b[32mpassed=%d\u001b[0m failed=%d\n", "tests_fmt_pass");
        var fmtFail = llvmBuilder.BuildGlobalStringPtr("Tests: passed=%d \u001b[31mfailed=%d\u001b[0m\n", "tests_fmt_fail");
        var passed = llvmBuilder.BuildSub(ConstInt(int32, totalTests), result, "tests.passed");
        var hasFailures = llvmBuilder.BuildICmp(LLVMIntPredicate.LLVMIntNE, result, ConstInt(int32, 0), "has_failures");
        var summaryFmt = llvmBuilder.BuildSelect(hasFailures, fmtFail, fmtPass, "tests_fmt");
        llvmBuilder.BuildCall2(printfType, printf, new[] { summaryFmt, passed, result }, "printf.tests");

        llvmBuilder.BuildRet(result);
    }

    private static LLVMValueRef AsBoolean(LLVMBuilderRef builder, LLVMValueRef value)
    {
        var type = value.TypeOf;
        if (type.Kind == LLVMTypeKind.LLVMIntegerTypeKind)
        {
            if (type.IntWidth == 1)
            {
                return value;
            }

            var zero = LLVMValueRef.CreateConstInt(type, 0, false);
            return builder.BuildICmp(LLVMIntPredicate.LLVMIntNE, value, zero, "to_bool");
        }

        if (type.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind)
        {
            var zero = LLVMValueRef.CreateConstReal(type, 0);
            return builder.BuildFCmp(LLVMRealPredicate.LLVMRealONE, value, zero, "to_bool");
        }

        return value;
    }

    private static LLVMValueRef ConstInt(LLVMTypeRef type, int value) =>
        LLVMValueRef.CreateConstInt(type, (ulong)value, true);

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclarePrintf(LlvmModuleBuilder builder)
    {
        var printf = builder.Module.GetNamedFunction("printf");
        LLVMTypeRef printfType;
        if (printf.Handle != IntPtr.Zero)
        {
            printfType = GetFunctionType(printf);
            return (printf, printfType);
        }

        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        printfType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new LLVMTypeRef[] { i8Ptr, LLVMTypeRef.Int32, LLVMTypeRef.Int32 }, false);
        printf = builder.Module.AddFunction("printf", printfType);
        return (printf, printfType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclarePuts(LlvmModuleBuilder builder)
    {
        var puts = builder.Module.GetNamedFunction("puts");
        if (puts.Handle != IntPtr.Zero)
        {
            var type = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
            return (puts, type);
        }

        var putsType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
        puts = builder.Module.AddFunction("puts", putsType);
        return (puts, putsType);
    }

    private static LLVMTypeRef GetFunctionType(LLVMValueRef fn)
    {
        var type = fn.TypeOf;
        return type.Kind == LLVMTypeKind.LLVMPointerTypeKind ? type.ElementType : type;
    }

    private sealed class FunctionLowerer
    {
        private readonly LlvmModuleBuilder _moduleBuilder;
        private readonly IReadOnlyDictionary<string, Symbol> _symbols;
        private readonly Dictionary<string, GlobalLayout> _globalLayouts;
        private readonly List<Diagnostic> _diagnostics;
        private Dictionary<string, StructDeclarationSyntax> _structs = new(StringComparer.Ordinal);
        private Dictionary<string, FunctionDeclarationSyntax> _functions = new(StringComparer.Ordinal);
        private Dictionary<string, TestDeclarationSyntax> _tests = new(StringComparer.Ordinal);
        private int _blockId;
        public FunctionLowerer(LlvmModuleBuilder moduleBuilder, IReadOnlyDictionary<string, Symbol> symbols, LayoutPlan layout, List<Diagnostic> diagnostics, bool includeTests)
        {
            _moduleBuilder = moduleBuilder;
            _symbols = symbols;
            _globalLayouts = layout.Globals.ToDictionary(g => g.Name, g => g, StringComparer.Ordinal);
            _diagnostics = diagnostics;
        }

        public void Lower(CompilationUnitSyntax compilationUnit, bool includeTests)
        {
            _structs = compilationUnit.Declarations
                .OfType<StructDeclarationSyntax>()
                .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
            _functions = compilationUnit.Declarations
                .OfType<FunctionDeclarationSyntax>()
                .ToDictionary(f => f.Name.Text, f => f, StringComparer.Ordinal);
            _tests = compilationUnit.Declarations
                .OfType<TestDeclarationSyntax>()
                .ToDictionary(t => t.Name.Text, t => t, StringComparer.Ordinal);

            foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
            {
                LowerFunction(fn);
            }

            if (includeTests)
            {
                foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
                {
                    LowerFunction(test);
                }
            }
        }

        private void LowerFunction(FunctionDeclarationSyntax fn)
        {
            LowerFunctionCore(fn.Name.Text, fn.Parameters, fn.ReturnType, fn.Body);
        }

        private void LowerFunction(TestDeclarationSyntax test)
        {
            LowerFunctionCore(test.Name.Text, test.Parameters, test.ReturnType, test.Body);
        }

        private readonly record struct LocalBinding(LLVMValueRef Value, LLVMTypeRef Type, bool IsAddress);

        private void LowerFunctionCore(string name, IReadOnlyList<ParameterSyntax> parameters, TypeSyntax? returnType, BlockStatementSyntax body)
        {
            var function = _moduleBuilder.Module.GetNamedFunction(name);
            if (function.Handle == IntPtr.Zero)
            {
                return;
            }

            using var builder = _moduleBuilder.Context.CreateBuilder();
            var entry = function.AppendBasicBlock("entry");
            builder.PositionAtEnd(entry);
            _blockId = 0;

            var locals = new Dictionary<string, LocalBinding>(StringComparer.Ordinal);

            for (int i = 0; i < parameters.Count; i++)
            {
                var param = parameters[i];
                var paramVal = function.GetParam((uint)i);
                var paramType = ResolveType(param.Type, _symbols);
                locals[param.Name.Text] = new LocalBinding(paramVal, _moduleBuilder.TypeMapper.Map(paramType), false);
            }

            var terminated = LowerBlock(builder, function, body, locals);
            var isVoid = returnType is null || (returnType is NamedTypeSyntax named && string.Equals(named.Name, "void", StringComparison.Ordinal));
            if (!terminated && isVoid)
            {
                builder.BuildRetVoid();
            }
        }

        private bool LowerBlock(LLVMBuilderRef builder, LLVMValueRef function, BlockStatementSyntax block, Dictionary<string, LocalBinding> locals)
        {
            var scope = new Dictionary<string, LocalBinding>(locals, StringComparer.Ordinal);
            foreach (var stmt in block.Statements)
            {
                if (LowerStatement(builder, function, stmt, scope))
                {
                    return true;
                }
            }

            return false;
        }

        private bool LowerStatement(LLVMBuilderRef builder, LLVMValueRef function, StatementSyntax stmt, Dictionary<string, LocalBinding> locals)
        {
            switch (stmt)
            {
                case BlockStatementSyntax block:
                    return LowerBlock(builder, function, block, locals);
                case VariableDeclarationSyntax decl:
                    LowerVariableDeclaration(builder, decl, locals);
                    return false;
                case ExpressionStatementSyntax exprStmt:
                    LowerExpression(builder, exprStmt.Expression, locals);
                    return false;
                case ReturnStatementSyntax ret:
                    if (ret.Expression is null)
                    {
                        builder.BuildRetVoid();
                    }
                    else
                    {
                        var value = LowerExpression(builder, ret.Expression, locals);
                        builder.BuildRet(value);
                    }

                    return true;
                case IfStatementSyntax ifs:
                    return LowerIf(builder, function, ifs, locals);
                case ForStatementSyntax @for:
                    return LowerFor(builder, function, @for, locals);
                case ForeachStatementSyntax foreachStmt:
                    return LowerForeach(builder, function, foreachStmt, locals);
                default:
                    return false;
            }
        }

        private void LowerVariableDeclaration(LLVMBuilderRef builder, VariableDeclarationSyntax decl, Dictionary<string, LocalBinding> locals)
        {
            if (decl.Type is null)
            {
                return;
            }

            var type = ResolveType(decl.Type, _symbols);
            var llvmType = _moduleBuilder.TypeMapper.Map(type);
            var alloca = builder.BuildAlloca(llvmType, decl.Name.Text);
            locals[decl.Name.Text] = new LocalBinding(alloca, llvmType, true);
        }

        private LLVMValueRef LowerExpression(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals)
        {
            switch (expr)
            {
                case LiteralExpressionSyntax lit:
                    return LowerLiteral(lit);
                case IdentifierExpressionSyntax id:
                    if (locals.TryGetValue(id.Identifier.Text, out var value))
                    {
                        if (value.IsAddress)
                        {
                            return builder.BuildLoad2(value.Type, value.Value, id.Identifier.Text);
                        }

                        return value.Value;
                    }

                    if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is not null)
                    {
                        var global = _moduleBuilder.Module.GetNamedGlobal(id.Identifier.Text);
                        var type = _moduleBuilder.TypeMapper.Map(sym.Type);
                        return builder.BuildLoad2(type, global, id.Identifier.Text);
                    }

                    return ConstI32(0);
                case MemberAccessExpressionSyntax member:
                    return LowerMemberAccess(builder, member, locals);
                case ArrayAccessExpressionSyntax arr:
                    return LowerArrayAccess(builder, arr, null, locals);
                case ParenthesizedExpressionSyntax paren:
                    return LowerExpression(builder, paren.Expression, locals);
                case UnaryExpressionSyntax unary:
                    return LowerUnary(builder, unary, locals);
                case CallExpressionSyntax call:
                    return LowerCall(builder, call, locals);
                case OperatorCallExpressionSyntax op:
                    return LowerOperatorCall(builder, op, locals);
                default:
                    AddDiagnostic("Expression not supported during lowering.", expr.Span);
                    return ConstI32(0);
            }
        }

        private LLVMValueRef LowerLiteral(LiteralExpressionSyntax lit)
        {
            switch (lit.Literal.Kind)
            {
                case TokenKind.IntegerLiteral when int.TryParse(lit.Literal.Text, out var ival):
                    return ConstI32(ival);
                case TokenKind.FloatLiteral when float.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var fval):
                    return LLVMValueRef.CreateConstReal(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("f32")), fval);
                case TokenKind.TrueKeyword:
                    return ConstI32(1);
                case TokenKind.FalseKeyword:
                    return ConstI32(0);
                default:
                    return ConstI32(0);
            }
        }

        private LLVMValueRef LowerUnary(LLVMBuilderRef builder, UnaryExpressionSyntax unary, Dictionary<string, LocalBinding> locals)
        {
            var operand = LowerExpression(builder, unary.Operand, locals);
            return unary.OperatorToken.Kind switch
            {
                TokenKind.Minus => LowerNeg(builder, operand),
                TokenKind.Bang => LowerLogicalNot(builder, operand),
                _ => operand
            };
        }

        private LLVMValueRef LowerCall(LLVMBuilderRef builder, CallExpressionSyntax call, Dictionary<string, LocalBinding> locals)
        {
            if (call.Callee is not IdentifierExpressionSyntax id)
            {
                AddDiagnostic("Only simple function calls are supported.", call.Span);
                return ConstI32(0);
            }

            if (!_symbols.TryGetValue(id.Identifier.Text, out var sym) || sym.Kind is not (SymbolKind.Function or SymbolKind.Test))
            {
                AddDiagnostic($"Unknown function '{id.Identifier.Text}'.", call.Span);
                return ConstI32(0);
            }

            var fn = _moduleBuilder.Module.GetNamedFunction(id.Identifier.Text);
            if (fn.Handle == IntPtr.Zero)
            {
                AddDiagnostic($"Function '{id.Identifier.Text}' missing from module.", call.Span);
                return ConstI32(0);
            }

            var argValues = call.Arguments.Select(a => LowerExpression(builder, a, locals)).ToArray();
            var signature = ResolveFunctionSignature(id.Identifier.Text);
            var fnType = LLVMTypeRef.CreateFunction(signature.ReturnType, signature.Parameters, false);

            var callValue = builder.BuildCall2(fnType, fn, argValues, $"{id.Identifier.Text}.call");
            var callRetType = fnType.ReturnType;
            if (callRetType.Kind == LLVMTypeKind.LLVMVoidTypeKind)
            {
                return ConstI32(0);
            }

            return callValue;
        }

        private LLVMValueRef LowerOperatorCall(LLVMBuilderRef builder, OperatorCallExpressionSyntax op, Dictionary<string, LocalBinding> locals)
        {
            var opText = op.OperatorToken.Text;
            if (op.Arguments.Count != 1)
            {
                AddDiagnostic($"Operator '.{opText}()' requires exactly one argument.", op.Span);
                return ConstI32(0);
            }

            if (opText == "=")
            {
                var receiver = op.Receiver as IdentifierExpressionSyntax;
                if (receiver is not null && locals.TryGetValue(receiver.Identifier.Text, out var target))
                {
                    var value = LowerExpression(builder, op.Arguments[0], locals);
                    if (target.IsAddress)
                    {
                        builder.BuildStore(value, target.Value);
                    }

                    return value;
                }

                if (receiver is not null && _symbols.TryGetValue(receiver.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is not null)
                {
                    var value = LowerExpression(builder, op.Arguments[0], locals);
                    var global = _moduleBuilder.Module.GetNamedGlobal(receiver.Identifier.Text);
                    var type = _moduleBuilder.TypeMapper.Map(sym.Type);
                    builder.BuildStore(value, global);
                    return value;
                }

                if (op.Receiver is ArrayAccessExpressionSyntax arr)
                {
                    if (TryLowerArrayElementPointer(builder, arr, null, locals, out var ptr, out var elemType))
                    {
                        var value = LowerExpression(builder, op.Arguments[0], locals);
                        builder.BuildStore(value, ptr);
                        return value;
                    }
                }

                if (op.Receiver is MemberAccessExpressionSyntax member && member.Receiver is ArrayAccessExpressionSyntax arrRecv)
                {
                    if (TryLowerArrayElementPointer(builder, arrRecv, member.Member.Text, locals, out var ptr, out _))
                    {
                        var value = LowerExpression(builder, op.Arguments[0], locals);
                        builder.BuildStore(value, ptr);
                        return value;
                    }
                }

                AddDiagnostic("Left side of .=( ) must be an assignable location (identifier, field, or array element).", op.Receiver.Span);
                return LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), 0, false);
            }

            var lhs = LowerExpression(builder, op.Receiver, locals);
            var rhs = LowerExpression(builder, op.Arguments[0], locals);
            return LowerBinary(builder, opText, lhs, rhs, op.Span);
        }

        private LLVMValueRef LowerBinary(LLVMBuilderRef builder, string op, LLVMValueRef lhs, LLVMValueRef rhs, SourceSpan span)
        {
            var type = lhs.TypeOf;
            var isFloat = type.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;
            return op switch
            {
                "+" when isFloat => builder.BuildFAdd(lhs, rhs, "faddtmp"),
                "+" => builder.BuildAdd(lhs, rhs, "addtmp"),
                "-" when isFloat => builder.BuildFSub(lhs, rhs, "fsubtmp"),
                "-" => builder.BuildSub(lhs, rhs, "subtmp"),
                "*" when isFloat => builder.BuildFMul(lhs, rhs, "fmultmp"),
                "*" => builder.BuildMul(lhs, rhs, "multmp"),
                "/" when isFloat => builder.BuildFDiv(lhs, rhs, "fdivtmp"),
                "/" => builder.BuildSDiv(lhs, rhs, "divtmp"),
                "%" when isFloat => lhs,
                "%" => builder.BuildSRem(lhs, rhs, "remtmp"),
                "<" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOLT, lhs, rhs, "flt")),
                "<" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, lhs, rhs, "ilt")),
                ">" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOGT, lhs, rhs, "fgt")),
                ">" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntSGT, lhs, rhs, "igt")),
                "==" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOEQ, lhs, rhs, "feq")),
                "==" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, lhs, rhs, "ieq")),
                _ => UnsupportedOperator(span, lhs)
            };
        }

        private LLVMValueRef BuildBoolResult(LLVMBuilderRef builder, LLVMValueRef value) =>
            builder.BuildZExt(value, LLVMTypeRef.Int32, "booltmp");

        private bool LowerIf(LLVMBuilderRef builder, LLVMValueRef function, IfStatementSyntax ifs, Dictionary<string, LocalBinding> locals)
        {
            var thenBlock = function.AppendBasicBlock(NextBlockName("if.then"));
            var mergeBlock = function.AppendBasicBlock(NextBlockName("if.end"));
            var elseBlock = ifs.ElseBlock is not null ? function.AppendBasicBlock(NextBlockName("if.else")) : default;

            var cond = AsBoolean(builder, LowerExpression(builder, ifs.Condition, locals));
            if (ifs.ElseBlock is null)
            {
                builder.BuildCondBr(cond, thenBlock, mergeBlock);
            }
            else
            {
                builder.BuildCondBr(cond, thenBlock, elseBlock);
            }

            builder.PositionAtEnd(thenBlock);
            var thenTerminated = LowerBlock(builder, function, ifs.ThenBlock, locals);
            if (!thenTerminated)
            {
                builder.BuildBr(mergeBlock);
            }

            var elseTerminated = false;
            if (ifs.ElseBlock is not null)
            {
                builder.PositionAtEnd(elseBlock);
                elseTerminated = LowerBlock(builder, function, ifs.ElseBlock, locals);
                if (!elseTerminated)
                {
                    builder.BuildBr(mergeBlock);
                }
            }

            if (!thenTerminated || ifs.ElseBlock is null || !elseTerminated)
            {
                builder.PositionAtEnd(mergeBlock);
                return false;
            }

            builder.PositionAtEnd(mergeBlock);
            builder.BuildUnreachable();
            return true;
        }

        private bool LowerFor(LLVMBuilderRef builder, LLVMValueRef function, ForStatementSyntax @for, Dictionary<string, LocalBinding> locals)
        {
            var condBlock = function.AppendBasicBlock(NextBlockName("for.cond"));
            var bodyBlock = function.AppendBasicBlock(NextBlockName("for.body"));
            var latchBlock = function.AppendBasicBlock(NextBlockName("for.latch"));
            var exitBlock = function.AppendBasicBlock(NextBlockName("for.end"));

            if (@for.Initializer is not null)
            {
                LowerExpression(builder, @for.Initializer, locals);
            }

            builder.BuildBr(condBlock);

            builder.PositionAtEnd(condBlock);
            var condValue = @for.Condition is null
                ? ConstBool(true)
                : AsBoolean(builder, LowerExpression(builder, @for.Condition, locals));
            builder.BuildCondBr(condValue, bodyBlock, exitBlock);

            builder.PositionAtEnd(bodyBlock);
            var bodyTerminated = LowerBlock(builder, function, @for.Body, locals);
            if (!bodyTerminated)
            {
                builder.BuildBr(latchBlock);
            }

            builder.PositionAtEnd(latchBlock);
            if (@for.Step is not null)
            {
                LowerExpression(builder, @for.Step, locals);
            }

            builder.BuildBr(condBlock);
            builder.PositionAtEnd(exitBlock);
            return false;
        }

        private bool LowerForeach(LLVMBuilderRef builder, LLVMValueRef function, ForeachStatementSyntax foreachStmt, Dictionary<string, LocalBinding> locals)
        {
            var condBlock = function.AppendBasicBlock(NextBlockName("foreach.cond"));
            var bodyBlock = function.AppendBasicBlock(NextBlockName("foreach.body"));
            var latchBlock = function.AppendBasicBlock(NextBlockName("foreach.latch"));
            var exitBlock = function.AppendBasicBlock(NextBlockName("foreach.end"));

            var i32 = _moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32"));
            var iterator = builder.BuildAlloca(i32, foreachStmt.Iterator.Text);
            var loopLocals = new Dictionary<string, LocalBinding>(locals, StringComparer.Ordinal)
            {
                [foreachStmt.Iterator.Text] = new LocalBinding(iterator, i32, true)
            };

            builder.BuildStore(ConstI32(0), iterator);

            var length = ResolveIterableLength(foreachStmt.Iterable);
            var lengthValue = LLVMValueRef.CreateConstInt(i32, (ulong)length, true);

            builder.BuildBr(condBlock);

            builder.PositionAtEnd(condBlock);
            var currentIndex = builder.BuildLoad2(i32, iterator, $"{foreachStmt.Iterator.Text}.idx");
            var cond = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, currentIndex, lengthValue, "foreach.cmp");
            builder.BuildCondBr(cond, bodyBlock, exitBlock);

            builder.PositionAtEnd(bodyBlock);
            var bodyTerminated = LowerBlock(builder, function, foreachStmt.Body, loopLocals);
            if (!bodyTerminated)
            {
                builder.BuildBr(latchBlock);
            }

            builder.PositionAtEnd(latchBlock);
            var next = builder.BuildAdd(currentIndex, ConstI32(1), "foreach.next");
            builder.BuildStore(next, iterator);
            builder.BuildBr(condBlock);

            builder.PositionAtEnd(exitBlock);
            return false;
        }

        private LLVMValueRef LowerArrayAccess(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, Dictionary<string, LocalBinding> locals)
        {
            if (TryLowerArrayElementPointer(builder, arr, fieldName: null, locals, out var ptr, out var elemType))
            {
                return builder.BuildLoad2(elemType, ptr, "elemload");
            }

            AddDiagnostic("Unable to lower array access.", arr.Span);
            return ConstI32(0);
        }

        private LLVMValueRef LowerMemberAccess(LLVMBuilderRef builder, MemberAccessExpressionSyntax member, Dictionary<string, LocalBinding> locals)
        {
            if (member.Receiver is ArrayAccessExpressionSyntax arr)
            {
                if (TryLowerArrayElementPointer(builder, arr, member.Member.Text, locals, out var ptr, out var elemType))
                {
                    return builder.BuildLoad2(elemType, ptr, "fieldload");
                }
            }

            AddDiagnostic("Unable to lower member access.", member.Span);
            return ConstI32(0);
        }

        private LLVMValueRef LowerArrayAccess(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, string? fieldName, Dictionary<string, LocalBinding> locals)
        {
            if (TryLowerArrayElementPointer(builder, arr, fieldName, locals, out var ptr, out var elemType))
            {
                return builder.BuildLoad2(elemType, ptr, "elemload");
            }

            AddDiagnostic("Unable to lower array access.", arr.Span);
            return ConstI32(0);
        }

        private bool TryLowerArrayElementPointer(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, string? fieldName, Dictionary<string, LocalBinding> locals, out LLVMValueRef ptr, out LLVMTypeRef elemType)
        {
            ptr = default;
            elemType = default;

            if (arr.Receiver is IdentifierExpressionSyntax id)
            {
                if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is ArrayTypeSymbol arrayType)
                {
                    var zero = ConstI32(0);
                    var index = LowerExpression(builder, arr.Index, locals);

                    if (arrayType.ElementType is NamedTypeSymbol namedElem && fieldName is not null && _structs.TryGetValue(namedElem.TypeName, out var structDecl))
                    {
                        var field = structDecl.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, fieldName, StringComparison.Ordinal));
                        if (field is not null)
                        {
                            var fieldType = ResolveType(field.Type, _symbols);
                            elemType = _moduleBuilder.TypeMapper.Map(fieldType);
                            var fieldGlobalName = TryResolveFieldGlobalName(id.Identifier.Text, namedElem.TypeName, fieldName);
                            var fieldGlobal = _moduleBuilder.Module.GetNamedGlobal(fieldGlobalName);
                            if (fieldGlobal.Handle != IntPtr.Zero)
                            {
                                ptr = builder.BuildGEP2(elemType, fieldGlobal, new[] { zero, index }, "fieldaddr");
                                return true;
                            }
                            AddDiagnostic($"Layout for global '{id.Identifier.Text}' missing field '{fieldName}'.", arr.Span);
                        }
                        else
                        {
                            AddDiagnostic($"Unknown field '{fieldName}' on struct '{namedElem.TypeName}'.", arr.Span);
                        }
                    }
                    else if (fieldName is not null)
                    {
                        AddDiagnostic($"Field access requires struct array; '{id.Identifier.Text}' is not a struct array.", arr.Span);
                    }
                    else
                    {
                        var globalName = TryResolveGlobalName(id.Identifier.Text);
                        var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
                        elemType = _moduleBuilder.TypeMapper.Map(arrayType.ElementType);
                        ptr = builder.BuildGEP2(elemType, global, new[] { zero, index }, "elemaddr");
                        return true;
                    }
                }
            }

            return false;
        }

        private LLVMValueRef ConstBool(bool value) =>
            LLVMValueRef.CreateConstInt(LLVMTypeRef.Int1, value ? 1u : 0u, false);

        private LLVMValueRef AsBoolean(LLVMBuilderRef builder, LLVMValueRef value)
        {
            var type = value.TypeOf;
            if (type.Kind == LLVMTypeKind.LLVMIntegerTypeKind)
            {
                if (type.IntWidth == 1)
                {
                    return value;
                }

                var zero = LLVMValueRef.CreateConstInt(type, 0, false);
                return builder.BuildICmp(LLVMIntPredicate.LLVMIntNE, value, zero, "to_bool");
            }

            if (type.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind)
            {
                var zero = LLVMValueRef.CreateConstReal(type, 0);
                return builder.BuildFCmp(LLVMRealPredicate.LLVMRealONE, value, zero, "to_bool");
            }

            return value;
        }

        private LLVMValueRef LowerNeg(LLVMBuilderRef builder, LLVMValueRef operand)
        {
            var type = operand.TypeOf;
            return type.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind
                ? builder.BuildFNeg(operand, "fnegtmp")
                : builder.BuildNeg(operand, "negtmp");
        }

        private LLVMValueRef LowerLogicalNot(LLVMBuilderRef builder, LLVMValueRef operand)
        {
            var boolVal = AsBoolean(builder, operand);
            var inverted = builder.BuildNot(boolVal, "nottmp");
            return builder.BuildZExt(inverted, LLVMTypeRef.Int32, "noti32");
        }

        private string NextBlockName(string prefix) => $"{prefix}.{_blockId++}";

        private int ResolveIterableLength(ExpressionSyntax iterable)
        {
            if (iterable is IdentifierExpressionSyntax id
                && _symbols.TryGetValue(id.Identifier.Text, out var sym)
                && sym.Type is ArrayTypeSymbol array)
            {
                return array.Size;
            }

            return 0;
        }

        private string TryResolveGlobalName(string name) =>
            _globalLayouts.TryGetValue(name, out var layout) ? layout.Name : name;

        private string TryResolveFieldGlobalName(string parentGlobal, string structName, string fieldName)
        {
            if (_globalLayouts.TryGetValue(parentGlobal, out var layout))
            {
                var candidate = $"{structName}_{fieldName}";
                var match = layout.Fields.FirstOrDefault(f => string.Equals(f.Name, candidate, StringComparison.Ordinal));
                if (match is not null)
                {
                    return match.Name;
                }
            }

            return $"{structName}_{fieldName}";
        }

        private (LLVMTypeRef ReturnType, LLVMTypeRef[] Parameters) ResolveFunctionSignature(string name)
        {
            if (_functions.TryGetValue(name, out var fn))
            {
                var retType = fn.ReturnType is null
                    ? LLVMTypeRef.Void
                    : _moduleBuilder.TypeMapper.Map(ResolveType(fn.ReturnType, _symbols));
                var paramTypes = fn.Parameters.Select(p => _moduleBuilder.TypeMapper.Map(ResolveType(p.Type, _symbols))).ToArray();
                return (retType, paramTypes);
            }

            if (_tests.TryGetValue(name, out var test))
            {
                var retType = test.ReturnType is null
                    ? _moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32"))
                    : _moduleBuilder.TypeMapper.Map(ResolveType(test.ReturnType, _symbols));
                var paramTypes = test.Parameters.Select(p => _moduleBuilder.TypeMapper.Map(ResolveType(p.Type, _symbols))).ToArray();
                return (retType, paramTypes);
            }

            return (LLVMTypeRef.Void, Array.Empty<LLVMTypeRef>());
        }

        private LLVMValueRef ConstI32(int value) =>
            LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), (ulong)value, true);

        private LLVMValueRef UnsupportedOperator(SourceSpan span, LLVMValueRef fallback)
        {
            AddDiagnostic("Unsupported operator-method during lowering.", span);
            return fallback;
        }

        private void AddDiagnostic(string message, SourceSpan span) =>
            _diagnostics.Add(new Diagnostic(message, span));
    }

    private static void EmitFunctionSignatures(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols, LlvmModuleBuilder builder, bool includeTests)
    {
        foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            EmitFunction(builder, symbols, fn.Name.Text, fn.ReturnType, fn.Parameters);
        }

        if (!includeTests)
        {
            return;
        }

        foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
        {
            EmitFunction(builder, symbols, test.Name.Text, test.ReturnType, test.Parameters);
        }
    }

    private static void EmitFunction(LlvmModuleBuilder builder, IReadOnlyDictionary<string, Symbol> symbols, string name, TypeSyntax? returnType, IReadOnlyList<ParameterSyntax> parameters)
    {
        var ret = returnType is null
            ? LLVMTypeRef.Void
            : builder.TypeMapper.Map(ResolveType(returnType, symbols));

        var paramTypes = parameters
            .Select(p => builder.TypeMapper.Map(ResolveType(p.Type, symbols)))
            .ToArray();

        builder.DefineFunction(name, ret, paramTypes);
    }

    private static TypeSymbol ResolveType(TypeSyntax syntax, IReadOnlyDictionary<string, Symbol> symbols)
    {
        switch (syntax)
        {
            case NamedTypeSyntax named:
                if (symbols.TryGetValue(named.Name, out var sym) && sym.Type is not null)
                {
                    return sym.Type;
                }

                if (string.Equals(named.Name, "void", StringComparison.Ordinal))
                {
                    return new VoidTypeSymbol();
                }

                return new NamedTypeSymbol(named.Name);
            case ArrayTypeSyntax array:
                var element = ResolveType(array.ElementType, symbols);
                var size = int.TryParse(array.SizeToken.Text, out var parsed) ? parsed : 0;
                return new ArrayTypeSymbol(element, size);
            default:
                return new NamedTypeSymbol("unknown");
        }
    }
}
