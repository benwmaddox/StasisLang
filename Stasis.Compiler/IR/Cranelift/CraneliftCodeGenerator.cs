using System.Text;
using Stasis.Compiler.Layout;
using Stasis.Compiler.Semantic;
using Stasis.Compiler.Syntax;

namespace Stasis.Compiler.IR.Cranelift;

/// <summary>
/// Cranelift-based code generator implementation.
///
/// Status: SCAFFOLDING - generates CLIF text but does not produce executable code yet.
///
/// Cranelift is a fast code generator designed for JIT compilation.
/// This implementation will provide faster compilation times for debug builds
/// compared to LLVM.
///
/// Implementation roadmap:
/// 1. [Current] Generate CLIF text representation
/// 2. Add native Cranelift bindings via P/Invoke or wasmtime-dotnet
/// 3. Implement JIT compilation to native code
/// 4. Full feature parity with LLVM backend
/// </summary>
public sealed class CraneliftCodeGenerator : ICodeGenerator
{
    private readonly string _moduleName;
    private string _lastIr = string.Empty;
    private bool _disposed;

    public CraneliftCodeGenerator(string moduleName = "module")
    {
        _moduleName = moduleName;
    }

    /// <inheritdoc />
    public string BackendName => "cranelift";

    /// <inheritdoc />
    public CodeGenerationResult Generate(
        CompilationUnitSyntax compilationUnit,
        SemanticResult semanticResult,
        LayoutPlan layout,
        CodeGenerationOptions options)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        var diagnostics = new List<Diagnostic>();

        try
        {
            using var builder = new CraneliftModuleBuilder(_moduleName);

            var reachableFunctions = Reachability.CollectReachableFunctions(compilationUnit, options.IncludeTests, options.AllowReachabilityFallback);
            var (builtins, stringLiterals) = CollectLoweringNeeds(compilationUnit, options.IncludeTests, reachableFunctions);
            if (options.IncludeTests && options.EmitTestHarness)
            {
                builtins.Add("run_tests");
            }

            // Declare external functions (C runtime) only when needed
            DeclareExternalFunctions(builder, builtins);
            DeclareExternFunctionsFromSource(builder, compilationUnit, semanticResult.Symbols, reachableFunctions);

            // Define string literals referenced by the program
            foreach (var literal in stringLiterals)
            {
                builder.DefineStringLiteral(literal);
            }

            // Emit globals
            EmitGlobals(compilationUnit, semanticResult.Symbols, layout, builder);

            // Emit functions with bodies
            EmitFunctions(compilationUnit, semanticResult.Symbols, builder, diagnostics, layout, options.IncludeTests, options.EmitTestHarness, reachableFunctions, _moduleName);

            // Generate CLIF text
            _lastIr = builder.EmitToString();

            return new CodeGenerationResult(_lastIr, diagnostics);
        }
        catch (Exception ex)
        {
            diagnostics.Add(new Diagnostic($"Cranelift code generation failed: {ex.Message}", new SourceSpan(0, 0)));
            return CodeGenerationResult.Fail(diagnostics);
        }
    }

    /// <inheritdoc />
    public string EmitIrString() => _lastIr;

    /// <inheritdoc />
    public void Dispose()
    {
        _disposed = true;
    }

    private static void DeclareExternalFunctions(CraneliftModuleBuilder builder, IReadOnlySet<string> builtins)
    {
        // Declare C standard library functions
        // Since Cranelift doesn't support variadic functions, we declare multiple signatures

            if (builtins.Overlaps(new[]
                {
                    "print_int", "print_char", "print_string",
                    "print_prompt", "print_invalid", "print_clue_error", "print_solved", "print_cell",
                    "run_tests"
                }))
            {
                // printf3(format: *i8, arg1: i64, arg2: i64) -> i32 (aliased to printf in AOT)
                builder.DeclareExternal("printf3", CraneliftTypeMapper.ClifType.I32,
                    CraneliftTypeMapper.ClifType.I64,
                    CraneliftTypeMapper.ClifType.I64,
                    CraneliftTypeMapper.ClifType.I64);
            }

        if (builtins.Contains("run_tests"))
        {
            // time(tloc: *i64) -> i64
            builder.DeclareExternal("time", CraneliftTypeMapper.ClifType.I64,
                CraneliftTypeMapper.ClifType.I64);
            // clock() -> i64
            builder.DeclareExternal("clock", CraneliftTypeMapper.ClifType.I64);
        }


        if (builtins.Contains("read_int") || builtins.Contains("read_char"))
        {
            // scanf(format: *i8, ptr: *i64) -> i32
            builder.DeclareExternal("scanf", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I64,  // format string pointer
                CraneliftTypeMapper.ClifType.I64); // pointer to result
        }

        if (builtins.Contains("sys_argc"))
        {
            builder.DeclareExternal("stasis_sys_argc", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("sys_argv"))
        {
            builder.DeclareExternal("stasis_sys_argv", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32, // idx
                CraneliftTypeMapper.ClifType.R64, // out
                CraneliftTypeMapper.ClifType.I32  // out_cap
            );
        }

        if (builtins.Contains("sys_read_file"))
        {
            builder.DeclareExternal("stasis_sys_read_file", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64, // path
                CraneliftTypeMapper.ClifType.R64, // out
                CraneliftTypeMapper.ClifType.I32  // out_cap
            );
        }

        if (builtins.Contains("sys_list_dir"))
        {
            builder.DeclareExternal("stasis_sys_list_dir", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64, // path
                CraneliftTypeMapper.ClifType.R64, // out
                CraneliftTypeMapper.ClifType.I32  // out_cap
            );
        }

        if (builtins.Contains("sys_write_file"))
        {
            builder.DeclareExternal("stasis_sys_write_file", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64, // path
                CraneliftTypeMapper.ClifType.R64, // data
                CraneliftTypeMapper.ClifType.I32  // len
            );
        }

        if (builtins.Contains("sys_file_exists"))
        {
            builder.DeclareExternal("stasis_sys_file_exists", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // path
            );
        }

        if (builtins.Contains("sys_file_size"))
        {
            builder.DeclareExternal("stasis_sys_file_size", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // path
            );
        }

        if (builtins.Contains("sys_file_mtime_ms"))
        {
            builder.DeclareExternal("stasis_sys_file_mtime_ms", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // path
            );
        }

        if (builtins.Contains("sys_exec"))
        {
            builder.DeclareExternal("stasis_sys_exec", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // command
            );
        }

        if (builtins.Contains("sys_spawn"))
        {
            builder.DeclareExternal("stasis_sys_spawn", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // command_line
            );
        }

        if (builtins.Contains("sys_spawn_async"))
        {
            builder.DeclareExternal("stasis_sys_spawn_async", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // command_line
            );
        }

        if (builtins.Contains("sys_sleep_ms"))
        {
            builder.DeclareExternal("stasis_sys_sleep_ms", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32 // ms
            );
        }

        if (builtins.Contains("sys_delete_file"))
        {
            builder.DeclareExternal("stasis_sys_delete_file", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64 // path
            );
        }

        if (builtins.Contains("sys_time_ms"))
        {
            builder.DeclareExternal("stasis_sys_time_ms", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("sys_flush"))
        {
            builder.DeclareExternal("stasis_sys_flush", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("sys_memcpy_u8"))
        {
            builder.DeclareExternal("stasis_sys_memcpy_u8", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64, // dst
                CraneliftTypeMapper.ClifType.I32, // dst_index
                CraneliftTypeMapper.ClifType.R64, // src
                CraneliftTypeMapper.ClifType.I32, // src_index
                CraneliftTypeMapper.ClifType.I32  // count
            );
        }

        if (builtins.Contains("sys_memcpy_i32"))
        {
            builder.DeclareExternal("stasis_sys_memcpy_i32", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64, // dst
                CraneliftTypeMapper.ClifType.I32, // dst_index
                CraneliftTypeMapper.ClifType.R64, // src
                CraneliftTypeMapper.ClifType.I32, // src_index
                CraneliftTypeMapper.ClifType.I32  // count
            );
        }

        if (builtins.Contains("sys_memcpy_f32"))
        {
            builder.DeclareExternal("stasis_sys_memcpy_f32", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64, // dst
                CraneliftTypeMapper.ClifType.I32, // dst_index
                CraneliftTypeMapper.ClifType.R64, // src
                CraneliftTypeMapper.ClifType.I32, // src_index
                CraneliftTypeMapper.ClifType.I32  // count
            );
        }

        if (builtins.Contains("sys_memmove_u8"))
        {
            builder.DeclareExternal("stasis_sys_memmove_u8", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64, // dst
                CraneliftTypeMapper.ClifType.I32, // dst_index
                CraneliftTypeMapper.ClifType.R64, // src
                CraneliftTypeMapper.ClifType.I32, // src_index
                CraneliftTypeMapper.ClifType.I32  // count
            );
        }

        if (builtins.Contains("sys_memmove_i32"))
        {
            builder.DeclareExternal("stasis_sys_memmove_i32", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64, // dst
                CraneliftTypeMapper.ClifType.I32, // dst_index
                CraneliftTypeMapper.ClifType.R64, // src
                CraneliftTypeMapper.ClifType.I32, // src_index
                CraneliftTypeMapper.ClifType.I32  // count
            );
        }

        if (builtins.Contains("sys_memmove_f32"))
        {
            builder.DeclareExternal("stasis_sys_memmove_f32", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64, // dst
                CraneliftTypeMapper.ClifType.I32, // dst_index
                CraneliftTypeMapper.ClifType.R64, // src
                CraneliftTypeMapper.ClifType.I32, // src_index
                CraneliftTypeMapper.ClifType.I32  // count
            );
        }

        // sys_memset_* is intentionally not exposed as a Stasis builtin; treat bulk clears as a compiler/runtime detail.

        if (builtins.Contains("time"))
        {
            // time(tloc: *i64) -> i64
            builder.DeclareExternal("time", CraneliftTypeMapper.ClifType.I64,
                CraneliftTypeMapper.ClifType.I64);
        }

        if (builtins.Contains("get_time_ms"))
        {
            // stasis_get_time_ms() -> i32
            builder.DeclareExternal("stasis_get_time_ms", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("get_time_us"))
        {
            // stasis_get_time_us() -> i32
            builder.DeclareExternal("stasis_get_time_us", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("sleep_ms"))
        {
            // stasis_sleep_ms(ms: i32) -> void
            builder.DeclareExternal("stasis_sleep_ms", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("audio_is_available"))
        {
            builder.DeclareExternal("stasis_audio_is_available", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("audio_get_sample_rate"))
        {
            builder.DeclareExternal("stasis_audio_get_sample_rate", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("audio_get_channels"))
        {
            builder.DeclareExternal("stasis_audio_get_channels", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("audio_get_queued_frames"))
        {
            builder.DeclareExternal("stasis_audio_get_queued_frames", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("audio_get_underruns"))
        {
            builder.DeclareExternal("stasis_audio_get_underruns", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("audio_push_f32_interleaved"))
        {
            builder.DeclareExternal("stasis_audio_push_f32_interleaved", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Overlaps(new[] { "sin", "sin_fast" }))
        {
            builder.DeclareExternal("sinf", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32);
        }

        if (builtins.Overlaps(new[] { "cos", "cos_fast" }))
        {
            builder.DeclareExternal("cosf", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32);
        }

        if (builtins.Contains("init_window"))
        {
            // stasis_init_window(width: i32, height: i32, title: *i8) -> i32
            builder.DeclareExternal("stasis_init_window", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("begin_frame"))
        {
            builder.DeclareExternal("stasis_begin_frame", CraneliftTypeMapper.ClifType.Void);
        }

        if (builtins.Contains("end_frame"))
        {
            builder.DeclareExternal("stasis_end_frame", CraneliftTypeMapper.ClifType.Void);
        }

        if (builtins.Contains("clear"))
        {
            builder.DeclareExternal("stasis_clear", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32);
        }

        if (builtins.Contains("draw_line"))
        {
            builder.DeclareExternal("stasis_draw_line", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32);
        }

        if (builtins.Contains("draw_lines_f32"))
        {
            builder.DeclareExternal("stasis_draw_lines_f32", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("host_get_frame"))
        {
            builder.DeclareExternal("stasis_host_get_frame", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("gfx_load_sprite"))
        {
            builder.DeclareExternal("stasis_gfx_load_sprite", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64,  // path
                CraneliftTypeMapper.ClifType.I32,  // max_w
                CraneliftTypeMapper.ClifType.I32); // max_h
        }

        if (builtins.Contains("gfx_draw_sprite"))
        {
            builder.DeclareExternal("stasis_gfx_draw_sprite", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.I32,  // handle
                CraneliftTypeMapper.ClifType.I32,  // x
                CraneliftTypeMapper.ClifType.I32,  // y
                CraneliftTypeMapper.ClifType.I32,  // w
                CraneliftTypeMapper.ClifType.I32,  // h
                CraneliftTypeMapper.ClifType.I32,  // rot_degrees
                CraneliftTypeMapper.ClifType.I32); // a
        }

        if (builtins.Contains("gfx_draw_sprites_i32"))
        {
            builder.DeclareExternal("stasis_gfx_draw_sprites_i32", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("gfx_submit"))
        {
            builder.DeclareExternal("stasis_gfx_submit", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("gfx_submit_u8"))
        {
            builder.DeclareExternal("stasis_gfx_submit_u8", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("gfx_poll_reload"))
        {
            builder.DeclareExternal("stasis_gfx_poll_reload", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("gfx_window_width"))
        {
            builder.DeclareExternal("stasis_gfx_window_width", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("gfx_window_height"))
        {
            builder.DeclareExternal("stasis_gfx_window_height", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("gfx_window_resized"))
        {
            builder.DeclareExternal("stasis_gfx_window_resized", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("gfx_debug_bake_hash"))
        {
            builder.DeclareExternal("stasis_gfx_debug_bake_hash", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("gfx_debug_enable_hash"))
        {
            builder.DeclareExternal("stasis_gfx_debug_enable_hash", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("gfx_debug_get_frame_hash"))
        {
            builder.DeclareExternal("stasis_gfx_debug_get_frame_hash", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("is_key_down"))
        {
            builder.DeclareExternal("stasis_is_key_down", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("should_quit"))
        {
            builder.DeclareExternal("stasis_should_quit", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_count"))
        {
            builder.DeclareExternal("stasis_input_pointer_count", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_id"))
        {
            builder.DeclareExternal("stasis_input_pointer_id", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_is_down"))
        {
            builder.DeclareExternal("stasis_input_pointer_is_down", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_went_down"))
        {
            builder.DeclareExternal("stasis_input_pointer_went_down", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_went_up"))
        {
            builder.DeclareExternal("stasis_input_pointer_went_up", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_x_px"))
        {
            builder.DeclareExternal("stasis_input_pointer_x_px", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_y_px"))
        {
            builder.DeclareExternal("stasis_input_pointer_y_px", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_dx_px"))
        {
            builder.DeclareExternal("stasis_input_pointer_dx_px", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_dy_px"))
        {
            builder.DeclareExternal("stasis_input_pointer_dy_px", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_x_n"))
        {
            builder.DeclareExternal("stasis_input_pointer_x_n", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_pointer_y_n"))
        {
            builder.DeclareExternal("stasis_input_pointer_y_n", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_dropped_pointers"))
        {
            builder.DeclareExternal("stasis_input_dropped_pointers", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_viewport_x_px"))
        {
            builder.DeclareExternal("stasis_input_viewport_x_px", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_viewport_y_px"))
        {
            builder.DeclareExternal("stasis_input_viewport_y_px", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_viewport_w_px"))
        {
            builder.DeclareExternal("stasis_input_viewport_w_px", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("input_viewport_h_px"))
        {
            builder.DeclareExternal("stasis_input_viewport_h_px", CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("get_window_size"))
        {
            builder.DeclareExternal("stasis_get_window_size", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("set_fullscreen"))
        {
            builder.DeclareExternal("stasis_set_fullscreen", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("set_postfx"))
        {
            builder.DeclareExternal("stasis_set_postfx", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32);
        }

        if (builtins.Contains("load_font"))
        {
            builder.DeclareExternal("stasis_load_font", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.I32);
        }

        if (builtins.Contains("draw_text"))
        {
            builder.DeclareExternal("stasis_draw_text", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.I32, CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.F32, CraneliftTypeMapper.ClifType.F32);
        }

        if (builtins.Contains("measure_text"))
        {
            builder.DeclareExternal("stasis_measure_text", CraneliftTypeMapper.ClifType.F32,
                CraneliftTypeMapper.ClifType.I32, CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("list_directory"))
        {
            builder.DeclareExternal("stasis_list_directory_struct", CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.R64);
        }

        if (builtins.Contains("dir_list_entry_copy_name"))
        {
            builder.DeclareExternal("stasis_copy_dir_entry_name", CraneliftTypeMapper.ClifType.Void,
                CraneliftTypeMapper.ClifType.R64,
                CraneliftTypeMapper.ClifType.I32,
                CraneliftTypeMapper.ClifType.R64);
        }

        var needsStringRuntime = builtins.Overlaps(new[]
            {
                "str_len", "str_is_empty", "str_get", "str_set", "str_eq", "str_cmp",
                "str_copy", "str_append", "str_append_char", "str_clear",
                "str_contains", "str_find", "str_find_char", "str_find_last_char",
                "str_starts_with", "str_ends_with", "str_substr"
            });

        if (needsStringRuntime)
        {
            builder.DeclareExternal("strlen", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strcmp", CraneliftTypeMapper.ClifType.I32, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strncmp", CraneliftTypeMapper.ClifType.I32, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strcpy", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strcat", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("strchr", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I32);
            builder.DeclareExternal("strrchr", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I32);
            builder.DeclareExternal("strstr", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("memcpy", CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64, CraneliftTypeMapper.ClifType.I64);
            builder.DeclareExternal("abort", CraneliftTypeMapper.ClifType.Void);
        }

        if (!needsStringRuntime && builtins.Overlaps(new[] { "i32_to_u8_checked", "i32_to_u16_checked" }))
        {
            builder.DeclareExternal("abort", CraneliftTypeMapper.ClifType.Void);
        }

        if (builtins.Contains("print_int"))
        {
            builder.DefineCStringLiteral(" %d");
        }

        if (builtins.Contains("print_char"))
        {
            builder.DefineCStringLiteral("%c");
        }

        if (builtins.Contains("print_string"))
        {
            builder.DefineCStringLiteral("%s");
        }

        if (builtins.Contains("print_prompt"))
        {
            builder.DefineCStringLiteral("Enter row col val (1-9, 0 clears), or q to quit:\n");
        }

        if (builtins.Contains("print_invalid"))
        {
            builder.DefineCStringLiteral("\u001b[31mInvalid move.\u001b[0m\n");
        }

        if (builtins.Contains("print_clue_error"))
        {
            builder.DefineCStringLiteral("\u001b[31mCannot change a clue.\u001b[0m\n");
        }

        if (builtins.Contains("print_solved"))
        {
            builder.DefineCStringLiteral("\u001b[32mSolved!\u001b[0m\n");
        }

        if (builtins.Contains("print_cell"))
        {
            builder.DefineCStringLiteral(". ");
            builder.DefineCStringLiteral("%s");
            builder.DefineCStringLiteral("%d");
            builder.DefineCStringLiteral("\u001b[36m");
            builder.DefineCStringLiteral("\u001b[32m");
            builder.DefineCStringLiteral("\u001b[0m ");
        }

        if (builtins.Contains("read_int"))
        {
            builder.DefineCStringLiteral("%d");
        }

        if (builtins.Contains("read_char"))
        {
            builder.DefineCStringLiteral(" %c");
        }

    }

    private static void DeclareExternFunctionsFromSource(
        CraneliftModuleBuilder builder,
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        IReadOnlySet<string> reachableFunctions)
    {
        foreach (var func in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            if (!func.IsExtern && !HasExternAttribute(func))
            {
                continue;
            }

            // Avoid emitting extern declarations that are never referenced (Windows COFF AOT can
            // treat declared-but-unused externs as link-time requirements).
            if (!reachableFunctions.Contains(func.Name.Text))
            {
                continue;
            }

            // Runtime-backed builtins are declared separately using their mapped symbol names
            // (e.g., init_window -> stasis_init_window). Declaring the source name would cause
            // unresolved externals on Windows.
            if (IsCraneliftBuiltin(func.Name.Text))
            {
                continue;
            }

            var externName = GetExternLinkName(func) ?? func.Name.Text;
            var returnTypeSymbol = func.ReturnType is null
                ? new VoidTypeSymbol()
                : ResolveType(func.ReturnType, symbols);
            var returnType = NormalizeFunctionType(builder.TypeMapper.Map(returnTypeSymbol));
            var paramTypes = func.Parameters
                .Select(p => NormalizeFunctionType(builder.TypeMapper.Map(ResolveType(p.Type, symbols))))
                .ToArray();

            builder.DeclareExternal(externName, returnType, paramTypes);
        }
    }

    private static bool HasExternAttribute(FunctionDeclarationSyntax func) =>
        func.Attributes.Any(attr => string.Equals(attr.Text, "extern", StringComparison.Ordinal));

    private static string? GetExternLinkName(FunctionDeclarationSyntax func)
    {
        var raw = func.Attributes
            .FirstOrDefault(a => string.Equals(a.Text, "extern", StringComparison.Ordinal))?
            .StringValue;

        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        if (raw.Length >= 2 && raw[0] == '"' && raw[^1] == '"')
        {
            return raw.Substring(1, raw.Length - 2);
        }

        return raw;
    }

    private static void EmitGlobals(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        LayoutPlan layout,
        CraneliftModuleBuilder builder)
    {
        var typeMapper = builder.TypeMapper;
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
        var globalsByName = compilationUnit.Declarations
            .OfType<GlobalDeclarationSyntax>()
            .ToDictionary(g => g.Name.Text, g => g, StringComparer.Ordinal);

        // Emit globals based on LayoutPlan to support SoA transformation
        foreach (var globalLayout in layout.Globals)
        {
            if (!globalsByName.TryGetValue(globalLayout.Name, out var globalDecl))
            {
                continue;
            }

            switch (globalDecl.Type)
            {
                case ArrayTypeSyntax arrayType when arrayType.ElementType is NamedTypeSyntax named &&
                                                   structs.TryGetValue(named.Name, out var structDecl):
                    // Struct array -> SoA fields
                    var structCount = ParseArrayLength(arrayType.SizeToken?.Text);
                    foreach (var field in structDecl.Fields)
                    {
                        var fieldType = ResolveType(field.Type, symbols);
                        var elemType = typeMapper.Map(fieldType);
                        builder.DefineGlobalArray($"{structDecl.Name.Text}__{field.Identifier.Text}", elemType, structCount);
                    }
                    break;
                case ArrayTypeSyntax arrayType:
                    {
                        var elemType = ResolveType(arrayType.ElementType, symbols);
                        var count = ParseArrayLength(arrayType.SizeToken?.Text);
                        if (elemType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                        {
                            var headerSize = HeaderSizeFor(prim.PrimitiveName);
                            builder.DefineGlobalArray(globalLayout.Name, CraneliftTypeMapper.ClifType.I8, count + headerSize);
                        }
                        else if (elemType is ArrayTypeSymbol arrayElem &&
                                 arrayElem.ElementType is PrimitiveTypeSymbol elemPrim &&
                                 HeaderSizeFor(elemPrim.PrimitiveName) > 0)
                        {
                            var headerSize = HeaderSizeFor(elemPrim.PrimitiveName);
                            var stride = arrayElem.Size + headerSize;
                            builder.DefineGlobalArray(globalLayout.Name, CraneliftTypeMapper.ClifType.I8, count * stride);
                        }
                        else
                        {
                            var clifElemType = typeMapper.Map(elemType);
                            builder.DefineGlobalArray(globalLayout.Name, clifElemType, count);
                        }
                        break;
                    }
                case NamedTypeSyntax namedType when structs.TryGetValue(namedType.Name, out var structInstance):
                    EmitStructInstanceGlobals(globalDecl.Name.Text, structInstance, symbols, structs, builder);
                    break;
                case NamedTypeSyntax:
                    {
                        if (!symbols.TryGetValue(globalLayout.Name, out var symbol) || symbol.Type == null)
                            continue;

                        var clifType = typeMapper.Map(symbol.Type);
                        builder.DefineGlobal(globalLayout.Name, clifType);
                        break;
                    }
            }
        }
    }

    private static void EmitFunctions(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        CraneliftModuleBuilder builder,
        List<Diagnostic> diagnostics,
        LayoutPlan layout,
        bool includeTests,
        bool emitTestHarness,
        HashSet<string> reachableFunctions,
        string moduleName)
    {
        var typeMapper = builder.TypeMapper;
        var structs = compilationUnit.Declarations
            .OfType<StructDeclarationSyntax>()
            .ToDictionary(s => s.Name.Text, s => s, StringComparer.Ordinal);
        var enums = compilationUnit.Declarations
            .OfType<EnumDeclarationSyntax>()
            .ToDictionary(e => e.Name.Text, e => e, StringComparer.Ordinal);
        var consts = CollectConstValues(compilationUnit, symbols, diagnostics);
        var functions = compilationUnit.Declarations
            .OfType<FunctionDeclarationSyntax>()
            .ToDictionary(f => f.Name.Text, f => f, StringComparer.Ordinal);
        var functionBuilder = new CraneliftFunctionBuilder(typeMapper, symbols, structs, enums, functions, builder.GlobalTypes, builder.StringLiterals, builder.CStringLiterals, layout, consts, diagnostics, moduleName);

        // Emit regular functions with bodies
        foreach (var func in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            if (!reachableFunctions.Contains(func.Name.Text))
            {
                continue;
            }
            if (func.IsExtern || func.Body is null)
            {
                continue;
            }
            if (!symbols.TryGetValue(func.Name.Text, out var symbol))
                continue;

            var returnTypeSymbol = func.ReturnType is null
                ? new VoidTypeSymbol()
                : ResolveType(func.ReturnType, symbols);
            var returnType = NormalizeFunctionType(typeMapper.Map(returnTypeSymbol));

            var paramTypes = func.Parameters
                .Select(p => NormalizeFunctionType(typeMapper.Map(ResolveType(p.Type, symbols))))
                .ToArray();

            var attributes = GetFunctionAttributes(func);

            // Generate function body
            var body = functionBuilder.BuildFunctionBody(func, symbol);
            if (attributes.Count > 0)
            {
                body = $"; attrs: {string.Join(" ", attributes)}{Environment.NewLine}{body}";
            }
            var mangledName = MangleFunctionName(moduleName, func.Name.Text);
            builder.DefineFunctionWithBody(mangledName, returnType, paramTypes, body);
        }

        // Emit test functions if requested
        if (includeTests)
        {
            foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
            {
                var testFuncName = $"test_{SanitizeTestName(test.Name.Text)}";
                var mangledTestName = MangleFunctionName(moduleName, testFuncName);
                var body = functionBuilder.BuildTestBody(test);
                builder.DefineFunctionWithBody(mangledTestName, CraneliftTypeMapper.ClifType.I32, Array.Empty<CraneliftTypeMapper.ClifType>(), body);
            }

            if (emitTestHarness)
            {
                EmitTestHarness(compilationUnit, builder, diagnostics, moduleName);
            }
        }
    }

    private static string SanitizeTestName(string name)
    {
        var sb = new StringBuilder();
        foreach (var c in name)
        {
            if (char.IsLetterOrDigit(c))
                sb.Append(c);
            else if (c == ' ')
                sb.Append('_');
        }
        return sb.ToString();
    }

    private static List<string> GetFunctionAttributes(FunctionDeclarationSyntax func) =>
        func.Attributes.Select(a => a.Text).ToList();

    private static CraneliftTypeMapper.ClifType NormalizeFunctionType(CraneliftTypeMapper.ClifType type) =>
        type switch
        {
            CraneliftTypeMapper.ClifType.I8 => CraneliftTypeMapper.ClifType.I32,
            CraneliftTypeMapper.ClifType.I16 => CraneliftTypeMapper.ClifType.I32,
            CraneliftTypeMapper.ClifType.B1 => CraneliftTypeMapper.ClifType.I32,
            CraneliftTypeMapper.ClifType.R64 => CraneliftTypeMapper.ClifType.I64,
            _ => type
        };

    private static void EmitTestHarness(
        CompilationUnitSyntax compilationUnit,
        CraneliftModuleBuilder builder,
        List<Diagnostic> diagnostics,
        string moduleName)
    {
        var tests = compilationUnit.Declarations
            .OfType<TestDeclarationSyntax>()
            .ToList();

        var body = new StringBuilder();
        body.AppendLine("block0:");
        var valueCounter = 0;

        if (tests.Count == 0)
        {
            body.AppendLine("    v0 = iconst.i32 0");
            body.AppendLine("    return v0");
            var harnessName = MangleFunctionName(moduleName, "run_tests");
            builder.DefineFunctionWithBody(harnessName, CraneliftTypeMapper.ClifType.I32, Array.Empty<CraneliftTypeMapper.ClifType>(), body.ToString());
            return;
        }

        var failures = NewValue(ref valueCounter);
        body.AppendLine($"    {failures} = iconst.i32 0");

        var passFmt = builder.DefineCStringLiteral("\u001b[32mPASS\u001b[0m: %s\n");
        var failFmt = builder.DefineCStringLiteral("\u001b[31mFAIL\u001b[0m: %s\n");
        var summaryPassFmt = builder.DefineCStringLiteral("Tests: \u001b[32mpassed=%d\u001b[0m failed=%d");
        var summaryFailFmt = builder.DefineCStringLiteral("Tests: passed=%d \u001b[31mfailed=%d\u001b[0m");
        var summaryTimeFmt = builder.DefineCStringLiteral(" test-time=%dms\n");

        var zero64 = NewValue(ref valueCounter);
        body.AppendLine($"    {zero64} = iconst.i64 0");
        var startTime = NewValue(ref valueCounter);
        body.AppendLine($"    {startTime} = call %time({zero64})");
        var startClock = NewValue(ref valueCounter);
        body.AppendLine($"    {startClock} = call %clock()");

        foreach (var testDecl in tests)
        {
            if (testDecl.Parameters.Count > 0)
            {
                diagnostics.Add(new Diagnostic("Test harness supports parameterless tests only.", testDecl.Name.Span));
                continue;
            }

            var testName = testDecl.Name.Text;
            var funcName = $"test_{SanitizeTestName(testName)}";
            var mangledFuncName = MangleFunctionName(moduleName, funcName);

            var result = NewValue(ref valueCounter);
            body.AppendLine($"    {result} = call %{mangledFuncName}()");

            var zero = NewValue(ref valueCounter);
            body.AppendLine($"    {zero} = iconst.i32 0");
            var isFail = NewValue(ref valueCounter);
            body.AppendLine($"    {isFail} = icmp eq {result}, {zero}");

            var failI32 = NewValue(ref valueCounter);
            body.AppendLine($"    {failI32} = bint.i32 {isFail}");
            var nextFailures = NewValue(ref valueCounter);
            body.AppendLine($"    {nextFailures} = iadd {failures}, {failI32}");
            failures = nextFailures;

            var isPass = NewValue(ref valueCounter);
            body.AppendLine($"    {isPass} = icmp eq {failI32}, {zero}");

            var testNameGlobal = builder.DefineCStringLiteral(testName);
            var nameAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {nameAddr} = global_value {testNameGlobal}");

            var passAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {passAddr} = global_value {passFmt}");
            var failAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {failAddr} = global_value {failFmt}");

            var fmtAddr = NewValue(ref valueCounter);
            body.AppendLine($"    {fmtAddr} = select {isPass}, {passAddr}, {failAddr}");
            var print = NewValue(ref valueCounter);
            body.AppendLine($"    {print} = call %printf3({fmtAddr}, {nameAddr}, {zero64})");
        }

        var totalTests = tests.Count;
        var totalVal = NewValue(ref valueCounter);
        body.AppendLine($"    {totalVal} = iconst.i32 {totalTests}");
        var passed = NewValue(ref valueCounter);
        body.AppendLine($"    {passed} = isub {totalVal}, {failures}");

        var endTime = NewValue(ref valueCounter);
        body.AppendLine($"    {endTime} = call %time({zero64})");
        var elapsedSeconds = NewValue(ref valueCounter);
        body.AppendLine($"    {elapsedSeconds} = isub {endTime}, {startTime}");
        var thousand64 = NewValue(ref valueCounter);
        body.AppendLine($"    {thousand64} = iconst.i64 1000");
        var timeMs64 = NewValue(ref valueCounter);
        body.AppendLine($"    {timeMs64} = imul {elapsedSeconds}, {thousand64}");

        var endClock = NewValue(ref valueCounter);
        body.AppendLine($"    {endClock} = call %clock()");
        var elapsedTicks = NewValue(ref valueCounter);
        body.AppendLine($"    {elapsedTicks} = isub {endClock}, {startClock}");
        var ticksTimesMs = NewValue(ref valueCounter);
        body.AppendLine($"    {ticksTimesMs} = imul {elapsedTicks}, {thousand64}");
        var clockDivisor = NewValue(ref valueCounter);
        body.AppendLine($"    {clockDivisor} = iconst.i64 1000");
        var clockMs64 = NewValue(ref valueCounter);
        body.AppendLine($"    {clockMs64} = sdiv {ticksTimesMs}, {clockDivisor}");

        var useTime = NewValue(ref valueCounter);
        body.AppendLine($"    {useTime} = icmp ne {timeMs64}, {zero64}");
        var elapsedMs64 = NewValue(ref valueCounter);
        body.AppendLine($"    {elapsedMs64} = select {useTime}, {timeMs64}, {clockMs64}");
        var elapsedMs = NewValue(ref valueCounter);
        body.AppendLine($"    {elapsedMs} = ireduce.i32 {elapsedMs64}");

        var zero32 = NewValue(ref valueCounter);
        body.AppendLine($"    {zero32} = iconst.i32 0");
        var hasFailures = NewValue(ref valueCounter);
        body.AppendLine($"    {hasFailures} = icmp ne {failures}, {zero32}");

        var passFmtAddr = NewValue(ref valueCounter);
        body.AppendLine($"    {passFmtAddr} = global_value {summaryPassFmt}");
        var failFmtAddr = NewValue(ref valueCounter);
        body.AppendLine($"    {failFmtAddr} = global_value {summaryFailFmt}");
        var summaryAddr = NewValue(ref valueCounter);
        body.AppendLine($"    {summaryAddr} = select {hasFailures}, {failFmtAddr}, {passFmtAddr}");
        var passed64 = NewValue(ref valueCounter);
        body.AppendLine($"    {passed64} = sextend.i64 {passed}");
        var failures64 = NewValue(ref valueCounter);
        body.AppendLine($"    {failures64} = sextend.i64 {failures}");
        var elapsed64 = NewValue(ref valueCounter);
        body.AppendLine($"    {elapsed64} = sextend.i64 {elapsedMs}");
        var summaryCall = NewValue(ref valueCounter);
        body.AppendLine($"    {summaryCall} = call %printf3({summaryAddr}, {passed64}, {failures64})");
        var summaryTimeAddr = NewValue(ref valueCounter);
        body.AppendLine($"    {summaryTimeAddr} = global_value {summaryTimeFmt}");
        var summaryTimeCall = NewValue(ref valueCounter);
        body.AppendLine($"    {summaryTimeCall} = call %printf3({summaryTimeAddr}, {elapsed64}, {zero64})");

        body.AppendLine($"    return {failures}");

        var harnessFunction = MangleFunctionName(moduleName, "run_tests");
        builder.DefineFunctionWithBody(harnessFunction, CraneliftTypeMapper.ClifType.I32, Array.Empty<CraneliftTypeMapper.ClifType>(), body.ToString());
    }

    private static string NewValue(ref int counter) => $"v{counter++}";

    private static string MangleFunctionName(string moduleName, string name) => $"{moduleName}__{name}";

    private static void EmitStructInstanceGlobals(
        string globalName,
        StructDeclarationSyntax structDecl,
        IReadOnlyDictionary<string, Symbol> symbols,
        IReadOnlyDictionary<string, StructDeclarationSyntax> structs,
        CraneliftModuleBuilder builder)
    {
        var typeMapper = builder.TypeMapper;
        foreach (var field in structDecl.Fields)
        {
            var fieldName = $"{globalName}__{field.Identifier.Text}";
            switch (field.Type)
            {
                case ArrayTypeSyntax arrayType when arrayType.ElementType is NamedTypeSyntax nestedNamed &&
                                                   structs.TryGetValue(nestedNamed.Name, out var nestedStruct):
                    {
                        var count = ParseArrayLength(arrayType.SizeToken?.Text);
                        foreach (var nestedField in nestedStruct.Fields)
                        {
                            var nestedFieldType = ResolveType(nestedField.Type, symbols);
                            var nestedElemType = typeMapper.Map(nestedFieldType);
                            var nestedName = $"{fieldName}__{nestedField.Identifier.Text}";
                            if (nestedFieldType is ArrayTypeSymbol nestedArray &&
                                nestedArray.ElementType is PrimitiveTypeSymbol prim &&
                                HeaderSizeFor(prim.PrimitiveName) > 0)
                            {
                                var headerSize = HeaderSizeFor(prim.PrimitiveName);
                                var stride = nestedArray.Size + headerSize;
                                builder.DefineGlobalArray(nestedName, CraneliftTypeMapper.ClifType.I8, count * stride);
                            }
                            else
                            {
                                builder.DefineGlobalArray(nestedName, nestedElemType, count);
                            }
                        }
                        break;
                    }
                case ArrayTypeSyntax arrayType:
                    {
                        var elemType = ResolveType(arrayType.ElementType, symbols);
                        var clifElemType = typeMapper.Map(elemType);
                        var count = ParseArrayLength(arrayType.SizeToken?.Text);
                        if (elemType is PrimitiveTypeSymbol prim && HeaderSizeFor(prim.PrimitiveName) > 0)
                        {
                            var headerSize = HeaderSizeFor(prim.PrimitiveName);
                            builder.DefineGlobalArray(fieldName, CraneliftTypeMapper.ClifType.I8, count + headerSize);
                        }
                        else
                        {
                            builder.DefineGlobalArray(fieldName, clifElemType, count);
                        }
                        break;
                    }
                case NamedTypeSyntax namedField when structs.TryGetValue(namedField.Name, out var nestedStructDecl):
                    EmitStructInstanceGlobals(fieldName, nestedStructDecl, symbols, structs, builder);
                    break;
                default:
                    {
                        var fieldType = ResolveType(field.Type, symbols);
                        var clifType = typeMapper.Map(fieldType);
                        builder.DefineGlobal(fieldName, clifType);
                        break;
                    }
            }
        }
    }

    private static int ParseArrayLength(string? text) =>
        int.TryParse(text, out var n) ? n : 1;

    private static int HeaderSizeFor(string name) =>
        name switch
        {
            "string" => 8,
            "utf8" => 8,
            "ascii" => 4,
            _ => 0
        };

    private static TypeSymbol ResolveType(TypeSyntax syntax, IReadOnlyDictionary<string, Symbol> symbols)
    {
        return syntax switch
        {
            NamedTypeSyntax named when symbols.TryGetValue(named.Name, out var sym) && sym.Type is not null => sym.Type,
            NamedTypeSyntax named when string.Equals(named.Name, "void", StringComparison.Ordinal) => new VoidTypeSymbol(),
            NamedTypeSyntax named => new NamedTypeSymbol(named.Name),
            ArrayTypeSyntax array => new ArrayTypeSymbol(
                ResolveType(array.ElementType, symbols),
                int.TryParse(array.SizeToken?.Text, out var parsed) ? parsed : -1),
            _ => new NamedTypeSymbol("unknown")
        };
    }

    private static (HashSet<string> Builtins, HashSet<string> StringLiterals) CollectLoweringNeeds(
        CompilationUnitSyntax compilationUnit,
        bool includeTests,
        HashSet<string> reachableFunctions)
    {
        var builtins = new HashSet<string>(StringComparer.Ordinal);
        var stringLiterals = new HashSet<string>(StringComparer.Ordinal);

        foreach (var func in compilationUnit.Declarations.OfType<FunctionDeclarationSyntax>())
        {
            if (!reachableFunctions.Contains(func.Name.Text))
            {
                continue;
            }
            if (func.Body is null)
            {
                continue;
            }
            CollectFromBlock(func.Body, builtins, stringLiterals);
        }

        foreach (var constDecl in compilationUnit.Declarations.OfType<ConstDeclarationSyntax>())
        {
            if (constDecl.Initializer is LiteralExpressionSyntax lit && lit.Literal.Kind == TokenKind.StringLiteral)
            {
                stringLiterals.Add(UnescapeString(lit.Literal.Text));
            }
        }

        if (includeTests)
        {
            foreach (var test in compilationUnit.Declarations.OfType<TestDeclarationSyntax>())
            {
                CollectFromBlock(test.Body, builtins, stringLiterals);
            }
        }

        return (builtins, stringLiterals);
    }

    private static void CollectFromBlock(
        BlockStatementSyntax block,
        HashSet<string> builtins,
        HashSet<string> stringLiterals)
    {
        foreach (var stmt in block.Statements)
        {
            CollectFromStatement(stmt, builtins, stringLiterals);
        }
    }

    private static void CollectFromStatement(
        StatementSyntax stmt,
        HashSet<string> builtins,
        HashSet<string> stringLiterals)
    {
        switch (stmt)
        {
            case VariableDeclarationSyntax decl:
                if (decl.Initializer != null)
                {
                    CollectFromExpression(decl.Initializer, builtins, stringLiterals);
                }
                break;
            case ExpressionStatementSyntax exprStmt:
                CollectFromExpression(exprStmt.Expression, builtins, stringLiterals);
                break;
            case ReturnStatementSyntax ret:
                if (ret.Expression != null)
                {
                    CollectFromExpression(ret.Expression, builtins, stringLiterals);
                }
                break;
            case IfStatementSyntax ifStmt:
                CollectFromExpression(ifStmt.Condition, builtins, stringLiterals);
                CollectFromBlock(ifStmt.ThenBlock, builtins, stringLiterals);
                if (ifStmt.ElseBlock != null)
                {
                    CollectFromBlock(ifStmt.ElseBlock, builtins, stringLiterals);
                }
                break;
            case ForStatementSyntax forStmt:
                if (forStmt.Initializer != null)
                {
                    CollectFromExpression(forStmt.Initializer, builtins, stringLiterals);
                }
                if (forStmt.Condition != null)
                {
                    CollectFromExpression(forStmt.Condition, builtins, stringLiterals);
                }
                if (forStmt.Step != null)
                {
                    CollectFromExpression(forStmt.Step, builtins, stringLiterals);
                }
                CollectFromBlock(forStmt.Body, builtins, stringLiterals);
                break;
            case ForeachStatementSyntax foreachStmt:
                CollectFromExpression(foreachStmt.Iterable, builtins, stringLiterals);
                CollectFromBlock(foreachStmt.Body, builtins, stringLiterals);
                break;
            case BlockStatementSyntax block:
                CollectFromBlock(block, builtins, stringLiterals);
                break;
        }
    }

    private static void CollectFromExpression(
        ExpressionSyntax expr,
        HashSet<string> builtins,
        HashSet<string> stringLiterals)
    {
        switch (expr)
        {
            case LiteralExpressionSyntax lit when lit.Literal.Kind == TokenKind.StringLiteral:
                stringLiterals.Add(UnescapeString(lit.Literal.Text));
                break;
            case IdentifierExpressionSyntax:
                break;
            case ParenthesizedExpressionSyntax paren:
                CollectFromExpression(paren.Expression, builtins, stringLiterals);
                break;
            case UnaryExpressionSyntax unary:
                CollectFromExpression(unary.Operand, builtins, stringLiterals);
                break;
            case MemberAccessExpressionSyntax member:
                CollectFromExpression(member.Receiver, builtins, stringLiterals);
                break;
            case ArrayAccessExpressionSyntax array:
                CollectFromExpression(array.Receiver, builtins, stringLiterals);
                CollectFromExpression(array.Index, builtins, stringLiterals);
                break;
            case AssignmentExpressionSyntax assign:
                CollectFromExpression(assign.Left, builtins, stringLiterals);
                CollectFromExpression(assign.Right, builtins, stringLiterals);
                break;
            case BinaryExpressionSyntax bin:
                CollectFromExpression(bin.Left, builtins, stringLiterals);
                CollectFromExpression(bin.Right, builtins, stringLiterals);
                break;
            case CallExpressionSyntax call:
                if (call.Callee is IdentifierExpressionSyntax id && IsCraneliftBuiltin(id.Identifier.Text))
                {
                    builtins.Add(id.Identifier.Text);
                }
                CollectFromExpression(call.Callee, builtins, stringLiterals);
                foreach (var arg in call.Arguments)
                {
                    CollectFromExpression(arg, builtins, stringLiterals);
                }
                break;
            case OperatorCallExpressionSyntax opCall:
                CollectFromExpression(opCall.Receiver, builtins, stringLiterals);
                foreach (var arg in opCall.Arguments)
                {
                    CollectFromExpression(arg, builtins, stringLiterals);
                }
                break;
        }
    }

    private static bool IsCraneliftBuiltin(string name) =>
        name is "print_int" or "print_char" or "print_string" or "read_int" or "read_char"
            or "print_prompt" or "print_invalid" or "print_clue_error" or "print_solved" or "print_cell"
            or "sys_argc" or "sys_argv"
            or "sys_read_file" or "sys_list_dir" or "sys_write_file" or "sys_file_exists" or "sys_file_size" or "sys_file_mtime_ms"
            or "sys_exec" or "sys_spawn" or "sys_sleep_ms"
             or "sys_memcpy_u8" or "sys_memcpy_i32" or "sys_memcpy_f32"
             or "sys_memmove_u8" or "sys_memmove_i32" or "sys_memmove_f32"
             or "time" or "get_time_ms" or "get_time_us" or "sleep_ms"
             or "audio_is_available" or "audio_get_sample_rate" or "audio_get_channels"
             or "audio_get_queued_frames" or "audio_get_underruns" or "audio_push_f32_interleaved"
             or "sin" or "cos" or "sin_fast" or "cos_fast"
            or "init_window" or "gfx_load_sprite" or "gfx_poll_reload" or "load_font" or "measure_text"
            or "char_is_digit" or "char_is_alpha" or "char_is_alnum" or "char_is_space"
            or "char_is_upper" or "char_is_lower" or "char_is_hex" or "char_is_print"
            or "char_to_upper" or "char_to_lower" or "char_to_digit" or "char_from_digit"
            or "char_to_hex" or "char_from_hex"
            or "i32_to_u8_checked" or "i32_to_u16_checked"
            or "str_len" or "str_is_empty" or "str_get" or "str_set" or "str_eq" or "str_cmp"
            or "str_copy" or "str_append" or "str_append_char" or "str_clear"
            or "str_contains" or "str_find" or "str_find_char" or "str_find_last_char"
            or "str_starts_with" or "str_ends_with" or "str_substr";

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

    private static IReadOnlyDictionary<string, CraneliftFunctionBuilder.ConstValue> CollectConstValues(
        CompilationUnitSyntax compilationUnit,
        IReadOnlyDictionary<string, Symbol> symbols,
        List<Diagnostic> diagnostics)
    {
        var consts = new Dictionary<string, CraneliftFunctionBuilder.ConstValue>(StringComparer.Ordinal);
        foreach (var decl in compilationUnit.Declarations.OfType<ConstDeclarationSyntax>())
        {
            if (decl.Initializer is null)
            {
                diagnostics.Add(new Diagnostic("Cranelift requires const initializers.", decl.Name.Span));
                continue;
            }

            if (decl.Initializer is not LiteralExpressionSyntax lit)
            {
                diagnostics.Add(new Diagnostic("Cranelift requires const initializers to be literals for now.", decl.Initializer.Span));
                continue;
            }

            var type = ResolveType(decl.Type, symbols);
            consts[decl.Name.Text] = new CraneliftFunctionBuilder.ConstValue(type, lit.Literal.Kind, lit.Literal.Text);
        }

        return consts;
    }
}
