#include "stasis_render_contract.h"

uint32_t stasis_render_trace_native(
    const int32_t *cmd_i32,
    const float *cmd_f32,
    const uint8_t *cmd_u8
) {
    return stasis_render_trace(cmd_i32, cmd_f32, cmd_u8);
}
