#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <unistd.h>
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

static int sys_is_space(char c)
{
    return c == ' ' || c == '\t' || c == '\r' || c == '\n';
}

#ifdef _WIN32
static void sys_init_argv(void)
{
    if (g_sys_argc >= 0)
    {
        return;
    }

    g_sys_argc = 0;
    memset(g_sys_argv, 0, sizeof(g_sys_argv));
    memset(g_sys_argv_len, 0, sizeof(g_sys_argv_len));

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
}
#endif

int stasis_sys_argc(void)
{
#ifdef _WIN32
    sys_init_argv();
    return g_sys_argc;
#else
    return 0;
#endif
}

int stasis_sys_argv(int idx, unsigned char *out, int out_cap)
{
    if (!out || out_cap <= 0)
    {
        return -1;
    }

    out[0] = 0;

#ifdef _WIN32
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
#else
    (void)idx;
    return -1;
#endif
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
    if (ms > 0x7fffffffULL)
    {
        return 0x7fffffff;
    }
    return (int)ms;
#else
    return -1;
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
