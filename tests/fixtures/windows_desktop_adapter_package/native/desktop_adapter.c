#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "stasis_desktop_adapter.h"
#include "stasis_host_exports.h"

static StasisDesktopAdapterContext adapter_context;
static DWORD runtime_thread_id;
static HANDLE worker_thread;
static volatile LONG async_ready;
static int32_t serial;
static int32_t initialize_count;
static int32_t pump_count;
static int32_t async_delivered;
static int32_t foreground_count;
static int32_t shutdown_count;
static int32_t mailbox_failures;
static int32_t close_posted;
static int32_t same_thread = 1;

static HWND adapter_hwnd(void) {
    return (HWND)adapter_context.native_window;
}

static int32_t publish_mailbox(int32_t kind, int32_t value) {
    if (GetCurrentThreadId() != runtime_thread_id) same_thread = 0;
    serial += 1;
    int32_t result = stasis_host_v1_adapter_mailbox(kind, serial, value);
    if (result != serial) mailbox_failures += 1;
    return result == serial ? 0 : -20;
}

static DWORD WINAPI enqueue_async_result(LPVOID unused) {
    (void)unused;
    InterlockedExchange(&async_ready, 1);
    return 0;
}

int32_t stasis_desktop_adapter_initialize(const StasisDesktopAdapterContext *context) {
    if (!context || context->struct_size != sizeof(*context) ||
            context->abi_version != STASIS_DESKTOP_ADAPTER_ABI_VERSION ||
            context->platform != STASIS_DESKTOP_ADAPTER_PLATFORM_WINDOWS ||
            context->native_window_kind != STASIS_DESKTOP_ADAPTER_WINDOW_WIN32_HWND ||
            context->native_window_ownership !=
                STASIS_DESKTOP_ADAPTER_WINDOW_BORROWED_RUNTIME ||
            context->native_window == (uintptr_t)0 ||
            !IsWindow((HWND)context->native_window)) {
        return -10;
    }
    adapter_context = *context;
    runtime_thread_id = GetCurrentThreadId();
    initialize_count = 1;
    if (publish_mailbox(1, 0) != 0) return -11;
    worker_thread = CreateThread(NULL, 0, enqueue_async_result, NULL, 0, NULL);
    return worker_thread ? 0 : -12;
}

int32_t stasis_desktop_adapter_pump(const StasisDesktopAdapterContext *context) {
    if (!context || context->native_window != adapter_context.native_window ||
            !IsWindow(adapter_hwnd())) {
        return -30;
    }
    pump_count += 1;
    if (publish_mailbox(2, pump_count) != 0) return -31;
    if (InterlockedExchange(&async_ready, 0) != 0) {
        async_delivered += 1;
        if (publish_mailbox(3, 665) != 0) return -32;
    }
    if (pump_count == 1) {
        ShowWindow(adapter_hwnd(), SW_MINIMIZE);
        SendMessageW(adapter_hwnd(), WM_SYSCOMMAND, SC_MINIMIZE, 0);
        Sleep(100);
        if (!IsIconic(adapter_hwnd())) return -35;
    } else if (pump_count < 5) {
        Sleep(20);
    } else if (pump_count == 5) {
        ShowWindow(adapter_hwnd(), SW_RESTORE);
        SendMessageW(adapter_hwnd(), WM_SYSCOMMAND, SC_RESTORE, 0);
        Sleep(100);
        if (IsIconic(adapter_hwnd())) return -36;
    } else if (foreground_count == 0) {
        Sleep(10);
    }
    if (!close_posted && pump_count >= 6 && async_delivered == 1 &&
            foreground_count == 1) {
        close_posted = 1;
        return STASIS_DESKTOP_ADAPTER_PUMP_REQUEST_EXIT;
    }
    if (pump_count > 300) return -34;
    return 0;
}

int32_t stasis_desktop_adapter_on_foreground(
    const StasisDesktopAdapterContext *context
) {
    if (!context || context->native_window != adapter_context.native_window) return -40;
    foreground_count += 1;
    return publish_mailbox(4, foreground_count);
}

void stasis_desktop_adapter_shutdown(const StasisDesktopAdapterContext *context) {
    shutdown_count += 1;
    if (!context || context->native_window != adapter_context.native_window ||
            publish_mailbox(5, shutdown_count) != 0) {
        mailbox_failures += 1;
    }
    if (worker_thread) {
        WaitForSingleObject(worker_thread, 5000);
        CloseHandle(worker_thread);
        worker_thread = NULL;
    }
    const char *path = getenv("STASIS_ADAPTER_RECEIPT");
    if (!path || !path[0]) return;
    FILE *file = fopen(path, "wb");
    if (!file) return;
    fprintf(
        file,
        "{\"schema\":\"stasis.desktop_adapter.acceptance.v1\","
        "\"abi_version\":%u,\"struct_size\":%u,\"platform\":%u,"
        "\"window_kind\":%u,\"ownership\":%u,\"hwnd_valid\":%s,"
        "\"initialize_count\":%d,\"pump_count\":%d,"
        "\"async_delivered\":%d,\"foreground_count\":%d,"
        "\"shutdown_count\":%d,\"mailbox_serial\":%d,"
        "\"mailbox_failures\":%d,\"same_thread\":%s}\n",
        adapter_context.abi_version,
        adapter_context.struct_size,
        adapter_context.platform,
        adapter_context.native_window_kind,
        adapter_context.native_window_ownership,
        IsWindow(adapter_hwnd()) ? "true" : "false",
        initialize_count,
        pump_count,
        async_delivered,
        foreground_count,
        shutdown_count,
        serial,
        mailbox_failures,
        same_thread ? "true" : "false"
    );
    fclose(file);
}
