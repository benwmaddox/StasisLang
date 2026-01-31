#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#ifdef __APPLE__
#include <crt_externs.h>
#endif
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
#endif

#ifndef STASIS_SYS_MAX_ARGS
#define STASIS_SYS_MAX_ARGS 128
#endif

#ifndef STASIS_SYS_MAX_ARG_BYTES
#define STASIS_SYS_MAX_ARG_BYTES 1024
#endif

static int g_sys_argc = -1;
static char g_sys_argv[STASIS_SYS_MAX_ARGS][STASIS_SYS_MAX_ARG_BYTES];
static int g_sys_argv_len[STASIS_SYS_MAX_ARGS];

static void sys_reset_argv(void)
{
    g_sys_argc = 0;
    memset(g_sys_argv, 0, sizeof(g_sys_argv));
    memset(g_sys_argv_len, 0, sizeof(g_sys_argv_len));
}

static void sys_push_argv(const char *arg)
{
    if (!arg || g_sys_argc < 0 || g_sys_argc >= STASIS_SYS_MAX_ARGS)
    {
        return;
    }

    char *dst = g_sys_argv[g_sys_argc];
    int cap = STASIS_SYS_MAX_ARG_BYTES;
    int len = 0;
    while (arg[len] && len + 1 < cap)
    {
        dst[len] = arg[len];
        len++;
    }
    dst[len] = '\0';
    g_sys_argv_len[g_sys_argc] = len;
    g_sys_argc++;
}

void stasis_sys_set_args(int argc, const char *const *argv)
{
    sys_reset_argv();
    if (!argv || argc <= 0)
    {
        return;
    }

    for (int i = 0; i < argc && g_sys_argc < STASIS_SYS_MAX_ARGS; i++)
    {
        sys_push_argv(argv[i]);
    }
}

static int sys_is_space(char c)
{
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

static void sys_init_argv(void)
{
    if (g_sys_argc >= 0)
    {
        return;
    }

    sys_reset_argv();

#ifdef _WIN32
    const char *cmd = GetCommandLineA();
    if (!cmd)
    {
        return;
    }

    const char *p = cmd;
    while (*p && g_sys_argc < STASIS_SYS_MAX_ARGS)
    {
        while (*p && sys_is_space(*p))
        {
            p++;
        }

        if (!*p)
        {
            break;
        }

        char *dst = g_sys_argv[g_sys_argc];
        int cap = STASIS_SYS_MAX_ARG_BYTES;
        int len = 0;

        if (*p == '"')
        {
            p++;
            while (*p && *p != '"')
            {
                if (len + 1 < cap)
                {
                    dst[len++] = *p;
                }
                p++;
            }
            if (*p == '"')
            {
                p++;
            }
        }
        else
        {
            while (*p && !sys_is_space(*p))
            {
                if (len + 1 < cap)
                {
                    dst[len++] = *p;
                }
                p++;
            }
        }

        dst[len] = '\0';
        g_sys_argv_len[g_sys_argc] = len;
        g_sys_argc++;
    }
#else
#if defined(__APPLE__)
    int argc = 0;
    char **argv = NULL;
    if (_NSGetArgc() && _NSGetArgv())
    {
        argc = *_NSGetArgc();
        argv = *_NSGetArgv();
    }
    if (argc > 0 && argv)
    {
        stasis_sys_set_args(argc, (const char *const *)argv);
    }
#elif defined(__linux__)
    FILE *f = fopen("/proc/self/cmdline", "rb");
    if (!f)
    {
        return;
    }
    unsigned char buf[8192];
    size_t n = fread(buf, 1, sizeof(buf) - 1, f);
    fclose(f);
    if (n == 0)
    {
        return;
    }
    buf[n] = 0;

    size_t i = 0;
    while (i < n && g_sys_argc < STASIS_SYS_MAX_ARGS)
    {
        size_t start = i;
        while (i < n && buf[i] != 0)
        {
            i++;
        }
        if (i > start)
        {
            sys_push_argv((const char *)(buf + start));
        }
        i++;
    }
#endif
#endif
}

int stasis_sys_argc(void)
{
    sys_init_argv();
    return g_sys_argc;
}

int stasis_sys_argv(int idx, unsigned char *out, int out_cap)
{
    if (!out || out_cap <= 0)
    {
        return -1;
    }

    out[0] = 0;

    sys_init_argv();
    if (idx < 0 || idx >= g_sys_argc)
    {
        return -1;
    }

    int len = g_sys_argv_len[idx];
    if (len < 0)
    {
        return -1;
    }

    int copy = len;
    if (copy > out_cap - 1)
    {
        copy = out_cap - 1;
    }

    memcpy(out, g_sys_argv[idx], (size_t)copy);
    out[copy] = 0;
    return copy;
}

int stasis_sys_read_file(const char *path, unsigned char *out, int out_cap)
{
    if (!path || !out || out_cap <= 0)
    {
        return -1;
    }

    out[0] = 0;

    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return -1;
    }

    size_t cap = (size_t)out_cap;
    if (cap == 0)
    {
        fclose(f);
        return -1;
    }

    size_t read = fread(out, 1, cap - 1, f);
    fclose(f);
    out[read] = 0;

    return (int)read;
}

int stasis_sys_write_file(const char *path, const unsigned char *data, int len)
{
    if (!path || !data || len < 0)
    {
        return 0;
    }

    FILE *f = fopen(path, "wb");
    if (!f)
    {
        return 0;
    }

    size_t wrote = fwrite(data, 1, (size_t)len, f);
    fclose(f);
    return wrote == (size_t)len ? 1 : 0;
}

int stasis_sys_file_exists(const char *path)
{
    if (!path)
    {
        return 0;
    }

#ifdef _WIN32
    DWORD attrs = GetFileAttributesA(path);
    return attrs == INVALID_FILE_ATTRIBUTES ? 0 : 1;
#else
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return 0;
    }
    fclose(f);
    return 1;
#endif
}

int stasis_sys_file_size(const char *path)
{
    if (!path)
    {
        return -1;
    }

#ifdef _WIN32
    WIN32_FILE_ATTRIBUTE_DATA data;
    if (!GetFileAttributesExA(path, GetFileExInfoStandard, &data))
    {
        return -1;
    }

    uint64_t size = ((uint64_t)data.nFileSizeHigh << 32) | (uint64_t)data.nFileSizeLow;
    if (size > 0x7fffffffULL)
    {
        return 0x7fffffff;
    }
    return (int)size;
#else
    FILE *f = fopen(path, "rb");
    if (!f)
    {
        return -1;
    }
    if (fseek(f, 0, SEEK_END) != 0)
    {
        fclose(f);
        return -1;
    }
    long size = ftell(f);
    fclose(f);
    if (size < 0)
    {
        return -1;
    }
    if ((uint64_t)size > 0x7fffffffULL)
    {
        return 0x7fffffff;
    }
    return (int)size;
#endif
}

int stasis_sys_file_mtime_ms(const char *path)
{
    if (!path)
    {
        return -1;
    }

#ifdef _WIN32
    WIN32_FILE_ATTRIBUTE_DATA data;
    if (!GetFileAttributesExA(path, GetFileExInfoStandard, &data))
    {
        return -1;
    }

    ULARGE_INTEGER ft;
    ft.LowPart = data.ftLastWriteTime.dwLowDateTime;
    ft.HighPart = data.ftLastWriteTime.dwHighDateTime;

    const uint64_t epoch_diff_100ns = 116444736000000000ULL;
    if (ft.QuadPart < epoch_diff_100ns)
    {
        return -1;
    }

    uint64_t unix_100ns = ft.QuadPart - epoch_diff_100ns;
    uint64_t ms = unix_100ns / 10000ULL;
    // Preserve change detection semantics while staying in i32: wrap into the signed range.
    return (int)(ms & 0x7fffffffULL);
#else
    struct stat st;
    if (stat(path, &st) != 0)
    {
        return -1;
    }

    struct timespec ts;
#ifdef __APPLE__
    ts = st.st_mtimespec;
#elif defined(__linux__) || defined(__unix__) || defined(__posix__) || defined(_POSIX_C_SOURCE)
    ts = st.st_mtim;
#else
    ts.tv_sec = st.st_mtime;
    ts.tv_nsec = 0;
#endif

    uint64_t ms = ((uint64_t)ts.tv_sec * 1000ULL) + ((uint64_t)ts.tv_nsec / 1000000ULL);
    return (int)(ms & 0x7fffffffULL);
#endif
}

int stasis_sys_exec(const char *command)
{
    if (!command)
    {
        return 1;
    }
    return system(command);
}

int stasis_sys_delete_file(const char *path)
{
    if (!path || !*path)
    {
        return 1;
    }
#ifdef _WIN32
    return DeleteFileA(path) ? 0 : 1;
#else
    return unlink(path) == 0 ? 0 : 1;
#endif
}

int stasis_sys_time_ms(void)
{
#ifdef _WIN32
    ULONGLONG ms = GetTickCount64();
    if (ms > 0x7fffffffULL)
    {
        return 0x7fffffff;
    }
    return (int)ms;
#else
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
    {
        return 0;
    }
    long long ms = (long long)ts.tv_sec * 1000LL + (long long)ts.tv_nsec / 1000000LL;
    if (ms > 0x7fffffffLL)
    {
        return 0x7fffffff;
    }
    if (ms < 0)
    {
        return 0;
    }
    return (int)ms;
#endif
}

int stasis_sys_flush(void)
{
    return fflush(NULL) == 0 ? 0 : 1;
}

// Spawn a process without invoking a shell.
// - Accepts a single command line string (the caller is responsible for quoting).
// - Waits for completion and returns the process exit code.
// - Does not support shell operators like pipes/redirection.
int stasis_sys_spawn(const char *command_line)
{
    if (!command_line || !*command_line)
    {
        return 1;
    }

#ifdef _WIN32
    // CreateProcess may mutate the command line buffer.
    const size_t n = strlen(command_line);
    char *mutable_cmd = (char *)malloc(n + 1);
    if (!mutable_cmd)
    {
        return 1;
    }
    memcpy(mutable_cmd, command_line, n + 1);

    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    ZeroMemory(&si, sizeof(si));
    ZeroMemory(&pi, sizeof(pi));
    si.cb = sizeof(si);

    BOOL ok = CreateProcessA(
        NULL,              // application name (use command line parsing + search)
        mutable_cmd,       // command line (mutable)
        NULL,              // process security
        NULL,              // thread security
        FALSE,             // inherit handles
        0,                 // flags
        NULL,              // environment
        NULL,              // current directory
        &si,
        &pi);

    free(mutable_cmd);

    if (!ok)
    {
        // Use 127 as a common "command not found" sentinel (like POSIX shells),
        // and 1 for other failures. We can't perfectly classify here.
        DWORD err = GetLastError();
        if (err == ERROR_FILE_NOT_FOUND || err == ERROR_PATH_NOT_FOUND)
        {
            return 127;
        }
        return 1;
    }

    WaitForSingleObject(pi.hProcess, INFINITE);

    DWORD exit_code = 1;
    GetExitCodeProcess(pi.hProcess, &exit_code);

    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);

    if (exit_code > 0x7fffffffU)
    {
        return 0x7fffffff;
    }
    return (int)exit_code;
#else
    // Minimal POSIX implementation: fork + execvp by splitting on spaces and honoring quotes.
    // This avoids shell invocation, but does not support complex quoting/escaping.
    char *buf = strdup(command_line);
    if (!buf)
    {
        return 1;
    }

    char *argv[STASIS_SYS_MAX_ARGS];
    int argc = 0;
    char *p = buf;
    while (*p && argc + 1 < STASIS_SYS_MAX_ARGS)
    {
        while (*p && sys_is_space(*p))
        {
            p++;
        }
        if (!*p)
        {
            break;
        }

        if (*p == '"')
        {
            p++;
            argv[argc++] = p;
            while (*p && *p != '"')
            {
                p++;
            }
            if (*p == '"')
            {
                *p++ = '\0';
            }
        }
        else
        {
            argv[argc++] = p;
            while (*p && !sys_is_space(*p))
            {
                p++;
            }
            if (*p)
            {
                *p++ = '\0';
            }
        }
    }
    argv[argc] = NULL;

    if (argc == 0 || !argv[0] || !*argv[0])
    {
        free(buf);
        return 1;
    }

    pid_t pid = fork();
    if (pid < 0)
    {
        free(buf);
        return 1;
    }

    if (pid == 0)
    {
        execvp(argv[0], argv);
        _exit(127);
    }

    int status = 0;
    (void)waitpid(pid, &status, 0);
    free(buf);

    if (WIFEXITED(status))
    {
        return WEXITSTATUS(status);
    }
    if (WIFSIGNALED(status))
    {
        return 128 + WTERMSIG(status);
    }
    return 1;
#endif
}

// Spawn a process without waiting. Returns a pid-like positive integer on success, 0 on failure.
int stasis_sys_spawn_async(const char *command_line)
{
    if (!command_line || !*command_line)
    {
        return 0;
    }

#ifdef _WIN32
    const size_t n = strlen(command_line);
    char *mutable_cmd = (char *)malloc(n + 1);
    if (!mutable_cmd)
    {
        return 0;
    }
    memcpy(mutable_cmd, command_line, n + 1);

    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    ZeroMemory(&si, sizeof(si));
    ZeroMemory(&pi, sizeof(pi));
    si.cb = sizeof(si);

    BOOL ok = CreateProcessA(
        NULL,
        mutable_cmd,
        NULL,
        NULL,
        FALSE,
        0,
        NULL,
        NULL,
        &si,
        &pi);

    free(mutable_cmd);

    if (!ok)
    {
        return 0;
    }

    DWORD pid = pi.dwProcessId;
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    if (pid == 0 || pid > 0x7fffffffU)
    {
        return 0;
    }
    return (int)pid;
#else
    char *buf = strdup(command_line);
    if (!buf)
    {
        return 0;
    }

    char *argv[STASIS_SYS_MAX_ARGS];
    int argc = 0;
    char *p = buf;
    while (*p && argc + 1 < STASIS_SYS_MAX_ARGS)
    {
        while (*p && sys_is_space(*p))
        {
            p++;
        }
        if (!*p)
        {
            break;
        }

        if (*p == '"')
        {
            p++;
            argv[argc++] = p;
            while (*p && *p != '"')
            {
                p++;
            }
            if (*p == '"')
            {
                *p++ = '\0';
            }
        }
        else
        {
            argv[argc++] = p;
            while (*p && !sys_is_space(*p))
            {
                p++;
            }
            if (*p)
            {
                *p++ = '\0';
            }
        }
    }
    argv[argc] = NULL;

    if (argc == 0 || !argv[0] || !*argv[0])
    {
        free(buf);
        return 0;
    }

    pid_t pid = fork();
    if (pid < 0)
    {
        free(buf);
        return 0;
    }

    if (pid == 0)
    {
        execvp(argv[0], argv);
        _exit(127);
    }

    free(buf);
    if (pid > 0x7fffffff)
    {
        return 0;
    }
    return (int)pid;
#endif
}

int stasis_sys_sleep_ms(int ms)
{
    if (ms <= 0)
    {
        return 0;
    }

#ifdef _WIN32
    Sleep((DWORD)ms);
    return 0;
#else
    usleep((useconds_t)ms * 1000U);
    return 0;
#endif
}

// Read a single character from stdin. Returns 0 on EOF.
int stasis_sys_read_char(void)
{
    int c = getchar();
    if (c == EOF)
    {
        return 0;
    }
    return c & 0xff;
}

// Read an integer from stdin. Returns 0 on EOF or parse failure.
int stasis_sys_read_int(void)
{
    char buf[64];
    if (!fgets(buf, sizeof(buf), stdin))
    {
        return 0;
    }

    char* end = NULL;
    long v = strtol(buf, &end, 10);
    if (end == buf)
    {
        return 0;
    }
    return (int)v;
}

/*
 * Fixed-arity printf wrapper for Cranelift output.
 *
 * On some ABIs (notably SysV), calling variadic functions requires extra
 * calling convention machinery (register save area). Treating printf as a
 * non-variadic import can crash at runtime, so Cranelift lowers print_* builtins
 * to this fixed signature instead.
 */
int stasis_printf3(const char *fmt, int64_t arg1, int64_t arg2)
{
    if (!fmt)
    {
        return 0;
    }

    /* Count up to two format specifiers (skip %%). */
    int spec_count = 0;
    char specs[2] = {0, 0};
    int lens[2] = {0, 0}; /* 0=default, 1=l, 2=ll */

    for (const char *p = fmt; *p && spec_count < 2; p++)
    {
        if (*p != '%')
        {
            continue;
        }

        p++;
        if (!*p)
        {
            break;
        }
        if (*p == '%')
        {
            continue;
        }

        /* length modifier */
        int len = 0;
        if (*p == 'l')
        {
            len = 1;
            if (p[1] == 'l')
            {
                len = 2;
                p++;
            }
            p++;
        }

        /* specifier */
        char s = *p;
        if (s == 'd' || s == 'i' || s == 'u' || s == 'x' || s == 'X' || s == 'c' || s == 's')
        {
            lens[spec_count] = len;
            specs[spec_count] = s;
            spec_count++;
        }
    }

    if (spec_count == 0)
    {
        return fputs(fmt, stdout) < 0 ? -1 : 0;
    }

    if (spec_count == 1)
    {
        switch (specs[0])
        {
            case 's':
                return printf(fmt, (const char *)(intptr_t)arg1);
            case 'c':
                return printf(fmt, (int)arg1);
            case 'u':
            case 'x':
            case 'X':
                if (lens[0] == 2)
                {
                    return printf(fmt, (unsigned long long)arg1);
                }
                if (lens[0] == 1)
                {
                    return printf(fmt, (unsigned long)arg1);
                }
                return printf(fmt, (unsigned int)arg1);
            case 'd':
            case 'i':
            default:
                if (lens[0] == 2)
                {
                    return printf(fmt, (long long)arg1);
                }
                if (lens[0] == 1)
                {
                    return printf(fmt, (long)arg1);
                }
                return printf(fmt, (int)arg1);
        }
    }

    /* spec_count == 2 */
    if (specs[0] == 's' && specs[1] == 's')
    {
        return printf(fmt, (const char *)(intptr_t)arg1, (const char *)(intptr_t)arg2);
    }
    if (specs[0] == 's' && specs[1] == 'd')
    {
        return printf(fmt, (const char *)(intptr_t)arg1, (int)arg2);
    }
    if (specs[0] == 's' && specs[1] == 'i')
    {
        return printf(fmt, (const char *)(intptr_t)arg1, (int)arg2);
    }
    if ((specs[0] == 'd' || specs[0] == 'i') && (specs[1] == 'd' || specs[1] == 'i'))
    {
        return printf(fmt, (int)arg1, (int)arg2);
    }
    if (specs[0] == 'c' && specs[1] == 'c')
    {
        return printf(fmt, (int)arg1, (int)arg2);
    }

    /* Fallback: treat args as int64. */
    return printf(fmt, (long long)arg1, (long long)arg2);
}

int stasis_sys_list_dir(const char *path, unsigned char *out, int out_cap)
{
    if (!out || out_cap <= 0)
    {
        return -1;
    }
    out[0] = 0;

    if (!path || !*path)
    {
        path = ".";
    }

    int wrote = 0;

#ifdef _WIN32
    char search_path[512];
    snprintf(search_path, sizeof(search_path), "%s\\*", path);

    WIN32_FIND_DATAA find_data;
    HANDLE hFind = FindFirstFileA(search_path, &find_data);
    if (hFind == INVALID_HANDLE_VALUE)
    {
        return -1;
    }

    do
    {
        const char *name = find_data.cFileName;
        if (!name)
        {
            continue;
        }
        if (strcmp(name, ".") == 0 || strcmp(name, "..") == 0)
        {
            continue;
        }

        const int is_dir = (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) ? 1 : 0;
        const char prefix0 = is_dir ? 'D' : 'F';

        int name_len = 0;
        while (name[name_len] != '\0')
        {
            name_len++;
        }

        const int line_len = 2 + name_len + 1;
        if (wrote + line_len >= out_cap)
        {
            break;
        }

        out[wrote + 0] = (unsigned char)prefix0;
        out[wrote + 1] = (unsigned char)' ';
        memcpy(out + wrote + 2, name, (size_t)name_len);
        out[wrote + 2 + name_len] = (unsigned char)'\n';
        wrote += line_len;
    } while (FindNextFileA(hFind, &find_data) != 0);

    FindClose(hFind);
#else
    (void)wrote;
    return -1;
#endif

    if (wrote >= out_cap)
    {
        wrote = out_cap - 1;
    }
    out[wrote] = 0;
    return wrote;
}

void stasis_sys_memcpy_u8(unsigned char *dst, int dst_index, const unsigned char *src, int src_index, int count)
{
    if (!dst || !src || count <= 0 || dst_index < 0 || src_index < 0)
    {
        return;
    }
    memcpy(dst + dst_index, src + src_index, (size_t)count);
}

void stasis_sys_memcpy_i32(int32_t *dst, int dst_index, const int32_t *src, int src_index, int count)
{
    if (!dst || !src || count <= 0 || dst_index < 0 || src_index < 0)
    {
        return;
    }
    memcpy(dst + dst_index, src + src_index, (size_t)count * sizeof(int32_t));
}

void stasis_sys_memcpy_f32(float *dst, int dst_index, const float *src, int src_index, int count)
{
    if (!dst || !src || count <= 0 || dst_index < 0 || src_index < 0)
    {
        return;
    }
    memcpy(dst + dst_index, src + src_index, (size_t)count * sizeof(float));
}

void stasis_sys_memmove_u8(unsigned char *dst, int dst_index, const unsigned char *src, int src_index, int count)
{
    if (!dst || !src || count <= 0 || dst_index < 0 || src_index < 0)
    {
        return;
    }
    memmove(dst + dst_index, src + src_index, (size_t)count);
}

void stasis_sys_memmove_i32(int32_t *dst, int dst_index, const int32_t *src, int src_index, int count)
{
    if (!dst || !src || count <= 0 || dst_index < 0 || src_index < 0)
    {
        return;
    }
    memmove(dst + dst_index, src + src_index, (size_t)count * sizeof(int32_t));
}

void stasis_sys_memmove_f32(float *dst, int dst_index, const float *src, int src_index, int count)
{
    if (!dst || !src || count <= 0 || dst_index < 0 || src_index < 0)
    {
        return;
    }
    memmove(dst + dst_index, src + src_index, (size_t)count * sizeof(float));
}

void stasis_sys_memset_u8(unsigned char *dst, int dst_index, int value, int count)
{
    if (!dst || count <= 0 || dst_index < 0)
    {
        return;
    }
    memset(dst + dst_index, (unsigned char)value, (size_t)count);
}

void stasis_sys_memset_i32(int32_t *dst, int dst_index, int32_t value, int count)
{
    if (!dst || count <= 0 || dst_index < 0)
    {
        return;
    }
    if (value == 0)
    {
        memset(dst + dst_index, 0, (size_t)count * sizeof(int32_t));
        return;
    }
    for (int i = 0; i < count; i++)
    {
        dst[dst_index + i] = value;
    }
}

void stasis_sys_memset_f32(float *dst, int dst_index, float value, int count)
{
    if (!dst || count <= 0 || dst_index < 0)
    {
        return;
    }
    if (value == 0.0f)
    {
        memset(dst + dst_index, 0, (size_t)count * sizeof(float));
        return;
    }
    for (int i = 0; i < count; i++)
    {
        dst[dst_index + i] = value;
    }
}
