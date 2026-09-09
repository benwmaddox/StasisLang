#include <windows.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <wchar.h>

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
        !SetEnvironmentVariableA("STASIS_WINDOW_HIDDEN", "0") ||
        !SetEnvironmentVariableA("STASIS_WINDOW_START_MINIMIZED", "0") ||
        !SetEnvironmentVariableA("STASIS_RECORDING_PRESENTATION", "0") ||
        !SetEnvironmentVariableA("SDL_VIDEODRIVER", "windows") ||
        !SetEnvironmentVariableA("SDL_RENDER_DRIVER", "software") ||
        !SetEnvironmentVariableA("SDL_AUDIODRIVER", "dummy")) return 14;
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

int main(int argc, char** argv) {
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
    return skipped ? 77 : 0;
}
