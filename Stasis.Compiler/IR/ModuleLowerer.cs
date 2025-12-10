using System;
using System.Globalization;
using System.Text;
using System.Runtime.InteropServices;
using LLVMSharp.Interop;
using Stasis.Compiler;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

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
        EmitConstants(compilationUnit, semantic.Symbols, builder);
        EmitFunctionSignatures(compilationUnit, semantic.Symbols, builder, opts.IncludeTests);

        var diagnostics = new List<Diagnostic>();
        var lowerer = new FunctionLowerer(builder, semantic.Symbols, layout, diagnostics, opts.IncludeTests, opts.HeadlessGraphics);
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
                        var count = ParseArrayLength(arrayType.SizeToken.Text);
                        foreach (var nestedField in nestedStruct.Fields)
                        {
                            var nestedFieldType = ResolveType(nestedField.Type, symbols);
                            var llvmElem = builder.TypeMapper.Map(nestedFieldType);
                            var nestedName = $"{fieldName}_{nestedField.Identifier.Text}";
                            builder.DefineGlobalArray(nestedName, llvmElem, count);
                        }
                        break;
                    }
                case ArrayTypeSyntax arrayType:
                    {
                        // Primitive array
                        var elemType = ResolveType(arrayType.ElementType, symbols);
                        var llvmElem = builder.TypeMapper.Map(elemType);
                        var count = ParseArrayLength(arrayType.SizeToken.Text);
                        builder.DefineGlobalArray(fieldName, llvmElem, count);
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

            // Evaluate the initializer to get a constant value
            var constValue = EvaluateConstantExpression(constDecl.Initializer, llvmType, builder.Context);
            if (constValue.Handle != IntPtr.Zero)
            {
                builder.DefineConstantScalar(constDecl.Name.Text, llvmType, constValue);
            }
        }
    }

    private static LLVMValueRef EvaluateConstantExpression(ExpressionSyntax expr, LLVMTypeRef targetType, LLVMContextRef context)
    {
        if (expr is LiteralExpressionSyntax lit)
        {
            return lit.Literal.Kind switch
            {
                TokenKind.IntegerLiteral => int.TryParse(lit.Literal.Text, out var i)
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

    private static (LLVMValueRef Fn, LLVMTypeRef Type) GetOrDeclareStasisSleepMs(LlvmModuleBuilder builder)
    {
        var fn = builder.Module.GetNamedFunction("stasis_sleep_ms");
        var fnType = LLVMTypeRef.CreateFunction(LLVMTypeRef.Void, new[] { LLVMTypeRef.Int32 }, false);
        if (fn.Handle != IntPtr.Zero)
            return (fn, fnType);
        fn = builder.Module.AddFunction("stasis_sleep_ms", fnType);
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
        private readonly HashSet<string> _builtIns = new(StringComparer.Ordinal)
        {
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
            "time",
            "init_window",
            "begin_frame",
            "end_frame",
            "clear",
            "draw_line",
            "is_key_down",
            "get_time_ms",
            "sleep_ms",
            "should_quit"
        };
        private int _blockId;
        private readonly bool _headlessGraphics;

        public FunctionLowerer(LlvmModuleBuilder moduleBuilder, IReadOnlyDictionary<string, Symbol> symbols, LayoutPlan layout, List<Diagnostic> diagnostics, bool includeTests, bool headlessGraphics)
        {
            _moduleBuilder = moduleBuilder;
            _symbols = symbols;
            _globalLayouts = layout.Globals.ToDictionary(g => g.Name, g => g, StringComparer.Ordinal);
            _diagnostics = diagnostics;
            _headlessGraphics = headlessGraphics;
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
                var paramType = ResolveType(param.Type, _symbols);
                var llvmType = _moduleBuilder.TypeMapper.Map(paramType);
                var paramVal = function.GetParam((uint)i);
                var alloca = builder.BuildAlloca(llvmType, param.Name.Text);
                builder.BuildStore(paramVal, alloca);
                locals[param.Name.Text] = new LocalBinding(alloca, llvmType, true);
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
                    return LowerLiteral(builder, lit);
                case IdentifierExpressionSyntax id:
                    if (locals.TryGetValue(id.Identifier.Text, out var value))
                    {
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
                case TokenKind.IntegerLiteral when int.TryParse(lit.Literal.Text, out var ival):
                    return ConstI32(ival);
                case TokenKind.FloatLiteral when float.TryParse(lit.Literal.Text, NumberStyles.Float, CultureInfo.InvariantCulture, out var fval):
                    return LLVMValueRef.CreateConstReal(_moduleBuilder.TypeMapper.Map(new PrimitiveTypeSymbol("f32")), fval);
                case TokenKind.StringLiteral:
                    {
                        var text = UnescapeString(lit.Literal.Text);
                        return builder.BuildGlobalStringPtr(text, $"str_{_blockId++}");
                    }
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

        private LLVMValueRef LowerAssignment(LLVMBuilderRef builder, AssignmentExpressionSyntax assign, Dictionary<string, LocalBinding> locals)
        {
            if (!TryGetPointer(builder, assign.Left, locals, out var ptr, out var ptrType))
            {
                AddDiagnostic("Left side of assignment must be an assignable location (identifier, field, or array element).", assign.Left.Span);
                return ConstI32(0);
            }

            var rhs = LowerExpression(builder, assign.Right, locals);
            // Convert RHS to target type if needed (e.g., i32 -> f32)
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
            builder.BuildStore(combined, ptr);
            return combined;
        }

        private bool TryGetPointer(LLVMBuilderRef builder, ExpressionSyntax target, Dictionary<string, LocalBinding> locals, out LLVMValueRef ptr, out LLVMTypeRef type)
        {
            ptr = default;
            type = default;

            switch (target)
            {
                case IdentifierExpressionSyntax id:
                    if (locals.TryGetValue(id.Identifier.Text, out var local))
                    {
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
                    if (TryLowerArrayElementPointer(builder, arr, fieldName: null, locals, out var elemPtr, out var elemType))
                    {
                        ptr = elemPtr;
                        type = elemType;
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
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(falseBlock);
                        var rhsBool = AsBoolean(builder, LowerExpression(builder, bin.Right, locals));
                        var rhsVal = BuildBoolResult(builder, rhsBool);
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(mergeBlock);
                        var phi = builder.BuildPhi(LLVMTypeRef.Int32, "or.result");
                        phi.AddIncoming(new[] { trueVal }, new[] { trueBlock }, 1u);
                        phi.AddIncoming(new[] { rhsVal }, new[] { falseBlock }, 1u);
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
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(falseBlock);
                        var falseVal = ConstI32(0);
                        builder.BuildBr(mergeBlock);

                        builder.PositionAtEnd(mergeBlock);
                        var phi = builder.BuildPhi(LLVMTypeRef.Int32, "and.result");
                        phi.AddIncoming(new[] { rhsVal }, new[] { trueBlock }, 1u);
                        phi.AddIncoming(new[] { falseVal }, new[] { falseBlock }, 1u);
                        return phi;
                    }
                default:
                    var lhs = LowerExpression(builder, bin.Left, locals);
                    var rhs = LowerExpression(builder, bin.Right, locals);
                    return LowerBinary(builder, bin.OperatorToken.Text, lhs, rhs, bin.OperatorToken.Span);
            }
        }

        private LLVMValueRef LowerCall(LLVMBuilderRef builder, CallExpressionSyntax call, Dictionary<string, LocalBinding> locals)
        {
            if (call.Callee is not IdentifierExpressionSyntax id)
            {
                AddDiagnostic("Only simple function calls are supported.", call.Span);
                return ConstI32(0);
            }

            if (_builtIns.Contains(id.Identifier.Text))
            {
                return LowerBuiltInCall(builder, id.Identifier.Text, call.Arguments, locals, call.Span);
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

            var callRetType = fnType.ReturnType;
            if (callRetType.Kind == LLVMTypeKind.LLVMVoidTypeKind)
            {
                builder.BuildCall2(fnType, fn, argValues, string.Empty);
                return ConstI32(0);
            }

            var callValue = builder.BuildCall2(fnType, fn, argValues, $"{id.Identifier.Text}.call");
            return callValue;
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
                            strPtr = builder.BuildGlobalStringPtr(text, $"strlit_{_blockId++}");
                        }
                        else
                        {
                            var value = LowerExpression(builder, args[0], locals);
                            var i8Ptr = LLVMTypeRef.CreatePointer(LLVMTypeRef.Int8, 0);
                            strPtr = value.TypeOf.Kind == LLVMTypeKind.LLVMPointerTypeKind
                                ? value
                                : builder.BuildIntToPtr(value, i8Ptr, "str.ptr");
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
                default:
                    AddDiagnostic($"Unknown built-in '{name}'.", span);
                    return ConstI32(0);
            }
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
            // Handle array[i].field syntax
            if (member.Receiver is ArrayAccessExpressionSyntax arr)
            {
                if (TryLowerArrayElementPointer(builder, arr, member.Member.Text, locals, out var ptr, out var elemType))
                {
                    return builder.BuildLoad2(elemType, ptr, "fieldload");
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
                            var llvmType = _moduleBuilder.TypeMapper.Map(fieldType);
                            return builder.BuildLoad2(llvmType, global, flattenedName);
                        }
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
                        var elemPtrType = LLVMTypeRef.CreatePointer(elemType, 0);
                        var casted = builder.BuildBitCast(global, elemPtrType, "elembase");
                        ptr = builder.BuildGEP2(elemType, casted, new[] { index }, "elemaddr");
                        return true;
                    }
                }
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

        /// <summary>
        /// Converts a value to the target type if needed (e.g., i32 -> f32 or f32 -> i32).
        /// </summary>
        private LLVMValueRef ConvertToType(LLVMBuilderRef builder, LLVMValueRef value, LLVMTypeRef targetType)
        {
            var sourceType = value.TypeOf;
            if (sourceType.Kind == targetType.Kind)
            {
                return value;
            }

            var sourceIsInt = sourceType.Kind == LLVMTypeKind.LLVMIntegerTypeKind;
            var sourceIsFloat = sourceType.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;
            var targetIsInt = targetType.Kind == LLVMTypeKind.LLVMIntegerTypeKind;
            var targetIsFloat = targetType.Kind is LLVMTypeKind.LLVMFloatTypeKind or LLVMTypeKind.LLVMDoubleTypeKind;

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
