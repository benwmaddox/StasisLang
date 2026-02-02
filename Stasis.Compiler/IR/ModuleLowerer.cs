using System;
using System.Globalization;
using System.Text;
using System.Runtime.InteropServices;
using LLVMSharp.Interop;
using Stasis.Compiler;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;
using Stasis.Compiler.IR.Llvm;
using System.Collections.Generic;

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
        using var builder = new LlvmModuleBuilder(moduleName, opts.TargetTriple);
        var reachableFunctions = Reachability.CollectReachableFunctions(compilationUnit, opts.IncludeTests, opts.AllowReachabilityFallback);
        EmitGlobals(compilationUnit, semantic.Symbols, layout, builder);
        EmitConstants(compilationUnit, semantic.Symbols, builder);
        EmitFunctionSignatures(compilationUnit, semantic.Symbols, builder, opts.IncludeTests, reachableFunctions);

        var diagnostics = new List<Diagnostic>();
        var lowerer = new FunctionLowerer(builder, semantic.Symbols, layout, diagnostics, opts.IncludeTests, opts.HeadlessGraphics, reachableFunctions);
        lowerer.Lower(compilationUnit, opts.IncludeTests);

        if (opts.IncludeTests && opts.EmitTestHarness)
        {
            EmitTestHarness(compilationUnit, builder, semantic.Symbols, diagnostics);
        }

        var ir = builder.EmitToString();
        // LLVM textual IR produced by LLVMSharp can include GEP flags not accepted by clang's IR parser.
        // Drop nuw on GEP to keep clang-compatible IR for tests and templates.
        ir = ir.Replace("getelementptr inbounds nuw ", "getelementptr inbounds ");
        return new LowerResult(ir, diagnostics);
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
                                ? ParseArrayLength(array.SizeToken?.Text ?? string.Empty)
                                : (uint)Math.Max(1, fieldLayout.Size / SizeOf(fieldType));
                            builder.DefineGlobalArray($"{structDecl.Name.Text}_{field.Identifier.Text}", llvmElem, length);
                        }

                        break;
                    }
                case ArrayTypeSyntax array:
                    {
                        var elementType = ResolveType(array.ElementType, symbols);

                        // Special handling for string arrays: use UTF-8 byte buffer
                        if (elementType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                        {
                            // Fixed-size string: header + data bytes.
                            // NOTE: Ignore layout sizing here. Layout sizes are in bytes for globals and do not map
                            // cleanly to "string element" sizes (ascii/utf8 are backed by i8 storage).
                            var length = ParseArrayLength(array.SizeToken?.Text ?? string.Empty);
                            var headerSize = HeaderSizeFor(prim.PrimitiveName);
                            builder.DefineGlobalArray(global.Name.Text, LLVMTypeRef.Int8, length + (uint)headerSize);
                        }
                        else
                        {
                            var length = globalLayout is null
                                ? ParseArrayLength(array.SizeToken?.Text ?? string.Empty)
                                : (uint)Math.Max(1, globalLayout.Size / SizeOf(elementType));
                            var llvmElem = builder.TypeMapper.Map(elementType);
                            builder.DefineGlobalArray(global.Name.Text, llvmElem, length);
                        }
                        break;
                    }
                case NamedTypeSyntax namedType when structs.TryGetValue(namedType.Name, out var structInstance):
                    {
                        // Struct instance → emit flattened fields
                        EmitStructInstanceGlobals(global.Name.Text, structInstance, symbols, structs, builder);
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

    private static void EmitStructInstanceGlobals(string globalName, StructDeclarationSyntax structDecl, IReadOnlyDictionary<string, Symbol> symbols, Dictionary<string, StructDeclarationSyntax> structs, LlvmModuleBuilder builder)
    {
        foreach (var field in structDecl.Fields)
        {
            var fieldName = $"{globalName}_{field.Identifier.Text}";

            switch (field.Type)
            {
                case ArrayTypeSyntax arrayType when arrayType.ElementType is NamedTypeSyntax nestedNamed && structs.TryGetValue(nestedNamed.Name, out var nestedStruct):
                    {
                        // Nested struct array → SoA
                        var count = ParseArrayLength(arrayType.SizeToken?.Text ?? string.Empty);
                        foreach (var nestedField in nestedStruct.Fields)
                        {
                            var nestedFieldType = ResolveType(nestedField.Type, symbols);
                            var nestedName = $"{fieldName}_{nestedField.Identifier.Text}";

                            // Special handling for string array fields in nested structs
                            if (nestedFieldType is ArrayTypeSymbol arrType &&
                                arrType.ElementType is PrimitiveTypeSymbol prim &&
                                HeaderSizeFor(prim.PrimitiveName) > 0)
                            {
                                // Each entry in the outer array contains a string buffer: [count x [strSize+8 x i8]]
                                var headerSize = HeaderSizeFor(prim.PrimitiveName);
                                var stringSize = arrType.Size + headerSize;  // UTF-8 header + data
                                var stringBufferType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)stringSize);
                                builder.DefineGlobalArray(nestedName, stringBufferType, count);
                            }
                            else
                            {
                                var llvmElem = builder.TypeMapper.Map(nestedFieldType);
                                builder.DefineGlobalArray(nestedName, llvmElem, count);
                            }
                        }
                        break;
                    }
                case ArrayTypeSyntax arrayType:
                    {
                        // Primitive array
                        var elemType = ResolveType(arrayType.ElementType, symbols);
                        var count = ParseArrayLength(arrayType.SizeToken?.Text ?? string.Empty);

                        // Special handling for string arrays: use UTF-8 byte buffer
                        if (elemType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                        {
                            // Fixed-size string: 8-byte header + data bytes
                            var headerSize = HeaderSizeFor(prim.PrimitiveName);
                            builder.DefineGlobalArray(fieldName, LLVMTypeRef.Int8, count + (uint)headerSize);
                        }
                        else
                        {
                            var llvmElem = builder.TypeMapper.Map(elemType);
                            builder.DefineGlobalArray(fieldName, llvmElem, count);
                        }
                        break;
                    }
                case NamedTypeSyntax namedField when structs.TryGetValue(namedField.Name, out var nestedStructDecl):
                    {
                        // Nested struct instance → recursively flatten
                        EmitStructInstanceGlobals(fieldName, nestedStructDecl, symbols, structs, builder);
                        break;
                    }
                default:
                    {
                        // Scalar field
                        var fieldType = ResolveType(field.Type, symbols);
                        var llvmType = builder.TypeMapper.Map(fieldType);
                        builder.DefineGlobalScalar(fieldName, llvmType);
                        break;
                    }
            }
        }
    }

    private static void EmitConstants(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols, LlvmModuleBuilder builder)
    {
        foreach (var constDecl in compilationUnit.Declarations.OfType<ConstDeclarationSyntax>())
        {
            var type = ResolveType(constDecl.Type, symbols);
            var llvmType = builder.TypeMapper.Map(type);

            if (type is PrimitiveTypeSymbol prim && prim.PrimitiveName == "string")
            {
                if (TryEmitStringConstant(constDecl, prim, builder))
                {
                    continue;
                }
            }

            // Evaluate the initializer to get a constant value
            var constValue = EvaluateConstantExpression(constDecl.Initializer, llvmType, builder.Context);
            if (constValue.Handle != IntPtr.Zero)
            {
                builder.DefineConstantScalar(constDecl.Name.Text, llvmType, constValue);
            }
        }
    }

    private static bool TryEmitStringConstant(ConstDeclarationSyntax constDecl, PrimitiveTypeSymbol prim, LlvmModuleBuilder builder)
    {
        if (constDecl.Initializer is not LiteralExpressionSyntax lit || lit.Literal.Kind != TokenKind.StringLiteral)
        {
            return false;
        }

        var text = UnescapeStringInline(lit.Literal.Text);
        var payloadBytes = BuildUtf8Payload(text);

        var arrType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)payloadBytes.Count);
        var values = payloadBytes
            .Select(b => LLVMValueRef.CreateConstInt(LLVMTypeRef.Int8, b, false))
            .ToArray();
        var initializer = LLVMValueRef.CreateConstArray(LLVMTypeRef.Int8, values);

        var dataName = $"{constDecl.Name.Text}.data";
        var dataGlobal = builder.Module.AddGlobal(arrType, dataName);
        dataGlobal.Linkage = LLVMLinkage.LLVMInternalLinkage;
        dataGlobal.IsGlobalConstant = true;
        dataGlobal.Initializer = initializer;

        var headerSize = HeaderSizeFor("string");
        var idx0 = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, 0, false);
        var idxHeader = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, (ulong)headerSize, false);
        var payloadPtr = LLVMValueRef.CreateConstGEP2(arrType, dataGlobal, new[] { idx0, idxHeader });

        builder.DefineConstantScalar(constDecl.Name.Text, builder.TypeMapper.Map(prim), payloadPtr);
        return true;
    }

    private static string UnescapeStringInline(string text)
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

    private static List<byte> BuildUtf8Payload(string text)
    {
        static void WriteInt32LEInline(List<byte> bytes, int value)
        {
            bytes.Add((byte)(value & 0xFF));
            bytes.Add((byte)((value >> 8) & 0xFF));
            bytes.Add((byte)((value >> 16) & 0xFF));
            bytes.Add((byte)((value >> 24) & 0xFF));
        }

        static int CountCodepointsInline(string value)
        {
            if (string.IsNullOrEmpty(value))
            {
                return 0;
            }

            var count = 0;
            foreach (var rune in value.EnumerateRunes())
            {
                count++;
            }

            return count;
        }

        var bytes = Encoding.UTF8.GetBytes(text);
        var byteLength = bytes.Length;
        var payloadBytes = new List<byte>(byteLength + 9);
        var charLength = CountCodepointsInline(text);
        WriteInt32LEInline(payloadBytes, byteLength);
        WriteInt32LEInline(payloadBytes, charLength);
        payloadBytes.AddRange(bytes);
        payloadBytes.Add(0);
        return payloadBytes;
    }

    private static LLVMValueRef EvaluateConstantExpression(ExpressionSyntax expr, LLVMTypeRef targetType, LLVMContextRef context)
    {
        if (expr is LiteralExpressionSyntax lit)
        {
            return lit.Literal.Kind switch
            {
                TokenKind.IntegerLiteral or TokenKind.U8Literal => int.TryParse(lit.Literal.Text, out var i)
                    ? LLVMValueRef.CreateConstInt(targetType, (ulong)i, true)
                    : default,
                TokenKind.FloatLiteral => double.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var f)
                    ? LLVMValueRef.CreateConstReal(targetType, f)
                    : default,
                TokenKind.TrueKeyword => LLVMValueRef.CreateConstInt(targetType, 1, false),
                TokenKind.FalseKeyword => LLVMValueRef.CreateConstInt(targetType, 0, false),
                _ => default
            };
        }

        // For non-literal expressions, return invalid value (will be skipped)
        return default;
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

    private static int HeaderSizeFor(string name) =>
        name switch
        {
            "string" => 8,
            "utf8" => 8,
            "ascii" => 4,
            _ => 0
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
        var (timeFn, timeType, timePtrType) = GetOrDeclareTime(builder);
        var (clockFn, clockType) = GetOrDeclareClock(builder);
        var nullTimePtr = LLVMValueRef.CreateConstPointerNull(timePtrType);
        var startTime = llvmBuilder.BuildCall2(timeType, timeFn, new[] { nullTimePtr }, "time.start");
        var startClock = llvmBuilder.BuildCall2(clockType, clockFn, Array.Empty<LLVMValueRef>(), "clock.start");

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
        var endTime = llvmBuilder.BuildCall2(timeType, timeFn, new[] { nullTimePtr }, "time.end");
        var elapsedSeconds = llvmBuilder.BuildSub(endTime, startTime, "time.elapsed_sec");
        var timeMs64 = llvmBuilder.BuildMul(elapsedSeconds, ConstInt64(1000), "time.elapsed_ms64");

        var endClock = llvmBuilder.BuildCall2(clockType, clockFn, Array.Empty<LLVMValueRef>(), "clock.end");
        var elapsedTicks = llvmBuilder.BuildSub(endClock, startClock, "clock.ticks");
        var ticksTimesMs = llvmBuilder.BuildMul(elapsedTicks, ConstInt64(1000), "clock.ticks_ms");
        var clocksPerSec = ConstInt64(RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? 1000 : 1000000);
        var clockMs64 = llvmBuilder.BuildUDiv(ticksTimesMs, clocksPerSec, "clock.ms64");

        var useTime = llvmBuilder.BuildICmp(LLVMIntPredicate.LLVMIntNE, timeMs64, ConstInt64(0), "time.nonzero");
        var elapsedMs64 = llvmBuilder.BuildSelect(useTime, timeMs64, clockMs64, "elapsed.ms64");
        var elapsedMs = llvmBuilder.BuildTrunc(elapsedMs64, int32, "elapsed.ms");

        // Print a simple summary: Tests: passed=X failed=Y
        var (printf, printfType) = GetOrDeclarePrintf(builder);
        var fmtPass = llvmBuilder.BuildGlobalStringPtr("Tests: \u001b[32mpassed=%d\u001b[0m failed=%d test-time=%dms\n", "tests_fmt_pass");
        var fmtFail = llvmBuilder.BuildGlobalStringPtr("Tests: passed=%d \u001b[31mfailed=%d\u001b[0m test-time=%dms\n", "tests_fmt_fail");
        var passed = llvmBuilder.BuildSub(ConstInt(int32, totalTests), result, "tests.passed");
        var hasFailures = llvmBuilder.BuildICmp(LLVMIntPredicate.LLVMIntNE, result, ConstInt(int32, 0), "has_failures");
        var summaryFmt = llvmBuilder.BuildSelect(hasFailures, fmtFail, fmtPass, "tests_fmt");
        var callType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { summaryFmt.TypeOf, passed.TypeOf, result.TypeOf, elapsedMs.TypeOf }, false);
        llvmBuilder.BuildCall2(callType, printf, new[] { summaryFmt, passed, result, elapsedMs }, "printf.tests");

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

    private static LLVMValueRef ConstInt64(long value) =>
        LLVMValueRef.CreateConstInt(LLVMTypeRef.Int64, (ulong)value, false);

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
        printfType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr }, true);
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

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareScanf(LlvmModuleBuilder builder)
    {
        var scanf = builder.Module.GetNamedFunction("scanf");
        LLVMTypeRef scanfType;
        if (scanf.Handle != IntPtr.Zero)
        {
            scanfType = GetFunctionType(scanf);
            return (scanf, scanfType);
        }

        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        scanfType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr }, true);
        scanf = builder.Module.AddFunction("scanf", scanfType);
        return (scanf, scanfType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type, LLVMTypeRef PtrType) GetOrDeclareTime(LlvmModuleBuilder builder)
    {
        var time = builder.Module.GetNamedFunction("time");
        var timeType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int64, new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int64, 0) }, false);
        if (time.Handle != IntPtr.Zero)
        {
            var ptrType = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int64, 0);
            return (time, timeType, ptrType);
        }

        time = builder.Module.AddFunction("time", timeType);
        var timePtr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int64, 0);
        return (time, timeType, timePtr);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareClock(LlvmModuleBuilder builder)
    {
        var clock = builder.Module.GetNamedFunction("clock");
        var clockType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int64, Array.Empty<LLVMTypeRef>(), false);
        if (clock.Handle != IntPtr.Zero)
        {
            return (clock, clockType);
        }

        clock = builder.Module.AddFunction("clock", clockType);
        return (clock, clockType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareLlvmSin(LlvmModuleBuilder builder)
    {
        var fnName = "llvm.sin.f32";
        var fn = builder.Module.GetNamedFunction(fnName);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Float }, false);
        if (fn.Handle != IntPtr.Zero)
        {
            return (fn, fnType);
        }

        fn = builder.Module.AddFunction(fnName, fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareLlvmCos(LlvmModuleBuilder builder)
    {
        var fnName = "llvm.cos.f32";
        var fn = builder.Module.GetNamedFunction(fnName);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Float }, false);
        if (fn.Handle != IntPtr.Zero)
        {
            return (fn, fnType);
        }

        fn = builder.Module.AddFunction(fnName, fnType);
        return (fn, fnType);
    }

    // Graphics runtime external functions
    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInitWindow(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_init_window");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32, LLVMTypeRef.Int32, i8Ptr }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_init_window", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisBeginFrame(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_begin_frame");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_begin_frame", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisEndFrame(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_end_frame");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_end_frame", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisClear(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_clear");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_clear", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisDrawLine(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_draw_line");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] {
            LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float,
            LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float
        }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_draw_line", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisDrawLinesF32(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_draw_lines_f32");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { i8Ptr, LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_draw_lines_f32", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisHostGetFrame(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_host_get_frame");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { i8Ptr, i8Ptr }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_host_get_frame", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxLoadSprite(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_load_sprite");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr, LLVMTypeRef.Int32, LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_load_sprite", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxDrawSprite(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_draw_sprite");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[]
        {
            LLVMTypeRef.Int32, // handle
            LLVMTypeRef.Int32, // x
            LLVMTypeRef.Int32, // y
            LLVMTypeRef.Int32, // w
            LLVMTypeRef.Int32, // h
            LLVMTypeRef.Int32, // rot_degrees
            LLVMTypeRef.Int32  // a
        }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_draw_sprite", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxDrawSpritesI32(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_draw_sprites_i32");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { i8Ptr, LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_draw_sprites_i32", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxSubmit(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_submit");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { i8Ptr, i8Ptr }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_submit", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxSubmitU8(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_submit_u8");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { i8Ptr, i8Ptr, i8Ptr }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_submit_u8", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxPollReload(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_poll_reload");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_poll_reload", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxWindowWidth(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_window_width");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_window_width", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxWindowHeight(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_window_height");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_window_height", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxWindowResized(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_window_resized");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_window_resized", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxDebugBakeHash(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_debug_bake_hash");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_debug_bake_hash", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxDebugEnableHash(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_debug_enable_hash");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_debug_enable_hash", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGfxDebugGetFrameHash(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_gfx_debug_get_frame_hash");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_gfx_debug_get_frame_hash", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisIsKeyDown(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_is_key_down");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_is_key_down", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGetTimeMs(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_get_time_ms");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_get_time_ms", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGetTimeUs(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_get_time_us");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_get_time_us", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisSleepMs(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_sleep_ms");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_sleep_ms", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisAudioIsAvailable(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_audio_is_available");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_audio_is_available", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisAudioGetSampleRate(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_audio_get_sample_rate");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_audio_get_sample_rate", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisAudioGetChannels(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_audio_get_channels");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_audio_get_channels", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisAudioGetQueuedFrames(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_audio_get_queued_frames");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_audio_get_queued_frames", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisAudioGetUnderruns(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_audio_get_underruns");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_audio_get_underruns", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisAudioPushF32Interleaved(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_audio_push_f32_interleaved");
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr, LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_audio_push_f32_interleaved", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisShouldQuit(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_should_quit");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_should_quit", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisGetWindowSize(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_get_window_size");
        var i32Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { i32Ptr, i32Ptr }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_get_window_size", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisSetFullscreen(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_set_fullscreen");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_set_fullscreen", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisSetPostfx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_set_postfx");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[]
        {
            LLVMTypeRef.Float, // strength
            LLVMTypeRef.Float, // phase/time
            LLVMTypeRef.Float, // speed
            LLVMTypeRef.Float, // r
            LLVMTypeRef.Float, // g
            LLVMTypeRef.Float  // b
        }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_set_postfx", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerCount(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_count");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_count", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerId(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_id");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_id", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerIsDown(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_is_down");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_is_down", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerWentDown(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_went_down");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_went_down", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerWentUp(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_went_up");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_went_up", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerXPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_x_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_x_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerYPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_y_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_y_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerDxPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_dx_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_dx_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerDyPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_dy_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_dy_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerXN(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_x_n");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_x_n", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputPointerYN(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_pointer_y_n");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_pointer_y_n", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputDroppedPointers(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_dropped_pointers");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_dropped_pointers", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputViewportXPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_viewport_x_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_viewport_x_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputViewportYPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_viewport_y_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_viewport_y_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputViewportWPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_viewport_w_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_viewport_w_px", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisInputViewportHPx(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_input_viewport_h_px");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_input_viewport_h_px", fnType);
        return (fn, fnType);
    }

    // ============================================================
    // Standard Library: C library string function declarations
    // ============================================================

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrlen(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int64, new[] { i8Ptr }, false);
        var fn = builder.Module.GetNamedFunction("strlen");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strlen", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrcmp(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr, i8Ptr }, false);
        var fn = builder.Module.GetNamedFunction("strcmp");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strcmp", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrncmp(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { i8Ptr, i8Ptr, LLVMTypeRef.Int64 }, false);
        var fn = builder.Module.GetNamedFunction("strncmp");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strncmp", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrcpy(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(i8Ptr, new[] { i8Ptr, i8Ptr }, false);
        var fn = builder.Module.GetNamedFunction("strcpy");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strcpy", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrcat(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(i8Ptr, new[] { i8Ptr, i8Ptr }, false);
        var fn = builder.Module.GetNamedFunction("strcat");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strcat", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrchr(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(i8Ptr, new[] { i8Ptr, LLVMTypeRef.Int32 }, false);
        var fn = builder.Module.GetNamedFunction("strchr");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strchr", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrrchr(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(i8Ptr, new[] { i8Ptr, LLVMTypeRef.Int32 }, false);
        var fn = builder.Module.GetNamedFunction("strrchr");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strrchr", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStrstr(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(i8Ptr, new[] { i8Ptr, i8Ptr }, false);
        var fn = builder.Module.GetNamedFunction("strstr");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("strstr", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareMemcpy(LlvmModuleBuilder builder)
    {
        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
        var fnType = LLVMTypeRef.CreateFunction(i8Ptr, new[] { i8Ptr, i8Ptr, LLVMTypeRef.Int64 }, false);
        var fn = builder.Module.GetNamedFunction("memcpy");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("memcpy", fnType);
        return (fn, fnType);
    }

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareAbort(LlvmModuleBuilder builder)
    {
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, Array.Empty<LLVMTypeRef>(), false);
        var fn = builder.Module.GetNamedFunction("abort");
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("abort", fnType);
        return (fn, fnType);
    }

    private static LLVMTypeRef GetFunctionType(LLVMValueRef fn)
    {
        var type = fn.TypeOf;
        return type.Kind == LLVMTypeKind.LLVMPointerTypeKind ? type.ElementType : type;
    }

    private sealed record ArrayDescriptorLayout(
        LLVMTypeRef DescriptorType,
        bool IsStructArray,
        StructDeclarationSyntax? StructDecl,
        Dictionary<string, int>? FieldOrder);

    private sealed class FunctionLowerer
    {
        private readonly LlvmModuleBuilder _moduleBuilder;
        private readonly IReadOnlyDictionary<string, Symbol> _symbols;
        private readonly Dictionary<string, GlobalLayout> _globalLayouts;
        private readonly List<Diagnostic> _diagnostics;
        private Dictionary<string, StructDeclarationSyntax> _structs = new(StringComparer.Ordinal);
        private Dictionary<string, EnumDeclarationSyntax> _enums = new(StringComparer.Ordinal);
        private Dictionary<string, FunctionDeclarationSyntax> _functions = new(StringComparer.Ordinal);
        private Dictionary<string, TestDeclarationSyntax> _tests = new(StringComparer.Ordinal);
        private readonly HashSet<string> _inlineStack = new(StringComparer.Ordinal);
        private readonly HashSet<string> _builtIns = new(StringComparer.Ordinal)
        {
            // Legacy I/O (to be deprecated)
            "print_string",
            "print",
            "print_int",
            "print_char",
            "print_cell",
            "print_prompt",
            "print_invalid",
            "print_clue_error",
            "print_solved",
            "read_char",
            "read_int",

            // System: sys_* (argv, file I/O, process execution)
            "sys_argc",
            "sys_argv",
            "sys_read_file",
            "sys_list_dir",
            "sys_write_file",
            "sys_file_exists",
            "sys_file_size",
            "sys_file_mtime_ms",
            "sys_exec",
            "sys_spawn",
            "sys_spawn_async",
            "sys_sleep_ms",
            "sys_delete_file",
            "sys_time_ms",
            "sys_flush",
            "sys_memcpy_u8",
            "sys_memcpy_i32",
            "sys_memcpy_f32",
            "sys_memmove_u8",
            "sys_memmove_i32",
            "sys_memmove_f32",

            // Legacy math (to be renamed)
            "sin",
            "cos",
            "sin_fast",
            "cos_fast",

            // Type conversion
            "i32_to_f32",
            "f32_to_i32",
            "u8_to_i32",
            "u16_to_i32",
            "i32_to_u8_trunc",
            "i32_to_u8_checked",
            "i32_to_u16_trunc",
            "i32_to_u16_checked",

            // Legacy system (to be renamed)
            "time",
            "get_time_ms",
            "get_time_us",
            "sleep_ms",

            // Legacy graphics (external runtime)
            "init_window",
            "gfx_load_sprite",
            "gfx_poll_reload",
            "load_font",
            "measure_text",

            // Legacy audio (external runtime)
            "audio_is_available",
            "audio_get_sample_rate",
            "audio_get_channels",
            "audio_get_queued_frames",
            "audio_get_underruns",
            "audio_push_f32_interleaved",

            // Standard Library: char_* module
            "char_is_digit",
            "char_is_alpha",
            "char_is_alnum",
            "char_is_space",
            "char_is_upper",
            "char_is_lower",
            "char_is_hex",
            "char_is_print",
            "char_to_upper",
            "char_to_lower",
            "char_to_digit",
            "char_from_digit",
            "char_to_hex",
            "char_from_hex",

        // Standard Library: str_* module
        "str_len",
        "str_is_empty",
        "str_get",
        "str_set",
        "str_eq",
        "str_cmp",
        "str_starts_with",
        "str_ends_with",
        "str_find",
        "str_find_char",
        "str_find_last_char",
        "str_contains",
        "str_clear",
        "str_copy",
        "str_append",
        "str_append_char",
        "str_substr",
        "str_trim_start",
        "str_trim_end",
        "str_trim",
        "str_to_upper",
        "str_to_lower",
        "str_from_i32",
        "str_from_f32",
        "str_to_i32",
        "str_to_f32"
    };
        private const int DirEntryStride = 268;
        private int _blockId;
        private readonly bool _headlessGraphics;
        private readonly HashSet<string> _reachableFunctions;

        public FunctionLowerer(LlvmModuleBuilder moduleBuilder, IReadOnlyDictionary<string, Symbol> symbols, LayoutPlan layout, List<Diagnostic> diagnostics, bool includeTests, bool headlessGraphics, HashSet<string> reachableFunctions)
        {
            _moduleBuilder = moduleBuilder;
            _symbols = symbols;
            _globalLayouts = layout.Globals.ToDictionary(g => g.Name, g => g, StringComparer.Ordinal);
            _diagnostics = diagnostics;
            _headlessGraphics = headlessGraphics;
            _reachableFunctions = reachableFunctions;
        }

        public void Lower(CompilationUnitSyntax compilationUnit, bool includeTests)
        {
            _structs = compilationUnit.Declarations
                .OfType<StructDeclarationSyntax>()
                .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
            _enums = compilationUnit.Declarations
                .OfType<EnumDeclarationSyntax>()
                .ToDictionary(e => e.Name.Text, e => e, StringComparer.Ordinal);
            _functions = compilationUnit.Declarations
                .OfType<FunctionDeclarationSyntax>()
                .ToDictionary(f => f.Name.Text, f => f, StringComparer.Ordinal);
            _tests = compilationUnit.Declarations
                .OfType<TestDeclarationSyntax>()
                .ToDictionary(t => t.Name.Text, t => t, StringComparer.Ordinal);

            foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
            {
                if (!_reachableFunctions.Contains(fn.Name.Text))
                {
                    continue;
                }
                if (fn.IsExtern)
                {
                    continue;
                }
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
            if (fn.Body is null)
            {
                return;
            }
            LowerFunctionCore(fn.Name.Text, fn.Parameters, fn.ReturnType, fn.Body, isTest: false);
        }

        private void LowerFunction(TestDeclarationSyntax test)
        {
            LowerFunctionCore(test.Name.Text, test.Parameters, test.ReturnType, test.Body, isTest: true);
        }

        private readonly record struct LocalBinding(LLVMValueRef Value, LLVMTypeRef Type, bool IsAddress, ElementBinding? Element = null, TypeSymbol? SemanticType = null, ArrayDescriptorLayout? ArrayLayout = null, bool IsArrayDescriptor = false);

        private readonly record struct ElementBinding(
            StructDeclarationSyntax? StructDecl,
            TypeSymbol ElementType,
            LLVMValueRef IndexAlloca,
            string? BaseName,
            LLVMValueRef? PrimitiveBasePtr = null,
            Dictionary<string, LLVMValueRef>? FieldPtrs = null);

        private void LowerFunctionCore(string name, IReadOnlyList<ParameterSyntax> parameters, TypeSyntax? returnType, BlockStatementSyntax body, bool isTest)
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
                var paramType = ResolveType(param.Type, _symbols);
                if (paramType is ArrayTypeSymbol arr)
                {
                    var layout = CreateArrayDescriptorLayout(arr, _moduleBuilder.TypeMapper, _structs, _symbols);
                    var llvmType = layout.DescriptorType;
                    var paramVal = function.GetParam((uint)i);
                    var alloca = builder.BuildAlloca(llvmType, param.Name.Text);
                    builder.BuildStore(paramVal, alloca);
                    locals[param.Name.Text] = new LocalBinding(alloca, llvmType, true, null, paramType, layout, true);
                }
                else
                {
                    var llvmType = _moduleBuilder.TypeMapper.Map(paramType);
                    var paramVal = function.GetParam((uint)i);
                    var alloca = builder.BuildAlloca(llvmType, param.Name.Text);
                    builder.BuildStore(paramVal, alloca);
                    locals[param.Name.Text] = new LocalBinding(alloca, llvmType, true, null, paramType);
                }
            }

            var terminated = LowerBlock(builder, function, body, locals);
            var isVoid = !isTest && (returnType is null || (returnType is NamedTypeSyntax named && string.Equals(named.Name, "void", StringComparison.Ordinal)));
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
                        var returnType = GetFunctionType(function).ReturnType;
                        value = ConvertToType(builder, value, returnType);
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
            locals[decl.Name.Text] = new LocalBinding(alloca, llvmType, true, null, type);

            if (decl.Initializer is not null)
            {
                var initValue = LowerExpression(builder, decl.Initializer, locals);
                if (TryLowerIntegerLiteralToType(decl.Initializer, llvmType, out var loweredInt))
                {
                    initValue = loweredInt;
                }
                else if (TryLowerFloatLiteralToType(decl.Initializer, llvmType, out var loweredFloat))
                {
                    initValue = loweredFloat;
                }
                var converted = ConvertToType(builder, initValue, llvmType);
                builder.BuildStore(converted, alloca);
            }
        }

        private LLVMValueRef LowerExpression(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals)
        {
            switch (expr)
            {
                case LiteralExpressionSyntax lit:
                    return LowerLiteral(builder, lit);
                case IdentifierExpressionSyntax id:
                    if (locals.TryGetValue(id.Identifier.Text, out var value))
                    {
                        if (value.Element is not null && value.Element.Value.StructDecl is null)
                        {
                            if (TryBuildElementPointer(builder, value, fieldName: null, id.Span, out var elemPtr, out var elemType))
                            {
                                return builder.BuildLoad2(elemType, elemPtr, id.Identifier.Text);
                            }
                        }

                        if (value.IsAddress)
                        {
                            return builder.BuildLoad2(value.Type, value.Value, id.Identifier.Text);
                        }

                        return value.Value;
                    }

                    if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && (sym.Kind == SymbolKind.Global || sym.Kind == SymbolKind.Const) && sym.Type is not null)
                    {
                        var global = _moduleBuilder.Module.GetNamedGlobal(id.Identifier.Text);
                        var type = _moduleBuilder.TypeMapper.Map(sym.Type);
                        return builder.BuildLoad2(type, global, id.Identifier.Text);
                    }

                    return ConstI32(0);
                case MemberAccessExpressionSyntax member when string.Equals(member.Member.Text, "length", StringComparison.Ordinal):
                    if (TryResolveArrayLength(member.Receiver, out var length))
                    {
                        return ConstI32(length);
                    }

                    AddDiagnostic("'.length' is only available on fixed-size arrays.", member.Span);
                    return ConstI32(0);
                case MemberAccessExpressionSyntax member when member.Receiver is IdentifierExpressionSyntax enumIdExpr && _enums.ContainsKey(enumIdExpr.Identifier.Text):
                    // Enum member access (e.g., State.Idle)
                    var enumDecl = _enums[enumIdExpr.Identifier.Text];
                    var memberValue = -1;
                    var nextValue = 0;
                    for (int i = 0; i < enumDecl.Members.Count; i++)
                    {
                        var m = enumDecl.Members[i];
                        var assigned = nextValue;
                        if (m.ValueToken is not null && int.TryParse(m.ValueToken.Text, out var explicitValue))
                        {
                            assigned = explicitValue;
                        }

                        if (string.Equals(m.Identifier.Text, member.Member.Text, StringComparison.Ordinal))
                        {
                            memberValue = assigned;
                            break;
                        }

                        nextValue = assigned + 1;
                    }

                    if (memberValue >= 0)
                    {
                        return ConstI32(memberValue);
                    }

                    AddDiagnostic($"Enum '{enumIdExpr.Identifier.Text}' does not have a member '{member.Member.Text}'.", member.Span);
                    return ConstI32(0);
                case MemberAccessExpressionSyntax member:
                    return LowerMemberAccess(builder, member, locals);
                case ArrayAccessExpressionSyntax arr:
                    return LowerArrayAccess(builder, arr, null, locals);
                case ParenthesizedExpressionSyntax paren:
                    return LowerExpression(builder, paren.Expression, locals);
                case UnaryExpressionSyntax unary:
                    return LowerUnary(builder, unary, locals);
                case AssignmentExpressionSyntax assign:
                    return LowerAssignment(builder, assign, locals);
                case BinaryExpressionSyntax bin:
                    return LowerBinary(builder, bin, locals);
                case CallExpressionSyntax call:
                    return LowerCall(builder, call, locals);
                case OperatorCallExpressionSyntax op:
                    return LowerOperatorCall(builder, op, locals);
                default:
                    AddDiagnostic("Expression not supported during lowering.", expr.Span);
                    return ConstI32(0);
            }
        }

        private LLVMValueRef LowerLiteral(LLVMBuilderRef builder, LiteralExpressionSyntax lit)
        {
            switch (lit.Literal.Kind)
            {
                case TokenKind.IntegerLiteral or TokenKind.U8Literal when int.TryParse(lit.Literal.Text, out var ival):
                    return ConstI32(ival);
                case TokenKind.FloatLiteral when float.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var fval):
                    return LLVMValueRef.CreateConstReal(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("f32")), fval);
                case TokenKind.StringLiteral:
                    {
                        var text = UnescapeString(lit.Literal.Text);
                        return EmitUtf8Literal(builder, text);
                    }
                case TokenKind.TrueKeyword:
                    return ConstI32(1);
                case TokenKind.FalseKeyword:
                    return ConstI32(0);
                default:
                    return ConstI32(0);
            }
        }

        private static bool TryLowerIntegerLiteralToType(ExpressionSyntax expr, LLVMTypeRef targetType, out LLVMValueRef lowered)
        {
            lowered = default;
            if (targetType.Kind != LLVMTypeKind.LLVMIntegerTypeKind)
            {
                return false;
            }

            long value;
            if (expr is LiteralExpressionSyntax lit &&
                (lit.Literal.Kind == TokenKind.IntegerLiteral || lit.Literal.Kind == TokenKind.U8Literal) &&
                long.TryParse(lit.Literal.Text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var parsed))
            {
                value = parsed;
            }
            else if (expr is UnaryExpressionSyntax unary &&
                     unary.OperatorToken.Kind == TokenKind.Minus &&
                     unary.Operand is LiteralExpressionSyntax innerLit &&
                     (innerLit.Literal.Kind == TokenKind.IntegerLiteral || innerLit.Literal.Kind == TokenKind.U8Literal) &&
                     long.TryParse(innerLit.Literal.Text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var innerParsed))
            {
                value = -innerParsed;
            }
            else
            {
                return false;
            }

            unchecked
            {
                lowered = LLVMValueRef.CreateConstInt(targetType, (ulong)value, true);
            }
            return true;
        }

        private LLVMValueRef LowerU8ExpressionAsI32(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals, string context, SourceSpan span)
        {
            LLVMValueRef value;
            if (TryLowerIntegerLiteralToType(expr, LLVMTypeRef.Int8, out var lowered))
            {
                value = lowered;
            }
            else
            {
                value = LowerExpression(builder, expr, locals);
            }

            if (value.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || value.TypeOf.IntWidth != 8)
            {
                AddDiagnostic($"{context} expects a u8 value.", span);
                return ConstI32(0);
            }

            return builder.BuildZExt(value, LLVMTypeRef.Int32, "u8.i32");
        }

        private static bool TryLowerFloatLiteralToType(ExpressionSyntax expr, LLVMTypeRef targetType, out LLVMValueRef lowered)
        {
            lowered = default;
            if (expr is not LiteralExpressionSyntax lit || lit.Literal.Kind != TokenKind.FloatLiteral)
            {
                return false;
            }

            if (targetType.Kind is not (LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind))
            {
                return false;
            }

            if (!double.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var value))
            {
                return false;
            }

            lowered = LLVMValueRef.CreateConstReal(targetType, value);
            return true;
        }

        private LLVMValueRef EmitUtf8Literal(LLVMBuilderRef builder, string text)
        {
            var bytes = Encoding.UTF8.GetBytes(text);
            var byteLength = bytes.Length;
            var payloadBytes = new List<byte>(byteLength + 9);
            var charLength = CountCodepoints(text);
            WriteInt32LE(payloadBytes, byteLength);
            WriteInt32LE(payloadBytes, charLength);
            payloadBytes.AddRange(bytes);
            payloadBytes.Add(0);

            var arrType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)payloadBytes.Count);
            var values = payloadBytes
                .Select(b => LLVMValueRef.CreateConstInt(LLVMTypeRef.Int8, b, false))
                .ToArray();
            var initializer = LLVMValueRef.CreateConstArray(LLVMTypeRef.Int8, values);

            var name = $"str_{_blockId++}";
            var global = _moduleBuilder.Module.AddGlobal(arrType, name);
            global.Linkage = LLVMLinkage.LLVMInternalLinkage;
            global.IsGlobalConstant = true;
            global.Initializer = initializer;

            var headerSize = HeaderSizeFor("string");
            return builder.BuildGEP2(arrType, global, new[] { ConstI32(0), ConstI32(headerSize) }, $"{name}.payload");
        }

        private static void WriteInt32LE(List<byte> bytes, int value)
        {
            bytes.Add((byte)(value & 0xFF));
            bytes.Add((byte)((value >> 8) & 0xFF));
            bytes.Add((byte)((value >> 16) & 0xFF));
            bytes.Add((byte)((value >> 24) & 0xFF));
        }

        private static int CountCodepoints(string value)
        {
            if (string.IsNullOrEmpty(value))
            {
                return 0;
            }

            var count = 0;
            foreach (var rune in value.EnumerateRunes())
            {
                _ = rune;
                count++;
            }

            return count;
        }

        private LLVMValueRef GetUtf8HeaderPtr(LLVMBuilderRef builder, LLVMValueRef payloadPtr)
        {
            var headerSize = HeaderSizeFor("string");
            var headerBytePtr = builder.BuildGEP2(LLVMTypeRef.Int8, payloadPtr, new[] { ConstI32(-headerSize) }, "utf8.header");
            return builder.BuildBitCast(headerBytePtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), "utf8.header.i32");
        }

        private LLVMValueRef LoadUtf8ByteLength(LLVMBuilderRef builder, LLVMValueRef payloadPtr)
        {
            var headerPtr = GetUtf8HeaderPtr(builder, payloadPtr);
            return builder.BuildLoad2(LLVMTypeRef.Int32, headerPtr, "utf8.byte_length");
        }

        private void StoreUtf8Lengths(LLVMBuilderRef builder, LLVMValueRef payloadPtr, LLVMValueRef byteLen)
        {
            var headerPtr = GetUtf8HeaderPtr(builder, payloadPtr);
            builder.BuildStore(byteLen, headerPtr);
            var charPtr = builder.BuildGEP2(LLVMTypeRef.Int32, headerPtr, new[] { ConstI32(1) }, "utf8.char_length.ptr");
            builder.BuildStore(byteLen, charPtr);
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

        private LLVMValueRef LowerAssignment(LLVMBuilderRef builder, AssignmentExpressionSyntax assign, Dictionary<string, LocalBinding> locals)
        {
            if (!TryGetPointer(builder, assign.Left, locals, out var ptr, out var ptrType))
            {
                AddDiagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", assign.Left.Span);
                return ConstI32(0);
            }

            if (assign.Right is StructInitializerExpressionSyntax init)
            {
                if (assign.OperatorToken.Kind != TokenKind.Equal)
                {
                    AddDiagnostic("Struct initializer only supports '=' assignment.", assign.OperatorToken.Span);
                    return ConstI32(0);
                }

                var targetType = ResolveExpressionType(assign.Left);
                LowerStructInitializerAssignment(builder, assign.Left, init, targetType, locals);
                return builder.BuildLoad2(ptrType, ptr, "assign.struct");
            }

            var rhs = LowerExpression(builder, assign.Right, locals);
            // Convert RHS to target type if needed (e.g., i32 -> f32)
            if (TryLowerIntegerLiteralToType(assign.Right, ptrType, out var loweredInt))
            {
                rhs = loweredInt;
            }
            else if (TryLowerFloatLiteralToType(assign.Right, ptrType, out var loweredFloat))
            {
                rhs = loweredFloat;
            }
            rhs = ConvertToType(builder, rhs, ptrType);
            if (assign.OperatorToken.Kind == TokenKind.Equal)
            {
                builder.BuildStore(rhs, ptr);
                return rhs;
            }

            var lhsValue = builder.BuildLoad2(ptrType, ptr, "assign.lhs");
            var opText = assign.OperatorToken.Kind switch
            {
                TokenKind.PlusEqual => "+",
                TokenKind.MinusEqual => "-",
                TokenKind.StarEqual => "*",
                TokenKind.SlashEqual => "/",
                TokenKind.PercentEqual => "%",
                _ => string.Empty
            };

            if (string.IsNullOrEmpty(opText))
            {
                AddDiagnostic($"Unsupported assignment operator '{assign.OperatorToken.Text}'.", assign.OperatorToken.Span);
                return rhs;
            }

            var combined = LowerBinary(builder, opText, lhsValue, rhs, assign.OperatorToken.Span);
            combined = ConvertToType(builder, combined, ptrType);
            builder.BuildStore(combined, ptr);
            return combined;
        }

        private void LowerStructInitializerAssignment(LLVMBuilderRef builder, ExpressionSyntax targetExpr, StructInitializerExpressionSyntax init, TypeSymbol? targetType, Dictionary<string, LocalBinding> locals)
        {
            if (targetType is not NamedTypeSymbol named || !_structs.TryGetValue(named.TypeName, out var structDecl))
            {
                AddDiagnostic("Struct initializer requires a struct assignment target.", init.Span);
                return;
            }

            foreach (var field in structDecl.Fields)
            {
                var fieldType = ResolveType(field.Type, _symbols);
                if (fieldType is ArrayTypeSymbol)
                {
                    AddDiagnostic($"Struct initializer does not support array field '{field.Identifier.Text}'.", init.Span);
                    return;
                }
            }

            var byName = new Dictionary<string, StructInitializerFieldSyntax>(StringComparer.Ordinal);
            foreach (var f in init.Fields)
            {
                byName[f.Name.Text] = f;
            }

            var dot = new Token(TokenKind.Dot, ".", init.OpenBrace.Span);

            foreach (var field in structDecl.Fields)
            {
                var fieldType = ResolveType(field.Type, _symbols);
                var memberAccess = new MemberAccessExpressionSyntax(targetExpr, dot, field.Identifier);

                if (fieldType is NamedTypeSymbol && byName.TryGetValue(field.Identifier.Text, out var initField) && initField.Value is StructInitializerExpressionSyntax nested)
                {
                    LowerStructInitializerAssignment(builder, memberAccess, nested, fieldType, locals);
                    continue;
                }

                if (byName.TryGetValue(field.Identifier.Text, out initField))
                {
                    if (initField.Value is StructInitializerExpressionSyntax)
                    {
                        AddDiagnostic($"Nested struct initializer requires a struct field target; field '{field.Identifier.Text}' is '{DescribeType(fieldType)}'.", initField.Value.Span);
                        continue;
                    }

                    var value = LowerExpression(builder, initField.Value, locals);
                    if (!TryGetPointer(builder, memberAccess, locals, out var fieldPtr, out var fieldPtrType))
                    {
                        AddDiagnostic($"Unable to assign to field '{field.Identifier.Text}' in struct initializer.", initField.Value.Span);
                        continue;
                    }

                    if (TryLowerIntegerLiteralToType(initField.Value, fieldPtrType, out var loweredInt))
                    {
                        value = loweredInt;
                    }
                    else if (TryLowerFloatLiteralToType(initField.Value, fieldPtrType, out var loweredFloat))
                    {
                        value = loweredFloat;
                    }

                    value = ConvertToType(builder, value, fieldPtrType);
                    builder.BuildStore(value, fieldPtr);
                }
                else
                {
                    ZeroStructField(builder, memberAccess, fieldType, init.Span, locals);
                }
            }
        }

        private void ZeroStructField(LLVMBuilderRef builder, ExpressionSyntax targetExpr, TypeSymbol fieldType, SourceSpan span, Dictionary<string, LocalBinding> locals)
        {
            if (fieldType is PrimitiveTypeSymbol prim)
            {
                LLVMValueRef zero = prim.PrimitiveName switch
                {
                    "f32" => LLVMValueRef.CreateConstReal(_moduleBuilder.TypeMapper.Map(prim), 0.0),
                    "f64" => LLVMValueRef.CreateConstReal(_moduleBuilder.TypeMapper.Map(prim), 0.0),
                    _ => ConstI32(0)
                };

                var fieldLlvmType = _moduleBuilder.TypeMapper.Map(fieldType);
                if (fieldLlvmType.Kind == LLVMTypeKind.LLVMIntegerTypeKind && fieldLlvmType.IntWidth != 32)
                {
                    zero = LLVMValueRef.CreateConstInt(fieldLlvmType, 0, false);
                }

                if (!TryGetPointer(builder, targetExpr, locals, out var ptr, out var ptrType))
                {
                    AddDiagnostic("Unable to zero struct field in initializer.", span);
                    return;
                }

                zero = ConvertToType(builder, zero, ptrType);
                builder.BuildStore(zero, ptr);
                return;
            }

            if (fieldType is NamedTypeSymbol named && _structs.TryGetValue(named.TypeName, out var structDecl))
            {
                var dot = new Token(TokenKind.Dot, ".", span);
                foreach (var f in structDecl.Fields)
                {
                    var nestedType = ResolveType(f.Type, _symbols);
                    if (nestedType is ArrayTypeSymbol)
                    {
                        AddDiagnostic($"Struct initializer does not support array field '{f.Identifier.Text}'.", span);
                        continue;
                    }
                    var member = new MemberAccessExpressionSyntax(targetExpr, dot, f.Identifier);
                    ZeroStructField(builder, member, nestedType, span, locals);
                }
                return;
            }

            AddDiagnostic($"Struct initializer cannot zero field of type '{DescribeType(fieldType)}'.", span);
        }

        private static string DescribeType(TypeSymbol type) =>
            type switch
            {
                PrimitiveTypeSymbol prim => prim.PrimitiveName,
                NamedTypeSymbol named => named.TypeName,
                ArrayTypeSymbol arr => $"{DescribeType(arr.ElementType)}[{arr.Size}]",
                VoidTypeSymbol => "void",
                _ => "unknown"
            };

        private bool TryGetPointer(LLVMBuilderRef builder, ExpressionSyntax target, Dictionary<string, LocalBinding> locals, out LLVMValueRef ptr, out LLVMTypeRef type)
        {
            ptr = default;
            type = default;

            switch (target)
            {
                case IdentifierExpressionSyntax id:
                    if (locals.TryGetValue(id.Identifier.Text, out var local))
                    {
                        if (local.Element is not null && local.Element.Value.StructDecl is null)
                        {
                            if (TryBuildElementPointer(builder, local, fieldName: null, id.Span, out var elemPtrLocal, out var elemTypeLocal))
                            {
                                ptr = elemPtrLocal;
                                type = elemTypeLocal;
                                return true;
                            }
                        }

                        if (!local.IsAddress)
                        {
                            var promoted = builder.BuildAlloca(local.Type, id.Identifier.Text);
                            builder.BuildStore(local.Value, promoted);
                            locals[id.Identifier.Text] = new LocalBinding(promoted, local.Type, true);
                            local = locals[id.Identifier.Text];
                        }

                        ptr = local.Value;
                        type = local.Type;
                        return true;
                    }

                    if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && (sym.Kind == SymbolKind.Global || sym.Kind == SymbolKind.Const) && sym.Type is not null)
                    {
                        ptr = _moduleBuilder.Module.GetNamedGlobal(id.Identifier.Text);
                        type = _moduleBuilder.TypeMapper.Map(sym.Type);
                        return true;
                    }

                    return false;

                case ArrayAccessExpressionSyntax arr:
                    if (TryLowerArrayElementPointer(builder, arr, fieldName: null, locals, out var elemPtrArray, out var elemTypeArray))
                    {
                        ptr = elemPtrArray;
                        type = elemTypeArray;
                        return true;
                    }

                    return false;

                case MemberAccessExpressionSyntax member when member.Receiver is ArrayAccessExpressionSyntax arrRecv:
                    if (TryLowerArrayElementPointer(builder, arrRecv, member.Member.Text, locals, out var fieldPtr, out var fieldElemType))
                    {
                        ptr = fieldPtr;
                        type = fieldElemType;
                        return true;
                    }

                    return false;

                case MemberAccessExpressionSyntax member when member.Receiver is IdentifierExpressionSyntax elemId && locals.TryGetValue(elemId.Identifier.Text, out var elemLocal) && elemLocal.Element is not null:
                    if (TryBuildElementPointer(builder, elemLocal, member.Member.Text, member.Span, out var elemPtrMember, out var elemTypeMember))
                    {
                        ptr = elemPtrMember;
                        type = elemTypeMember;
                        return true;
                    }

                    return false;

                case MemberAccessExpressionSyntax member when member.Receiver is IdentifierExpressionSyntax memberId:
                    // Handle state.field assignment
                    if (_symbols.TryGetValue(memberId.Identifier.Text, out var memberSym) &&
                        (memberSym.Kind == SymbolKind.Global || memberSym.Kind == SymbolKind.Const) &&
                        memberSym.Type is NamedTypeSymbol memberType &&
                        _structs.TryGetValue(memberType.TypeName, out var memberStruct))
                    {
                        var flattenedName = $"{memberId.Identifier.Text}_{member.Member.Text}";
                        var global = _moduleBuilder.Module.GetNamedGlobal(flattenedName);
                        if (global.Handle != IntPtr.Zero)
                        {
                            var field = memberStruct.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
                            if (field is not null)
                            {
                                var fieldType = ResolveType(field.Type, _symbols);
                                ptr = global;
                                type = _moduleBuilder.TypeMapper.Map(fieldType);
                                return true;
                            }
                        }
                    }

                    return false;

                case MemberAccessExpressionSyntax nestedMember:
                    // Handle nested member access like state.ship.x
                    var flattenedPath = BuildFlattenedMemberPath(nestedMember);
                    if (flattenedPath is not null)
                    {
                        var flatGlobal = _moduleBuilder.Module.GetNamedGlobal(flattenedPath);
                        if (flatGlobal.Handle != IntPtr.Zero)
                        {
                            // Determine the type by walking the member chain
                            if (TryResolveMemberType(nestedMember, out var resolvedType))
                            {
                                ptr = flatGlobal;
                                type = _moduleBuilder.TypeMapper.Map(resolvedType);
                                return true;
                            }
                        }
                    }

                    return false;

                default:
                    return false;
            }
        }

        private LLVMValueRef LowerBinary(LLVMBuilderRef builder, BinaryExpressionSyntax bin, Dictionary<string, LocalBinding> locals)
        {
            var function = builder.InsertBlock.Parent;
            switch (bin.OperatorToken.Kind)
            {
                case TokenKind.PipePipe:
                    {
                        var lhsBool = AsBoolean(builder, LowerExpression(builder, bin.Left, locals));
                        var trueBlock = AppendBlock(function, NextBlockName("or.true"));
                        var falseBlock = AppendBlock(function, NextBlockName("or.false"));
                        var mergeBlock = AppendBlock(function, NextBlockName("or.merge"));

                        builder.BuildCondBr(lhsBool, trueBlock, falseBlock);

                        builder.PositionAtEnd(trueBlock);
                        var trueVal = ConstI32(1);
                        var trueIncoming = builder.InsertBlock;
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(falseBlock);
                        var rhsBool = AsBoolean(builder, LowerExpression(builder, bin.Right, locals));
                        var rhsVal = BuildBoolResult(builder, rhsBool);
                        var falseIncoming = builder.InsertBlock;
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(mergeBlock);
                        var phi = builder.BuildPhi(LLVMTypeRef.Int32, "or.result");
                        phi.AddIncoming(new[] { trueVal }, new[] { trueIncoming }, 1u);
                        phi.AddIncoming(new[] { rhsVal }, new[] { falseIncoming }, 1u);
                        return phi;
                    }
                case TokenKind.AmpAmp:
                    {
                        var lhsBool = AsBoolean(builder, LowerExpression(builder, bin.Left, locals));
                        var trueBlock = AppendBlock(function, NextBlockName("and.true"));
                        var falseBlock = AppendBlock(function, NextBlockName("and.false"));
                        var mergeBlock = AppendBlock(function, NextBlockName("and.merge"));

                        builder.BuildCondBr(lhsBool, trueBlock, falseBlock);

                        builder.PositionAtEnd(trueBlock);
                        var rhsBool = AsBoolean(builder, LowerExpression(builder, bin.Right, locals));
                        var rhsVal = BuildBoolResult(builder, rhsBool);
                        var trueIncoming = builder.InsertBlock;
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(falseBlock);
                        var falseVal = ConstI32(0);
                        var falseIncoming = builder.InsertBlock;
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(mergeBlock);
                        var phi = builder.BuildPhi(LLVMTypeRef.Int32, "and.result");
                        phi.AddIncoming(new[] { rhsVal }, new[] { trueIncoming }, 1u);
                        phi.AddIncoming(new[] { falseVal }, new[] { falseIncoming }, 1u);
                        return phi;
                    }
                default:
                    var lhs = LowerExpression(builder, bin.Left, locals);
                    var rhs = LowerExpression(builder, bin.Right, locals);
                    if (TryLowerIntegerLiteralToType(bin.Left, rhs.TypeOf, out var leftInt))
                    {
                        lhs = leftInt;
                    }
                    else if (TryLowerFloatLiteralToType(bin.Left, rhs.TypeOf, out var leftFloat))
                    {
                        lhs = leftFloat;
                    }

                    if (TryLowerIntegerLiteralToType(bin.Right, lhs.TypeOf, out var rightInt))
                    {
                        rhs = rightInt;
                    }
                    else if (TryLowerFloatLiteralToType(bin.Right, lhs.TypeOf, out var rightFloat))
                    {
                        rhs = rightFloat;
                    }
                    return LowerBinary(builder, bin.OperatorToken.Text, lhs, rhs, bin.OperatorToken.Span);
            }
        }

        private LLVMValueRef LowerCall(LLVMBuilderRef builder, CallExpressionSyntax call, Dictionary<string, LocalBinding> locals)
        {
            if (call.Callee is MemberAccessExpressionSyntax member &&
                string.Equals(member.Member.Text, "clear", StringComparison.Ordinal))
            {
                if (call.Arguments.Count != 0)
                {
                    AddDiagnostic("clear() takes no arguments.", call.Span);
                    return ConstI32(0);
                }

                LowerClear(builder, member.Receiver, locals, call.Span);
                return ConstI32(0);
            }

            if (call.Callee is not IdentifierExpressionSyntax id)
            {
                AddDiagnostic("Only simple function calls are supported.", call.Span);
                return ConstI32(0);
            }

            if (_builtIns.Contains(id.Identifier.Text))
            {
                return LowerBuiltInCall(builder, id.Identifier.Text, call.Arguments, locals, call.Span);
            }

            if (TryInlineCall(builder, call, id.Identifier.Text, locals, out var inlined))
            {
                return inlined;
            }

            if (!_symbols.TryGetValue(id.Identifier.Text, out var sym) || sym.Kind is not (SymbolKind.Function or SymbolKind.Test))
            {
                AddDiagnostic($"Unknown function '{id.Identifier.Text}'.", call.Span);
                return ConstI32(0);
            }

            var fn = _moduleBuilder.Module.GetNamedFunction(id.Identifier.Text);
            if (fn.Handle == IntPtr.Zero)
            {
                if (_functions.TryGetValue(id.Identifier.Text, out var decl) && decl.IsExtern)
                {
                    var externSignature = ResolveFunctionSignature(id.Identifier.Text);
                    var externType = LLVMTypeRef.CreateFunction(externSignature.ReturnType, externSignature.Parameters, false);
                    fn = _moduleBuilder.Module.AddFunction(id.Identifier.Text, externType);
                }
            }

            if (fn.Handle == IntPtr.Zero)
            {
                AddDiagnostic($"Function '{id.Identifier.Text}' missing from module.", call.Span);
                return ConstI32(0);
            }

            var paramSymbols = ResolveParameterTypes(id.Identifier.Text);
            var argValues = new LLVMValueRef[call.Arguments.Count];
            for (int i = 0; i < call.Arguments.Count; i++)
            {
                if (i < paramSymbols.Length && paramSymbols[i] is ArrayTypeSymbol arr)
                {
                    var layout = CreateArrayDescriptorLayout(arr, _moduleBuilder.TypeMapper, _structs, _symbols);
                    argValues[i] = BuildArrayDescriptorValue(builder, call.Arguments[i], arr, layout, locals);
                }
                else
                {
                    argValues[i] = LowerExpression(builder, call.Arguments[i], locals);
                }
            }
            var signature = ResolveFunctionSignature(id.Identifier.Text);
            var fnType = LLVMTypeRef.CreateFunction(signature.ReturnType, signature.Parameters, false);

            var callRetType = fnType.ReturnType;
            if (callRetType.Kind == LLVMTypeKind.LLVMVoidTypeKind)
            {
                builder.BuildCall2(fnType, fn, argValues, string.Empty);
                return ConstI32(0);
            }

            var callValue = builder.BuildCall2(fnType, fn, argValues, $"{id.Identifier.Text}.call");
            return callValue;
        }

        private void LowerClear(LLVMBuilderRef builder, ExpressionSyntax receiver, Dictionary<string, LocalBinding> locals, SourceSpan span)
        {
            var receiverType = ResolveExpressionType(receiver);
            if (receiverType is ArrayTypeSymbol array)
            {
                LowerClearArray(builder, receiver, array, locals, span);
                return;
            }

            if (receiverType is NamedTypeSymbol named && _structs.TryGetValue(named.TypeName, out var structDecl))
            {
                var baseName = receiver switch
                {
                    IdentifierExpressionSyntax id => TryResolveGlobalName(id.Identifier.Text),
                    MemberAccessExpressionSyntax member => BuildFlattenedMemberPath(member),
                    _ => null
                };

                if (string.IsNullOrEmpty(baseName))
                {
                    AddDiagnostic("clear() receiver must be a global or global field.", receiver.Span);
                    return;
                }

                LowerClearStruct(builder, baseName!, structDecl, span);
                return;
            }

            AddDiagnostic("clear() is only supported for zeroable fixed arrays and structs.", receiver.Span);
        }

        private void LowerClearArray(LLVMBuilderRef builder, ExpressionSyntax receiver, ArrayTypeSymbol array, Dictionary<string, LocalBinding> locals, SourceSpan span)
        {
            if (array.Size <= 0)
            {
                AddDiagnostic("clear() requires a fixed-size array.", receiver.Span);
                return;
            }

            if (array.ElementType is NamedTypeSymbol elemNamed && _structs.TryGetValue(elemNamed.TypeName, out var structDecl))
            {
                // Global AoS struct array lowers to SoA field arrays (e.g., global units: Unit[8] -> Unit_hp[], Unit_x[], ...).
                // This is a compiler storage detail; clear() means "zero all backing SoA arrays".
                foreach (var field in structDecl.Fields)
                {
                    var fieldType = ResolveType(field.Type, _symbols);
                    if (fieldType is null)
                    {
                        continue;
                    }

                    // Only zeroable primitive scalars/arrays-of-primitive are supported for now.
                    if (fieldType is PrimitiveTypeSymbol primField &&
                        primField.PrimitiveName is ("u8" or "u16" or "u32" or "i32" or "f32" or "f64" or "bool"))
                    {
                        var backingGlobal = $"{elemNamed.TypeName}_{field.Identifier.Text}";
                        EmitClearGlobalArray(builder, backingGlobal, primField, array.Size, field.Span);
                        continue;
                    }

                    if (fieldType is ArrayTypeSymbol arrField &&
                        arrField.Size > 0 &&
                        arrField.ElementType is PrimitiveTypeSymbol primElem &&
                        primElem.PrimitiveName is ("u8" or "u16" or "u32" or "i32" or "f32" or "f64" or "bool"))
                    {
                        // Array fields inside struct arrays are laid out as a byte buffer in the global lowering.
                        // Treat as u8 backing for clearing.
                        var backingGlobal = $"{elemNamed.TypeName}_{field.Identifier.Text}";
                        EmitClearGlobalArray(builder, backingGlobal, new PrimitiveTypeSymbol("u8"), array.Size * arrField.Size, field.Span);
                        continue;
                    }

                    AddDiagnostic("clear() only supports struct arrays with zeroable primitive fields.", field.Span);
                }

                return;
            }

            if (array.ElementType is not PrimitiveTypeSymbol prim ||
                prim.PrimitiveName is not ("u8" or "u16" or "u32" or "i32" or "f32" or "f64" or "bool"))
            {
                AddDiagnostic("clear() only supports arrays of zeroable primitive elements.", receiver.Span);
                return;
            }

            var dstPtr = LowerArrayPointer(builder, receiver, locals);
            if (dstPtr.Handle == IntPtr.Zero)
            {
                AddDiagnostic("clear() requires an array receiver.", receiver.Span);
                return;
            }

            var count = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, (ulong)array.Size, true);

            if (prim.PrimitiveName is "u8" or "bool")
            {
                EmitSysMemsetU8(builder, dstPtr, ConstI32(0), ConstI32(0), count);
                return;
            }

            if (prim.PrimitiveName is "u16")
            {
                // Treat u16 as i32 element-wise for clearing: write zeros with scalar stores in a loop.
                // (u16 has no dedicated sys_memset helper.)
                EmitClearLoop(builder, dstPtr, LLVMTypeRef.Int16, count);
                return;
            }

            if (prim.PrimitiveName is "u32" or "i32")
            {
                EmitSysMemsetI32(builder, dstPtr, ConstI32(0), ConstI32(0), count);
                return;
            }

            if (prim.PrimitiveName is "f32")
            {
                EmitSysMemsetF32(builder, dstPtr, ConstI32(0), LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0), count);
                return;
            }

            // f64: clear via loop (no dedicated helper).
            EmitClearLoop(builder, dstPtr, LLVMTypeRef.Double, count);
        }

        private void LowerClearStruct(LLVMBuilderRef builder, string baseName, StructDeclarationSyntax structDecl, SourceSpan span)
        {
            foreach (var field in structDecl.Fields)
            {
                var fieldName = $"{baseName}_{field.Identifier.Text}";

                switch (field.Type)
                {
                    case NamedTypeSyntax namedField when _structs.TryGetValue(namedField.Name, out var nestedStruct):
                        LowerClearStruct(builder, fieldName, nestedStruct, span);
                        continue;
                    case ArrayTypeSyntax arrayType when arrayType.ElementType is NamedTypeSyntax nestedNamed &&
                                                       _structs.TryGetValue(nestedNamed.Name, out var nestedStructArray):
                        {
                            if (!int.TryParse(arrayType.SizeToken?.Text ?? string.Empty, out var count) || count <= 0)
                            {
                                AddDiagnostic("clear() requires fixed-size arrays inside structs.", field.Span);
                                continue;
                            }

                            foreach (var nestedField in nestedStructArray.Fields)
                            {
                                if (nestedField.Type is not NamedTypeSyntax leafNamed)
                                {
                                    AddDiagnostic("clear() only supports scalar fields inside struct arrays.", nestedField.Span);
                                    continue;
                                }

                                var leafType = ResolveType(leafNamed, _symbols);
                                if (leafType is not PrimitiveTypeSymbol leafPrim ||
                                    leafPrim.PrimitiveName is not ("u8" or "bool" or "u16" or "u32" or "i32" or "f32" or "f64"))
                                {
                                    AddDiagnostic("clear() only supports zeroable primitive fields inside struct arrays.", nestedField.Span);
                                    continue;
                                }

                                var leafGlobal = $"{fieldName}_{nestedField.Identifier.Text}";
                                EmitClearGlobalArray(builder, leafGlobal, leafPrim, count, nestedField.Span);
                            }

                            continue;
                        }
                    case ArrayTypeSyntax arrayType:
                        {
                            if (arrayType.ElementType is not NamedTypeSyntax elemNamed)
                            {
                                AddDiagnostic("clear() only supports primitive arrays.", field.Span);
                                continue;
                            }

                            var elemType = ResolveType(elemNamed, _symbols);
                            if (elemType is not PrimitiveTypeSymbol elemPrim ||
                                elemPrim.PrimitiveName is not ("u8" or "bool" or "u16" or "u32" or "i32" or "f32" or "f64"))
                            {
                                AddDiagnostic("clear() only supports arrays of zeroable primitives.", field.Span);
                                continue;
                            }

                            if (!int.TryParse(arrayType.SizeToken?.Text ?? string.Empty, out var count) || count <= 0)
                            {
                                AddDiagnostic("clear() requires fixed-size arrays.", field.Span);
                                continue;
                            }

                            EmitClearGlobalArray(builder, fieldName, elemPrim, count, field.Span);
                            continue;
                        }
                }

                // Scalar field
                var fieldType = ResolveType(field.Type, _symbols);
                if (fieldType is not PrimitiveTypeSymbol fieldPrim ||
                    fieldPrim.PrimitiveName is not ("u8" or "bool" or "u16" or "u32" or "i32" or "f32" or "f64"))
                {
                    AddDiagnostic("clear() only supports zeroable primitive scalar fields.", field.Span);
                    continue;
                }

                var global = _moduleBuilder.Module.GetNamedGlobal(fieldName);
                if (global.Handle == IntPtr.Zero)
                {
                    AddDiagnostic($"Missing global backing for '{fieldName}'.", field.Span);
                    continue;
                }

                var zero = LLVMValueRef.CreateConstNull(global.TypeOf.ElementType);
                builder.BuildStore(zero, global);
            }
        }

        private void EmitClearGlobalArray(LLVMBuilderRef builder, string globalName, PrimitiveTypeSymbol elemPrim, int count, SourceSpan span)
        {
            var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
            if (global.Handle == IntPtr.Zero)
            {
                AddDiagnostic($"Missing array global '{globalName}'.", span);
                return;
            }

            var arrayType = global.TypeOf.ElementType;
            var ptr = builder.BuildGEP2(arrayType, global, new[] { ConstI32(0), ConstI32(0) }, $"{globalName}.ptr");
            var countVal = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, (ulong)Math.Max(0, count), true);

            if (elemPrim.PrimitiveName is "u8" or "bool")
            {
                EmitSysMemsetU8(builder, ptr, ConstI32(0), ConstI32(0), countVal);
                return;
            }
            if (elemPrim.PrimitiveName is "u32" or "i32")
            {
                EmitSysMemsetI32(builder, ptr, ConstI32(0), ConstI32(0), countVal);
                return;
            }
            if (elemPrim.PrimitiveName is "f32")
            {
                EmitSysMemsetF32(builder, ptr, ConstI32(0), LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0), countVal);
                return;
            }

            // u16/f64: fallback loop
            var elemTy = elemPrim.PrimitiveName == "u16"
                ? LLVMTypeRef.Int16
                : LLVMTypeRef.Double;
            EmitClearLoop(builder, ptr, elemTy, countVal);
        }

        private void EmitSysMemsetU8(LLVMBuilderRef builder, LLVMValueRef dstPtr, LLVMValueRef dstIndex, LLVMValueRef valueI32, LLVMValueRef count)
        {
            var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memset_u8");
            var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                new[]
                {
                    LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                    LLVMTypeRef.Int32,
                    LLVMTypeRef.Int32,
                    LLVMTypeRef.Int32
                }, false);
            if (fn.Handle == IntPtr.Zero)
                fn = _moduleBuilder.Module.AddFunction("stasis_sys_memset_u8", fnType);

            var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "clear.memset_u8.dst");
            builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, valueI32, count }, string.Empty);
        }

        private void EmitSysMemsetI32(LLVMBuilderRef builder, LLVMValueRef dstPtr, LLVMValueRef dstIndex, LLVMValueRef valueI32, LLVMValueRef count)
        {
            var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memset_i32");
            var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                new[]
                {
                    LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0),
                    LLVMTypeRef.Int32,
                    LLVMTypeRef.Int32,
                    LLVMTypeRef.Int32
                }, false);
            if (fn.Handle == IntPtr.Zero)
                fn = _moduleBuilder.Module.AddFunction("stasis_sys_memset_i32", fnType);

            var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), "clear.memset_i32.dst");
            builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, valueI32, count }, string.Empty);
        }

        private void EmitSysMemsetF32(LLVMBuilderRef builder, LLVMValueRef dstPtr, LLVMValueRef dstIndex, LLVMValueRef valueF32, LLVMValueRef count)
        {
            var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memset_f32");
            var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                new[]
                {
                    LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0),
                    LLVMTypeRef.Int32,
                    LLVMTypeRef.Float,
                    LLVMTypeRef.Int32
                }, false);
            if (fn.Handle == IntPtr.Zero)
                fn = _moduleBuilder.Module.AddFunction("stasis_sys_memset_f32", fnType);

            var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0), "clear.memset_f32.dst");
            builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, valueF32, count }, string.Empty);
        }

        private void EmitClearLoop(LLVMBuilderRef builder, LLVMValueRef basePtr, LLVMTypeRef elemType, LLVMValueRef countI32)
        {
            var function = builder.InsertBlock.Parent;
            var idxAlloca = builder.BuildAlloca(LLVMTypeRef.Int32, "clear.i");
            builder.BuildStore(ConstI32(0), idxAlloca);

            var condBlock = AppendBlock(function, NextBlockName("clear.cond"));
            var bodyBlock = AppendBlock(function, NextBlockName("clear.body"));
            var endBlock = AppendBlock(function, NextBlockName("clear.end"));

            builder.BuildBr(condBlock);
            builder.PositionAtEnd(condBlock);
            var idx = builder.BuildLoad2(LLVMTypeRef.Int32, idxAlloca, "clear.i.load");
            var cond = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, idx, countI32, "clear.i.lt");
            builder.BuildCondBr(cond, bodyBlock, endBlock);

            builder.PositionAtEnd(bodyBlock);
            var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
            var typedBase = builder.BuildBitCast(basePtr, elemPtrType, "clear.base");
            var elemPtr = builder.BuildGEP2(elemType, typedBase, new[] { idx }, "clear.elem");
            var zero = LLVMValueRef.CreateConstNull(elemType);
            builder.BuildStore(zero, elemPtr);
            var next = builder.BuildAdd(idx, ConstI32(1), "clear.i.next");
            builder.BuildStore(next, idxAlloca);
            builder.BuildBr(condBlock);

            builder.PositionAtEnd(endBlock);
        }

        private bool TryInlineCall(
            LLVMBuilderRef builder,
            CallExpressionSyntax call,
            string funcName,
            Dictionary<string, LocalBinding> locals,
            out LLVMValueRef result)
        {
            result = default;

            if (!_functions.TryGetValue(funcName, out var func))
            {
                return false;
            }

            if (func.IsExtern || func.Body is null)
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

            // Restrict v1 inlining to straight-line blocks (no control flow) so we don't
            // need to rewrite returns/labels in the caller.
            foreach (var stmt in func.Body.Statements)
            {
                if (stmt is VariableDeclarationSyntax or ExpressionStatementSyntax or ReturnStatementSyntax)
                {
                    continue;
                }

                return false;
            }

            var signature = ResolveFunctionSignature(funcName);
            var fnType = LLVMTypeRef.CreateFunction(signature.ReturnType, signature.Parameters, false);
            var isVoid = fnType.ReturnType.Kind == LLVMTypeKind.LLVMVoidTypeKind;

            if (!isVoid)
            {
                if (func.Body.Statements.Count == 0)
                {
                    return false;
                }

                if (func.Body.Statements[^1] is not ReturnStatementSyntax { Expression: not null })
                {
                    return false;
                }
            }

            var savedLocals = new List<(string Name, bool HadValue, LocalBinding Value)>();
            void SaveLocal(string name)
            {
                for (int i = 0; i < savedLocals.Count; i++)
                {
                    if (string.Equals(savedLocals[i].Name, name, StringComparison.Ordinal))
                    {
                        return;
                    }
                }

                if (locals.TryGetValue(name, out var existing))
                {
                    savedLocals.Add((name, true, existing));
                }
                else
                {
                    savedLocals.Add((name, false, default));
                }
            }

            _inlineStack.Add(funcName);

            // Bind parameters by creating stack slots in the caller and storing evaluated args.
            for (int i = 0; i < func.Parameters.Count; i++)
            {
                var param = func.Parameters[i];
                var paramType = ResolveType(param.Type, _symbols);

                if (paramType is ArrayTypeSymbol arr)
                {
                    var layout = CreateArrayDescriptorLayout(arr, _moduleBuilder.TypeMapper, _structs, _symbols);
                    var llvmType = layout.DescriptorType;
                    var argValue = BuildArrayDescriptorValue(builder, call.Arguments[i], arr, layout, locals);

                    SaveLocal(param.Name.Text);
                    var alloca = builder.BuildAlloca(llvmType, param.Name.Text);
                    builder.BuildStore(argValue, alloca);
                    locals[param.Name.Text] = new LocalBinding(alloca, llvmType, true, null, paramType, layout, true);
                }
                else
                {
                    var llvmType = _moduleBuilder.TypeMapper.Map(paramType);
                    var argValue = LowerExpression(builder, call.Arguments[i], locals);
                    argValue = ConvertToType(builder, argValue, llvmType);

                    SaveLocal(param.Name.Text);
                    var alloca = builder.BuildAlloca(llvmType, param.Name.Text);
                    builder.BuildStore(argValue, alloca);
                    locals[param.Name.Text] = new LocalBinding(alloca, llvmType, true, null, paramType);
                }
            }

            LLVMValueRef inlineResult = isVoid ? ConstI32(0) : LLVMValueRef.CreateConstNull(fnType.ReturnType);
            var hasResult = false;

            foreach (var stmt in func.Body.Statements)
            {
                switch (stmt)
                {
                    case VariableDeclarationSyntax decl:
                        SaveLocal(decl.Name.Text);
                        LowerVariableDeclaration(builder, decl, locals);
                        break;
                    case ExpressionStatementSyntax exprStmt:
                        LowerExpression(builder, exprStmt.Expression, locals);
                        break;
                    case ReturnStatementSyntax ret:
                        if (!isVoid)
                        {
                            if (ret.Expression is null)
                            {
                                RestoreLocals(savedLocals, locals);
                                _inlineStack.Remove(funcName);
                                return false;
                            }

                            inlineResult = LowerExpression(builder, ret.Expression, locals);
                            inlineResult = ConvertToType(builder, inlineResult, fnType.ReturnType);
                            hasResult = true;
                        }

                        goto done;
                }
            }

        done:
            RestoreLocals(savedLocals, locals);
            _inlineStack.Remove(funcName);

            if (!isVoid && !hasResult)
            {
                return false;
            }

            result = isVoid ? ConstI32(0) : inlineResult;
            return true;
        }

        private static void RestoreLocals(List<(string Name, bool HadValue, LocalBinding Value)> savedLocals, Dictionary<string, LocalBinding> locals)
        {
            for (int i = savedLocals.Count - 1; i >= 0; i--)
            {
                var saved = savedLocals[i];
                if (!saved.HadValue)
                {
                    locals.Remove(saved.Name);
                }
                else
                {
                    locals[saved.Name] = saved.Value;
                }
            }
        }

        private static bool HasInlineAttribute(FunctionDeclarationSyntax func) =>
            func.Attributes.Any(attr => string.Equals(attr.Text, "inline", StringComparison.Ordinal));

        private LLVMValueRef BuildArrayDescriptorValue(LLVMBuilderRef builder, ExpressionSyntax expr, ArrayTypeSymbol arrayType, ArrayDescriptorLayout layout, Dictionary<string, LocalBinding> locals)
        {
            if (expr is IdentifierExpressionSyntax id && locals.TryGetValue(id.Identifier.Text, out var binding) && binding.IsArrayDescriptor)
            {
                return builder.BuildLoad2(layout.DescriptorType, binding.Value, $"{id.Identifier.Text}.desc");
            }

            if (expr is LiteralExpressionSyntax lit && lit.Literal.Kind == TokenKind.StringLiteral)
            {
                if (layout.IsStructArray)
                {
                    AddDiagnostic("String literal array arguments are only supported for primitive byte arrays.", expr.Span);
                    return LLVMValueRef.CreateConstNull(layout.DescriptorType);
                }

                var elemType = _moduleBuilder.TypeMapper.Map(arrayType.ElementType);
                if (elemType.Kind != LLVMTypeKind.LLVMIntegerTypeKind || elemType.IntWidth != 8)
                {
                    AddDiagnostic("String literal array arguments require element type u8.", expr.Span);
                    return LLVMValueRef.CreateConstNull(layout.DescriptorType);
                }

                var text = UnescapeString(lit.Literal.Text);
                var ptr = EmitUtf8Literal(builder, text);
                var lenVal = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, (ulong)(Encoding.UTF8.GetByteCount(text) + 1), true);

                var desc = LLVMValueRef.CreateConstNull(layout.DescriptorType);
                desc = builder.BuildInsertValue(desc, ptr, 0, "lit.ptr");
                desc = builder.BuildInsertValue(desc, lenVal, 1, "lit.len");
                return desc;
            }

            string? baseName = null;
            var resolvedLength = ResolveArrayLength(expr, arrayType);
            if (expr is IdentifierExpressionSyntax globalId)
            {
                baseName = TryResolveGlobalName(globalId.Identifier.Text);
            }
            else if (expr is MemberAccessExpressionSyntax member)
            {
                // Handle nested member access like state.frame_timer.samples_ms
                baseName = BuildFlattenedMemberPath(member);
            }

            if (string.IsNullOrEmpty(baseName))
            {
                AddDiagnostic("Array arguments must be globals, struct fields, or existing array parameters.", expr.Span);
                return LLVMValueRef.CreateConstNull(layout.DescriptorType);
            }

            if (layout.IsStructArray)
            {
                if (arrayType.ElementType is not NamedTypeSymbol namedElem || layout.StructDecl is null || layout.FieldOrder is null)
                {
                    AddDiagnostic("Unable to build struct array descriptor.", expr.Span);
                    return LLVMValueRef.CreateConstNull(layout.DescriptorType);
                }

                var desc = LLVMValueRef.CreateConstNull(layout.DescriptorType);
                foreach (var field in layout.StructDecl.Fields)
                {
                    if (!layout.FieldOrder.TryGetValue(field.Identifier.Text, out var idx))
                    {
                        continue;
                    }
                    var fieldType = ResolveType(field.Type, _symbols);
                    var llvmFieldType = _moduleBuilder.TypeMapper.Map(fieldType);
                    var ptrType = LLVMTypeRef.CreatePointer(llvmFieldType, 0);
                    var globalName = TryResolveFieldGlobalName(baseName, namedElem.TypeName, field.Identifier.Text);
                    var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
                    if (global.Handle == IntPtr.Zero)
                    {
                        AddDiagnostic($"Missing global backing for field '{field.Identifier.Text}' on '{baseName}'.", expr.Span);
                        return LLVMValueRef.CreateConstNull(layout.DescriptorType);
                    }
                    var bitcast = builder.BuildBitCast(global, ptrType, $"{globalName}.ptr");
                    desc = builder.BuildInsertValue(desc, bitcast, (uint)idx, $"{globalName}.desc");
                }
                var lenIndex = (uint)layout.FieldOrder.Count;
                var lenVal = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, (ulong)Math.Max(0, resolvedLength), true);
                desc = builder.BuildInsertValue(desc, lenVal, lenIndex, $"{baseName}.len");
                return desc;
            }
            else
            {
                var elementType = _moduleBuilder.TypeMapper.Map(arrayType.ElementType);
                var ptrType = LLVMTypeRef.CreatePointer(elementType, 0);
                var global = _moduleBuilder.Module.GetNamedGlobal(baseName);
                if (global.Handle == IntPtr.Zero)
                {
                    AddDiagnostic($"Missing array global '{baseName}'.", expr.Span);
                    return LLVMValueRef.CreateConstNull(layout.DescriptorType);
                }

                LLVMValueRef dataPtr;
                if (arrayType.ElementType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                {
                    var headerSize = HeaderSizeFor(prim.PrimitiveName);
                    var backingBytes = Math.Max(1, resolvedLength + headerSize);
                    var backingArrayType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)backingBytes);
                    var payloadPtr = builder.BuildGEP2(backingArrayType, global, new[] { ConstI32(0), ConstI32(headerSize) }, $"{baseName}.payload");
                    dataPtr = payloadPtr;
                }
                else
                {
                    dataPtr = builder.BuildBitCast(global, ptrType, $"{baseName}.ptr");
                }

                var desc = LLVMValueRef.CreateConstNull(layout.DescriptorType);
                desc = builder.BuildInsertValue(desc, dataPtr, 0, $"{baseName}.desc");
                var lenVal = LLVMValueRef.CreateConstInt(LLVMTypeRef.Int32, (ulong)Math.Max(0, resolvedLength), true);
                desc = builder.BuildInsertValue(desc, lenVal, 1, $"{baseName}.len");
                return desc;
            }
        }

        private int ResolveArrayLength(ExpressionSyntax expr, ArrayTypeSymbol fallbackType)
        {
            if (fallbackType.Size > 0)
            {
                return fallbackType.Size;
            }

            if (expr is IdentifierExpressionSyntax id && _symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Type is ArrayTypeSymbol arrSym && arrSym.Size > 0)
            {
                return arrSym.Size;
            }

            if (expr is MemberAccessExpressionSyntax member && member.Receiver is IdentifierExpressionSyntax recv && _symbols.TryGetValue(recv.Identifier.Text, out var recvSym) && recvSym.Type is NamedTypeSymbol named && _structs.TryGetValue(named.TypeName, out var structDecl))
            {
                var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
                if (field?.Type is ArrayTypeSyntax arraySyntax && int.TryParse(arraySyntax.SizeToken?.Text ?? string.Empty, out var parsed) && parsed > 0)
                {
                    return parsed;
                }
            }

            return Math.Max(0, fallbackType.Size);
        }

        private LLVMValueRef LowerBuiltInCall(LLVMBuilderRef builder, string name, IReadOnlyList<ExpressionSyntax> args, Dictionary<string, LocalBinding> locals, SourceSpan span)
        {
            switch (name)
            {
                case "print_string":
                case "print":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("print_string expects 1 argument.", span);
                            return ConstI32(0);
                        }

                        LLVMValueRef strPtr;
                        if (args[0] is LiteralExpressionSyntax lit && lit.Literal.Kind == TokenKind.StringLiteral)
                        {
                            var text = UnescapeString(lit.Literal.Text);
                            strPtr = EmitUtf8Literal(builder, text);
                        }
                        else
                        {
                            strPtr = LowerCStringPointer(builder, args[0], locals);
                        }

                        EmitPrintf(builder, "%s", strPtr);
                        return ConstI32(0);
                    }
                case "print_int":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("print_int expects 1 argument.", span);
                            return ConstI32(0);
                        }

                        var value = LowerExpression(builder, args[0], locals);
                        EmitPrintf(builder, " %d", value);
                        return ConstI32(0);
                    }
                case "print_char":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("print_char expects 1 argument.", span);
                            return ConstI32(0);
                        }

                        var value = LowerExpression(builder, args[0], locals);
                        EmitPrintf(builder, "%c", value);
                        return ConstI32(0);
                    }
                case "print_prompt":
                    EmitPrintf(builder, "Enter row col val (1-9, 0 clears), or q to quit:\n");
                    return ConstI32(0);
                case "print_invalid":
                    EmitPrintf(builder, "\u001b[31mInvalid move.\u001b[0m\n");
                    return ConstI32(0);
                case "print_clue_error":
                    EmitPrintf(builder, "\u001b[31mCannot change a clue.\u001b[0m\n");
                    return ConstI32(0);
                case "print_solved":
                    EmitPrintf(builder, "\u001b[32mSolved!\u001b[0m\n");
                    return ConstI32(0);
                case "print_cell":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("print_cell expects 2 arguments (value, is_clue).", span);
                            return ConstI32(0);
                        }

                        var val = LowerExpression(builder, args[0], locals);
                        var isClue = AsBoolean(builder, LowerExpression(builder, args[1], locals));
                        var zero = ConstI32(0);
                        var isEmpty = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, val, zero, "cell.empty");

                        var emptyBlock = AppendBlock(builder.InsertBlock.Parent, "cell.empty");
                        var printBlock = AppendBlock(builder.InsertBlock.Parent, "cell.print");
                        var contBlock = AppendBlock(builder.InsertBlock.Parent, "cell.cont");

                        builder.BuildCondBr(isEmpty, emptyBlock, printBlock);

                        builder.PositionAtEnd(emptyBlock);
                        EmitPrintf(builder, ". ");
                        builder.BuildBr(contBlock);

                        builder.PositionAtEnd(printBlock);
                        var cluePrefix = builder.BuildGlobalStringPtr("\u001b[36m", $"cell_clue_prefix_{_blockId++}");
                        var userPrefix = builder.BuildGlobalStringPtr("\u001b[32m", $"cell_user_prefix_{_blockId++}");
                        var reset = builder.BuildGlobalStringPtr("\u001b[0m ", $"cell_reset_{_blockId++}");
                        var prefix = builder.BuildSelect(isClue, cluePrefix, userPrefix, "cell_prefix");
                        EmitPrintf(builder, "%s", prefix);
                        EmitPrintf(builder, "%d", val);
                        EmitPrintf(builder, "%s", reset);
                        builder.BuildBr(contBlock);

                        builder.PositionAtEnd(contBlock);
                        return ConstI32(0);
                    }
                case "read_char":
                    return EmitReadChar(builder);
                case "read_int":
                    return EmitReadInt(builder);

                // ============================================================
                // System: sys_* (argv, file I/O, process execution)
                // ============================================================

                case "sys_argc":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("sys_argc expects no arguments.", span);
                            return ConstI32(0);
                        }

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_argc");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_argc", fnType);

                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "sys_argc.call");
                    }

                case "sys_argv":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("sys_argv expects (idx: i32, out: string, out_cap: i32).", span);
                            return ConstI32(-1);
                        }

                        var idx = LowerExpression(builder, args[0], locals);
                        var outPtr = LowerCStringPointer(builder, args[1], locals);
                        if (outPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_argv requires an output string buffer.", span);
                            return ConstI32(-1);
                        }
                        var outCap = LowerExpression(builder, args[2], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_argv");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[]
                            {
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_argv", fnType);

                        var outCast = builder.BuildBitCast(outPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_argv.out");
                        return builder.BuildCall2(fnType, fn, new[] { idx, outCast, outCap }, "sys_argv.call");
                    }

                case "sys_read_file":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("sys_read_file expects (path: string, out: u8[], out_cap: i32).", span);
                            return ConstI32(-1);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var outPtr = LowerCStringPointer(builder, args[1], locals);
                        if (outPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_read_file requires an output buffer.", span);
                            return ConstI32(-1);
                        }
                        var outCap = LowerExpression(builder, args[2], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_read_file");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_read_file", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_read_file.path");
                        var outCast = builder.BuildBitCast(outPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_read_file.out");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast, outCast, outCap }, "sys_read_file.call");
                    }

                case "sys_list_dir":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("sys_list_dir expects (path: string, out: u8[], out_cap: i32).", span);
                            return ConstI32(-1);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var outPtr = LowerCStringPointer(builder, args[1], locals);
                        if (outPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_list_dir requires an output buffer.", span);
                            return ConstI32(-1);
                        }
                        var outCap = LowerExpression(builder, args[2], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_list_dir");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_list_dir", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_list_dir.path");
                        var outCast = builder.BuildBitCast(outPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_list_dir.out");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast, outCast, outCap }, "sys_list_dir.call");
                    }

                case "sys_write_file":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("sys_write_file expects (path: string, data: u8[], len: i32).", span);
                            return ConstI32(0);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var dataPtr = LowerCStringPointer(builder, args[1], locals);
                        if (dataPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_write_file requires a byte buffer.", span);
                            return ConstI32(0);
                        }
                        var len = LowerExpression(builder, args[2], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_write_file");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_write_file", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_write_file.path");
                        var dataCast = builder.BuildBitCast(dataPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_write_file.data");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast, dataCast, len }, "sys_write_file.call");
                    }

                case "sys_file_exists":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_file_exists expects (path: string).", span);
                            return ConstI32(0);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_file_exists");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_file_exists", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_file_exists.path");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast }, "sys_file_exists.call");
                    }

                case "sys_file_size":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_file_size expects (path: string).", span);
                            return ConstI32(-1);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_file_size");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_file_size", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_file_size.path");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast }, "sys_file_size.call");
                    }

                case "sys_file_mtime_ms":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_file_mtime_ms expects (path: string).", span);
                            return ConstI32(-1);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_file_mtime_ms");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_file_mtime_ms", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_file_mtime_ms.path");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast }, "sys_file_mtime_ms.call");
                    }

                case "sys_exec":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_exec expects (command: string).", span);
                            return ConstI32(1);
                        }

                        var cmd = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_exec");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_exec", fnType);

                        var cmdCast = builder.BuildBitCast(cmd, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_exec.cmd");
                        return builder.BuildCall2(fnType, fn, new[] { cmdCast }, "sys_exec.call");
                    }

                case "sys_spawn":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_spawn expects (command_line: string).", span);
                            return ConstI32(1);
                        }

                        var cmd = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_spawn");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_spawn", fnType);

                        var cmdCast = builder.BuildBitCast(cmd, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_spawn.cmd");
                        return builder.BuildCall2(fnType, fn, new[] { cmdCast }, "sys_spawn.call");
                    }

                case "sys_spawn_async":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_spawn_async expects (command_line: string).", span);
                            return ConstI32(0);
                        }

                        var cmd = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_spawn_async");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_spawn_async", fnType);

                        var cmdCast = builder.BuildBitCast(cmd, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_spawn_async.cmd");
                        return builder.BuildCall2(fnType, fn, new[] { cmdCast }, "sys_spawn_async.call");
                    }

                case "sys_sleep_ms":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_sleep_ms expects (ms: i32).", span);
                            return ConstI32(0);
                        }

                        var ms = LowerExpression(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_sleep_ms");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.Int32 }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_sleep_ms", fnType);

                        return builder.BuildCall2(fnType, fn, new[] { ms }, "sys_sleep_ms.call");
                    }

                case "sys_delete_file":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sys_delete_file expects (path: string).", span);
                            return ConstI32(1);
                        }

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_delete_file");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_delete_file", fnType);

                        var pathCast = builder.BuildBitCast(path, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_delete_file.path");
                        return builder.BuildCall2(fnType, fn, new[] { pathCast }, "sys_delete_file.call");
                    }

                case "sys_time_ms":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("sys_time_ms expects no arguments.", span);
                            return ConstI32(0);
                        }

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_time_ms");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_time_ms", fnType);

                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "sys_time_ms.call");
                    }

                case "sys_flush":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("sys_flush expects no arguments.", span);
                            return ConstI32(1);
                        }

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_flush");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, Array.Empty<LLVMTypeRef>(), false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_flush", fnType);

                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "sys_flush.call");
                    }

                case "sys_memcpy_u8":
                    {
                        if (args.Count != 5)
                        {
                            AddDiagnostic("sys_memcpy_u8 expects (dst: u8[], dst_index: i32, src: u8[], src_index: i32, count: i32).", span);
                            return ConstI32(0);
                        }

                        var dstPtr = LowerArrayPointer(builder, args[0], locals);
                        var srcPtr = LowerArrayPointer(builder, args[2], locals);
                        if (dstPtr.Handle == IntPtr.Zero || srcPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_memcpy_u8 expects array arguments (dst, src).", span);
                            return ConstI32(0);
                        }

                        var dstIndex = LowerExpression(builder, args[1], locals);
                        var srcIndex = LowerExpression(builder, args[3], locals);
                        var count = LowerExpression(builder, args[4], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memcpy_u8");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_memcpy_u8", fnType);

                        var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_memcpy_u8.dst");
                        var srcCast = builder.BuildBitCast(srcPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_memcpy_u8.src");
                        builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, srcCast, srcIndex, count }, string.Empty);
                        return ConstI32(0);
                    }

                case "sys_memcpy_i32":
                    {
                        if (args.Count != 5)
                        {
                            AddDiagnostic("sys_memcpy_i32 expects (dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32).", span);
                            return ConstI32(0);
                        }

                        var dstPtr = LowerArrayPointer(builder, args[0], locals);
                        var srcPtr = LowerArrayPointer(builder, args[2], locals);
                        if (dstPtr.Handle == IntPtr.Zero || srcPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_memcpy_i32 expects array arguments (dst, src).", span);
                            return ConstI32(0);
                        }

                        var dstIndex = LowerExpression(builder, args[1], locals);
                        var srcIndex = LowerExpression(builder, args[3], locals);
                        var count = LowerExpression(builder, args[4], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memcpy_i32");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_memcpy_i32", fnType);

                        var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), "sys_memcpy_i32.dst");
                        var srcCast = builder.BuildBitCast(srcPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), "sys_memcpy_i32.src");
                        builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, srcCast, srcIndex, count }, string.Empty);
                        return ConstI32(0);
                    }

                case "sys_memcpy_f32":
                    {
                        if (args.Count != 5)
                        {
                            AddDiagnostic("sys_memcpy_f32 expects (dst: f32[], dst_index: i32, src: f32[], src_index: i32, count: i32).", span);
                            return ConstI32(0);
                        }

                        var dstPtr = LowerArrayPointer(builder, args[0], locals);
                        var srcPtr = LowerArrayPointer(builder, args[2], locals);
                        if (dstPtr.Handle == IntPtr.Zero || srcPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_memcpy_f32 expects array arguments (dst, src).", span);
                            return ConstI32(0);
                        }

                        var dstIndex = LowerExpression(builder, args[1], locals);
                        var srcIndex = LowerExpression(builder, args[3], locals);
                        var count = LowerExpression(builder, args[4], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memcpy_f32");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_memcpy_f32", fnType);

                        var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0), "sys_memcpy_f32.dst");
                        var srcCast = builder.BuildBitCast(srcPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0), "sys_memcpy_f32.src");
                        builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, srcCast, srcIndex, count }, string.Empty);
                        return ConstI32(0);
                    }

                case "sys_memmove_u8":
                    {
                        if (args.Count != 5)
                        {
                            AddDiagnostic("sys_memmove_u8 expects (dst: u8[], dst_index: i32, src: u8[], src_index: i32, count: i32).", span);
                            return ConstI32(0);
                        }

                        var dstPtr = LowerArrayPointer(builder, args[0], locals);
                        var srcPtr = LowerArrayPointer(builder, args[2], locals);
                        if (dstPtr.Handle == IntPtr.Zero || srcPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_memmove_u8 expects array arguments (dst, src).", span);
                            return ConstI32(0);
                        }

                        var dstIndex = LowerExpression(builder, args[1], locals);
                        var srcIndex = LowerExpression(builder, args[3], locals);
                        var count = LowerExpression(builder, args[4], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memmove_u8");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_memmove_u8", fnType);

                        var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_memmove_u8.dst");
                        var srcCast = builder.BuildBitCast(srcPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "sys_memmove_u8.src");
                        builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, srcCast, srcIndex, count }, string.Empty);
                        return ConstI32(0);
                    }

                case "sys_memmove_i32":
                    {
                        if (args.Count != 5)
                        {
                            AddDiagnostic("sys_memmove_i32 expects (dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32).", span);
                            return ConstI32(0);
                        }

                        var dstPtr = LowerArrayPointer(builder, args[0], locals);
                        var srcPtr = LowerArrayPointer(builder, args[2], locals);
                        if (dstPtr.Handle == IntPtr.Zero || srcPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_memmove_i32 expects array arguments (dst, src).", span);
                            return ConstI32(0);
                        }

                        var dstIndex = LowerExpression(builder, args[1], locals);
                        var srcIndex = LowerExpression(builder, args[3], locals);
                        var count = LowerExpression(builder, args[4], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memmove_i32");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_memmove_i32", fnType);

                        var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), "sys_memmove_i32.dst");
                        var srcCast = builder.BuildBitCast(srcPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), "sys_memmove_i32.src");
                        builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, srcCast, srcIndex, count }, string.Empty);
                        return ConstI32(0);
                    }

                case "sys_memmove_f32":
                    {
                        if (args.Count != 5)
                        {
                            AddDiagnostic("sys_memmove_f32 expects (dst: f32[], dst_index: i32, src: f32[], src_index: i32, count: i32).", span);
                            return ConstI32(0);
                        }

                        var dstPtr = LowerArrayPointer(builder, args[0], locals);
                        var srcPtr = LowerArrayPointer(builder, args[2], locals);
                        if (dstPtr.Handle == IntPtr.Zero || srcPtr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("sys_memmove_f32 expects array arguments (dst, src).", span);
                            return ConstI32(0);
                        }

                        var dstIndex = LowerExpression(builder, args[1], locals);
                        var srcIndex = LowerExpression(builder, args[3], locals);
                        var count = LowerExpression(builder, args[4], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_sys_memmove_f32");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0),
                                LLVMTypeRef.Int32,
                                LLVMTypeRef.Int32
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_sys_memmove_f32", fnType);

                        var dstCast = builder.BuildBitCast(dstPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0), "sys_memmove_f32.dst");
                        var srcCast = builder.BuildBitCast(srcPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Float, 0), "sys_memmove_f32.src");
                        builder.BuildCall2(fnType, fn, new[] { dstCast, dstIndex, srcCast, srcIndex, count }, string.Empty);
                        return ConstI32(0);
                    }

                case "sin":
                case "cos":
                case "sin_fast":
                case "cos_fast":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic($"{name} expects a single f32 argument.", span);
                            return ConstI32(0);
                        }

                        var arg = LowerExpression(builder, args[0], locals);
                        var floatArg = ConvertToType(builder, arg, LLVMTypeRef.Float);
                        var useSin = name.StartsWith("sin", StringComparison.Ordinal);
                        var fast = name.EndsWith("_fast", StringComparison.Ordinal);
                        var (fn, fnType) = useSin ? GetOrDeclareLlvmSin(_moduleBuilder) : GetOrDeclareLlvmCos(_moduleBuilder);
                        var call = builder.BuildCall2(fnType, fn, new[] { floatArg }, $"{name}.call");
                        if (fast)
                        {
                            unsafe
                            {
                                LLVM.SetFastMathFlags(call, (uint)LLVMFastMathFlags.LLVMFastMathAll);
                            }
                        }
                        return call;
                    }
                case "i32_to_f32":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("i32_to_f32 expects a single i32 argument.", span);
                            return ConstF32(0);
                        }
                        var arg = LowerExpression(builder, args[0], locals);
                        return builder.BuildSIToFP(arg, LLVMTypeRef.Float, "i32tof32");
                    }
                case "f32_to_i32":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("f32_to_i32 expects a single f32 argument.", span);
                            return ConstI32(0);
                        }
                        var arg = LowerExpression(builder, args[0], locals);
                        return builder.BuildFPToSI(arg, LLVMTypeRef.Int32, "f32toi32");
                    }
                case "u8_to_i32":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("u8_to_i32 expects a single u8 argument.", span);
                            return ConstI32(0);
                        }

                        LLVMValueRef arg;
                        if (TryLowerIntegerLiteralToType(args[0], LLVMTypeRef.Int8, out var lowered))
                        {
                            arg = lowered;
                        }
                        else
                        {
                            arg = LowerExpression(builder, args[0], locals);
                        }

                        if (arg.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || arg.TypeOf.IntWidth != 8)
                        {
                            AddDiagnostic("u8_to_i32 expects a u8 value.", span);
                            return ConstI32(0);
                        }

                        return builder.BuildZExt(arg, LLVMTypeRef.Int32, "u8toi32");
                    }
                case "u16_to_i32":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("u16_to_i32 expects a single u16 argument.", span);
                            return ConstI32(0);
                        }

                        LLVMValueRef arg;
                        if (TryLowerIntegerLiteralToType(args[0], LLVMTypeRef.Int16, out var lowered))
                        {
                            arg = lowered;
                        }
                        else
                        {
                            arg = LowerExpression(builder, args[0], locals);
                        }

                        if (arg.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || arg.TypeOf.IntWidth != 16)
                        {
                            AddDiagnostic("u16_to_i32 expects a u16 value.", span);
                            return ConstI32(0);
                        }

                        return builder.BuildZExt(arg, LLVMTypeRef.Int32, "u16toi32");
                    }
                case "i32_to_u8_trunc":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("i32_to_u8_trunc expects a single i32 argument.", span);
                            return ConstI8(0);
                        }

                        var arg = LowerExpression(builder, args[0], locals);
                        if (arg.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || arg.TypeOf.IntWidth != 32)
                        {
                            AddDiagnostic("i32_to_u8_trunc expects an i32 value.", span);
                            return ConstI8(0);
                        }

                        return builder.BuildTrunc(arg, LLVMTypeRef.Int8, "i32tou8.trunc");
                    }
                case "i32_to_u8_checked":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("i32_to_u8_checked expects a single i32 argument.", span);
                            return ConstI8(0);
                        }

                        var arg = LowerExpression(builder, args[0], locals);
                        if (arg.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || arg.TypeOf.IntWidth != 32)
                        {
                            AddDiagnostic("i32_to_u8_checked expects an i32 value.", span);
                            return ConstI8(0);
                        }

                        var (abortFn, abortType) = GetOrDeclareAbort(_moduleBuilder);
                        var isNeg = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, arg, ConstI32(0), "i32tou8.neg");
                        var isGt = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGT, arg, ConstI32(255), "i32tou8.gt");
                        var bad = builder.BuildOr(isNeg, isGt, "i32tou8.bad");

                        var fn = builder.InsertBlock.Parent;
                        var okBlock = AppendBlock(fn, NextBlockName("i32tou8.ok"));
                        var abortBlock = AppendBlock(fn, NextBlockName("i32tou8.abort"));
                        builder.BuildCondBr(bad, abortBlock, okBlock);

                        builder.PositionAtEnd(abortBlock);
                        builder.BuildCall2(abortType, abortFn, Array.Empty<LLVMValueRef>(), "");
                        builder.BuildUnreachable();

                        builder.PositionAtEnd(okBlock);
                        return builder.BuildTrunc(arg, LLVMTypeRef.Int8, "i32tou8.checked");
                    }
                case "i32_to_u16_trunc":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("i32_to_u16_trunc expects a single i32 argument.", span);
                            return ConstI16(0);
                        }

                        var arg = LowerExpression(builder, args[0], locals);
                        if (arg.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || arg.TypeOf.IntWidth != 32)
                        {
                            AddDiagnostic("i32_to_u16_trunc expects an i32 value.", span);
                            return ConstI16(0);
                        }

                        return builder.BuildTrunc(arg, LLVMTypeRef.Int16, "i32tou16.trunc");
                    }
                case "i32_to_u16_checked":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("i32_to_u16_checked expects a single i32 argument.", span);
                            return ConstI16(0);
                        }

                        var arg = LowerExpression(builder, args[0], locals);
                        if (arg.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || arg.TypeOf.IntWidth != 32)
                        {
                            AddDiagnostic("i32_to_u16_checked expects an i32 value.", span);
                            return ConstI16(0);
                        }

                        var (abortFn, abortType) = GetOrDeclareAbort(_moduleBuilder);
                        var isNeg = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, arg, ConstI32(0), "i32tou16.neg");
                        var isGt = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGT, arg, ConstI32(65535), "i32tou16.gt");
                        var bad = builder.BuildOr(isNeg, isGt, "i32tou16.bad");

                        var fn = builder.InsertBlock.Parent;
                        var okBlock = AppendBlock(fn, NextBlockName("i32tou16.ok"));
                        var abortBlock = AppendBlock(fn, NextBlockName("i32tou16.abort"));
                        builder.BuildCondBr(bad, abortBlock, okBlock);

                        builder.PositionAtEnd(abortBlock);
                        builder.BuildCall2(abortType, abortFn, Array.Empty<LLVMValueRef>(), "");
                        builder.BuildUnreachable();

                        builder.PositionAtEnd(okBlock);
                        return builder.BuildTrunc(arg, LLVMTypeRef.Int16, "i32tou16.checked");
                    }
                case "time":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("time expects no arguments.", span);
                            return ConstI32(0);
                        }
                        return EmitTime(builder);
                    }
                case "init_window":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("init_window expects width, height, and title.", span);
                            return ConstI32(0);
                        }

                        var w = LowerExpression(builder, args[0], locals);
                        var h = LowerExpression(builder, args[1], locals);
                        var title = LowerExpression(builder, args[2], locals);

                        if (_headlessGraphics)
                            return ConstI32(1);

                        var (fn, fnType) = GetOrDeclareStasisInitWindow(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { w, h, title }, "init_window.call");
                    }
                case "begin_frame":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("begin_frame expects no arguments.", span);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisBeginFrame(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "");
                        return ConstI32(0);
                    }
                case "end_frame":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("end_frame expects no arguments.", span);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisEndFrame(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "");
                        return ConstI32(0);
                    }
                case "clear":
                    {
                        if (args.Count != 4)
                        {
                            AddDiagnostic("clear expects four components (r,g,b,a).", span);
                            return ConstI32(0);
                        }

                        var r = LowerExpression(builder, args[0], locals);
                        var g = LowerExpression(builder, args[1], locals);
                        var b = LowerExpression(builder, args[2], locals);
                        var a = LowerExpression(builder, args[3], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisClear(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, new[] { r, g, b, a }, "");
                        return ConstI32(0);
                    }
                case "draw_line":
                    {
                        if (args.Count != 8)
                        {
                            AddDiagnostic("draw_line expects eight arguments (x1,y1,x2,y2,r,g,b,a).", span);
                            return ConstI32(0);
                        }

                        var loweredArgs = args.Select(arg => LowerExpression(builder, arg, locals)).ToArray();

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisDrawLine(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, loweredArgs, "");
                        return ConstI32(0);
                    }
                case "draw_lines_f32":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("draw_lines_f32 expects (lines, count).", span);
                            return ConstI32(0);
                        }

                        var lines = LowerArrayPointer(builder, args[0], locals);
                        if (lines.Handle == IntPtr.Zero)
                            lines = LowerExpression(builder, args[0], locals);
                        var count = LowerExpression(builder, args[1], locals);

                        if (lines.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind)
                        {
                            AddDiagnostic("draw_lines_f32 expects an array/pointer for 'lines'.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisDrawLinesF32(_moduleBuilder);
                        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                        var cast = builder.BuildBitCast(lines, i8Ptr, "draw_lines_f32.ptr");
                        builder.BuildCall2(fnType, fn, new[] { cast, count }, "");
                        return ConstI32(0);
                    }
                case "host_get_frame":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("host_get_frame expects (out_i32: i32[], out_f32: f32[]).", span);
                            return ConstI32(0);
                        }

                        var outI32 = LowerArrayPointer(builder, args[0], locals);
                        if (outI32.Handle == IntPtr.Zero)
                            outI32 = LowerExpression(builder, args[0], locals);
                        var outF32 = LowerArrayPointer(builder, args[1], locals);
                        if (outF32.Handle == IntPtr.Zero)
                            outF32 = LowerExpression(builder, args[1], locals);

                        if (outI32.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind || outF32.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind)
                        {
                            AddDiagnostic("host_get_frame expects array/pointer arguments.", span);
                            return ConstI32(0);
                        }

                        var (fn, fnType) = GetOrDeclareStasisHostGetFrame(_moduleBuilder);
                        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                        var castI32 = builder.BuildBitCast(outI32, i8Ptr, "host_get_frame.i32");
                        var castF32 = builder.BuildBitCast(outF32, i8Ptr, "host_get_frame.f32");
                        builder.BuildCall2(fnType, fn, new[] { castI32, castF32 }, "");
                        return ConstI32(0);
                    }
                case "gfx_load_sprite":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("gfx_load_sprite expects (path, max_w, max_h).", span);
                            return ConstI32(0);
                        }

                        var loweredArgs = args.Select(arg => LowerExpression(builder, arg, locals)).ToArray();

                        if (_headlessGraphics)
                            return ConstI32(1);

                        var (fn, fnType) = GetOrDeclareStasisGfxLoadSprite(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, loweredArgs, "gfx_load_sprite.call");
                    }
                case "gfx_draw_sprite":
                    {
                        if (args.Count != 7)
                        {
                            AddDiagnostic("gfx_draw_sprite expects (handle,x,y,w,h,rot_degrees,a).", span);
                            return ConstI32(0);
                        }

                        var loweredArgs = args.Select(arg => LowerExpression(builder, arg, locals)).ToArray();

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxDrawSprite(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, loweredArgs, "");
                        return ConstI32(0);
                    }
                case "gfx_draw_sprites_i32":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("gfx_draw_sprites_i32 expects (cmds, count).", span);
                            return ConstI32(0);
                        }

                        var cmds = LowerArrayPointer(builder, args[0], locals);
                        if (cmds.Handle == IntPtr.Zero)
                            cmds = LowerExpression(builder, args[0], locals);
                        var count = LowerExpression(builder, args[1], locals);

                        if (cmds.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind)
                        {
                            AddDiagnostic("gfx_draw_sprites_i32 expects an array/pointer for 'cmds'.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxDrawSpritesI32(_moduleBuilder);
                        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                        var cast = builder.BuildBitCast(cmds, i8Ptr, "gfx_draw_sprites_i32.ptr");
                        builder.BuildCall2(fnType, fn, new[] { cast, count }, "");
                        return ConstI32(0);
                    }
                case "gfx_submit":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("gfx_submit expects (cmd_i32, cmd_f32).", span);
                            return ConstI32(0);
                        }

                        var cmdI32 = LowerArrayPointer(builder, args[0], locals);
                        if (cmdI32.Handle == IntPtr.Zero)
                            cmdI32 = LowerExpression(builder, args[0], locals);
                        var cmdF32 = LowerArrayPointer(builder, args[1], locals);
                        if (cmdF32.Handle == IntPtr.Zero)
                            cmdF32 = LowerExpression(builder, args[1], locals);

                        if (cmdI32.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind || cmdF32.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind)
                        {
                            AddDiagnostic("gfx_submit expects array/pointer arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxSubmit(_moduleBuilder);
                        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                        var castI32 = builder.BuildBitCast(cmdI32, i8Ptr, "gfx_submit.i32");
                        var castF32 = builder.BuildBitCast(cmdF32, i8Ptr, "gfx_submit.f32");
                        builder.BuildCall2(fnType, fn, new[] { castI32, castF32 }, "");
                        return ConstI32(0);
                    }
                case "gfx_submit_u8":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("gfx_submit_u8 expects (cmd_i32, cmd_f32, cmd_u8).", span);
                            return ConstI32(0);
                        }

                        var cmdI32 = LowerArrayPointer(builder, args[0], locals);
                        if (cmdI32.Handle == IntPtr.Zero)
                            cmdI32 = LowerExpression(builder, args[0], locals);
                        var cmdF32 = LowerArrayPointer(builder, args[1], locals);
                        if (cmdF32.Handle == IntPtr.Zero)
                            cmdF32 = LowerExpression(builder, args[1], locals);
                        var cmdU8 = LowerArrayPointer(builder, args[2], locals);
                        if (cmdU8.Handle == IntPtr.Zero)
                            cmdU8 = LowerExpression(builder, args[2], locals);

                        if (cmdI32.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind ||
                            cmdF32.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind ||
                            cmdU8.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind)
                        {
                            AddDiagnostic("gfx_submit_u8 expects array/pointer arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxSubmitU8(_moduleBuilder);
                        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                        var castI32 = builder.BuildBitCast(cmdI32, i8Ptr, "gfx_submit_u8.i32");
                        var castF32 = builder.BuildBitCast(cmdF32, i8Ptr, "gfx_submit_u8.f32");
                        var castU8 = builder.BuildBitCast(cmdU8, i8Ptr, "gfx_submit_u8.u8");
                        builder.BuildCall2(fnType, fn, new[] { castI32, castF32, castU8 }, "");
                        return ConstI32(0);
                    }
                case "gfx_poll_reload":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("gfx_poll_reload expects a sprite handle.", span);
                            return ConstI32(0);
                        }

                        var handle = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxPollReload(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { handle }, "gfx_poll_reload.call");
                    }
                case "gfx_window_width":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("gfx_window_width expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(800);

                        var (fn2, fnType2) = GetOrDeclareStasisGfxWindowWidth(_moduleBuilder);
                        return builder.BuildCall2(fnType2, fn2, Array.Empty<LLVMValueRef>(), "gfx_window_width.call");
                    }
                case "gfx_window_height":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("gfx_window_height expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(600);

                        var (fn2, fnType2) = GetOrDeclareStasisGfxWindowHeight(_moduleBuilder);
                        return builder.BuildCall2(fnType2, fn2, Array.Empty<LLVMValueRef>(), "gfx_window_height.call");
                    }
                case "gfx_window_resized":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("gfx_window_resized expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn2, fnType2) = GetOrDeclareStasisGfxWindowResized(_moduleBuilder);
                        return builder.BuildCall2(fnType2, fn2, Array.Empty<LLVMValueRef>(), "gfx_window_resized.call");
                    }
                case "gfx_debug_bake_hash":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("gfx_debug_bake_hash expects a path string.", span);
                            return ConstI32(0);
                        }

                        var path = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxDebugBakeHash(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { path }, "gfx_debug_bake_hash.call");
                    }
                case "gfx_debug_enable_hash":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("gfx_debug_enable_hash expects (enabled).", span);
                            return ConstI32(0);
                        }

                        var enabled = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxDebugEnableHash(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, new[] { enabled }, "");
                        return ConstI32(0);
                    }
                case "gfx_debug_get_frame_hash":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("gfx_debug_get_frame_hash expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisGfxDebugGetFrameHash(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "gfx_debug_get_frame_hash.call");
                    }
                case "set_postfx":
                    {
                        if (args.Count != 6)
                        {
                            AddDiagnostic("set_postfx expects strength, phase, speed, r, g, b.", span);
                            return ConstI32(0);
                        }

                        var loweredArgs = args.Select(arg => LowerExpression(builder, arg, locals)).ToArray();

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisSetPostfx(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, loweredArgs, "");
                        return ConstI32(0);
                    }
                case "is_key_down":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("is_key_down expects a key code.", span);
                            return ConstI32(0);
                        }

                        var key = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisIsKeyDown(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { key }, "is_key_down.call");
                    }
                case "get_time_ms":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("get_time_ms expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return EmitGetTimeMs(builder);

                        var (fn, fnType) = GetOrDeclareStasisGetTimeMs(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "get_time_ms.call");
                    }
                case "get_time_us":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("get_time_us expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return EmitGetTimeUs(builder);

                        var (fn, fnType) = GetOrDeclareStasisGetTimeUs(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "get_time_us.call");
                    }
                case "sleep_ms":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("sleep_ms expects the duration in milliseconds.", span);
                            return ConstI32(0);
                        }

                        var ms = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisSleepMs(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, new[] { ms }, "");
                        return ConstI32(0);
                    }
                case "audio_is_available":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("audio_is_available expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisAudioIsAvailable(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "audio_is_available.call");
                    }
                case "audio_get_sample_rate":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("audio_get_sample_rate expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisAudioGetSampleRate(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "audio_get_sample_rate.call");
                    }
                case "audio_get_channels":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("audio_get_channels expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisAudioGetChannels(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "audio_get_channels.call");
                    }
                case "audio_get_queued_frames":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("audio_get_queued_frames expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisAudioGetQueuedFrames(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "audio_get_queued_frames.call");
                    }
                case "audio_get_underruns":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("audio_get_underruns expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisAudioGetUnderruns(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "audio_get_underruns.call");
                    }
                case "audio_push_f32_interleaved":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("audio_push_f32_interleaved expects (samples, frame_count).", span);
                            return ConstI32(0);
                        }

                        var samples = LowerArrayPointer(builder, args[0], locals);
                        if (samples.Handle == IntPtr.Zero)
                            samples = LowerExpression(builder, args[0], locals);
                        var frames = LowerExpression(builder, args[1], locals);

                        if (samples.TypeOf.Kind != LLVMTypeKind.LLVMPointerTypeKind)
                        {
                            AddDiagnostic("audio_push_f32_interleaved expects an array/pointer for 'samples'.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                        var samplesPtr = builder.BuildBitCast(samples, i8Ptr, "audio.samples");

                        var (fn, fnType) = GetOrDeclareStasisAudioPushF32Interleaved(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { samplesPtr, frames }, "audio_push_f32_interleaved.call");
                    }
                case "should_quit":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("should_quit expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisShouldQuit(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "should_quit.call");
                    }
                case "input_pointer_count":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("input_pointer_count expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerCount(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "input_pointer_count.call");
                    }
                case "input_pointer_id":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_id expects (idx: i32).", span);
                            return ConstI32(0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerId(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_id.call");
                    }
                case "input_pointer_is_down":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_is_down expects (idx: i32).", span);
                            return ConstI32(0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerIsDown(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_is_down.call");
                    }
                case "input_pointer_went_down":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_went_down expects (idx: i32).", span);
                            return ConstI32(0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerWentDown(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_went_down.call");
                    }
                case "input_pointer_went_up":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_went_up expects (idx: i32).", span);
                            return ConstI32(0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerWentUp(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_went_up.call");
                    }
                case "input_pointer_x_px":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_x_px expects (idx: i32).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerXPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_x_px.call");
                    }
                case "input_pointer_y_px":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_y_px expects (idx: i32).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerYPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_y_px.call");
                    }
                case "input_pointer_dx_px":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_dx_px expects (idx: i32).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerDxPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_dx_px.call");
                    }
                case "input_pointer_dy_px":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_dy_px expects (idx: i32).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerDyPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_dy_px.call");
                    }
                case "input_pointer_x_n":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_x_n expects (idx: i32).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerXN(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_x_n.call");
                    }
                case "input_pointer_y_n":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("input_pointer_y_n expects (idx: i32).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        var idx = LowerExpression(builder, args[0], locals);

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var (fn, fnType) = GetOrDeclareStasisInputPointerYN(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { idx }, "input_pointer_y_n.call");
                    }
                case "input_dropped_pointers":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("input_dropped_pointers expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputDroppedPointers(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "input_dropped_pointers.call");
                    }
                case "input_viewport_x_px":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("input_viewport_x_px expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputViewportXPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "input_viewport_x_px.call");
                    }
                case "input_viewport_y_px":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("input_viewport_y_px expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputViewportYPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "input_viewport_y_px.call");
                    }
                case "input_viewport_w_px":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("input_viewport_w_px expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputViewportWPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "input_viewport_w_px.call");
                    }
                case "input_viewport_h_px":
                    {
                        if (args.Count != 0)
                        {
                            AddDiagnostic("input_viewport_h_px expects no arguments.", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var (fn, fnType) = GetOrDeclareStasisInputViewportHPx(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, Array.Empty<LLVMValueRef>(), "input_viewport_h_px.call");
                    }
                case "get_window_size":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("get_window_size expects two arguments (width variable, height variable).", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        // Allocate stack space for the out parameters
                        var widthPtr = builder.BuildAlloca(LLVMTypeRef.Int32, "width_ptr");
                        var heightPtr = builder.BuildAlloca(LLVMTypeRef.Int32, "height_ptr");

                        // Call the function
                        var (fn, fnType) = GetOrDeclareStasisGetWindowSize(_moduleBuilder);
                        builder.BuildCall2(fnType, fn, new[] { widthPtr, heightPtr }, "");

                        // Load the results and store them to the variables
                        var widthVal = builder.BuildLoad2(LLVMTypeRef.Int32, widthPtr, "width_val");
                        var heightVal = builder.BuildLoad2(LLVMTypeRef.Int32, heightPtr, "height_val");

                        // Store to the variables (args[0] and args[1] should be l-values)
                        if (args[0] is IdentifierExpressionSyntax widthIdent)
                        {
                            var widthName = widthIdent.Identifier.Text;
                            if (locals.ContainsKey(widthName))
                            {
                                var binding = locals[widthName];
                                locals[widthName] = binding with { Value = widthVal, IsAddress = false };
                            }
                        }

                        if (args[1] is IdentifierExpressionSyntax heightIdent)
                        {
                            var heightName = heightIdent.Identifier.Text;
                            if (locals.ContainsKey(heightName))
                            {
                                var binding = locals[heightName];
                                locals[heightName] = binding with { Value = heightVal, IsAddress = false };
                            }
                        }

                        return ConstI32(0);
                    }
                case "set_fullscreen":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("set_fullscreen expects one argument (fullscreen: 0 or 1).", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var fullscreen = LowerExpression(builder, args[0], locals);

                        var (fn, fnType) = GetOrDeclareStasisSetFullscreen(_moduleBuilder);
                        return builder.BuildCall2(fnType, fn, new[] { fullscreen }, "set_fullscreen.call");
                    }
                case "load_font":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("load_font expects (path: string, size: i32).", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(1);

                        var path = LowerCStringPointer(builder, args[0], locals);
                        var size = LowerExpression(builder, args[1], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_load_font");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[] { LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), LLVMTypeRef.Int32 }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_load_font", fnType);

                        return builder.BuildCall2(fnType, fn, new[] { path, size }, "load_font.call");
                    }
                case "draw_text":
                    {
                        if (args.Count != 8)
                        {
                            AddDiagnostic("draw_text expects (font_handle: i32, text: string, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32).", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var fontHandle = LowerExpression(builder, args[0], locals);
                        var text = LowerCStringPointer(builder, args[1], locals);
                        var x = LowerExpression(builder, args[2], locals);
                        var y = LowerExpression(builder, args[3], locals);
                        var r = LowerExpression(builder, args[4], locals);
                        var g = LowerExpression(builder, args[5], locals);
                        var b = LowerExpression(builder, args[6], locals);
                        var a = LowerExpression(builder, args[7], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_draw_text");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void,
                            new[] { LLVMTypeRef.Int32, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),
                                   LLVMTypeRef.Float, LLVMTypeRef.Float,
                                   LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float, LLVMTypeRef.Float }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_draw_text", fnType);

                        builder.BuildCall2(fnType, fn, new[] { fontHandle, text, x, y, r, g, b, a }, "");
                        return ConstI32(0);
                    }
                case "measure_text":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("measure_text expects (font_handle: i32, text: string).", span);
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);
                        }

                        if (_headlessGraphics)
                            return LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, 0.0);

                        var fontHandle = LowerExpression(builder, args[0], locals);
                        var text = LowerCStringPointer(builder, args[1], locals);

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_measure_text");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Float,
                            new[] { LLVMTypeRef.Int32, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0) }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_measure_text", fnType);

                            return builder.BuildCall2(fnType, fn, new[] { fontHandle, text }, "measure_text.call");
                    }
                case "dir_list_entry_is_dir":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("dir_list_entry_is_dir expects (dir_list: DirList, idx: i32).", span);
                            return ConstI32(0);
                        }

                        var idx = LowerExpression(builder, args[1], locals);
                        if (!TryLowerDirListArgument(builder, args[0], locals, out _, out var flagsPtr, out _))
                        {
                            AddDiagnostic("dir_list_entry_is_dir requires a DirList with entries, is_dir, and count fields.", span);
                            return ConstI32(0);
                        }

                        var elemPtr = builder.BuildGEP2(LLVMTypeRef.Int32, flagsPtr, new[] { idx }, "dir_list_entry_is_dir.ptr");
                        var flag = builder.BuildLoad2(LLVMTypeRef.Int32, elemPtr, "dir_list_entry_is_dir.load");
                        var isDir = builder.BuildICmp(LLVMIntPredicate.LLVMIntNE, flag, ConstI32(0), "dir_list_entry_is_dir.cmp");
                        return BuildBoolResult(builder, isDir);
                    }
                case "dir_list_entry_copy_name":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("dir_list_entry_copy_name expects (dir_list: DirList, idx: i32, dst: string).", span);
                            return ConstI32(0);
                        }

                        var idx = LowerExpression(builder, args[1], locals);
                        var dstPayload = LowerCStringPointer(builder, args[2], locals);
                        if (!TryLowerDirListArgument(builder, args[0], locals, out var namesPtr, out _, out _))
                        {
                            AddDiagnostic("dir_list_entry_copy_name requires a DirList with entries, is_dir, and count fields.", span);
                            return ConstI32(0);
                        }

                        var dstHeader = GetUtf8HeaderPtr(builder, dstPayload);
                        var dst = builder.BuildBitCast(dstHeader, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), "dir_list_entry_copy.dst");

                        var stride = ConstI32(DirEntryStride);
                        var offset = builder.BuildMul(idx, stride, "dir_list_entry_copy_name.offset");
                        var srcPtr = builder.BuildGEP2(LLVMTypeRef.Int8, namesPtr, new[] { offset }, "dir_list_entry_copy_name.ptr");

                        var function = builder.InsertBlock.Parent;
                        var condBlock = AppendBlock(function, "dir_list_entry_copy.cond");
                        var loopBlock = AppendBlock(function, "dir_list_entry_copy.loop");
                        var exitBlock = AppendBlock(function, "dir_list_entry_copy.exit");

                        var idxAlloca = builder.BuildAlloca(LLVMTypeRef.Int32, "dir_list_entry_copy.idx");
                        builder.BuildStore(ConstI32(0), idxAlloca);
                        builder.BuildBr(condBlock);

                        builder.PositionAtEnd(condBlock);
                        var currentIdx = builder.BuildLoad2(LLVMTypeRef.Int32, idxAlloca, "dir_list_entry_copy.idx.load");
                        var loopCond = builder.BuildICmp(LLVMIntPredicate.LLVMIntULT, currentIdx, ConstI32(DirEntryStride), "dir_list_entry_copy.cond");
                        builder.BuildCondBr(loopCond, loopBlock, exitBlock);

                        builder.PositionAtEnd(loopBlock);
                        var srcElem = builder.BuildGEP2(LLVMTypeRef.Int8, srcPtr, new[] { currentIdx }, "dir_list_entry_copy.src.elem");
                        var dstElem = builder.BuildGEP2(LLVMTypeRef.Int8, dst, new[] { currentIdx }, "dir_list_entry_copy.dst.elem");
                        var byteVal = builder.BuildLoad2(LLVMTypeRef.Int8, srcElem, "dir_list_entry_copy.load");
                        builder.BuildStore(byteVal, dstElem);
                        var nextIdx = builder.BuildAdd(currentIdx, ConstI32(1), "dir_list_entry_copy.next");
                        builder.BuildStore(nextIdx, idxAlloca);
                        builder.BuildBr(condBlock);

                        builder.PositionAtEnd(exitBlock);
                        return ConstI32(0);
                    }
                case "list_directory":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("list_directory expects (path: string, dir_list: DirList).", span);
                            return ConstI32(0);
                        }

                        if (_headlessGraphics)
                            return ConstI32(0);

                        var path = LowerCStringPointer(builder, args[0], locals);
                        if (path.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("list_directory requires a string path.", span);
                            return ConstI32(0);
                        }

                        if (!TryLowerDirListArgument(builder, args[1], locals, out var namesPtr, out var flagsPtr, out var countPtr))
                        {
                            AddDiagnostic("list_directory requires a DirList with entries, is_dir, and count fields.", span);
                            return ConstI32(0);
                        }

                        var fn = _moduleBuilder.Module.GetNamedFunction("stasis_list_directory_struct");
                        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32,
                            new[]
                            {
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),  // path
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0),  // names
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), // is_dir
                                LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0)  // out_count
                            }, false);
                        if (fn.Handle == IntPtr.Zero)
                            fn = _moduleBuilder.Module.AddFunction("stasis_list_directory_struct", fnType);

                        return builder.BuildCall2(fnType, fn, new[] { path, namesPtr, flagsPtr, countPtr }, "list_directory.call");
                    }

                // ============================================================
                // Standard Library: char_* module (character/byte utilities)
                // ============================================================

                case "char_is_digit":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_digit expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_digit", span);
                        // c >= '0' && c <= '9'
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('0'), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('9'), "le9");
                        var result = builder.BuildAnd(ge0, le9, "is_digit");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_digit.i32");
                    }

                case "char_is_alpha":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_alpha expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_alpha", span);
                        // (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('a'), "gea");
                        var leZ = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('z'), "lez");
                        var lower = builder.BuildAnd(geA, leZ, "lower");
                        var geAU = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('A'), "geA");
                        var leZU = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('Z'), "leZ");
                        var upper = builder.BuildAnd(geAU, leZU, "upper");
                        var result = builder.BuildOr(lower, upper, "is_alpha");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_alpha.i32");
                    }

                case "char_is_alnum":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_alnum expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_alnum", span);
                        // digit or alpha
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('0'), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('9'), "le9");
                        var digit = builder.BuildAnd(ge0, le9, "digit");
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('a'), "gea");
                        var leZ = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('z'), "lez");
                        var lower = builder.BuildAnd(geA, leZ, "lower");
                        var geAU = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('A'), "geA");
                        var leZU = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('Z'), "leZ");
                        var upper = builder.BuildAnd(geAU, leZU, "upper");
                        var alpha = builder.BuildOr(lower, upper, "alpha");
                        var result = builder.BuildOr(digit, alpha, "is_alnum");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_alnum.i32");
                    }

                case "char_is_space":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_space expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_space", span);
                        // c == ' ' || c == '\t' || c == '\n' || c == '\r'
                        var isSpace = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, c, ConstI32(' '), "isSpace");
                        var isTab = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, c, ConstI32('\t'), "isTab");
                        var isNewline = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, c, ConstI32('\n'), "isNewline");
                        var isCr = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, c, ConstI32('\r'), "isCr");
                        var r1 = builder.BuildOr(isSpace, isTab, "r1");
                        var r2 = builder.BuildOr(r1, isNewline, "r2");
                        var result = builder.BuildOr(r2, isCr, "is_space");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_space.i32");
                    }

                case "char_is_upper":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_upper expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_upper", span);
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('A'), "geA");
                        var leZ = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('Z'), "leZ");
                        var result = builder.BuildAnd(geA, leZ, "is_upper");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_upper.i32");
                    }

                case "char_is_lower":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_lower expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_lower", span);
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('a'), "gea");
                        var leZ = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('z'), "lez");
                        var result = builder.BuildAnd(geA, leZ, "is_lower");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_lower.i32");
                    }

                case "char_is_hex":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_hex expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_hex", span);
                        // digit or 'a'-'f' or 'A'-'F'
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('0'), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('9'), "le9");
                        var digit = builder.BuildAnd(ge0, le9, "digit");
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('a'), "gea");
                        var leF = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('f'), "lef");
                        var lowerHex = builder.BuildAnd(geA, leF, "lowerHex");
                        var geAU = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('A'), "geA");
                        var leFU = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('F'), "leF");
                        var upperHex = builder.BuildAnd(geAU, leFU, "upperHex");
                        var hex = builder.BuildOr(lowerHex, upperHex, "hex");
                        var result = builder.BuildOr(digit, hex, "is_hex");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_hex.i32");
                    }

                case "char_is_print":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_is_print expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_is_print", span);
                        // c >= 32 && c <= 126
                        var ge32 = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32(32), "ge32");
                        var le126 = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32(126), "le126");
                        var result = builder.BuildAnd(ge32, le126, "is_print");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "is_print.i32");
                    }

                case "char_to_upper":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_to_upper expects 1 argument (c: u8).", span);
                            return ConstI8(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_to_upper", span);
                        // if c >= 'a' && c <= 'z' then c - 32 else c
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('a'), "gea");
                        var leZ = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('z'), "lez");
                        var isLower = builder.BuildAnd(geA, leZ, "isLower");
                        var upper = builder.BuildSub(c, ConstI32(32), "upper");
                        var selected = builder.BuildSelect(isLower, upper, c, "char_to_upper");
                        return builder.BuildTrunc(selected, LLVMTypeRef.Int8, "char_to_upper.u8");
                    }

                case "char_to_lower":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_to_lower expects 1 argument (c: u8).", span);
                            return ConstI8(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_to_lower", span);
                        // if c >= 'A' && c <= 'Z' then c + 32 else c
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('A'), "geA");
                        var leZ = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('Z'), "leZ");
                        var isUpper = builder.BuildAnd(geA, leZ, "isUpper");
                        var lower = builder.BuildAdd(c, ConstI32(32), "lower");
                        var selected = builder.BuildSelect(isUpper, lower, c, "char_to_lower");
                        return builder.BuildTrunc(selected, LLVMTypeRef.Int8, "char_to_lower.u8");
                    }

                case "char_to_digit":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_to_digit expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_to_digit", span);
                        // if c >= '0' && c <= '9' then c - '0' else -1
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('0'), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('9'), "le9");
                        var isDigit = builder.BuildAnd(ge0, le9, "isDigit");
                        var digit = builder.BuildSub(c, ConstI32('0'), "digit");
                        return builder.BuildSelect(isDigit, digit, ConstI32(-1), "char_to_digit");
                    }

                case "char_from_digit":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_from_digit expects 1 argument (d: i32).", span);
                            return ConstI8(0);
                        }
                        var d = LowerExpression(builder, args[0], locals);
                        // if d >= 0 && d <= 9 then d + '0' else '?'
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGE, d, ConstI32(0), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLE, d, ConstI32(9), "le9");
                        var valid = builder.BuildAnd(ge0, le9, "valid");
                        var ch = builder.BuildAdd(d, ConstI32('0'), "ch");
                        var selected = builder.BuildSelect(valid, ch, ConstI32('?'), "char_from_digit");
                        return builder.BuildTrunc(selected, LLVMTypeRef.Int8, "char_from_digit.u8");
                    }

                case "char_to_hex":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_to_hex expects 1 argument (c: u8).", span);
                            return ConstI32(0);
                        }
                        var c = LowerU8ExpressionAsI32(builder, args[0], locals, "char_to_hex", span);
                        var function = builder.InsertBlock.Parent;

                        // Check digit first
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('0'), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('9'), "le9");
                        var isDigit = builder.BuildAnd(ge0, le9, "isDigit");

                        var digitBlock = AppendBlock(function, NextBlockName("hex.digit"));
                        var lowerBlock = AppendBlock(function, NextBlockName("hex.lower"));
                        var upperBlock = AppendBlock(function, NextBlockName("hex.upper"));
                        var invalidBlock = AppendBlock(function, NextBlockName("hex.invalid"));
                        var mergeBlock = AppendBlock(function, NextBlockName("hex.merge"));

                        builder.BuildCondBr(isDigit, digitBlock, lowerBlock);

                        builder.PositionAtEnd(digitBlock);
                        var digitVal = builder.BuildSub(c, ConstI32('0'), "digitVal");
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(lowerBlock);
                        var geA = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('a'), "gea");
                        var leF = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('f'), "lef");
                        var isLower = builder.BuildAnd(geA, leF, "isLower");
                        builder.BuildCondBr(isLower, AppendBlock(function, NextBlockName("hex.lowerVal")), upperBlock);

                        var lowerValBlock = function.LastBasicBlock;
                        builder.PositionAtEnd(lowerValBlock);
                        var lowerVal = builder.BuildSub(c, ConstI32('a' - 10), "lowerVal");
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(upperBlock);
                        var geAU = builder.BuildICmp(LLVMIntPredicate.LLVMIntUGE, c, ConstI32('A'), "geA");
                        var leFU = builder.BuildICmp(LLVMIntPredicate.LLVMIntULE, c, ConstI32('F'), "leF");
                        var isUpper = builder.BuildAnd(geAU, leFU, "isUpper");
                        builder.BuildCondBr(isUpper, AppendBlock(function, NextBlockName("hex.upperVal")), invalidBlock);

                        var upperValBlock = function.LastBasicBlock;
                        builder.PositionAtEnd(upperValBlock);
                        var upperVal = builder.BuildSub(c, ConstI32('A' - 10), "upperVal");
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(invalidBlock);
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(mergeBlock);
                        var phi = builder.BuildPhi(LLVMTypeRef.Int32, "hex.result");
                        phi.AddIncoming(new[] { digitVal }, new[] { digitBlock }, 1);
                        phi.AddIncoming(new[] { lowerVal }, new[] { lowerValBlock }, 1);
                        phi.AddIncoming(new[] { upperVal }, new[] { upperValBlock }, 1);
                        phi.AddIncoming(new[] { ConstI32(-1) }, new[] { invalidBlock }, 1);
                        return phi;
                    }

                case "char_from_hex":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("char_from_hex expects 1 argument (d: i32).", span);
                            return ConstI8(0);
                        }
                        var d = LowerExpression(builder, args[0], locals);
                        // if d >= 0 && d <= 9 then d + '0'
                        // else if d >= 10 && d <= 15 then d - 10 + 'a'
                        // else '?'
                        var ge0 = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGE, d, ConstI32(0), "ge0");
                        var le9 = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLE, d, ConstI32(9), "le9");
                        var isDigit = builder.BuildAnd(ge0, le9, "isDigit");
                        var ge10 = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGE, d, ConstI32(10), "ge10");
                        var le15 = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLE, d, ConstI32(15), "le15");
                        var isHexLetter = builder.BuildAnd(ge10, le15, "isHexLetter");
                        var digitCh = builder.BuildAdd(d, ConstI32('0'), "digitCh");
                        var hexCh = builder.BuildAdd(builder.BuildSub(d, ConstI32(10), "d10"), ConstI32('a'), "hexCh");
                        var temp = builder.BuildSelect(isHexLetter, hexCh, ConstI32('?'), "temp");
                        var selected = builder.BuildSelect(isDigit, digitCh, temp, "char_from_hex");
                        return builder.BuildTrunc(selected, LLVMTypeRef.Int8, "char_from_hex.u8");
                    }

                // ============================================================
                // Standard Library: str_* module (string operations)
                // ============================================================

                case "str_len":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("str_len expects 1 argument (s: u8[]).", span);
                            return ConstI32(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_len requires an array argument.", span);
                            return ConstI32(0);
                        }
                        return LoadUtf8ByteLength(builder, ptr);
                    }

                case "str_is_empty":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("str_is_empty expects 1 argument (s: u8[]).", span);
                            return ConstI32(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_is_empty requires an array argument.", span);
                            return ConstI32(0);
                        }
                        var len = LoadUtf8ByteLength(builder, ptr);
                        var isZero = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, len, ConstI32(0), "isZero");
                        return builder.BuildZExt(isZero, LLVMTypeRef.Int32, "str_is_empty");
                    }

                case "str_get":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_get expects 2 arguments (s: u8[], index: i32).", span);
                            return ConstI8(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        var index = LowerExpression(builder, args[1], locals);
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_get requires an array argument.", span);
                            return ConstI8(0);
                        }
                        var elemPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptr, new[] { index }, "elemPtr");
                        var val = builder.BuildLoad2(LLVMTypeRef.Int8, elemPtr, "str_get");
                        return val;
                    }

                case "str_set":
                    {
                        if (args.Count != 3)
                        {
                            AddDiagnostic("str_set expects 3 arguments (s: u8[], index: i32, byte: u8).", span);
                            return ConstI32(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        var index = LowerExpression(builder, args[1], locals);
                        LLVMValueRef byteVal;
                        if (TryLowerIntegerLiteralToType(args[2], LLVMTypeRef.Int8, out var lowered))
                        {
                            byteVal = lowered;
                        }
                        else
                        {
                            byteVal = LowerExpression(builder, args[2], locals);
                        }
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_set requires an array argument.", span);
                            return ConstI32(0);
                        }
                        if (byteVal.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || byteVal.TypeOf.IntWidth != 8)
                        {
                            AddDiagnostic("str_set expects a u8 value for 'byte'.", span);
                            return ConstI32(0);
                        }
                        var elemPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptr, new[] { index }, "elemPtr");
                        builder.BuildStore(byteVal, elemPtr);
                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var len64 = builder.BuildCall2(strlenType, strlenFn, new[] { ptr }, "strlen.call");
                        var len = builder.BuildTrunc(len64, LLVMTypeRef.Int32, "str_set.len");
                        StoreUtf8Lengths(builder, ptr, len);
                        return ConstI32(0);
                    }

                case "str_eq":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_eq expects 2 arguments (a: u8[], b: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strcmpFn, strcmpType) = GetOrDeclareStrcmp(_moduleBuilder);
                        var ptrA = LowerArrayPointer(builder, args[0], locals);
                        var ptrB = LowerArrayPointer(builder, args[1], locals);
                        if (ptrA.Handle == IntPtr.Zero || ptrB.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_eq requires array arguments.", span);
                            return ConstI32(0);
                        }
                        var cmp = builder.BuildCall2(strcmpType, strcmpFn, new[] { ptrA, ptrB }, "strcmp.call");
                        var isEqual = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cmp, ConstI32(0), "isEqual");
                        return builder.BuildZExt(isEqual, LLVMTypeRef.Int32, "str_eq");
                    }

                case "str_cmp":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_cmp expects 2 arguments (a: u8[], b: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strcmpFn, strcmpType) = GetOrDeclareStrcmp(_moduleBuilder);
                        var ptrA = LowerArrayPointer(builder, args[0], locals);
                        var ptrB = LowerArrayPointer(builder, args[1], locals);
                        if (ptrA.Handle == IntPtr.Zero || ptrB.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_cmp requires array arguments.", span);
                            return ConstI32(0);
                        }
                        return builder.BuildCall2(strcmpType, strcmpFn, new[] { ptrA, ptrB }, "str_cmp");
                    }

                case "str_copy":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_copy expects 2 arguments (dst: u8[], src: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strcpyFn, strcpyType) = GetOrDeclareStrcpy(_moduleBuilder);
                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var ptrDst = LowerArrayPointer(builder, args[0], locals);
                        var ptrSrc = LowerArrayPointer(builder, args[1], locals);
                        if (ptrDst.Handle == IntPtr.Zero || ptrSrc.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_copy requires array arguments.", span);
                            return ConstI32(0);
                        }
                        builder.BuildCall2(strcpyType, strcpyFn, new[] { ptrDst, ptrSrc }, "");
                        var len64 = builder.BuildCall2(strlenType, strlenFn, new[] { ptrDst }, "strlen.call");
                        var len = builder.BuildTrunc(len64, LLVMTypeRef.Int32, "str_copy.len");
                        StoreUtf8Lengths(builder, ptrDst, len);
                        return len;
                    }

                case "str_append":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_append expects 2 arguments (dst: u8[], src: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strcatFn, strcatType) = GetOrDeclareStrcat(_moduleBuilder);
                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var ptrDst = LowerArrayPointer(builder, args[0], locals);
                        var ptrSrc = LowerArrayPointer(builder, args[1], locals);
                        if (ptrDst.Handle == IntPtr.Zero || ptrSrc.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_append requires array arguments.", span);
                            return ConstI32(0);
                        }
                        builder.BuildCall2(strcatType, strcatFn, new[] { ptrDst, ptrSrc }, "");
                        var len64 = builder.BuildCall2(strlenType, strlenFn, new[] { ptrDst }, "strlen.call");
                        var len = builder.BuildTrunc(len64, LLVMTypeRef.Int32, "str_append.len");
                        StoreUtf8Lengths(builder, ptrDst, len);
                        return len;
                    }

                case "str_append_char":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_append_char expects 2 arguments (dst: u8[], byte: u8).", span);
                            return ConstI32(0);
                        }
                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var ptrDst = LowerArrayPointer(builder, args[0], locals);
                        LLVMValueRef byteVal;
                        if (TryLowerIntegerLiteralToType(args[1], LLVMTypeRef.Int8, out var lowered))
                        {
                            byteVal = lowered;
                        }
                        else
                        {
                            byteVal = LowerExpression(builder, args[1], locals);
                        }
                        if (ptrDst.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_append_char requires an array argument.", span);
                            return ConstI32(0);
                        }
                        if (byteVal.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || byteVal.TypeOf.IntWidth != 8)
                        {
                            AddDiagnostic("str_append_char expects a u8 value for 'byte'.", span);
                            return ConstI32(0);
                        }
                        var len64 = builder.BuildCall2(strlenType, strlenFn, new[] { ptrDst }, "strlen.call");
                        var len = builder.BuildTrunc(len64, LLVMTypeRef.Int32, "len");
                        var elemPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptrDst, new[] { len }, "elemPtr");
                        builder.BuildStore(byteVal, elemPtr);
                        var nextPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptrDst, new[] { builder.BuildAdd(len, ConstI32(1), "next") }, "nextPtr");
                        builder.BuildStore(ConstI8(0), nextPtr);
                        var newLen = builder.BuildAdd(len, ConstI32(1), "str_append_char.len");
                        StoreUtf8Lengths(builder, ptrDst, newLen);
                        return newLen;
                    }

                case "str_clear":
                    {
                        if (args.Count != 1)
                        {
                            AddDiagnostic("str_clear expects 1 argument (s: u8[]).", span);
                            return ConstI32(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_clear requires an array argument.", span);
                            return ConstI32(0);
                        }
                        builder.BuildStore(ConstI8(0), ptr);
                        StoreUtf8Lengths(builder, ptr, ConstI32(0));
                        return ConstI32(0);
                    }

                case "str_find_char":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_find_char expects 2 arguments (s: u8[], byte: u8).", span);
                            return ConstI32(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        LLVMValueRef byteVal;
                        if (TryLowerIntegerLiteralToType(args[1], LLVMTypeRef.Int8, out var lowered))
                        {
                            byteVal = lowered;
                        }
                        else
                        {
                            byteVal = LowerExpression(builder, args[1], locals);
                        }
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_find_char requires an array argument.", span);
                            return ConstI32(0);
                        }
                        if (byteVal.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || byteVal.TypeOf.IntWidth != 8)
                        {
                            AddDiagnostic("str_find_char expects a u8 value for 'byte'.", span);
                            return ConstI32(0);
                        }
                        var idxAlloca = builder.BuildAlloca(LLVMTypeRef.Int32, "find_char.idx");
                        var resultAlloca = builder.BuildAlloca(LLVMTypeRef.Int32, "find_char.result");
                        builder.BuildStore(ConstI32(0), idxAlloca);
                        builder.BuildStore(ConstI32(-1), resultAlloca);

                        var fn = builder.InsertBlock.Parent;
                        var loopBlock = AppendBlock(fn, NextBlockName("find_char.loop"));
                        var checkZeroBlock = AppendBlock(fn, NextBlockName("find_char.zero"));
                        var incBlock = AppendBlock(fn, NextBlockName("find_char.inc"));
                        var hitBlock = AppendBlock(fn, NextBlockName("find_char.hit"));
                        var missBlock = AppendBlock(fn, NextBlockName("find_char.miss"));
                        var exitBlock = AppendBlock(fn, NextBlockName("find_char.exit"));

                        builder.BuildBr(loopBlock);

                        builder.PositionAtEnd(loopBlock);
                        var idx = builder.BuildLoad2(LLVMTypeRef.Int32, idxAlloca, "find_char.idx.cur");
                        var elemPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptr, new[] { idx }, "find_char.ptr");
                        var cur = builder.BuildLoad2(LLVMTypeRef.Int8, elemPtr, "find_char.cur");
                        var isMatch = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cur, byteVal, "find_char.match");
                        var isZero = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cur, ConstI8(0), "find_char.zero");
                        builder.BuildCondBr(isMatch, hitBlock, checkZeroBlock);

                        builder.PositionAtEnd(checkZeroBlock);
                        builder.BuildCondBr(isZero, missBlock, incBlock);

                        builder.PositionAtEnd(incBlock);
                        var next = builder.BuildAdd(idx, ConstI32(1), "find_char.next");
                        builder.BuildStore(next, idxAlloca);
                        builder.BuildBr(loopBlock);

                        builder.PositionAtEnd(hitBlock);
                        builder.BuildStore(idx, resultAlloca);
                        builder.BuildBr(exitBlock);

                        builder.PositionAtEnd(missBlock);
                        builder.BuildStore(ConstI32(-1), resultAlloca);
                        builder.BuildBr(exitBlock);

                        builder.PositionAtEnd(exitBlock);
                        var result = builder.BuildLoad2(LLVMTypeRef.Int32, resultAlloca, "find_char.result.val");
                        return result;
                    }

                case "str_contains":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_contains expects 2 arguments (s: u8[], needle: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strstrFn, strstrType) = GetOrDeclareStrstr(_moduleBuilder);
                        var ptrS = LowerArrayPointer(builder, args[0], locals);
                        var ptrNeedle = LowerArrayPointer(builder, args[1], locals);
                        if (ptrS.Handle == IntPtr.Zero || ptrNeedle.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_contains requires array arguments.", span);
                            return ConstI32(0);
                        }
                        var found = builder.BuildCall2(strstrType, strstrFn, new[] { ptrS, ptrNeedle }, "strstr.call");
                        var isNotNull = builder.BuildICmp(LLVMIntPredicate.LLVMIntNE, found, LLVMValueRef.CreateConstPointerNull(LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0)), "isNotNull");
                        return builder.BuildZExt(isNotNull, LLVMTypeRef.Int32, "str_contains");
                    }

                case "str_find":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_find expects 2 arguments (s: u8[], needle: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strstrFn, strstrType) = GetOrDeclareStrstr(_moduleBuilder);
                        var ptrS = LowerArrayPointer(builder, args[0], locals);
                        var ptrNeedle = LowerArrayPointer(builder, args[1], locals);
                        if (ptrS.Handle == IntPtr.Zero || ptrNeedle.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_find requires array arguments.", span);
                            return ConstI32(0);
                        }
                        var found = builder.BuildCall2(strstrType, strstrFn, new[] { ptrS, ptrNeedle }, "strstr.call");
                        var isNull = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, found, LLVMValueRef.CreateConstPointerNull(LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0)), "isNull");
                        // Manual pointer diff: convert both pointers to int64, subtract, then truncate to i32
                        var foundInt = builder.BuildPtrToInt(found, LLVMTypeRef.Int64, "found.int");
                        var ptrSInt = builder.BuildPtrToInt(ptrS, LLVMTypeRef.Int64, "ptrS.int");
                        var diff = builder.BuildSub(foundInt, ptrSInt, "diff");
                        var idx = builder.BuildTrunc(diff, LLVMTypeRef.Int32, "idx");
                        return builder.BuildSelect(isNull, ConstI32(-1), idx, "str_find");
                    }

                case "str_starts_with":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_starts_with expects 2 arguments (s: u8[], prefix: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strncmpFn, strncmpType) = GetOrDeclareStrncmp(_moduleBuilder);
                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var ptrS = LowerArrayPointer(builder, args[0], locals);
                        var ptrPrefix = LowerArrayPointer(builder, args[1], locals);
                        if (ptrS.Handle == IntPtr.Zero || ptrPrefix.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_starts_with requires array arguments.", span);
                            return ConstI32(0);
                        }
                        var prefixLen = builder.BuildCall2(strlenType, strlenFn, new[] { ptrPrefix }, "prefixLen");
                        var cmp = builder.BuildCall2(strncmpType, strncmpFn, new[] { ptrS, ptrPrefix, prefixLen }, "strncmp.call");
                        var isZero = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cmp, ConstI32(0), "isZero");
                        return builder.BuildZExt(isZero, LLVMTypeRef.Int32, "str_starts_with");
                    }

                case "str_ends_with":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_ends_with expects 2 arguments (s: u8[], suffix: u8[]).", span);
                            return ConstI32(0);
                        }
                        var (strncmpFn, strncmpType) = GetOrDeclareStrncmp(_moduleBuilder);
                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var ptrS = LowerArrayPointer(builder, args[0], locals);
                        var ptrSuffix = LowerArrayPointer(builder, args[1], locals);
                        if (ptrS.Handle == IntPtr.Zero || ptrSuffix.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_ends_with requires array arguments.", span);
                            return ConstI32(0);
                        }
                        var sLen = builder.BuildCall2(strlenType, strlenFn, new[] { ptrS }, "sLen");
                        var suffixLen = builder.BuildCall2(strlenType, strlenFn, new[] { ptrSuffix }, "suffixLen");
                        var offset = builder.BuildSub(sLen, suffixLen, "offset");
                        var offset32 = builder.BuildTrunc(offset, LLVMTypeRef.Int32, "offset32");
                        var endPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptrS, new[] { offset32 }, "endPtr");
                        var cmp = builder.BuildCall2(strncmpType, strncmpFn, new[] { endPtr, ptrSuffix, suffixLen }, "strncmp.call");
                        // Also check that sLen >= suffixLen
                        var lenOk = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGE, sLen, suffixLen, "lenOk");
                        var isZero = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cmp, ConstI32(0), "isZero");
                        var result = builder.BuildAnd(lenOk, isZero, "ends_with");
                        return builder.BuildZExt(result, LLVMTypeRef.Int32, "str_ends_with");
                    }

                case "str_find_last_char":
                    {
                        if (args.Count != 2)
                        {
                            AddDiagnostic("str_find_last_char expects 2 arguments (s: u8[], byte: u8).", span);
                            return ConstI32(0);
                        }
                        var ptr = LowerArrayPointer(builder, args[0], locals);
                        LLVMValueRef byteVal;
                        if (TryLowerIntegerLiteralToType(args[1], LLVMTypeRef.Int8, out var lowered))
                        {
                            byteVal = lowered;
                        }
                        else
                        {
                            byteVal = LowerExpression(builder, args[1], locals);
                        }
                        if (ptr.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_find_last_char requires an array argument.", span);
                            return ConstI32(0);
                        }
                        if (byteVal.TypeOf.Kind != LLVMTypeKind.LLVMIntegerTypeKind || byteVal.TypeOf.IntWidth != 8)
                        {
                            AddDiagnostic("str_find_last_char expects a u8 value for 'byte'.", span);
                            return ConstI32(0);
                        }
                        var idxAlloca = builder.BuildAlloca(LLVMTypeRef.Int32, "find_last.idx");
                        var lastAlloca = builder.BuildAlloca(LLVMTypeRef.Int32, "find_last.last");
                        builder.BuildStore(ConstI32(0), idxAlloca);
                        builder.BuildStore(ConstI32(-1), lastAlloca);

                        var fn = builder.InsertBlock.Parent;
                        var loopBlock = AppendBlock(fn, NextBlockName("find_last.loop"));
                        var afterMatchBlock = AppendBlock(fn, NextBlockName("find_last.after"));
                        var endBlock = AppendBlock(fn, NextBlockName("find_last.end"));

                        builder.BuildBr(loopBlock);

                        builder.PositionAtEnd(loopBlock);
                        var idx = builder.BuildLoad2(LLVMTypeRef.Int32, idxAlloca, "find_last.idx.cur");
                        var elemPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptr, new[] { idx }, "find_last.ptr");
                        var cur = builder.BuildLoad2(LLVMTypeRef.Int8, elemPtr, "find_last.cur");
                        var isZero = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cur, ConstI8(0), "find_last.zero");
                        var isMatch = builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, cur, byteVal, "find_last.match");
                        builder.BuildCondBr(isZero, endBlock, afterMatchBlock);

                        builder.PositionAtEnd(afterMatchBlock);
                        var lastCur = builder.BuildLoad2(LLVMTypeRef.Int32, lastAlloca, "find_last.last.cur");
                        var newLast = builder.BuildSelect(isMatch, idx, lastCur, "find_last.last.next");
                        builder.BuildStore(newLast, lastAlloca);
                        var nextIdx = builder.BuildAdd(idx, ConstI32(1), "find_last.next");
                        builder.BuildStore(nextIdx, idxAlloca);
                        builder.BuildBr(loopBlock);

                        builder.PositionAtEnd(endBlock);
                        return builder.BuildLoad2(LLVMTypeRef.Int32, lastAlloca, "find_last.result");
                    }

                case "str_substr":
                    {
                        if (args.Count != 4)
                        {
                            AddDiagnostic("str_substr expects 4 arguments (dst: u8[], src: u8[], start: i32, byte_len: i32).", span);
                            return ConstI32(0);
                        }

                        var ptrDst = LowerArrayPointer(builder, args[0], locals);
                        var ptrSrc = LowerArrayPointer(builder, args[1], locals);
                        var start = LowerExpression(builder, args[2], locals);
                        var byteLen = LowerExpression(builder, args[3], locals);
                        if (ptrDst.Handle == IntPtr.Zero || ptrSrc.Handle == IntPtr.Zero)
                        {
                            AddDiagnostic("str_substr requires array arguments.", span);
                            return ConstI32(0);
                        }

                        var (strlenFn, strlenType) = GetOrDeclareStrlen(_moduleBuilder);
                        var (memcpyFn, memcpyType) = GetOrDeclareMemcpy(_moduleBuilder);
                        var (abortFn, abortType) = GetOrDeclareAbort(_moduleBuilder);

                        var srcLen64 = builder.BuildCall2(strlenType, strlenFn, new[] { ptrSrc }, "substr.srclen64");
                        var srcLen = builder.BuildTrunc(srcLen64, LLVMTypeRef.Int32, "substr.srclen");
                        var end = builder.BuildAdd(start, byteLen, "substr.end");

                        // Validate start/end ranges: start >= 0, byteLen >= 0, end <= srcLen
                        var startNeg = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, start, ConstI32(0), "substr.startNeg");
                        var lenNeg = builder.BuildICmp(LLVMIntPredicate.LLVMIntSLT, byteLen, ConstI32(0), "substr.lenNeg");
                        var endGt = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGT, end, srcLen, "substr.endGt");
                        var badRange = builder.BuildOr(builder.BuildOr(startNeg, lenNeg, "substr.badRange1"), endGt, "substr.badRange");

                        var fn = builder.InsertBlock.Parent;
                        var rangeOkBlock = AppendBlock(fn, NextBlockName("substr.range_ok"));
                        var abortBlock = AppendBlock(fn, NextBlockName("substr.abort"));
                        builder.BuildCondBr(badRange, abortBlock, rangeOkBlock);

                        builder.PositionAtEnd(abortBlock);
                        builder.BuildCall2(abortType, abortFn, Array.Empty<LLVMValueRef>(), "");
                        builder.BuildUnreachable();

                        builder.PositionAtEnd(rangeOkBlock);
                        // Boundary validation: bytes at start and end must not be UTF-8 continuation bytes
                        var startPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptrSrc, new[] { start }, "substr.startPtr");
                        var startByte = builder.BuildLoad2(LLVMTypeRef.Int8, startPtr, "substr.startByte");
                        var startCont = BuildIsContinuationByte(builder, startByte);

                        var endPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptrSrc, new[] { end }, "substr.endPtr");
                        var endByte = builder.BuildLoad2(LLVMTypeRef.Int8, endPtr, "substr.endByte");
                        var endCont = BuildIsContinuationByte(builder, endByte);

                        var misaligned = builder.BuildOr(startCont, endCont, "substr.misaligned");

                        var okBlock = AppendBlock(fn, NextBlockName("substr.ok"));
                        builder.BuildCondBr(misaligned, abortBlock, okBlock);

                        builder.PositionAtEnd(okBlock);
                        var len64 = builder.BuildZExt(byteLen, LLVMTypeRef.Int64, "substr.len64");
                        builder.BuildCall2(memcpyType, memcpyFn, new[] { ptrDst, startPtr, len64 }, "substr.copy");
                        var terminatorPtr = builder.BuildGEP2(LLVMTypeRef.Int8, ptrDst, new[] { byteLen }, "substr.term");
                        builder.BuildStore(ConstI8(0), terminatorPtr);
                        StoreUtf8Lengths(builder, ptrDst, byteLen);
                        return byteLen;
                    }

                case "str_trim_start":
                case "str_trim_end":
                case "str_trim":
                case "str_to_upper":
                case "str_to_lower":
                case "str_from_i32":
                case "str_from_f32":
                case "str_to_i32":
                case "str_to_f32":
                    AddDiagnostic($"Built-in '{name}' is not yet implemented.", span);
                    return ConstI32(0);

                default:
                    AddDiagnostic($"Unknown built-in '{name}'.", span);
                    return ConstI32(0);
            }
        }

        private LLVMValueRef LowerArrayPointer(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals)
        {
            // Get a pointer to the first element of an array
            if (expr is IdentifierExpressionSyntax id)
            {
                // Check if it's a global array
                if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Kind == SymbolKind.Global && sym.Type is ArrayTypeSymbol arrayTypeSym)
                {
                    var global = _moduleBuilder.Module.GetNamedGlobal(id.Identifier.Text);
                    if (global.Handle != IntPtr.Zero)
                    {
                        if (arrayTypeSym.ElementType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                        {
                            var headerSize = HeaderSizeFor(prim.PrimitiveName);
                            var payloadSize = arrayTypeSym.Size + headerSize;
                            var llvmPayloadArrayType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)Math.Max(1, payloadSize));
                            return builder.BuildGEP2(llvmPayloadArrayType, global, new[] { ConstI32(0), ConstI32(headerSize) }, $"{id.Identifier.Text}.payload");
                        }

                        // Get element type from the semantic type symbol
                        var elemType = _moduleBuilder.TypeMapper.Map(arrayTypeSym.ElementType);
                        var arraySize = arrayTypeSym.Size;
                        var llvmElemArrayType = LLVMTypeRef.CreateArray(elemType, (uint)Math.Max(1, arraySize));
                        // GEP to get pointer to first element
                        return builder.BuildGEP2(llvmElemArrayType, global, new[] { ConstI32(0), ConstI32(0) }, $"{id.Identifier.Text}.ptr");
                    }
                }
                // Check for local array binding
                if (locals.TryGetValue(id.Identifier.Text, out var local) && local.IsAddress)
                {
                    // If this is an array descriptor (fat pointer), extract the pointer field
                    if (local.IsArrayDescriptor)
                    {
                        var descriptorPtr = local.Value;  // Pointer to { ptr, i32 } on stack
                        var ptrFieldPtr = builder.BuildStructGEP2(local.Type, descriptorPtr, 0, $"{id.Identifier.Text}.ptr_field");
                        var arrayPtr = builder.BuildLoad2(LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), ptrFieldPtr, $"{id.Identifier.Text}.ptr");
                        return arrayPtr;
                    }
                    return local.Value;
                }
            }
            else if (expr is MemberAccessExpressionSyntax member)
            {
                // Handle struct field arrays like state.buffer
                var flattenedPath = BuildFlattenedMemberPath(member);
                if (flattenedPath is not null)
                {
                    // Try to resolve the type from member access
                    if (TryResolveMemberType(member, out var memberType) && memberType is ArrayTypeSymbol memberArrayType)
                    {
                        var global = _moduleBuilder.Module.GetNamedGlobal(flattenedPath);
                        if (global.Handle != IntPtr.Zero)
                        {
                            if (memberArrayType.ElementType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                            {
                                var headerSize = HeaderSizeFor(prim.PrimitiveName);
                                var payloadSize = memberArrayType.Size + headerSize;
                                var llvmPayloadArrayType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)Math.Max(1, payloadSize));
                                return builder.BuildGEP2(llvmPayloadArrayType, global, new[] { ConstI32(0), ConstI32(headerSize) }, $"{flattenedPath}.payload");
                            }

                            var elemType = _moduleBuilder.TypeMapper.Map(memberArrayType.ElementType);
                            var arraySize = memberArrayType.Size;
                            var llvmElemArrayType = LLVMTypeRef.CreateArray(elemType, (uint)Math.Max(1, arraySize));
                            return builder.BuildGEP2(llvmElemArrayType, global, new[] { ConstI32(0), ConstI32(0) }, $"{flattenedPath}.ptr");
                        }
                    }
                }
            }
            return default;
        }

        private LLVMValueRef LowerCStringPointer(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals)
        {
            var arrayPtr = LowerArrayPointer(builder, expr, locals);
            if (arrayPtr.Handle != IntPtr.Zero)
            {
                return arrayPtr;
            }

            var value = LowerExpression(builder, expr, locals);
            return value.TypeOf.Kind == LLVMTypeKind.LLVMPointerTypeKind
                ? value
                : LLVMValueRef.CreateConstNull(LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0));
        }

        private static LLVMValueRef ConstI8(int value) =>
            LLVMValueRef.CreateConstInt(LLVMTypeRef.Int8, (ulong)value, false);

        private static LLVMValueRef ConstI16(int value) =>
            LLVMValueRef.CreateConstInt(LLVMTypeRef.Int16, (ulong)value, false);

        private LLVMValueRef BuildIsContinuationByte(LLVMBuilderRef builder, LLVMValueRef byteVal)
        {
            var byteI32 = builder.BuildZExt(byteVal, LLVMTypeRef.Int32, "byte.i32");
            var masked = builder.BuildAnd(byteI32, ConstI32(0xC0), "byte.mask");
            return builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, masked, ConstI32(0x80), "byte.is_cont");
        }

        private LLVMValueRef LowerOperatorCall(LLVMBuilderRef builder, OperatorCallExpressionSyntax op, Dictionary<string, LocalBinding> locals)
        {
            var opText = op.OperatorToken.Text;
            if (op.Arguments.Count != 1)
            {
                AddDiagnostic($"Operator '.{opText}()' requires exactly one argument.", op.Span);
                return ConstI32(0);
            }

            var rhs = LowerExpression(builder, op.Arguments[0], locals);
            if (opText == "=")
            {
                AddDiagnostic("Use infix '=' for assignment.", op.Span);
                if (TryGetPointer(builder, op.Receiver, locals, out var ptr, out _))
                {
                    builder.BuildStore(rhs, ptr);
                    return rhs;
                }

                AddDiagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", op.Receiver.Span);
                return rhs;
            }

            var lhs = LowerExpression(builder, op.Receiver, locals);
            return LowerBinary(builder, opText, lhs, rhs, op.Span);
        }

        private LLVMValueRef LowerBinary(LLVMBuilderRef builder, string op, LLVMValueRef lhs, LLVMValueRef rhs, SourceSpan span)
        {
            var lhsType = lhs.TypeOf;
            var rhsType = rhs.TypeOf;

            var lhsIsFloat = lhsType.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;
            var rhsIsFloat = rhsType.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;
            if (lhsIsFloat || rhsIsFloat)
            {
                if (!(lhsIsFloat && rhsIsFloat) || lhsType.Kind != rhsType.Kind)
                {
                    AddDiagnostic("Binary operands must have the same float type; use an explicit conversion.", span);
                    return lhs;
                }
            }
            else if (lhsType.Kind == LLVMTypeKind.LLVMIntegerTypeKind && rhsType.Kind == LLVMTypeKind.LLVMIntegerTypeKind)
            {
                if (lhsType.IntWidth != rhsType.IntWidth)
                {
                    AddDiagnostic("Binary operands must have the same integer type; use an explicit conversion.", span);
                    return lhs;
                }
            }

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
                "<=" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOLE, lhs, rhs, "fle")),
                "<=" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntSLE, lhs, rhs, "ile")),
                ">" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOGT, lhs, rhs, "fgt")),
                ">" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntSGT, lhs, rhs, "igt")),
                ">=" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOGE, lhs, rhs, "fge")),
                ">=" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntSGE, lhs, rhs, "ige")),
                "==" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealOEQ, lhs, rhs, "feq")),
                "==" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntEQ, lhs, rhs, "ieq")),
                "!=" when isFloat => BuildBoolResult(builder, builder.BuildFCmp(LLVMRealPredicate.LLVMRealONE, lhs, rhs, "fne")),
                "!=" => BuildBoolResult(builder, builder.BuildICmp(LLVMIntPredicate.LLVMIntNE, lhs, rhs, "ine")),
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
            var loopLocals = new Dictionary<string, LocalBinding>(locals, StringComparer.Ordinal);
            if (!TryResolveIterableInfo(builder, foreachStmt.Iterable, loopLocals, out var iterable))
            {
                AddDiagnostic("foreach target must be an array.", foreachStmt.Iterable.Span);
                return false;
            }

            var iterator = builder.BuildAlloca(i32, $"{foreachStmt.Iterator.Text}.idx");
            if (foreachStmt.BindByElement && iterable.ElementBinding is not null)
            {
                var boundElement = iterable.ElementBinding.Value with { IndexAlloca = iterator };
                loopLocals[foreachStmt.Iterator.Text] = new LocalBinding(iterator, i32, true, boundElement);
            }
            else
            {
                loopLocals[foreachStmt.Iterator.Text] = new LocalBinding(iterator, i32, true);
            }

            // If an index variable is provided, expose the iterator as a separate local
            if (foreachStmt.IndexVariable is not null)
            {
                loopLocals[foreachStmt.IndexVariable.Text] = new LocalBinding(iterator, i32, true);
            }

            builder.BuildStore(ConstI32(0), iterator);

            if (iterable.LengthValue is null && iterable.ConstLength <= 0)
            {
                AddDiagnostic("foreach array requires a known length.", foreachStmt.Iterable.Span);
                return false;
            }

            var lengthValue = iterable.LengthValue ?? LLVMValueRef.CreateConstInt(i32, (ulong)iterable.ConstLength, true);

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
            if (arr.Receiver is IdentifierExpressionSyntax id &&
                _symbols.TryGetValue(id.Identifier.Text, out var sym) &&
                (sym.Kind == SymbolKind.Global || sym.Kind == SymbolKind.Const) &&
                sym.Type is ArrayTypeSymbol arrayType &&
                arrayType.ElementType is ArrayTypeSymbol innerArray &&
                innerArray.ElementType is PrimitiveTypeSymbol prim &&
                HeaderSizeFor(prim.PrimitiveName) > 0)
            {
                var globalName = TryResolveGlobalName(id.Identifier.Text);
                var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
                if (global.Handle != IntPtr.Zero)
                {
                    var index = LowerExpression(builder, arr.Index, locals);
                    var headerSize = HeaderSizeFor(prim.PrimitiveName);
                    var stride = innerArray.Size + headerSize;
                    var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                    var basePtr = builder.BuildBitCast(global, i8Ptr, "strbase");
                    var offset = builder.BuildMul(index, ConstI32(stride), "str.offset");
                    var elemBase = builder.BuildGEP2(LLVMTypeRef.Int8, basePtr, new[] { offset }, "str.elem.base");
                    var payloadPtr = builder.BuildGEP2(LLVMTypeRef.Int8, elemBase, new[] { ConstI32(headerSize) }, "str.elem.payload");
                    return payloadPtr;
                }
            }

            if (TryLowerArrayElementPointer(builder, arr, fieldName: null, locals, out var ptr, out var elemType))
            {
                var loaded = builder.BuildLoad2(elemType, ptr, "elemload");
                return loaded;
            }

            AddDiagnostic("Unable to lower array access.", arr.Span);
            return ConstI32(0);
        }

        private LLVMValueRef LowerMemberAccess(LLVMBuilderRef builder, MemberAccessExpressionSyntax member, Dictionary<string, LocalBinding> locals)
        {
            if (member.Receiver is IdentifierExpressionSyntax elemId &&
                locals.TryGetValue(elemId.Identifier.Text, out var elemBinding) &&
                elemBinding.Element is not null)
            {
                    if (TryBuildElementPointer(builder, elemBinding, member.Member.Text, member.Span, out var ptr, out var elemType))
                    {
                        var loaded = builder.BuildLoad2(elemType, ptr, "fieldload");
                        return loaded;
                    }

                return ConstI32(0);
            }

            // Handle array[i].field syntax
            if (member.Receiver is ArrayAccessExpressionSyntax arr)
            {
                if (TryLowerArrayElementPointer(builder, arr, member.Member.Text, locals, out var ptr, out var elemType))
                {
                    var loaded = builder.BuildLoad2(elemType, ptr, "fieldload");
                    return loaded;
                }
            }

            // Handle state.field syntax (global struct instance)
            if (member.Receiver is IdentifierExpressionSyntax id &&
                _symbols.TryGetValue(id.Identifier.Text, out var sym) &&
                (sym.Kind == SymbolKind.Global || sym.Kind == SymbolKind.Const) &&
                sym.Type is NamedTypeSymbol namedType)
            {
                // Check if this is a struct type
                if (_structs.TryGetValue(namedType.TypeName, out var structDecl))
                {
                    // Load from flattened global: state.ship_x → state_ship_x
                    var flattenedName = $"{id.Identifier.Text}_{member.Member.Text}";
                    var global = _moduleBuilder.Module.GetNamedGlobal(flattenedName);
                    if (global.Handle != IntPtr.Zero)
                    {
                        // Determine the type by looking up the field in the struct
                        var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
                        if (field is not null)
                        {
                            var fieldType = ResolveType(field.Type, _symbols);

                            // For array fields, return a pointer to the array instead of loading it
                            if (fieldType is ArrayTypeSymbol arrayType)
                            {
                                // Special handling for string arrays
                                if (arrayType.ElementType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                                {
                                    // String array: [N+8 x i8] - return pointer to first element
                                    var headerSize = HeaderSizeFor(prim.PrimitiveName);
                                    var llvmArrayType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)(arrayType.Size + headerSize));
                                    return builder.BuildGEP2(llvmArrayType, global, new[] { ConstI32(0), ConstI32(headerSize) }, $"{flattenedName}.payload");
                                }
                                else
                                {
                                    // Other arrays - return pointer to first element
                                    var elemType = _moduleBuilder.TypeMapper.Map(arrayType.ElementType);
                                    var llvmArrayType = LLVMTypeRef.CreateArray(elemType, (uint)Math.Max(1, arrayType.Size));
                                    return builder.BuildGEP2(llvmArrayType, global, new[] { ConstI32(0), ConstI32(0) }, $"{flattenedName}.ptr");
                                }
                            }

                            var llvmType = _moduleBuilder.TypeMapper.Map(fieldType);
                            var loaded = builder.BuildLoad2(llvmType, global, flattenedName);
                            return loaded;
                        }
                    }
                }
            }

            // Handle nested member access like state.ship.x (read)
            if (member.Receiver is MemberAccessExpressionSyntax)
            {
                var flattenedPath = BuildFlattenedMemberPath(member);
                if (flattenedPath is not null)
                {
                    var global = _moduleBuilder.Module.GetNamedGlobal(flattenedPath);
                    if (global.Handle != IntPtr.Zero && TryResolveMemberType(member, out var resolvedType))
                    {
                        var llvmType = _moduleBuilder.TypeMapper.Map(resolvedType);
                        var loaded = builder.BuildLoad2(llvmType, global, flattenedPath);
                        return loaded;
                    }
                }
            }

            AddDiagnostic("Unable to lower member access.", member.Span);
            return ConstI32(0);
        }

        private LLVMValueRef LowerArrayAccess(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, string? fieldName, Dictionary<string, LocalBinding> locals)
        {
            if (TryLowerArrayElementPointer(builder, arr, fieldName, locals, out var ptr, out var elemType))
            {
                var loaded = builder.BuildLoad2(elemType, ptr, "elemload");
                return loaded;
            }

            AddDiagnostic("Unable to lower array access.", arr.Span);
            return ConstI32(0);
        }

        private bool TryLowerArrayElementPointer(LLVMBuilderRef builder, ArrayAccessExpressionSyntax arr, string? fieldName, Dictionary<string, LocalBinding> locals, out LLVMValueRef ptr, out LLVMTypeRef elemType)
        {
            ptr = default;
            elemType = default;

            // Handle state.field[i] pattern (nested member access)
            if (arr.Receiver is MemberAccessExpressionSyntax memberAccess &&
                memberAccess.Receiver is IdentifierExpressionSyntax structId &&
                _symbols.TryGetValue(structId.Identifier.Text, out var structSym) &&
                (structSym.Kind == SymbolKind.Global || structSym.Kind == SymbolKind.Const) &&
                structSym.Type is NamedTypeSymbol structType &&
                _structs.TryGetValue(structType.TypeName, out var parentStructDecl))
            {
                // Find the field in the struct that represents the array
                var arrayField = parentStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == memberAccess.Member.Text);
                if (arrayField?.Type is ArrayTypeSyntax arrayTypeSyntax)
                {
                    var index = LowerExpression(builder, arr.Index, locals);

                    // Struct array with nested fields (state.units[i].x)
                    if (arrayTypeSyntax.ElementType is NamedTypeSyntax arrayElemType &&
                        _structs.TryGetValue(arrayElemType.Name, out var elemStructDecl))
                    {
                        if (fieldName is not null)
                        {
                            // state.asteroids[i].x → state_asteroids_x[i]
                            var flattenedName = $"{structId.Identifier.Text}_{memberAccess.Member.Text}_{fieldName}";
                            var global = _moduleBuilder.Module.GetNamedGlobal(flattenedName);
                            if (global.Handle != IntPtr.Zero)
                            {
                                var field = elemStructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == fieldName);
                                if (field is not null)
                                {
                                    var fieldType = ResolveType(field.Type, _symbols);
                                    elemType = _moduleBuilder.TypeMapper.Map(fieldType);
                                    var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                                    var casted = builder.BuildBitCast(global, elemPtrType, "fieldbase");
                                    ptr = builder.BuildGEP2(elemType, casted, new[] { index }, "fieldaddr");
                                    return true;
                                }
                            }
                        }
                    }
                    else if (fieldName is null)
                    {
                        // Primitive array stored on a struct field (state.lane_lookup[i])
                        var elemTypeSymbol = ResolveType(arrayTypeSyntax.ElementType, _symbols);
                        elemType = _moduleBuilder.TypeMapper.Map(elemTypeSymbol);
                        var baseName = TryResolveGlobalName($"{structId.Identifier.Text}_{memberAccess.Member.Text}");
                        var global = _moduleBuilder.Module.GetNamedGlobal(baseName);
                        if (global.Handle != IntPtr.Zero)
                        {
                            if (elemTypeSymbol is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                            {
                                var headerSize = HeaderSizeFor(prim.PrimitiveName);
                                var count = (int)ParseArrayLength(arrayTypeSyntax.SizeToken?.Text ?? string.Empty);
                                var backingBytes = Math.Max(1, count + headerSize);
                                var backingArrayType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)backingBytes);
                                var payloadPtr = builder.BuildGEP2(backingArrayType, global, new[] { ConstI32(0), ConstI32(headerSize) }, "payload");
                                ptr = builder.BuildGEP2(LLVMTypeRef.Int8, payloadPtr, new[] { index }, "elemaddr");
                                elemType = LLVMTypeRef.Int8;
                                return true;
                            }

                            var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                            var casted = builder.BuildBitCast(global, elemPtrType, "elembase");
                            ptr = builder.BuildGEP2(elemType, casted, new[] { index }, "elemaddr");
                            return true;
                        }
                    }
                }
            }

            // Handle simple array[i] pattern
            if (arr.Receiver is IdentifierExpressionSyntax id)
            {
                // Check if it's a local array (e.g., function parameter)
                if (locals.TryGetValue(id.Identifier.Text, out var local) && local.SemanticType is ArrayTypeSymbol localArrayType)
                {
                    var index = LowerExpression(builder, arr.Index, locals);
                    elemType = _moduleBuilder.TypeMapper.Map(localArrayType.ElementType);

                    // For array descriptors (struct { ptr, ... }), extract the pointer first
                    if (local.IsArrayDescriptor && local.ArrayLayout is not null)
                    {
                        // Load the descriptor struct
                        var descriptor = builder.BuildLoad2(local.Type, local.Value, "arrdesc");
                        // Extract the pointer (first field)
                        var basePtr = builder.BuildExtractValue(descriptor, 0, "arrptr");
                        // Calculate element address
                        ptr = builder.BuildGEP2(elemType, basePtr, new[] { index }, "elemaddr");
                        return true;
                    }

                    // Simple pointer case
                    var basePtrSimple = local.IsAddress
                        ? builder.BuildLoad2(local.Type, local.Value, "arrbase")
                        : local.Value;

                    ptr = builder.BuildGEP2(elemType, basePtrSimple, new[] { index }, "elemaddr");
                    return true;
                }

                if (_symbols.TryGetValue(id.Identifier.Text, out var sym) && (sym.Kind == SymbolKind.Global || sym.Kind == SymbolKind.Const) && sym.Type is ArrayTypeSymbol arrayType)
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
                                var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                                var casted = builder.BuildBitCast(fieldGlobal, elemPtrType, "fieldbase");
                                ptr = builder.BuildGEP2(elemType, casted, new[] { index }, "fieldaddr");
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
                        LLVMValueRef basePtr;
                        if (arrayType.ElementType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                        {
                            var headerSize = HeaderSizeFor(prim.PrimitiveName);
                            var backingBytes = arrayType.Size > 0 ? arrayType.Size + headerSize : headerSize;
                            backingBytes = Math.Max(1, backingBytes);
                            var backingArrayType = LLVMTypeRef.CreateArray(LLVMTypeRef.Int8, (uint)backingBytes);
                            var payloadPtr = builder.BuildGEP2(backingArrayType, global, new[] { ConstI32(0), ConstI32(headerSize) }, "str.payload");
                            var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                            basePtr = builder.BuildBitCast(payloadPtr, elemPtrType, "elembase");
                        }
                        else
                        {
                            var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                            basePtr = builder.BuildBitCast(global, elemPtrType, "elembase");
                        }

                        ptr = builder.BuildGEP2(elemType, basePtr, new[] { index }, "elemaddr");
                        return true;
                    }
                }
            }

            return false;
        }

        private bool TryBuildElementPointer(LLVMBuilderRef builder, LocalBinding binding, string? fieldName, SourceSpan span, out LLVMValueRef ptr, out LLVMTypeRef elemType)
        {
            ptr = default;
            elemType = default;

            if (binding.Element is null)
            {
                return false;
            }

            var element = binding.Element.Value;
            var index = builder.BuildLoad2(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), element.IndexAlloca, "foreach.idx");

            if (element.StructDecl is not null)
            {
                if (fieldName is null)
                {
                    AddDiagnostic("Struct elements in foreach require field access.", span);
                    return false;
                }

                var field = element.StructDecl.Fields.FirstOrDefault(f => f.Identifier.Text == fieldName);
                if (field is null)
                {
                    AddDiagnostic($"Unknown field '{fieldName}' on struct '{element.StructDecl.Name.Text}'.", span);
                    return false;
                }

                var fieldType = ResolveType(field.Type, _symbols);
                elemType = _moduleBuilder.TypeMapper.Map(fieldType);
                LLVMValueRef? basePtr = null;
                if (element.FieldPtrs is not null && element.FieldPtrs.TryGetValue(fieldName, out var fp))
                {
                    basePtr = fp;
                }
                else
                {
                    var globalName = ResolveStructFieldGlobalName(element.BaseName ?? string.Empty, element.StructDecl, fieldName);
                    var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
                    if (global.Handle == IntPtr.Zero)
                    {
                        AddDiagnostic($"Layout for '{element.BaseName}' missing field '{fieldName}'.", span);
                        return false;
                    }

                    var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                    basePtr = builder.BuildBitCast(global, elemPtrType, "fieldbase");
                }

                ptr = builder.BuildGEP2(elemType, basePtr.Value, new[] { index }, "fieldaddr");
                return true;
            }

            if (fieldName is not null)
            {
                AddDiagnostic("Field access requires struct array elements.", span);
                return false;
            }

            elemType = _moduleBuilder.TypeMapper.Map(element.ElementType);
            LLVMValueRef? basePrimitive = element.PrimitiveBasePtr;
            if (basePrimitive is null && !string.IsNullOrEmpty(element.BaseName))
            {
                var baseGlobal = _moduleBuilder.Module.GetNamedGlobal(element.BaseName);
                if (baseGlobal.Handle != IntPtr.Zero)
                {
                    var elemPtrTypePrimitive = LLVMTypeRef.CreatePointer(elemType, 0);
                    basePrimitive = builder.BuildBitCast(baseGlobal, elemPtrTypePrimitive, "elembase");
                }
            }

            if (basePrimitive is null)
            {
                AddDiagnostic($"Unknown array '{element.BaseName}' in foreach.", span);
                return false;
            }

            ptr = builder.BuildGEP2(elemType, basePrimitive.Value, new[] { index }, "elemaddr");
            return true;
        }

        private string ResolveStructFieldGlobalName(string baseName, StructDeclarationSyntax structDecl, string fieldName)
        {
            var candidate = $"{baseName}_{fieldName}";
            if (_moduleBuilder.Module.GetNamedGlobal(candidate).Handle != IntPtr.Zero)
            {
                return candidate;
            }

            var structCandidate = $"{structDecl.Name.Text}_{fieldName}";
            return structCandidate;
        }

        private bool TryLowerDirListArgument(LLVMBuilderRef builder, ExpressionSyntax expr, Dictionary<string, LocalBinding> locals, out LLVMValueRef namesPtr, out LLVMValueRef flagsPtr, out LLVMValueRef countPtr)
        {
            namesPtr = default;
            flagsPtr = default;
            countPtr = default;

            var exprType = ResolveExpressionType(expr);
            if (exprType is not NamedTypeSymbol named || !_structs.TryGetValue(named.TypeName, out var dirListStruct) || !string.Equals(dirListStruct.Name.Text, "DirList", StringComparison.Ordinal))
            {
                return false;
            }

            if (!TryResolveStructBaseName(expr, out var baseName))
            {
                return false;
            }

            if (!_structs.TryGetValue("DirEntry", out var dirEntryStruct))
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

            var entriesBase = $"{baseName}_entries";
            var nameGlobal = $"{entriesBase}_name";
            var isDirGlobal = $"{entriesBase}_is_dir";
            var countGlobal = $"{baseName}_count";

            var nameType = ResolveType(nameField.Type, _symbols);
            var nameLlvm = _moduleBuilder.TypeMapper.Map(nameType);
            var nameGlobalValue = _moduleBuilder.Module.GetNamedGlobal(nameGlobal);
            if (nameGlobalValue.Handle == IntPtr.Zero)
            {
                return false;
            }
            var namePtr = builder.BuildGEP2(nameLlvm, nameGlobalValue, new[] { ConstI32(0), ConstI32(0) }, $"{nameGlobal}.ptr");
            namesPtr = builder.BuildBitCast(namePtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0), $"{nameGlobal}.i8ptr");

            var isDirType = ResolveType(isDirField.Type, _symbols);
            var isDirLlvm = _moduleBuilder.TypeMapper.Map(isDirType);
            var isDirGlobalValue = _moduleBuilder.Module.GetNamedGlobal(isDirGlobal);
            if (isDirGlobalValue.Handle == IntPtr.Zero)
            {
                return false;
            }
            var isDirPtr = builder.BuildGEP2(isDirLlvm, isDirGlobalValue, new[] { ConstI32(0), ConstI32(0) }, $"{isDirGlobal}.ptr");
            flagsPtr = builder.BuildBitCast(isDirPtr, LLVMTypeRef.CreatePointer(LLVMTypeRef.Int32, 0), $"{isDirGlobal}.i32ptr");

            var countGlobalValue = _moduleBuilder.Module.GetNamedGlobal(countGlobal);
            if (countGlobalValue.Handle == IntPtr.Zero)
            {
                return false;
            }

            countPtr = countGlobalValue;
            return true;
        }

        private bool TryResolveStructBaseName(ExpressionSyntax expr, out string baseName)
        {
            baseName = string.Empty;
            switch (expr)
            {
                case IdentifierExpressionSyntax id:
                    baseName = TryResolveGlobalName(id.Identifier.Text);
                    return !string.IsNullOrEmpty(baseName);
                case MemberAccessExpressionSyntax member:
                    var flattened = BuildFlattenedMemberPath(member);
                    if (!string.IsNullOrEmpty(flattened))
                    {
                        baseName = flattened;
                        return true;
                    }
                    break;
            }

            return false;
        }

        private LLVMValueRef ConstBool(bool value) =>
            LLVMValueRef.CreateConstInt(LLVMTypeRef.Int1, value ? 1u : 0u, false);

        private LLVMBasicBlockRef AppendBlock(LLVMValueRef function, string name) =>
            function.AppendBasicBlock(name);

        private LLVMValueRef EmitPrintf(LLVMBuilderRef builder, string format, params LLVMValueRef[] values)
        {
            var (printf, printfType) = GetOrDeclarePrintf(_moduleBuilder);
            var fmt = builder.BuildGlobalStringPtr(format, $"fmt_{_blockId++}");
            var args = new LLVMValueRef[values.Length + 1];
            args[0] = fmt;
            for (int i = 0; i < values.Length; i++)
            {
                args[i + 1] = values[i];
            }

            var callType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, args.Select(a => a.TypeOf).ToArray(), false);
            return builder.BuildCall2(callType, printf, args, "printf.call");
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

        private LLVMValueRef EmitReadInt(LLVMBuilderRef builder)
        {
            var alloca = builder.BuildAlloca(LLVMTypeRef.Int32, "read_int.tmp");
            var (scanf, scanfType) = GetOrDeclareScanf(_moduleBuilder);
            var fmt = builder.BuildGlobalStringPtr("%d", $"fmt_read_{_blockId++}");
            var callType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { fmt.TypeOf, alloca.TypeOf }, false);
            builder.BuildCall2(callType, scanf, new[] { fmt, alloca }, "scanf.call");
            return builder.BuildLoad2(LLVMTypeRef.Int32, alloca, "read_int.val");
        }

        private LLVMValueRef EmitReadChar(LLVMBuilderRef builder)
        {
            var alloca = builder.BuildAlloca(LLVMTypeRef.Int32, "read_char.tmp");
            builder.BuildStore(ConstI32(0), alloca);
            var (scanf, scanfType) = GetOrDeclareScanf(_moduleBuilder);
            var fmt = builder.BuildGlobalStringPtr("%c", $"fmt_readc_{_blockId++}");
            var callType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Int32, new[] { fmt.TypeOf, alloca.TypeOf }, false);
            var result = builder.BuildCall2(callType, scanf, new[] { fmt, alloca }, "scanf.char.call");

            var parent = builder.InsertBlock.Parent;
            var cont = AppendBlock(parent, "read_char.cont");
            var eof = AppendBlock(parent, "read_char.eof");
            var ok = builder.BuildICmp(LLVMIntPredicate.LLVMIntSGE, result, ConstI32(1), "read_char.ok");
            builder.BuildCondBr(ok, cont, eof);

            builder.PositionAtEnd(eof);
            builder.BuildStore(ConstI32(0), alloca);
            builder.BuildBr(cont);

            builder.PositionAtEnd(cont);
            return builder.BuildLoad2(LLVMTypeRef.Int32, alloca, "read_char.val");
        }

        private LLVMValueRef EmitTime(LLVMBuilderRef builder)
        {
            var (timeFn, timeType, timePtrType) = GetOrDeclareTime(_moduleBuilder);
            var nullPtr = LLVMValueRef.CreateConstPointerNull(timePtrType);
            var value = builder.BuildCall2(timeType, timeFn, new[] { nullPtr }, "time.call");
            return builder.BuildTrunc(value, LLVMTypeRef.Int32, "time.i32");
        }

        private LLVMValueRef EmitGetTimeMs(LLVMBuilderRef builder)
        {
            var (clockFn, clockType) = GetOrDeclareClock(_moduleBuilder);
            var ticks = builder.BuildCall2(clockType, clockFn, Array.Empty<LLVMValueRef>(), "gfx.clock");
            var ticksMs = builder.BuildMul(ticks, ConstInt64(1000), "gfx.clock_ms");
            var clocksPerSec = ConstInt64(RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? 1000 : 1000000);
            var ms64 = builder.BuildUDiv(ticksMs, clocksPerSec, "gfx.ms64");
            return builder.BuildTrunc(ms64, LLVMTypeRef.Int32, "gfx.ms");
        }

        private LLVMValueRef EmitGetTimeUs(LLVMBuilderRef builder)
        {
            var (clockFn, clockType) = GetOrDeclareClock(_moduleBuilder);
            var ticks = builder.BuildCall2(clockType, clockFn, Array.Empty<LLVMValueRef>(), "gfx.clock");
            var ticksUs = builder.BuildMul(ticks, ConstInt64(1000000), "gfx.clock_us");
            var clocksPerSec = ConstInt64(RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? 1000 : 1000000);
            var us64 = builder.BuildUDiv(ticksUs, clocksPerSec, "gfx.us64");
            return builder.BuildTrunc(us64, LLVMTypeRef.Int32, "gfx.us");
        }

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

        private readonly record struct IterableInfo(int ConstLength, LLVMValueRef? LengthValue, ElementBinding? ElementBinding);

        private bool TryResolveArrayLength(ExpressionSyntax expr, out int length)
        {
            length = 0;
            var type = ResolveExpressionType(expr);
            if (type is ArrayTypeSymbol array)
            {
                length = array.Size;
                return true;
            }

            return false;
        }

        private TypeSymbol? ResolveExpressionType(ExpressionSyntax expr)
        {
            switch (expr)
            {
                case IdentifierExpressionSyntax id:
                    if (_symbols.TryGetValue(id.Identifier.Text, out var sym))
                    {
                        return sym.Type;
                    }

                    return null;
                case StructInitializerExpressionSyntax:
                    return null;
                case MemberAccessExpressionSyntax member:
                    var receiverType = ResolveExpressionType(member.Receiver);
                    if (receiverType is NamedTypeSymbol named && _structs.TryGetValue(named.TypeName, out var structDecl))
                    {
                        var field = structDecl.Fields.FirstOrDefault(f => string.Equals(f.Identifier.Text, member.Member.Text, StringComparison.Ordinal));
                        if (field is not null)
                        {
                            return ResolveType(field.Type, _symbols);
                        }
                    }

                    if (receiverType is ArrayTypeSymbol && string.Equals(member.Member.Text, "length", StringComparison.Ordinal))
                    {
                        return new PrimitiveTypeSymbol("i32");
                    }

                    return null;
                case ArrayAccessExpressionSyntax arr:
                    if (ResolveExpressionType(arr.Receiver) is ArrayTypeSymbol array)
                    {
                        return array.ElementType;
                    }

                    return null;
                case ParenthesizedExpressionSyntax paren:
                    return ResolveExpressionType(paren.Expression);
                default:
                    return null;
            }
        }

        private string TryResolveGlobalName(string name) =>
            _globalLayouts.TryGetValue(name, out var layout) ? layout.Name : name;

        private bool TryResolveIterableInfo(LLVMBuilderRef builder, ExpressionSyntax iterable, Dictionary<string, LocalBinding> locals, out IterableInfo info)
        {
            // Local/parameter array descriptor
            if (iterable is IdentifierExpressionSyntax localId && locals.TryGetValue(localId.Identifier.Text, out var localBinding) && localBinding.IsArrayDescriptor && localBinding.ArrayLayout is not null && localBinding.SemanticType is ArrayTypeSymbol localArray)
            {
                var descriptor = builder.BuildLoad2(localBinding.ArrayLayout.DescriptorType, localBinding.Value, $"{localId.Identifier.Text}.desc.load");
                var lenIndex = localBinding.ArrayLayout.IsStructArray
                    ? (uint)(localBinding.ArrayLayout.FieldOrder?.Count ?? 0)
                    : 1u;

                LLVMValueRef? lengthValue = null;
                if (localArray.Size <= 0)
                {
                    lengthValue = builder.BuildExtractValue(descriptor, lenIndex, $"{localId.Identifier.Text}.len");
                }

                if (localBinding.ArrayLayout.IsStructArray && localBinding.ArrayLayout.StructDecl is not null && localBinding.ArrayLayout.FieldOrder is not null)
                {
                    var fieldPtrs = new Dictionary<string, LLVMValueRef>(StringComparer.Ordinal);
                    foreach (var (fieldName, idx) in localBinding.ArrayLayout.FieldOrder)
                    {
                        var fieldPtr = builder.BuildExtractValue(descriptor, (uint)idx, $"{localId.Identifier.Text}.{fieldName}.ptr");
                        fieldPtrs[fieldName] = fieldPtr;
                    }

                    info = new IterableInfo(localArray.Size, lengthValue, new ElementBinding(localBinding.ArrayLayout.StructDecl, localArray.ElementType, default, null, null, fieldPtrs));
                    return true;
                }
                else
                {
                    var elemPtr = builder.BuildExtractValue(descriptor, 0, $"{localId.Identifier.Text}.ptr");
                    info = new IterableInfo(localArray.Size, lengthValue, new ElementBinding(null, localArray.ElementType, default, null, elemPtr, null));
                    return true;
                }
            }

            // Global array identifier
            if (iterable is IdentifierExpressionSyntax id && _symbols.TryGetValue(id.Identifier.Text, out var sym) && sym.Type is ArrayTypeSymbol array)
            {
                if (array.ElementType is NamedTypeSymbol namedElem && _structs.TryGetValue(namedElem.TypeName, out var structDecl))
                {
                    var fieldPtrs = new Dictionary<string, LLVMValueRef>(StringComparer.Ordinal);
                    foreach (var field in structDecl.Fields)
                    {
                        var fieldType = ResolveType(field.Type, _symbols);
                        var llvmField = _moduleBuilder.TypeMapper.Map(fieldType);
                        var ptrType = LLVMTypeRef.CreatePointer(llvmField, 0);
                        var globalName = TryResolveFieldGlobalName(id.Identifier.Text, namedElem.TypeName, field.Identifier.Text);
                        var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
                        if (global.Handle == IntPtr.Zero)
                        {
                            continue;
                        }

                        var bitcast = builder.BuildBitCast(global, ptrType, $"{globalName}.ptr");
                        fieldPtrs[field.Identifier.Text] = bitcast;
                    }

                    info = new IterableInfo(array.Size, null, new ElementBinding(structDecl, array.ElementType, default, id.Identifier.Text, null, fieldPtrs));
                    return true;
                }
                else
                {
                    var elemType = _moduleBuilder.TypeMapper.Map(array.ElementType);
                    var ptrType = LLVMTypeRef.CreatePointer(elemType, 0);
                    var global = _moduleBuilder.Module.GetNamedGlobal(TryResolveGlobalName(id.Identifier.Text));
                    if (global.Handle != IntPtr.Zero)
                    {
                        var bitcast = builder.BuildBitCast(global, ptrType, $"{id.Identifier.Text}.ptr");
                        info = new IterableInfo(array.Size, null, new ElementBinding(null, array.ElementType, default, id.Identifier.Text, bitcast, null));
                        return true;
                    }
                }
            }

            // Member access state.field arrays
            if (iterable is MemberAccessExpressionSyntax member && member.Receiver is IdentifierExpressionSyntax recv && _symbols.TryGetValue(recv.Identifier.Text, out var recvSym) && recvSym.Type is NamedTypeSymbol recvType && _structs.TryGetValue(recvType.TypeName, out var structDecl2))
            {
                var field = structDecl2.Fields.FirstOrDefault(f => f.Identifier.Text == member.Member.Text);
                if (field?.Type is ArrayTypeSyntax arraySyntax)
                {
                    var elementType = ResolveType(arraySyntax.ElementType, _symbols);
                    var size = int.TryParse(arraySyntax.SizeToken?.Text ?? string.Empty, out var parsed) ? parsed : -1;
                    var baseName = $"{recv.Identifier.Text}_{member.Member.Text}";

                    if (elementType is NamedTypeSymbol nested && _structs.TryGetValue(nested.TypeName, out var nestedStruct))
                    {
                        var fieldPtrs = new Dictionary<string, LLVMValueRef>(StringComparer.Ordinal);
                        foreach (var nf in nestedStruct.Fields)
                        {
                            var nfType = ResolveType(nf.Type, _symbols);
                            var llvmField = _moduleBuilder.TypeMapper.Map(nfType);
                            var ptrType = LLVMTypeRef.CreatePointer(llvmField, 0);
                            var globalName = TryResolveFieldGlobalName(baseName, nested.TypeName, nf.Identifier.Text);
                            var global = _moduleBuilder.Module.GetNamedGlobal(globalName);
                            if (global.Handle == IntPtr.Zero)
                            {
                                continue;
                            }

                            var bitcast = builder.BuildBitCast(global, ptrType, $"{globalName}.ptr");
                            fieldPtrs[nf.Identifier.Text] = bitcast;
                        }

                        info = new IterableInfo(size, null, new ElementBinding(nestedStruct, elementType, default, baseName, null, fieldPtrs));
                        return true;
                    }
                    else
                    {
                        var elemType = _moduleBuilder.TypeMapper.Map(elementType);
                        var ptrType = LLVMTypeRef.CreatePointer(elemType, 0);
                        var global = _moduleBuilder.Module.GetNamedGlobal(baseName);
                        if (global.Handle != IntPtr.Zero)
                        {
                            var bitcast = builder.BuildBitCast(global, ptrType, $"{baseName}.ptr");
                            info = new IterableInfo(size, null, new ElementBinding(null, elementType, default, baseName, bitcast, null));
                            return true;
                        }
                    }
                }
            }

            info = default;
            return false;
        }
private string ResolveStructArrayBaseName(StructDeclarationSyntax structDecl, string preferredName)
        {
            var firstField = structDecl.Fields.FirstOrDefault();
            if (firstField is null)
            {
                return preferredName;
            }

            var candidatePrimary = $"{preferredName}_{firstField.Identifier.Text}";
            if (_moduleBuilder.Module.GetNamedGlobal(candidatePrimary).Handle != IntPtr.Zero)
            {
                return preferredName;
            }

            var candidateStruct = $"{structDecl.Name.Text}_{firstField.Identifier.Text}";
            if (_moduleBuilder.Module.GetNamedGlobal(candidateStruct).Handle != IntPtr.Zero)
            {
                return structDecl.Name.Text;
            }

            return preferredName;
        }

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

        private string? BuildFlattenedMemberPath(MemberAccessExpressionSyntax member)
        {
            // Build the flattened path by recursively walking the member chain
            var parts = new List<string>();
            var current = (ExpressionSyntax)member;

            while (current is MemberAccessExpressionSyntax m)
            {
                parts.Add(m.Member.Text);
                current = m.Receiver;
            }

            if (current is IdentifierExpressionSyntax id)
            {
                parts.Add(id.Identifier.Text);
                parts.Reverse();
                return string.Join("_", parts);
            }

            return null;
        }

        private bool TryResolveMemberType(MemberAccessExpressionSyntax member, out TypeSymbol type)
        {
            type = null!;

            // Start from the root identifier
            var current = (ExpressionSyntax)member;
            var chain = new List<MemberAccessExpressionSyntax>();

            while (current is MemberAccessExpressionSyntax m)
            {
                chain.Add(m);
                current = m.Receiver;
            }

            if (current is not IdentifierExpressionSyntax rootId)
            {
                return false;
            }

            // Resolve the root symbol
            if (!_symbols.TryGetValue(rootId.Identifier.Text, out var sym) || sym.Type is null)
            {
                return false;
            }

            var currentType = sym.Type;
            chain.Reverse();

            // Walk the chain from root to leaf
            foreach (var memberAccess in chain)
            {
                if (currentType is not NamedTypeSymbol namedType)
                {
                    return false;
                }

                if (!_structs.TryGetValue(namedType.TypeName, out var structDecl))
                {
                    return false;
                }

                var field = structDecl.Fields.FirstOrDefault(f => f.Identifier.Text == memberAccess.Member.Text);
                if (field is null)
                {
                    return false;
                }

                currentType = ResolveType(field.Type, _symbols);
            }

            type = currentType;
            return true;
        }

        private (LLVMTypeRef ReturnType, LLVMTypeRef[] Parameters) ResolveFunctionSignature(string name)
        {
            if (_functions.TryGetValue(name, out var fn))
            {
                var retType = fn.ReturnType is null
                    ? LLVMTypeRef.Void
                    : _moduleBuilder.TypeMapper.Map(ResolveType(fn.ReturnType, _symbols));
                var paramTypes = fn.Parameters.Select(p =>
                {
                    var pType = ResolveType(p.Type, _symbols);
                    if (pType is ArrayTypeSymbol arr)
                    {
                        var layout = CreateArrayDescriptorLayout(arr, _moduleBuilder.TypeMapper, _structs, _symbols);
                        return layout.DescriptorType;
                    }

                    return _moduleBuilder.TypeMapper.Map(pType);
                }).ToArray();
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

        private TypeSymbol[] ResolveParameterTypes(string name)
        {
            if (_functions.TryGetValue(name, out var fn))
            {
                return fn.Parameters.Select(p => ResolveType(p.Type, _symbols)).ToArray();
            }

            if (_tests.TryGetValue(name, out var test))
            {
                return test.Parameters.Select(p => ResolveType(p.Type, _symbols)).ToArray();
            }

            return Array.Empty<TypeSymbol>();
        }

        private LLVMValueRef ConstI32(int value) =>
            LLVMValueRef.CreateConstInt(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("i32")), (ulong)value, true);

        private LLVMValueRef ConstI64(long value) =>
            LLVMValueRef.CreateConstInt(LLVMTypeRef.Int64, (ulong)value, true);

        private LLVMValueRef ConstF32(float value) =>
            LLVMValueRef.CreateConstReal(LLVMTypeRef.Float, value);

        /// <summary>
        /// Converts a value to the target type if needed (e.g., i32 -> f32 or f32 -> i32).
        /// </summary>
        private LLVMValueRef ConvertToType(LLVMBuilderRef builder, LLVMValueRef value, LLVMTypeRef targetType)
        {
            var sourceType = value.TypeOf;
            if (sourceType.Handle == targetType.Handle)
            {
                return value;
            }

            var sourceIsInt = sourceType.Kind == LLVMTypeKind.LLVMIntegerTypeKind;
            var sourceIsFloat = sourceType.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;
            var targetIsInt = targetType.Kind == LLVMTypeKind.LLVMIntegerTypeKind;
            var targetIsFloat = targetType.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;

            if (sourceIsInt && targetIsInt)
            {
                if (sourceType.IntWidth == targetType.IntWidth)
                {
                    return value;
                }

                if (sourceType.IntWidth < targetType.IntWidth)
                {
                    return builder.BuildZExt(value, targetType, "zext");
                }

                return builder.BuildTrunc(value, targetType, "trunc");
            }

            if (sourceIsFloat && targetIsFloat)
            {
                if (sourceType.Kind == LLVMTypeKind.LLVMFloatTypeKind && targetType.Kind == LLVMTypeKind.LLVMDoubleTypeKind)
                {
                    return builder.BuildFPExt(value, targetType, "fpext");
                }

                if (sourceType.Kind == LLVMTypeKind.LLVMDoubleTypeKind && targetType.Kind == LLVMTypeKind.LLVMFloatTypeKind)
                {
                    return builder.BuildFPTrunc(value, targetType, "fptrunc");
                }

                return value;
            }

            // i32 -> f32: signed int to float
            if (sourceIsInt && targetIsFloat)
            {
                return builder.BuildSIToFP(value, targetType, "i2f");
            }

            // f32 -> i32: float to signed int
            if (sourceIsFloat && targetIsInt)
            {
                return builder.BuildFPToSI(value, targetType, "f2i");
            }

            // No conversion needed or unsupported
            return value;
        }

        private LLVMValueRef UnsupportedOperator(SourceSpan span, LLVMValueRef fallback)
        {
            AddDiagnostic("Unsupported operator-method during lowering.", span);
            return fallback;
        }

        private void AddDiagnostic(string message, SourceSpan span) =>
            _diagnostics.Add(new Diagnostic(message, span));
    }

    private static void EmitFunctionSignatures(CompilationUnitSyntax compilationUnit, IReadOnlyDictionary<string, Symbol> symbols, LlvmModuleBuilder builder, bool includeTests, HashSet<string> reachableFunctions)
    {
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);

        foreach (var fn in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            if (!reachableFunctions.Contains(fn.Name.Text))
            {
                continue;
            }
            EmitFunction(builder, symbols, structs, fn.Name.Text, fn.ReturnType, fn.Parameters, isTest: false);
        }

        if (!includeTests)
        {
            return;
        }

        foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
        {
            EmitFunction(builder, symbols, structs, test.Name.Text, test.ReturnType, test.Parameters, isTest: true);
        }
    }

    private static void EmitFunction(LlvmModuleBuilder builder, IReadOnlyDictionary<string, Symbol> symbols, IReadOnlyDictionary<string, StructDeclarationSyntax> structs, string name, TypeSyntax? returnType, IReadOnlyList<ParameterSyntax> parameters, bool isTest)
    {
        var ret = returnType is null
            ? (isTest ? LLVMTypeRef.Int32 : LLVMTypeRef.Void)
            : builder.TypeMapper.Map(ResolveType(returnType, symbols));

        var paramTypes = parameters
            .Select(p =>
            {
                var paramType = ResolveType(p.Type, symbols);
                if (paramType is ArrayTypeSymbol arr)
                {
                    var layout = CreateArrayDescriptorLayout(arr, builder.TypeMapper, structs, symbols);
                    return layout.DescriptorType;
                }

                return builder.TypeMapper.Map(paramType);
            })
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
                var sizeText = array.SizeToken?.Text ?? string.Empty;
                var size = int.TryParse(sizeText, out var parsed) ? parsed : -1;
                return new ArrayTypeSymbol(element, size);
            default:
                return new NamedTypeSymbol("unknown");
        }
    }

    private static ArrayDescriptorLayout CreateArrayDescriptorLayout(ArrayTypeSymbol arrayType, LlvmTypeMapper mapper, IReadOnlyDictionary<string, StructDeclarationSyntax> structs, IReadOnlyDictionary<string, Symbol> symbols)
    {
        var int32 = LLVMTypeRef.Int32;
        if (arrayType.ElementType is NamedTypeSymbol named && structs.TryGetValue(named.TypeName, out var structDecl))
        {
            var fieldOrder = new Dictionary<string, int>(StringComparer.Ordinal);
            var elements = new List<LLVMTypeRef>();
            for (int i = 0; i < structDecl.Fields.Count; i++)
            {
                var field = structDecl.Fields[i];
                var fieldType = ResolveType(field.Type, symbols);
                var fieldPtr = LLVMTypeRef.CreatePointer(mapper.Map(fieldType), 0);
                fieldOrder[field.Identifier.Text] = i;
                elements.Add(fieldPtr);
            }

            elements.Add(int32); // length
            var descriptor = LLVMTypeRef.CreateStruct(elements.ToArray(), false);
            return new ArrayDescriptorLayout(descriptor, true, structDecl, fieldOrder);
        }

        var elemPtr = LLVMTypeRef.CreatePointer(mapper.Map(arrayType.ElementType), 0);
        var primitiveDescriptor = LLVMTypeRef.CreateStruct(new[] { elemPtr, int32 }, false);
        return new ArrayDescriptorLayout(primitiveDescriptor, false, null, null);
    }
}
