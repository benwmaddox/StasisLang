#ifndef STASIS_DESKTOP_ADAPTER_H
#define STASIS_DESKTOP_ADAPTER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define STASIS_DESKTOP_ADAPTER_ABI_VERSION 1U
#define STASIS_DESKTOP_ADAPTER_CONTEXT_NATIVE_WINDOW_OFFSET 24U
#define STASIS_DESKTOP_ADAPTER_CONTEXT_SIZE_32 28U
#define STASIS_DESKTOP_ADAPTER_CONTEXT_SIZE_64 32U
#define STASIS_DESKTOP_ADAPTER_PUMP_CONTINUE 0
#define STASIS_DESKTOP_ADAPTER_PUMP_REQUEST_EXIT 1

enum StasisDesktopAdapterPlatform {
    STASIS_DESKTOP_ADAPTER_PLATFORM_WINDOWS = 1,
    STASIS_DESKTOP_ADAPTER_PLATFORM_LINUX = 2,
    STASIS_DESKTOP_ADAPTER_PLATFORM_MACOS = 3
};

enum StasisDesktopAdapterWindowKind {
    STASIS_DESKTOP_ADAPTER_WINDOW_NONE = 0,
    STASIS_DESKTOP_ADAPTER_WINDOW_WIN32_HWND = 1,
    STASIS_DESKTOP_ADAPTER_WINDOW_COCOA = 2,
    STASIS_DESKTOP_ADAPTER_WINDOW_X11 = 3,
    STASIS_DESKTOP_ADAPTER_WINDOW_WAYLAND = 4
};

enum StasisDesktopAdapterWindowOwnership {
    /* The runtime owns this borrowed handle. It is valid only during hooks. */
    STASIS_DESKTOP_ADAPTER_WINDOW_BORROWED_RUNTIME = 1
};

typedef struct StasisDesktopAdapterContext {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t platform;
    uint32_t native_window_kind;
    uint32_t native_window_ownership;
    uint32_t reserved;
    uintptr_t native_window;
} StasisDesktopAdapterContext;

#define STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(name, condition) \
    typedef char name[(condition) ? 1 : -1]

/* Compile both pointer-width layouts on every host, not only the active one. */
typedef struct StasisDesktopAdapterContextLayout32Oracle {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t platform;
    uint32_t native_window_kind;
    uint32_t native_window_ownership;
    uint32_t reserved;
    uint32_t native_window;
} StasisDesktopAdapterContextLayout32Oracle;

typedef struct StasisDesktopAdapterContextLayout64Oracle {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t platform;
    uint32_t native_window_kind;
    uint32_t native_window_ownership;
    uint32_t reserved;
    uint64_t native_window;
} StasisDesktopAdapterContextLayout64Oracle;

STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(
    StasisDesktopAdapterContextLayout32NativeWindowOffsetOracle,
    offsetof(StasisDesktopAdapterContextLayout32Oracle, native_window) ==
        STASIS_DESKTOP_ADAPTER_CONTEXT_NATIVE_WINDOW_OFFSET);
STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(
    StasisDesktopAdapterContextLayout32SizeOracle,
    sizeof(StasisDesktopAdapterContextLayout32Oracle) ==
        STASIS_DESKTOP_ADAPTER_CONTEXT_SIZE_32);
STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(
    StasisDesktopAdapterContextLayout64NativeWindowOffsetOracle,
    offsetof(StasisDesktopAdapterContextLayout64Oracle, native_window) ==
        STASIS_DESKTOP_ADAPTER_CONTEXT_NATIVE_WINDOW_OFFSET);
STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(
    StasisDesktopAdapterContextLayout64SizeOracle,
    sizeof(StasisDesktopAdapterContextLayout64Oracle) ==
        STASIS_DESKTOP_ADAPTER_CONTEXT_SIZE_64);
STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(
    StasisDesktopAdapterContextNativeWindowOffsetOracle,
    offsetof(StasisDesktopAdapterContext, native_window) ==
        STASIS_DESKTOP_ADAPTER_CONTEXT_NATIVE_WINDOW_OFFSET);
STASIS_DESKTOP_ADAPTER_STATIC_ASSERT(
    StasisDesktopAdapterContextSizeOracle,
    sizeof(StasisDesktopAdapterContext) ==
        (UINTPTR_MAX == UINT64_MAX ? STASIS_DESKTOP_ADAPTER_CONTEXT_SIZE_64
                                  : STASIS_DESKTOP_ADAPTER_CONTEXT_SIZE_32));

#undef STASIS_DESKTOP_ADAPTER_STATIC_ASSERT

/*
 * Hooks run on the UI/runtime thread. initialize runs after guest main and
 * before the first tick. pump runs exactly once between guest ticks. Async
 * callbacks must only enqueue native results; pump may invoke declarations in
 * the generated stasis_host_exports.h to publish them to the guest mailbox.
 * on_foreground requests a requery after the runtime window regains input focus
 * or is restored from a minimized/hidden state.
 * shutdown runs exactly once after a successful initialize and before the
 * runtime invalidates native_window. initialize and on_foreground return zero
 * on success. pump returns CONTINUE or REQUEST_EXIT; REQUEST_EXIT asks Stasis to
 * leave the loop cleanly and still run both shutdown layers in order.
 */
int32_t stasis_desktop_adapter_initialize(const StasisDesktopAdapterContext *context);
int32_t stasis_desktop_adapter_pump(const StasisDesktopAdapterContext *context);
int32_t stasis_desktop_adapter_on_foreground(const StasisDesktopAdapterContext *context);
void stasis_desktop_adapter_shutdown(const StasisDesktopAdapterContext *context);

#ifdef __cplusplus
}
#endif

#endif
