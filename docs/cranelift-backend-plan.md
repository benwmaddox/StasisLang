# Cranelift Backend Implementation Plan

This document outlines the strategy for adding Cranelift as a second code generation backend to the Stasis compiler, alongside the existing LLVM backend.

## Executive Summary

**Goal**: Add Cranelift as a fast-compilation backend for development/debug builds while retaining LLVM for optimized release builds.

| Build Mode | Backend   | Optimization | Use Case                    |
| ---------- | --------- | ------------ | --------------------------- |
| Debug/Dev  | Cranelift | Minimal      | Fast iteration, debugging   |
| Release    | LLVM      | -O3 -flto    | Production, max performance |

---

## 1. Background

### Current Architecture

The Stasis compiler currently uses LLVM exclusively:

```
Source (.stasis)
    │
    ▼
┌─────────────────────┐
│ Lexer → Parser      │  (frontend - backend agnostic)
│ CompilationUnitSyntax│
└─────────────────────┘
    │
    ▼
┌─────────────────────┐
│ SemanticAnalyzer    │  (semantic analysis - backend agnostic)
│ Symbol table        │
└─────────────────────┘
    │
    ▼
┌─────────────────────┐
│ LayoutPlanner       │  (memory layout - backend agnostic)
│ SoA transformation  │
└─────────────────────┘
    │
    ▼
┌─────────────────────┐
│ ModuleLowerer       │  ← LLVM-specific (needs abstraction)
│ LlvmModuleBuilder   │
│ FunctionLowerer     │
└─────────────────────┘
    │
    ▼
┌─────────────────────┐
│ LLVM IR (.ll)       │
│ → clang → binary    │
└─────────────────────┘
```

### Key Files

- `Stasis.Compiler/IR/ModuleLowerer.cs` - Main lowering orchestration (~4200 lines)
- `Stasis.Compiler/IR/LlvmModuleBuilder.cs` - LLVM module/context management
- `Stasis.Compiler/IR/LlvmTypeMapper.cs` - Stasis → LLVM type mapping
- `Stasis.Compiler/IR/LowerOptions.cs` - Lowering configuration
- `Stasis.Cli/Program.cs` - CLI entry point with build modes

### Why Cranelift?

| Aspect            | LLVM                       | Cranelift                         |
| ----------------- | -------------------------- | --------------------------------- |
| Compilation speed | Slow (heavyweight)         | Fast (designed for JIT)           |
| Code quality      | Excellent                  | Good (not as optimized)           |
| Dependencies      | Native library (LLVMSharp) | Pure Rust (C# bindings available) |
| Setup complexity  | High                       | Lower                             |
| Debug builds      | Overkill                   | Perfect fit                       |

---

## 2. Design Goals

### 2.1 Backend Selection Strategy

```
┌─────────────────────────────────────────────────────────┐
│                   stasis build <file>                    │
│                                                         │
│  Debug (default)     │     Release (--release/-O3)      │
│  ─────────────────   │     ────────────────────────     │
│  Backend: Cranelift  │     Backend: LLVM                │
│  Optimizations: 0    │     Optimizations: -O3 -flto     │
│  Speed: Fast         │     Speed: Slow                  │
│  Output: Debug info  │     Output: Optimized binary     │
└─────────────────────────────────────────────────────────┘
```

### 2.2 CLI Interface Changes

```bash
# Existing commands (unchanged behavior)
stasis run samples/basic.stasis       # Uses Cranelift (fast)
stasis test samples/tests.stasis      # Uses Cranelift (fast)
stasis build samples/basic.stasis     # Uses Cranelift (fast)
stasis release samples/basic.stasis   # Uses LLVM -O3 -flto

# New flags
stasis build --backend=cranelift ...  # Force Cranelift
stasis build --backend=llvm ...       # Force LLVM
stasis run --backend=llvm ...         # Force LLVM for run
stasis test --backend=both ...        # Run tests on both backends
```

Defaults fall back to LLVM when the Cranelift AOT tool is unavailable (or on non-Windows), unless `--backend=cranelift` is explicitly set.

### 2.3 Code Organization

```
Stasis.Compiler/
├── IR/
│   ├── ICodeGenerator.cs           # NEW: Backend interface
│   ├── CodeGeneratorFactory.cs     # NEW: Backend selection
│   ├── ModuleLowerer.cs            # REFACTOR: Use ICodeGenerator
│   │
│   ├── LLVM/                       # NEW: LLVM-specific code
│   │   ├── LlvmCodeGenerator.cs    # Implements ICodeGenerator
│   │   ├── LlvmModuleBuilder.cs    # MOVE from IR/
│   │   ├── LlvmTypeMapper.cs       # MOVE from IR/
│   │   └── LlvmFunctionEmitter.cs  # EXTRACT from ModuleLowerer
│   │
│   └── Cranelift/                  # NEW: Cranelift-specific code
│       ├── CraneliftCodeGenerator.cs
│       ├── CraneliftModuleBuilder.cs
│       ├── CraneliftTypeMapper.cs
│       └── CraneliftFunctionEmitter.cs
```

---

## 3. Backend Abstraction Design

### 3.1 Core Interface

```csharp
namespace Stasis.Compiler.IR;

/// <summary>
/// Abstraction for code generation backends (LLVM, Cranelift, etc.)
/// </summary>
public interface ICodeGenerator : IDisposable
{
    /// <summary>
    /// Unique identifier for this backend.
    /// </summary>
    string BackendName { get; }

    /// <summary>
    /// Generates executable code from analyzed Stasis source.
    /// </summary>
    CodeGenerationResult Generate(
        CompilationUnitSyntax compilationUnit,
        SemanticResult semanticResult,
        LayoutPlan layout,
        CodeGenerationOptions options);

    /// <summary>
    /// Returns the intermediate representation as a string (for debugging).
    /// </summary>
    string EmitIrString();
}

public record CodeGenerationOptions(
    string ModuleName = "module",
    bool IncludeTests = true,
    bool EmitTestHarness = true,
    bool HeadlessGraphics = true,
    OptimizationLevel Optimization = OptimizationLevel.None,
    bool AllowReachabilityFallback = true);

public enum OptimizationLevel
{
    None = 0,
    Basic = 1,
    Standard = 2,
    Aggressive = 3,
    Size = 4,
    MinSize = 5
}

public record CodeGenerationResult(
    byte[] ObjectCode,           // Compiled object file
    string? IrString,            // Optional IR for debugging
    IReadOnlyList<Diagnostic> Diagnostics);
```

### 3.2 Type Mapper Interface

```csharp
/// <summary>
/// Maps Stasis types to backend-specific type representations.
/// </summary>
public interface ITypeMapper<TType>
{
    TType MapPrimitive(PrimitiveTypeSymbol type);
    TType MapArray(ArrayTypeSymbol type);
    TType MapNamed(NamedTypeSymbol type);
    TType MapVoid();
    TType MapPointer(TType elementType);
}
```

### 3.3 Function Emitter Interface

```csharp
/// <summary>
/// Emits function bodies for a specific backend.
/// </summary>
public interface IFunctionEmitter
{
    void EmitFunction(FunctionDeclarationSyntax function, Symbol symbol);
    void EmitTest(TestDeclarationSyntax test, Symbol symbol);
    void EmitGlobal(GlobalDeclarationSyntax global, Symbol symbol);
    void EmitConstant(ConstDeclarationSyntax constant, Symbol symbol);
}
```

### 3.4 Backend Factory

```csharp
public enum BackendType { Cranelift, Llvm }

public static class CodeGeneratorFactory
{
    public static ICodeGenerator Create(BackendType backend, string moduleName)
    {
        return backend switch
        {
            BackendType.Cranelift => new CraneliftCodeGenerator(moduleName),
            BackendType.Llvm => new LlvmCodeGenerator(moduleName),
            _ => throw new ArgumentException($"Unknown backend: {backend}")
        };
    }

    public static BackendType GetDefaultBackend(bool isRelease)
        => isRelease ? BackendType.Llvm : BackendType.Cranelift;
}
```

---

## 4. Cranelift Integration

### 4.1 Cranelift Overview

Cranelift is a code generator designed for:

- Fast compilation (JIT-friendly)
- Reasonable code quality
- Portability (written in Rust)

### 4.2 C# Bindings

**Option A: cranelift-jit-sys (FFI)**

- Use P/Invoke to call Cranelift from C#
- Requires native library distribution
- Most mature approach

**Option B: wasmtime-dotnet**

- Wasmtime includes Cranelift internally
- Compile Stasis → Wasm → Native via Wasmtime
- Simpler but indirect

**Option C: Custom Rust wrapper**

- Create thin Rust wrapper exposing C API
- Full control over Cranelift features
- More maintenance burden

**Recommended: Option A (cranelift-jit-sys FFI)**

Decision: Use Option A

### 4.3 Native Library Distribution

```
Stasis.Compiler/
├── runtimes/
│   ├── win-x64/native/cranelift.dll
│   ├── linux-x64/native/libcranelift.so
│   └── osx-x64/native/libcranelift.dylib
```

Add to `Stasis.Compiler.csproj`:

```xml
<ItemGroup>
  <None Include="runtimes/**/*" Pack="true" PackagePath="runtimes" />
</ItemGroup>
```

### 4.4 Cranelift IR Generation

Cranelift uses its own IR (CLIF):

```clif
function u0:0() -> i32 system_v {
block0:
    v0 = iconst.i32 42
    return v0
}
```

Key Cranelift concepts:

- `Module` - compilation unit
- `FunctionBuilder` - builds function IR
- `Block` - basic block
- `Value` - SSA value
- `Inst` - instruction

### 4.5 Type Mapping

| Stasis Type | Cranelift Type |
| ----------- | -------------- |
| bool        | i8             |
| u8          | i8             |
| u16         | i16            |
| u32         | i32            |
| i32         | i32            |
| f32         | f32            |
| f64         | f64            |
| string      | i64 (pointer)  |
| void        | -              |
| array       | i64 (pointer)  |
| struct ref  | i32 (index)    |

---

## 5. Testing Strategy

### 5.1 Backend Conformance Tests

Create a new test category that runs the same tests on both backends:

```csharp
[Theory]
[InlineData(BackendType.Llvm)]
[InlineData(BackendType.Cranelift)]
public void ArithmeticOperations_ProduceSameResults(BackendType backend)
{
    var source = @"
        function add(a: i32, b: i32): i32 {
            return a + b;
        }
        test `addition works`(): bool {
            return add(2, 3) == 5;
        }
    ";

    var result = CompileAndRun(source, backend);
    Assert.True(result.TestsPassed);
}
```

### 5.2 Language Feature Coverage

All features must work identically on both backends:

**Primitives & Literals**

- [ ] Integer types (u8, u16, u32, i32)
- [ ] Float types (f32, f64)
- [ ] Boolean type
- [ ] String literals

**Operators**

- [ ] Arithmetic: +, -, \*, /, %
- [ ] Comparison: <, <=, >, >=, ==, !=
- [ ] Logical: &&, ||, !
- [ ] Assignment: =, +=, -=, \*=, /=, %=
- [ ] Unary: -, !

**Control Flow**

- [ ] If/else statements
- [ ] For loops (C-style)
- [ ] Foreach loops (with element/index)
- [ ] Return statements
- [ ] Early returns

**Data Structures**

- [ ] Structs (SoA transformation)
- [ ] Enums (type-safe)
- [ ] Fixed-size arrays
- [ ] Global variables
- [ ] Constants
- [ ] Nested struct access
- [ ] Array indexing

**Functions**

- [ ] Function declarations
- [ ] Function calls
- [ ] Parameters
- [ ] Return values
- [ ] Recursion
- [ ] Test functions

**Built-in Functions**

- [ ] I/O: print_string, print, print_int, read_char, read_int
- [ ] Math: sin, cos, sin_fast, cos_fast
- [ ] Time: time, get_time_ms, sleep_ms
- [ ] String operations (30+ functions)
- [ ] Character utilities (15+ functions)

**Advanced Features**

- [ ] Member access (dot notation)
- [ ] Array length property
- [ ] Method-style operators (.+, .-, etc.)
- [ ] Graphics runtime integration

### 5.3 Test Organization

```
Stasis.Compiler.Tests/
├── BackendConformanceTests/
│   ├── ArithmeticTests.cs
│   ├── ControlFlowTests.cs
│   ├── DataStructureTests.cs
│   ├── FunctionTests.cs
│   ├── BuiltinTests.cs
│   └── AdvancedFeatureTests.cs
├── LlvmSpecificTests/
│   └── OptimizationTests.cs
└── CraneliftSpecificTests/
    └── JitTests.cs
```

### 5.4 Performance Benchmarks

Track both compilation speed and execution speed:

```csharp
[Benchmark]
public void CompileTime_Cranelift() => Compile(source, BackendType.Cranelift);

[Benchmark]
public void CompileTime_Llvm() => Compile(source, BackendType.Llvm);

[Benchmark]
public void ExecutionTime_Cranelift() => Execute(source, BackendType.Cranelift);

[Benchmark]
public void ExecutionTime_Llvm() => Execute(source, BackendType.Llvm);
```

---

## 6. Implementation Phases

### Phase 1: Backend Abstraction (Week 1-2)

**Goals:**

- Extract backend interface from existing LLVM code
- No functional changes to compilation

**Tasks:**

1. Create `ICodeGenerator` interface
2. Create `ITypeMapper<T>` interface
3. Create `IFunctionEmitter` interface
4. Extract LLVM code into `IR/LLVM/` directory
5. Create `LlvmCodeGenerator` implementing `ICodeGenerator`
6. Update `ModuleLowerer` to use abstraction
7. Ensure all existing tests pass

**Deliverables:**

- Clean separation between frontend and LLVM backend
- No breaking changes to CLI or tests

### Phase 2: Native Cranelift (Windows x64) (Week 3-4)

**Goal:** Produce a native Windows x64 executable with no WASM runtime, as close as possible to the existing LLVM `clang` link path (including the C graphics runtime wrapper).

**Approach (AOT):**

```
Source (.stasis)
  -> Lexer/Parser/Semantics/Layout (C#)
  -> Cranelift lowering (C#)
  -> tools/cranelift-aot (Rust): Cranelift -> COFF .obj (x86_64-pc-windows-msvc)
  -> clang/lld-link: .obj + CRT + runtime libs -> .exe
```

**Key constraints:**

- Windows x64 only initially (`x86_64-pc-windows-msvc`).
- No WASM: executable must be a normal `.exe` produced by linking a `.obj`.
- Graphics integration uses the same runtime library (`stasis_graphics.lib`) as the LLVM backend.

**Tasks:**

1. Implement `tools/cranelift-aot` (Rust):
   - Input: a lowered representation (short-term: CLIF text; longer-term: stable serialized IR).
   - Output: COFF `.obj` with correct relocations and exported symbols.
2. Ensure correct Windows ABI:
   - Functions intended to be called by the CRT must use Windows x64 calling convention.
   - CLIF must use `windows_fastcall` (not `system_v`) when targeting Windows.
3. Wire `Stasis.Cli`:
   - `stasis build/run --backend=cranelift` should no longer force `--emit-ir`.
   - For `build`/`run`, call `tools/cranelift-aot` to produce `.obj`, then link with `clang` like LLVM.
4. Start with a minimal runnable subset:
   - `main(): i32` programs with arithmetic/control-flow and calls within the module.
   - No built-ins and no globals at first; then add them incrementally.
5. Add smoke tests:
   - A Windows-only test that compiles `samples/basic.stasis` with Cranelift, links an `.exe`, runs it, and asserts exit code.

**Deliverables:**

- `stasis run samples/basic.stasis --backend=cranelift` runs and returns the expected exit code.
- `stasis build samples/basic.stasis --backend=cranelift` emits a runnable `.exe` (Windows x64).

**Risks / gotchas:**

- Calling convention mismatch (`system_v` vs `windows_fastcall`) can cause runtime crashes even if linking succeeds.
- External symbol naming and import libraries must match the Windows COFF toolchain expectations.
- `test` mode currently uses `/entry:run_tests` on Windows; for Cranelift we may need a small CRT-aware entry stub for reliability.

**Current status (2025-12-18):**

✅ **Phase 2 Complete - Basic Native Execution Works:**
- Implemented AOT tool: `tools/cranelift-aot` (Rust) compiles CLIF text to COFF `.obj` using Cranelift.
- CLI wiring (Windows only): `stasis run/build --backend cranelift` does CLIF → `.obj` → `clang` link → `.exe`.
- End-to-end execution: `samples/basic.stasis` runs and returns correct exit code.
- Calling convention: `windows_fastcall` for Windows x64 compatibility.
- Native DLL runner for Cranelift `run/test`: `stasis_runner` loads compiled DLLs to avoid relinking the host exe.

**Working Language Features (~20-30% parity with LLVM):**
- ✅ Arithmetic operations (+, -, *, /, %)
- ✅ Comparison operations (<, <=, >, >=, ==, !=)
- ✅ Logical operations (&&, ||, !)
- ✅ Control flow (if/else, for loops, return)
- ✅ Function calls (intra-module)
- ✅ Local variables and reassignment (SSA-based)
- ✅ Function parameters
- ✅ Integer/float/boolean literals
- ✅ Test harness (run_tests entry point + PASS/FAIL summary)

**Remaining Gaps (blocking full parity):**
- ❌ **Array allocation/initialization** - Arrays require explicit allocation/initialization work
- ❌ **Remaining built-ins** - Math + advanced string helpers (trim/case/num conversions)
- ✅ **Foreach loops** - Lowered to indexed loops with element/index bindings
- ✅ **Test-time reporting** - Harness prints elapsed time per test run

**Architecture Quality:**
- Clean separation: CraneliftCodeGenerator, CraneliftModuleBuilder, CraneliftFunctionBuilder, CraneliftTypeMapper
- SSA value tracking works correctly
- CLIF text generation is readable and debuggable
- AOT tool is extensible (just needs more instruction support)

**How to build/run (Windows x64):**

1. Build the AOT tool: `cd tools/cranelift-aot && cargo build --release`
2. Run a sample: `dotnet run --project Stasis.Cli -- run samples/basic.stasis --backend cranelift`
3. Optional: set `STASIS_CRANELIFT_AOT` to point at `stasis-cranelift-aot.exe` if discovery fails.

**Next steps (Roadmap to Full Execution - Phase 2.5):**

**Priority 1: Memory & Global Variables (Week 1)**
- [x] Add `global_value` instruction support to AOT tool
- [x] Add `load` and `store` instruction support to AOT tool
- [x] Implement global variable declarations with proper memory allocation
- [x] Wire up global variable loads in CraneliftFunctionBuilder (replace TODO at line 332)
- [x] Wire up global variable stores in CraneliftFunctionBuilder (replace TODO at line 449)
- [ ] Test: Global variable read/write in samples/basic.stasis

**Priority 2: Built-in Functions (Week 1-2)**
- [x] Add external function declaration support in CraneliftModuleBuilder
- [x] Implement `print_int` built-in (critical for debugging)
- [x] Implement `print_string` built-in (requires string literal support)
- [x] Implement `read_int` built-in (stack slot + scanf)
- [x] Implement `read_char` built-in (stack slot + scanf)
- [x] Implement time functions: `get_time_ms`, `sleep_ms`, `time`
- [x] Implement Sudoku helpers (`print_prompt`, `print_invalid`, `print_clue_error`, `print_solved`, `print_cell`)
- [x] Implement directory list helpers (`list_directory`, `dir_list_entry_is_dir`, `dir_list_entry_copy_name`)
- [x] Implement `char_*` helpers (classification + conversion)
- [x] Support unsigned `icmp` conditions in the AOT tool (ult/ule/ugt/uge)
- [x] Fix `get_window_size` out-parameter lowering
- [x] Pass array/string arguments by pointer in external calls
- [ ] Test: Sample program that prints and reads values

**Priority 3: String Support (Week 2)**
- [x] Implement string literal storage (global data section)
- [x] Wire up string literal loads in CraneliftFunctionBuilder (replace TODO at line 308)
- [x] Emit UTF-8 headers (byte_length + char_length) for literals and string buffers
- [x] Update string helpers to read/write header lengths instead of strlen
- [x] Implement basic string built-ins (strlen, strcmp, strcpy, strncmp, strcat, strchr, strrchr, strstr)
- [x] Implement advanced string built-in `str_substr`
- [ ] Implement remaining advanced string built-ins (trim, case transform, numeric conversions)
- [ ] Test: Hello world with string printing

**Priority 4: Arrays & SoA Layout (Week 2-3)**
- [ ] Study LayoutPlanner output format and SoA transformation
- [x] Implement array access with SoA offset calculation (replace TODO at line 483)
- [x] Implement array length property (replace TODO at line 467)
- [x] Support local/parameter array element access
- [ ] Add array allocation and initialization
- [ ] Test: Fibonacci with array storage

**Priority 5: Struct Member Access (Week 3)**
- [x] Implement struct field access with SoA transformation (replace TODO at line 472)
- [ ] Add nested struct/array access support
- [ ] Test: Struct creation and field access

**Priority 6: Test Harness (Week 3)**
- [x] Generate `run_tests` entry point function
- [x] Implement test result collection and reporting (PASS/FAIL + summary counts)
- [x] Add test-time reporting to Cranelift harness
- [x] Remove `--emit-ir` forcing in CLI for test mode
- [x] Route `stasis test --all` through the Cranelift harness when using the Cranelift backend
- [x] Run/test with Cranelift via the native DLL runner (`stasis_runner`)
- [x] Test: `stasis test samples/fib_tests.stasis --backend cranelift`

**Priority 7: Advanced Features (Week 4)**
- [x] Graphics integration (SDL2/OpenGL calls)
- [ ] Remaining math built-ins (sin, cos, sqrt, etc.)
- [x] Foreach loop support (currently only for loops work)
- [x] Compound assignment to complex l-values

**Priority 8: Polish & Optimization (Week 4)**
- [x] Resolve CRT link warnings (LNK4098)
- [x] Front-end reachability DCE (entrypoints: main/export/test builds)
- [x] Replace Cranelift TODO fallbacks with explicit diagnostics
- [ ] Use bulk memory operations where possible (prefer mem_copy/mem_set style loops or intrinsics over per-byte helpers)
- [ ] Add compilation time benchmarks
- [ ] Improve error diagnostics for Cranelift-specific issues
- [ ] Make IR boundary more robust (consider binary format vs CLIF text)

**Priority 9: Iteration & Hot Reload (Week 5)**

**Goal:** Enable fast edit/reload loops for running samples, aiming for <100ms compile+run on small edits.

- [x] CLI `--watch` mode restarts run/test on file changes (process-based reload)
- [ ] Use a stable host process with hot-swapped logic DLLs (no process restart)
- [ ] Reduce link time for Cranelift DLLs (lld fast-link profile, fewer inputs)
- [ ] Optional in-memory loading path to avoid disk writes for test-only runs
- [ ] Track per-phase timings in watch mode and report P90/P95 compile+run latency

**Current bottleneck:** link time dominates larger samples; optimize link surface area and reuse host process to hit the 100ms goal.

**Definition of "Full Execution":**
After Priority 1-6 are complete, the Cranelift backend should be able to:
- Run all basic samples (basic.stasis, fib_tests.stasis, operators.stasis)
- Execute test harnesses with `stasis test`
- Support programs with globals, arrays, structs, and built-in I/O
- Achieve ~70-80% feature parity with LLVM (excluding graphics/advanced math)

### Phase 3: Feature Parity (Week 5-8)

**Goals:**

- Implement all language features in Cranelift
- Full test coverage on both backends

**Tasks:**

1. Implement arithmetic operations
2. Implement comparison and logical operations
3. Implement control flow (if/else, for, foreach)
4. Implement function calls
5. Implement struct/array access (SoA)
6. Implement built-in functions
7. Implement test harness
8. Create conformance test suite

**Deliverables:**

- All existing samples compile with Cranelift
- Backend conformance tests passing

### Phase 4: Build Mode Integration (Week 9-10)

**Goals:**

- Automatic backend selection based on build mode
- Polish CLI experience

**Tasks:**

1. ✅ Set Cranelift as default for `run`, `test`, `build` (fallback to LLVM when Cranelift AOT is unavailable)
2. ✅ Set LLVM as default for `release`
3. Implement `--backend=both` for testing
4. Add compilation time metrics
5. Update documentation
6. Performance benchmarks

**Deliverables:**

- Transparent backend selection
- Updated docs and README

### Phase 5: Optimization & Polish (Week 11-12)

**Goals:**

- Performance optimization
- Edge case handling

**Tasks:**

1. Profile and optimize Cranelift compilation speed
2. Handle edge cases (large functions, complex SoA)
3. Improve error messages for backend-specific issues
4. Final documentation pass
5. Release preparation

**Deliverables:**

- Production-ready dual-backend compiler
- Complete documentation

---

## 7. Risk Assessment

| Risk                            | Likelihood | Impact | Mitigation                           |
| ------------------------------- | ---------- | ------ | ------------------------------------ |
| Cranelift C# bindings immature  | Medium     | High   | Evaluate wasmtime-dotnet as fallback |
| SoA transformation complexity   | Medium     | Medium | Reuse existing layout logic          |
| Built-in function compatibility | Low        | Medium | Implement same signatures            |
| Performance regression          | Low        | Low    | Benchmark early and often            |
| Platform support gaps           | Medium     | Medium | Start with Windows, expand           |

---

## 8. Success Criteria

1. **Compilation Speed**: Cranelift builds complete 5x faster than LLVM builds
2. **Feature Parity**: All language features work on both backends
3. **Test Coverage**: 100% of existing tests pass on both backends
4. **User Experience**: Default build mode is noticeably faster
5. **Code Quality**: Clean abstraction allows future backends

---

## 9. Dependencies

### NuGet Packages (Tentative)

```xml
<!-- Existing -->
<PackageReference Include="LLVMSharp" Version="20.1.2" />

<!-- New for Cranelift -->
<PackageReference Include="Wasmtime" Version="22.0.0" />
<!-- OR custom P/Invoke bindings -->
```

### Native Libraries

- `cranelift.dll` / `libcranelift.so` / `libcranelift.dylib`
- May be bundled with wasmtime if using that approach

---

## 10. Open Questions

1. **Cranelift binding approach**: FFI vs Wasmtime vs custom wrapper?
   Answer: Custom wrapper, like used for llvm backend
2. **Debug info format**: DWARF support in Cranelift?
   Answer: no
3. **Graphics runtime**: Same linking approach for both backends?
   Answer: yes
4. **JIT mode**: Should Cranelift support runtime compilation?
   Answer: No
5. **Cross-compilation**: Target different platforms?
   Answer: Eventually, not yet.

---

## Appendix A: Sample Cranelift Code

```rust
// What Cranelift IR looks like for a simple function
fn codegen_add() {
    let mut module = JITModule::new(default_libcall_names());
    let mut ctx = module.make_context();
    let mut func_ctx = FunctionBuilderContext::new();

    // function add(a: i32, b: i32) -> i32
    ctx.func.signature.params.push(AbiParam::new(types::I32));
    ctx.func.signature.params.push(AbiParam::new(types::I32));
    ctx.func.signature.returns.push(AbiParam::new(types::I32));

    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut func_ctx);
    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);

    let a = builder.block_params(block)[0];
    let b = builder.block_params(block)[1];
    let result = builder.ins().iadd(a, b);
    builder.ins().return_(&[result]);

    builder.finalize();
}
```

---

## Appendix B: LLVM vs Cranelift IR Comparison

**Stasis Source:**

```stasis
function add(a: i32, b: i32): i32 {
    return a + b;
}
```

**LLVM IR:**

```llvm
define i32 @add(i32 %a, i32 %b) {
entry:
    %result = add i32 %a, %b
    ret i32 %result
}
```

**Cranelift IR (CLIF):**

```clif
function %add(i32, i32) -> i32 system_v {
block0(v0: i32, v1: i32):
    v2 = iadd v0, v1
    return v2
}
```

---

## Revision History

| Date       | Version | Author | Changes      |
| ---------- | ------- | ------ | ------------ |
| 2024-12-18 | 1.0     | Claude | Initial plan |
