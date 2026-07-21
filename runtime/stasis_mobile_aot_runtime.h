#ifndef STASIS_MOBILE_AOT_RUNTIME_H
#define STASIS_MOBILE_AOT_RUNTIME_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void stasis_jit_register_global_i32_ptr(int32_t path_hash, int32_t *ptr);
void stasis_jit_register_global_f32_ptr(int32_t path_hash, float *ptr);
void stasis_jit_register_global_f64_ptr(int32_t path_hash, double *ptr);
void stasis_jit_register_global_i32_array(
    int32_t collection_hash, int32_t field_hash, int32_t *ptr, int32_t len);
void stasis_jit_register_global_f32_array(
    int32_t collection_hash, int32_t field_hash, float *ptr, int32_t len);
void stasis_jit_register_global_f64_array(
    int32_t collection_hash, int32_t field_hash, double *ptr, int32_t len);
void stasis_jit_register_global_u8_array(
    int32_t collection_hash, int32_t field_hash, uint8_t *ptr, int32_t len);
void stasis_jit_register_code_ptr(int32_t fn_id, int64_t code_ptr);
void stasis_jit_clear_string_literal_table(void);
void stasis_jit_upsert_string_literal(int32_t id, const char *value);
int64_t stasis_jit_lookup_code_ptr(int32_t fn_id);
int32_t stasis_jit_global_i32_load(int32_t hash);
void stasis_jit_global_i32_store(int32_t hash, int32_t value);
float stasis_jit_global_f32_load(int32_t hash);
void stasis_jit_global_f32_store(int32_t hash, float value);
double stasis_jit_global_f64_load(int32_t hash);
void stasis_jit_global_f64_store(int32_t hash, double value);
int32_t stasis_jit_global_i32_array_load(int32_t c, int32_t f, int32_t i);
void stasis_jit_global_i32_array_store(int32_t c, int32_t f, int32_t i, int32_t value);
int32_t *stasis_jit_global_i32_array_ptr(int32_t c, int32_t f, int32_t len);
float stasis_jit_global_f32_array_load(int32_t c, int32_t f, int32_t i);
void stasis_jit_global_f32_array_store(int32_t c, int32_t f, int32_t i, float value);
float *stasis_jit_global_f32_array_ptr(int32_t c, int32_t f, int32_t len);
double stasis_jit_global_f64_array_load(int32_t c, int32_t f, int32_t i);
void stasis_jit_global_f64_array_store(int32_t c, int32_t f, int32_t i, double value);
double *stasis_jit_global_f64_array_ptr(int32_t c, int32_t f, int32_t len);
int32_t stasis_jit_collection_i32_load(int32_t hash, int32_t kind);
void stasis_jit_collection_i32_store(int32_t hash, int32_t kind, int32_t value);
int32_t stasis_jit_call_i32_0(int32_t fn);
int32_t stasis_jit_call_i32_1(int32_t fn, int32_t a0);
int32_t stasis_jit_call_i32_2(int32_t fn, int32_t a0, int32_t a1);
int32_t stasis_jit_call_i32_3(int32_t fn, int32_t a0, int32_t a1, int32_t a2);
int32_t stasis_jit_call_i32_4(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3);
int32_t stasis_jit_call_i32_5(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4);
int32_t stasis_jit_call_i32_6(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4, int32_t a5);
int32_t stasis_jit_call_i32_7(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4, int32_t a5, int32_t a6);
int32_t stasis_jit_call_i32_8(int32_t fn, int32_t a0, int32_t a1, int32_t a2, int32_t a3, int32_t a4, int32_t a5, int32_t a6, int32_t a7);
int32_t stasis_jit_call_i32_f32_1(int32_t fn, float a0);
int32_t stasis_jit_call_i32_f32_2(int32_t fn, float a0, float a1);
int32_t stasis_jit_call_i32_f32_3(int32_t fn, float a0, float a1, float a2);
int32_t stasis_jit_call_i32_f32_4(int32_t fn, float a0, float a1, float a2, float a3);
int32_t stasis_jit_call_i32_f32_5(int32_t fn, float a0, float a1, float a2, float a3, float a4);
int32_t stasis_jit_call_i32_f32_6(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5);
int32_t stasis_jit_call_i32_f32_7(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6);
int32_t stasis_jit_call_i32_f32_8(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6, float a7);
float stasis_jit_call_f32_0(int32_t fn);
float stasis_jit_call_f32_1(int32_t fn, float a0);
float stasis_jit_call_f32_2(int32_t fn, float a0, float a1);
float stasis_jit_call_f32_3(int32_t fn, float a0, float a1, float a2);
float stasis_jit_call_f32_4(int32_t fn, float a0, float a1, float a2, float a3);
float stasis_jit_call_f32_5(int32_t fn, float a0, float a1, float a2, float a3, float a4);
float stasis_jit_call_f32_6(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5);
float stasis_jit_call_f32_7(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6);
float stasis_jit_call_f32_8(int32_t fn, float a0, float a1, float a2, float a3, float a4, float a5, float a6, float a7);
float stasis_jit_call_f32_i32_1(int32_t fn, int32_t a0);
void stasis_jit_sys_memcpy_u8(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n);
void stasis_jit_sys_memcpy_i32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n);
void stasis_jit_sys_memcpy_f32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n);
void stasis_jit_reject_code_swap(void);
void stasis_jit_sys_memmove_u8(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n);
void stasis_jit_sys_memmove_i32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n);
void stasis_jit_sys_memmove_f32(int32_t d, int32_t di, int32_t s, int32_t si, int32_t n);
int stasis_jit_audio_init(int32_t rate, int32_t channels, int32_t latency);
void stasis_jit_audio_shutdown(void);
int stasis_jit_audio_is_available(void);
int stasis_jit_audio_get_sample_rate(void);
int stasis_jit_audio_get_channels(void);
int stasis_jit_audio_get_queued_frames(void);
int stasis_jit_audio_get_underruns(void);
int stasis_jit_audio_push_f32_interleaved(int32_t samples, int32_t frames);
int stasis_jit_gfx_load_sprite(int32_t path, int32_t max_w, int32_t max_h);
void stasis_jit_gfx_release_sprite(int32_t handle);
int stasis_jit_gfx_dump_bmp(int32_t path);
int stasis_jit_gfx_dump_png(int32_t path);
int stasis_jit_gfx_cache_text(int32_t font, int32_t text);
int stasis_jit_gfx_poll_reload(int32_t handle);
float stasis_jit_gfx_measure_text_cached(int32_t handle);
int stasis_jit_load_font(int32_t path, int32_t size);
float stasis_jit_measure_text(int32_t font, int32_t text);
void stasis_jit_sleep_ms(int32_t ms);
void stasis_mobile_aot_reset(void);

#ifdef __cplusplus
}
#endif

#endif
