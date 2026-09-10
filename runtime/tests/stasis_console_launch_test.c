#include <windows.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <wchar.h>

#ifndef PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE
#define PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE 0x00020016
#endif

int stasis_init_window(int width, int height, const char* title);
int stasis_set_recording_config(int width, int height, uint32_t fps);
void stasis_set_window_size(int width, int height);
void stasis_shutdown(void);

static int wait_iconic(HWND window, int expected, DWORD timeout_ms) {
    ULONGLONG deadline = GetTickCount64() + timeout_ms;
    do {
        if (!!IsIconic(window) == expected) return 1;
        Sleep(10);
    } while (GetTickCount64() < deadline);
    return 0;
}

static int remains_restored(HWND window) {
    ULONGLONG deadline = GetTickCount64() + 300;
    do {
        if (IsIconic(window)) return 0;
        Sleep(10);
    } while (GetTickCount64() < deadline);
    return 1;
}

static int game_visible(const char* title) {
    HWND game = FindWindowA(NULL, title);
    DWORD process_id = 0;
    if (!game) return 0;
    GetWindowThreadProcessId(game, &process_id);
    return process_id == GetCurrentProcessId() && IsWindowVisible(game) && !IsIconic(game);
}

static int configure_child_environment(void) {
    return SetEnvironmentVariableA("STASIS_WINDOW_HIDDEN", "0") &&
        SetEnvironmentVariableA("STASIS_WINDOW_START_MINIMIZED", "0") &&
        SetEnvironmentVariableA("STASIS_RECORDING_PRESENTATION", "0") &&
        SetEnvironmentVariableA("SDL_VIDEODRIVER", "windows") &&
        SetEnvironmentVariableA("SDL_RENDER_DRIVER", "software") &&
        SetEnvironmentVariableA("SDL_AUDIODRIVER", "dummy");
}

static int run_child(const char* mode) {
    HWND console = GetConsoleWindow();
    HWND terminal = console ? GetAncestor(console, GA_ROOTOWNER) : NULL;
    if (!terminal) terminal = console;
    if (!terminal) return 77;
    ULONGLONG deadline = GetTickCount64() + 2000;
    while (!IsWindowVisible(terminal) && GetTickCount64() < deadline) Sleep(10);
    if (!IsWindowVisible(terminal)) return 77;
    ShowWindowAsync(terminal, SW_RESTORE);
    if (!wait_iconic(terminal, 0, 1000)) return 1;

    const int hidden = strcmp(mode, "hidden") == 0;
    const int opt_out = strcmp(mode, "opt-out") == 0;
    char title[128];
    snprintf(title, sizeof(title), "Stasis console test %lu %s", GetCurrentProcessId(), mode);
    if (!SetEnvironmentVariableA("STASIS_CONSOLE_START_MINIMIZED", opt_out ? "0" : NULL) ||
        !configure_child_environment()) return 14;
    if (hidden && !stasis_set_recording_config(320, 240, 60)) return 15;
    if (!stasis_init_window(320, 240, title)) return 2;
    if (hidden || opt_out) {
        if (!remains_restored(terminal)) return 3;
    } else if (!wait_iconic(terminal, 1, 2000)) {
        return 4;
    }
    if (!hidden && !game_visible(title)) return 11;
    if (hidden && game_visible(title)) return 16;

    if (hidden) {
        stasis_shutdown();
        if (!stasis_init_window(320, 240, title)) return 5;
        if (!wait_iconic(terminal, 1, 2000)) return 6;
        if (!game_visible(title)) return 12;
    }
    ShowWindowAsync(terminal, SW_RESTORE);
    if (!wait_iconic(terminal, 0, 1000)) return 7;
    stasis_set_window_size(400, 300);
    if (!remains_restored(terminal)) return 8;
    stasis_shutdown();
    if (!stasis_init_window(320, 240, title)) return 9;
    if (!remains_restored(terminal)) return 10;
    if (!game_visible(title)) return 13;
    stasis_shutdown();
    return 0;
}

static int run_conpty_child(void) {
    if (!SetEnvironmentVariableA("STASIS_CONSOLE_START_MINIMIZED", NULL) ||
        !configure_child_environment()) return 20;
    const char* title = "Stasis ConPTY console test";
    if (!stasis_init_window(320, 240, title)) return 21;
    if (!game_visible(title)) return 22;
    stasis_shutdown();
    return 0;
}

static int drain_conpty_output(HANDLE output_read, size_t* iconify_match, int* saw_iconify) {
    static const char iconify[] = "\x1b[2t";
    DWORD available = 0;
    if (!PeekNamedPipe(output_read, NULL, 0, NULL, &available, NULL)) return 0;
    while (available > 0) {
        char output[4096];
        DWORD requested = available < sizeof(output) ? available : sizeof(output);
        DWORD read = 0;
        if (!ReadFile(output_read, output, requested, &read, NULL)) return 0;
        for (DWORD i = 0; i < read; ++i) {
            if (output[i] == iconify[*iconify_match]) {
                *iconify_match += 1;
                if (*iconify_match == sizeof(iconify) - 1) {
                    *saw_iconify = 1;
                    *iconify_match = 0;
                }
            } else {
                *iconify_match = output[i] == iconify[0] ? 1 : 0;
            }
        }
        if (!PeekNamedPipe(output_read, NULL, 0, NULL, &available, NULL)) return 0;
    }
    return 1;
}

static int run_conpty_parent(const wchar_t* executable) {
    HANDLE input_read = NULL;
    HANDLE input_write = NULL;
    HANDLE output_read = NULL;
    HANDLE output_write = NULL;
    HPCON pseudoconsole = NULL;
    PPROC_THREAD_ATTRIBUTE_LIST attributes = NULL;
    int attributes_initialized = 0;
    PROCESS_INFORMATION process = {0};
    int result = 1;

    if (!CreatePipe(&input_read, &input_write, NULL, 0) ||
        !CreatePipe(&output_read, &output_write, NULL, 0)) goto cleanup;
    COORD size = {80, 25};
    if (FAILED(CreatePseudoConsole(size, input_read, output_write, 0, &pseudoconsole))) goto cleanup;
    CloseHandle(input_read);
    input_read = NULL;
    CloseHandle(output_write);
    output_write = NULL;

    SIZE_T attribute_size = 0;
    InitializeProcThreadAttributeList(NULL, 1, 0, &attribute_size);
    attributes = (PPROC_THREAD_ATTRIBUTE_LIST)HeapAlloc(
        GetProcessHeap(), 0, attribute_size);
    if (!attributes ||
        !InitializeProcThreadAttributeList(attributes, 1, 0, &attribute_size)) goto cleanup;
    attributes_initialized = 1;
    if (!UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
        pseudoconsole, sizeof(pseudoconsole), NULL, NULL)) goto cleanup;

    wchar_t command[32800];
    if (swprintf(command, 32800, L"\"%ls\" conpty", executable) < 0) goto cleanup;
    STARTUPINFOEXW startup = {0};
    startup.StartupInfo.cb = sizeof(startup);
    startup.lpAttributeList = attributes;
    if (!CreateProcessW(executable, command, NULL, NULL, FALSE,
            EXTENDED_STARTUPINFO_PRESENT, NULL, NULL, &startup.StartupInfo, &process)) goto cleanup;

    size_t iconify_match = 0;
    int saw_iconify = 0;
    ULONGLONG deadline = GetTickCount64() + 8000;
    for (;;) {
        if (!drain_conpty_output(output_read, &iconify_match, &saw_iconify)) break;
        if (WaitForSingleObject(process.hProcess, 10) == WAIT_OBJECT_0) {
            ULONGLONG drain_deadline = GetTickCount64() + 250;
            while (GetTickCount64() < drain_deadline) {
                if (!drain_conpty_output(
                    output_read, &iconify_match, &saw_iconify)) goto cleanup;
                Sleep(10);
            }
            break;
        }
        if (GetTickCount64() >= deadline) break;
    }
    DWORD child_result = 1;
    GetExitCodeProcess(process.hProcess, &child_result);
    result = child_result == 0 && saw_iconify ? 0 : 1;

cleanup:
    if (process.hProcess) {
        if (WaitForSingleObject(process.hProcess, 0) != WAIT_OBJECT_0) {
            TerminateProcess(process.hProcess, 1);
            WaitForSingleObject(process.hProcess, 1000);
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    if (attributes) {
        if (attributes_initialized) DeleteProcThreadAttributeList(attributes);
        HeapFree(GetProcessHeap(), 0, attributes);
    }
    if (input_read) CloseHandle(input_read);
    if (input_write) CloseHandle(input_write);
    if (output_read) CloseHandle(output_read);
    if (output_write) CloseHandle(output_write);
    if (pseudoconsole) ClosePseudoConsole(pseudoconsole);
    return result;
}

int main(int argc, char** argv) {
    if (argc == 2 && strcmp(argv[1], "conpty") == 0) return run_conpty_child();
    if (argc == 2) return run_child(argv[1]);
    wchar_t executable[32768];
    DWORD length = GetModuleFileNameW(NULL, executable, 32768);
    if (!length || length >= 32768) return 1;
    const wchar_t* modes[] = {L"default", L"opt-out", L"hidden"};
    int skipped = 0;
    for (int i = 0; i < 3; ++i) {
        wchar_t command[32800];
        if (swprintf(command, 32800, L"\"%ls\" %ls", executable, modes[i]) < 0) return 1;
        STARTUPINFOW startup = {0};
        PROCESS_INFORMATION process = {0};
        startup.cb = sizeof(startup);
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_SHOWNORMAL;
        if (!CreateProcessW(executable, command, NULL, NULL, FALSE,
                CREATE_NEW_CONSOLE, NULL, NULL, &startup, &process)) {
            fprintf(stderr, "Console test child %ls failed to launch: %lu\n", modes[i], GetLastError());
            return 1;
        }
        DWORD status = WaitForSingleObject(process.hProcess, 8000);
        DWORD result = 1;
        if (status == WAIT_OBJECT_0) {
            GetExitCodeProcess(process.hProcess, &result);
        } else {
            TerminateProcess(process.hProcess, 1);
            WaitForSingleObject(process.hProcess, 1000);
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        if (result == 77) {
            fprintf(stdout, "SKIP %ls: no visible console host\n", modes[i]);
            skipped = 1;
        } else if (result != 0) {
            fprintf(stderr, "Console test %ls failed at stage %lu\n", modes[i], result);
            return 1;
        } else {
            fprintf(stdout, "PASS %ls\n", modes[i]);
        }
    }
    if (run_conpty_parent(executable) != 0) {
        fprintf(stderr, "ConPTY console test did not receive the iconify request\n");
        return 1;
    }
    fprintf(stdout, "PASS conpty\n");
    return skipped ? 77 : 0;
}
