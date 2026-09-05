#ifndef STASIS_MOBILE_RUNTIME_H
#define STASIS_MOBILE_RUNTIME_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define STASIS_MOBILE_RUNTIME_ABI_VERSION 2
#define STASIS_MOBILE_FRAME_INTERVAL_NS 16666667ULL

enum StasisMobileRuntimeResult {
    STASIS_MOBILE_RUNTIME_OK = 0,
    STASIS_MOBILE_RUNTIME_STOP_REQUESTED = 1,
    STASIS_MOBILE_RUNTIME_INVALID_ARGUMENT = -1,
    STASIS_MOBILE_RUNTIME_NOT_INITIALIZED = -2,
    STASIS_MOBILE_RUNTIME_ALREADY_INITIALIZED = -3,
    STASIS_MOBILE_RUNTIME_GRAPHICS_UNAVAILABLE = -4
};

enum StasisMobileRuntimeEntry {
    STASIS_MOBILE_RUNTIME_ENTRY_NONE = 0,
    STASIS_MOBILE_RUNTIME_ENTRY_MAIN = 1,
    STASIS_MOBILE_RUNTIME_ENTRY_TICK = 2,
    STASIS_MOBILE_RUNTIME_ENTRY_RENDER = 3
};

typedef void (*StasisMobileBindEntry)(void);
typedef int32_t (*StasisMobileI32Entry)(void);

typedef struct StasisMobileGameEntries {
    StasisMobileBindEntry bind_runtime_entry;
    StasisMobileI32Entry main_entry;
    StasisMobileI32Entry tick_entry;
    StasisMobileI32Entry render_entry;
} StasisMobileGameEntries;

typedef struct StasisMobileRuntimeConfig {
    int32_t width;
    int32_t height;
    const char *title;
} StasisMobileRuntimeConfig;

typedef struct StasisMobileFramePacer {
    uint64_t next_deadline_ns;
} StasisMobileFramePacer;

/*
 * The shell owns wall-clock pacing so a display's refresh rate cannot redefine
 * the deterministic game tick. Call reset immediately before the loop, then
 * wait_ns after every successful step and sleep for the returned duration.
 */
static inline void stasis_mobile_frame_pacer_reset(
    StasisMobileFramePacer *pacer,
    uint64_t now_ns
) {
    pacer->next_deadline_ns = now_ns + STASIS_MOBILE_FRAME_INTERVAL_NS;
}

static inline uint64_t stasis_mobile_frame_pacer_wait_ns(
    StasisMobileFramePacer *pacer,
    uint64_t now_ns
) {
    uint64_t deadline_ns = pacer->next_deadline_ns;
    if (now_ns < deadline_ns) {
        pacer->next_deadline_ns = deadline_ns + STASIS_MOBILE_FRAME_INTERVAL_NS;
        return deadline_ns - now_ns;
    }
    if (now_ns - deadline_ns >= STASIS_MOBILE_FRAME_INTERVAL_NS) {
        /* A suspended or overloaded app resumes without catch-up ticks. */
        pacer->next_deadline_ns = now_ns + STASIS_MOBILE_FRAME_INTERVAL_NS;
    } else {
        pacer->next_deadline_ns = deadline_ns + STASIS_MOBILE_FRAME_INTERVAL_NS;
    }
    return 0;
}

/* Initializes the SDL-only host APIs and calls the game main entry exactly once. */
int32_t stasis_mobile_runtime_initialize(
    const StasisMobileRuntimeConfig *config,
    const StasisMobileGameEntries *entries
);

/* Pumps input, advances one deterministic tick, and renders one frame. */
int32_t stasis_mobile_runtime_step(void);

/* Paused runtimes remain initialized but do not tick or render. */
void stasis_mobile_runtime_set_paused(int32_t paused);
int32_t stasis_mobile_runtime_is_initialized(void);
/* Exact non-zero main/tick/render result; read before shutdown resets state. */
int32_t stasis_mobile_runtime_last_entry_result(void);
/* Entry associated with last_entry_result; read before shutdown resets state. */
int32_t stasis_mobile_runtime_last_entry(void);

/* Releases graphics and audio state. Safe to call more than once. */
void stasis_mobile_runtime_shutdown(void);

#ifdef __cplusplus
}
#endif

#endif
