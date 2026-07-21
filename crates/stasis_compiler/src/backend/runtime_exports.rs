pub(crate) const AOT_RUNTIME_EXPORT_SYMBOLS: &[&str] = &[
    "stasis_get_time_ms",
    "stasis_get_time_us",
    "stasis_gfx_cache_text",
    "stasis_gfx_measure_text_cached",
    "stasis_gfx_poll_reload",
    "stasis_jit_audio_get_channels",
    "stasis_jit_audio_get_queued_frames",
    "stasis_jit_audio_get_sample_rate",
    "stasis_jit_audio_get_underruns",
    "stasis_jit_audio_init",
    "stasis_jit_audio_is_available",
    "stasis_jit_audio_push_f32_interleaved",
    "stasis_jit_audio_shutdown",
    "stasis_jit_collection_i32_load",
    "stasis_jit_collection_i32_store",
    "stasis_jit_cos_fast",
    "stasis_jit_gfx_cache_text",
    "stasis_jit_gfx_dump_bmp",
    "stasis_jit_gfx_dump_png",
    "stasis_jit_gfx_load_sprite",
    "stasis_jit_gfx_release_sprite",
    "stasis_jit_gfx_measure_text_cached",
    "stasis_jit_gfx_poll_reload",
    "stasis_jit_global_f32_array_load",
    "stasis_jit_global_f32_array_ptr",
    "stasis_jit_global_f32_array_store",
    "stasis_jit_global_f32_load",
    "stasis_jit_global_f32_store",
    "stasis_jit_global_f64_array_load",
    "stasis_jit_global_f64_array_ptr",
    "stasis_jit_global_f64_array_store",
    "stasis_jit_global_f64_load",
    "stasis_jit_global_f64_store",
    "stasis_jit_global_i32_array_load",
    "stasis_jit_global_i32_array_ptr",
    "stasis_jit_global_i32_array_store",
    "stasis_jit_global_i32_load",
    "stasis_jit_global_i32_store",
    "stasis_jit_load_font",
    "stasis_jit_measure_text",
    "stasis_jit_print_i32",
    "stasis_jit_print_string",
    "stasis_jit_reject_code_swap",
    "stasis_jit_sin_fast",
    "stasis_jit_sleep_ms",
    "stasis_jit_sys_memcpy_f32",
    "stasis_jit_sys_memcpy_i32",
    "stasis_jit_sys_memcpy_u8",
    "stasis_jit_sys_memmove_f32",
    "stasis_jit_sys_memmove_i32",
    "stasis_jit_sys_memmove_u8",
];

pub(crate) fn is_aot_runtime_export_symbol(symbol: &str) -> bool {
    AOT_RUNTIME_EXPORT_SYMBOLS.contains(&symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aot_runtime_export_contract_requires_exact_symbol_matches() {
        assert!(is_aot_runtime_export_symbol("stasis_jit_gfx_load_sprite"));
        assert!(is_aot_runtime_export_symbol(
            "stasis_jit_gfx_release_sprite"
        ));
        assert!(!is_aot_runtime_export_symbol(
            "stasis_jit_gfx_totally_missing"
        ));
    }
}
