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

#define ICONIFY_TIMEOUT_MS 5000
#define CHILD_PROCESS_TIMEOUT_MS 12000
#define CONSOLE_DIAG_ENV L"STASIS_CONSOLE_START_MINIMIZED_TRACE"
#define CONSOLE_DIAG_MAX_BYTES 32768
#define CONSOLE_DIAG_LINE_CAPACITY 2048

static wchar_t console_diag_trace_path[32768];

static void initialize_console_diag_path(void) {
    const DWORD length = GetEnvironmentVariableW(
        CONSOLE_DIAG_ENV, console_diag_trace_path,
        (DWORD)(sizeof(console_diag_trace_path) / sizeof(console_diag_trace_path[0])));
    if (!length || length >= sizeof(console_diag_trace_path) / sizeof(console_diag_trace_path[0])) {
        console_diag_trace_path[0] = L'\0';
    }
}

static void write_console_diag_line(const char* line, size_t length) {
    if (!console_diag_trace_path[0] || !length || length > CONSOLE_DIAG_LINE_CAPACITY) return;
    HANDLE file = CreateFileW(console_diag_trace_path, FILE_APPEND_DATA,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, NULL,
        OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (file == INVALID_HANDLE_VALUE) return;
    LARGE_INTEGER current_size;
    if (GetFileSizeEx(file, &current_size) && current_size.QuadPart >= 0 &&
        current_size.QuadPart <= CONSOLE_DIAG_MAX_BYTES - (LONGLONG)length) {
        DWORD bytes_written = 0;
        WriteFile(file, line, (DWORD)length, &bytes_written, NULL);
    }
    CloseHandle(file);
}

static size_t append_window_snapshot(
    char* output, size_t capacity, size_t used, const char* label, HWND window) {
    if (used >= capacity) return capacity;
    char class_name[128] = {0};
    DWORD process_id = 0;
    DWORD thread_id = 0;
    LONG_PTR style = 0;
    LONG_PTR extended_style = 0;
    int visible = 0;
    int iconic = 0;
    if (window) {
        GetClassNameA(window, class_name, (int)sizeof(class_name));
        thread_id = GetWindowThreadProcessId(window, &process_id);
        style = GetWindowLongPtrW(window, GWL_STYLE);
        extended_style = GetWindowLongPtrW(window, GWL_EXSTYLE);
        visible = !!IsWindowVisible(window);
        iconic = !!IsIconic(window);
    }
    const int written = snprintf(output + used, capacity - used,
        " %s={%p pid=%lu tid=%lu class=%s visible=%d iconic=%d style=0x%llx exstyle=0x%llx}",
        label, (void*)window, (unsigned long)process_id, (unsigned long)thread_id,
        class_name, visible, iconic,
        (unsigned long long)(ULONG_PTR)style,
        (unsigned long long)(ULONG_PTR)extended_style);
    if (written < 0 || (size_t)written >= capacity - used) return capacity;
    return used + (size_t)written;
}

static void trace_wait_sample(
    const char* label, HWND cached_console, HWND cached_terminal,
    int expected_iconic, int actual_iconic, const char* sample) {
    if (!console_diag_trace_path[0]) return;
    HWND fresh_console = GetConsoleWindow();
    HWND fresh_terminal = fresh_console ? GetAncestor(fresh_console, GA_ROOTOWNER) : NULL;
    char line[CONSOLE_DIAG_LINE_CAPACITY];
    const int prefix_length = snprintf(line, sizeof(line),
        "test tick=%llu wait=%s sample=%s expected_iconic=%d sampled_iconic=%d cached_iconic=%d fresh_iconic=%d",
        (unsigned long long)GetTickCount64(), label, sample, expected_iconic, actual_iconic,
        cached_terminal ? !!IsIconic(cached_terminal) : 0,
        fresh_terminal ? !!IsIconic(fresh_terminal) : 0);
    if (prefix_length < 0 || (size_t)prefix_length >= sizeof(line)) return;
    size_t length = (size_t)prefix_length;
    length = append_window_snapshot(line, sizeof(line), length, "cached_console", cached_console);
    length = append_window_snapshot(line, sizeof(line), length, "cached_terminal", cached_terminal);
    length = append_window_snapshot(line, sizeof(line), length, "fresh_console", fresh_console);
    length = append_window_snapshot(line, sizeof(line), length, "fresh_root_owner", fresh_terminal);
    if (length >= sizeof(line) - 1) return;
    line[length++] = '\n';
    write_console_diag_line(line, length);
}

static int wait_iconic(
    const char* label, HWND cached_console, HWND window, int expected, DWORD timeout_ms) {
    ULONGLONG deadline = GetTickCount64() + timeout_ms;
    int previous_iconic = -1;
    do {
        const int actual_iconic = !!IsIconic(window);
        if (actual_iconic != previous_iconic) {
            trace_wait_sample(label, cached_console, window, expected, actual_iconic, "transition");
            previous_iconic = actual_iconic;
        }
        if (actual_iconic == expected) return 1;
        Sleep(10);
    } while (GetTickCount64() < deadline);
    trace_wait_sample(label, cached_console, window, expected, !!IsIconic(window), "timeout");
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
    ShowWindow(terminal, SW_RESTORE);
    if (!wait_iconic("initial-restore", console, terminal, 0, 1000)) return 1;

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
    } else if (!wait_iconic("launch-minimize", console, terminal, 1, ICONIFY_TIMEOUT_MS)) {
        return 4;
    }
    if (!hidden && !game_visible(title)) return 11;
    if (hidden && game_visible(title)) return 16;

    if (hidden) {
        stasis_shutdown();
        if (!stasis_init_window(320, 240, title)) return 5;
        if (!wait_iconic("hidden-reinit-minimize", console, terminal, 1, ICONIFY_TIMEOUT_MS)) return 6;
        if (!game_visible(title)) return 12;
    }
    ShowWindow(terminal, SW_RESTORE);
    if (!wait_iconic("post-restore", console, terminal, 0, 1000)) return 7;
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

static int create_console_diag_directory(wchar_t* directory, size_t capacity) {
    wchar_t temporary_root[MAX_PATH + 1];
    const DWORD root_length = GetTempPathW((DWORD)(sizeof(temporary_root) / sizeof(temporary_root[0])),
        temporary_root);
    if (!root_length || root_length >= sizeof(temporary_root) / sizeof(temporary_root[0]) ||
        capacity < MAX_PATH) return 0;
    if (!GetTempFileNameW(temporary_root, L"stc", 0, directory)) return 0;
    if (!DeleteFileW(directory)) {
        directory[0] = L'\0';
        return 0;
    }
    if (!CreateDirectoryW(directory, NULL)) {
        directory[0] = L'\0';
        return 0;
    }
    return 1;
}

static int make_console_diag_path(
    const wchar_t* directory, unsigned int child_index, wchar_t* path, size_t capacity) {
    const int length = swprintf(path, capacity, L"%ls\\child-%u.trace", directory, child_index);
    if (length < 0 || (size_t)length >= capacity) {
        path[0] = L'\0';
        return 0;
    }
    return 1;
}

static void print_console_diag_failure(const wchar_t* label, const wchar_t* path) {
    if (!path || !path[0]) return;
    HANDLE file = CreateFileW(path, GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, NULL,
        OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (file == INVALID_HANDLE_VALUE) return;
    LARGE_INTEGER file_size;
    if (!GetFileSizeEx(file, &file_size) || file_size.QuadPart < 0) {
        CloseHandle(file);
        return;
    }

    char contents[CONSOLE_DIAG_MAX_BYTES + 1];
    const DWORD requested = file_size.QuadPart > CONSOLE_DIAG_MAX_BYTES
        ? CONSOLE_DIAG_MAX_BYTES : (DWORD)file_size.QuadPart;
    DWORD bytes_read = 0;
    const BOOL read_ok = ReadFile(file, contents, requested, &bytes_read, NULL);
    CloseHandle(file);
    if (!read_ok) return;

    fprintf(stderr, "STASIS_CONSOLE_DIAG_BEGIN child=%ls bytes=%lu truncated=%d\n",
        label, (unsigned long)bytes_read, file_size.QuadPart > CONSOLE_DIAG_MAX_BYTES);
    size_t line_start = 0;
    for (size_t i = 0; i <= bytes_read; ++i) {
        if (i != bytes_read && contents[i] != '\n') continue;
        size_t line_length = i - line_start;
        if (line_length && contents[line_start + line_length - 1] == '\r') --line_length;
        fprintf(stderr, "STASIS_CONSOLE_DIAG %.*s\n", (int)line_length, contents + line_start);
        line_start = i + 1;
    }
    fprintf(stderr, "STASIS_CONSOLE_DIAG_END child=%ls\n", label);
}

int main(int argc, char** argv) {
    if (argc == 2 && strcmp(argv[1], "conpty") == 0) {
        initialize_console_diag_path();
        return run_conpty_child();
    }
    if (argc == 2) {
        initialize_console_diag_path();
        return run_child(argv[1]);
    }
    wchar_t executable[32768];
    DWORD length = GetModuleFileNameW(NULL, executable, 32768);
    if (!length || length >= 32768) return 1;

    /* Never pass an inherited trace path to a child; each child gets a file in
       the unique directory created below, or has tracing explicitly disabled. */
    if (!SetEnvironmentVariableW(CONSOLE_DIAG_ENV, NULL)) return 1;
    wchar_t diag_directory[32768] = {0};
    const int diag_directory_created = create_console_diag_directory(
        diag_directory, sizeof(diag_directory) / sizeof(diag_directory[0]));
    const wchar_t* modes[] = {L"default", L"opt-out", L"hidden"};
    int skipped = 0;
    for (int i = 0; i < 3; ++i) {
        wchar_t trace_path[32768] = {0};
        const int trace_path_created = diag_directory_created &&
            make_console_diag_path(diag_directory, (unsigned int)i, trace_path,
                sizeof(trace_path) / sizeof(trace_path[0]));
        if (trace_path_created && !SetEnvironmentVariableW(CONSOLE_DIAG_ENV, trace_path)) {
            trace_path[0] = L'\0';
        }
        wchar_t command[32800];
        if (swprintf(command, 32800, L"\"%ls\" %ls", executable, modes[i]) < 0) {
            SetEnvironmentVariableW(CONSOLE_DIAG_ENV, NULL);
            if (trace_path[0]) DeleteFileW(trace_path);
            if (diag_directory_created) RemoveDirectoryW(diag_directory);
            return 1;
        }
        STARTUPINFOW startup = {0};
        PROCESS_INFORMATION process = {0};
        startup.cb = sizeof(startup);
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_SHOWNORMAL;
        const BOOL created = CreateProcessW(executable, command, NULL, NULL, FALSE,
            CREATE_NEW_CONSOLE, NULL, NULL, &startup, &process);
        const DWORD creation_error = created ? ERROR_SUCCESS : GetLastError();
        SetEnvironmentVariableW(CONSOLE_DIAG_ENV, NULL);
        if (!created) {
            fprintf(stderr, "Console test child %ls failed to launch: %lu\n", modes[i], creation_error);
            if (trace_path[0]) DeleteFileW(trace_path);
            if (diag_directory_created) RemoveDirectoryW(diag_directory);
            return 1;
        }
        DWORD status = WaitForSingleObject(process.hProcess, CHILD_PROCESS_TIMEOUT_MS);
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
            print_console_diag_failure(modes[i], trace_path);
            if (trace_path[0]) DeleteFileW(trace_path);
            if (diag_directory_created) RemoveDirectoryW(diag_directory);
            fprintf(stderr, "Console test %ls failed at stage %lu\n", modes[i], result);
            return 1;
        } else {
            fprintf(stdout, "PASS %ls\n", modes[i]);
        }
        if (trace_path[0]) DeleteFileW(trace_path);
    }
    wchar_t conpty_trace_path[32768] = {0};
    const int conpty_trace_path_created = diag_directory_created &&
        make_console_diag_path(diag_directory, 3, conpty_trace_path,
            sizeof(conpty_trace_path) / sizeof(conpty_trace_path[0]));
    if (conpty_trace_path_created &&
        !SetEnvironmentVariableW(CONSOLE_DIAG_ENV, conpty_trace_path)) {
        conpty_trace_path[0] = L'\0';
    }
    const int conpty_result = run_conpty_parent(executable);
    SetEnvironmentVariableW(CONSOLE_DIAG_ENV, NULL);
    if (conpty_result != 0) {
        print_console_diag_failure(L"conpty", conpty_trace_path);
        if (conpty_trace_path[0]) DeleteFileW(conpty_trace_path);
        if (diag_directory_created) RemoveDirectoryW(diag_directory);
        fprintf(stderr, "ConPTY console test did not receive the iconify request\n");
        return 1;
    }
    if (conpty_trace_path[0]) DeleteFileW(conpty_trace_path);
    if (diag_directory_created) RemoveDirectoryW(diag_directory);
    fprintf(stdout, "PASS conpty\n");
    return skipped ? 77 : 0;
}
