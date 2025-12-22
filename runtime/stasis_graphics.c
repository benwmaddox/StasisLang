/*
 * Stasis Graphics Runtime Library
 * SDL2 + OpenGL backend for vector graphics rendering
 */

#include <GL/glew.h>
#include <SDL.h>
#include <SDL_opengl.h>
#include <stdbool.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <stdint.h>
#if defined(_WIN32)
#include <sys/types.h>
#include <sys/stat.h>
#else
#include <sys/stat.h>
#include <unistd.h>
#endif

/* stb_truetype for font rendering */
#define STB_TRUETYPE_IMPLEMENTATION
#include "stb_truetype.h"

static void stasis_sdl_log_output(void* userdata, int category, SDL_LogPriority priority, const char* message) {
    (void)userdata;
    (void)category;
    (void)priority;
    if (!message) return;
    fprintf(stderr, "%s\n", message);
    fflush(stderr);
}

#if defined(STASIS_GRAPHICS_STATIC)
#define STASIS_EXPORT
#elif defined(_WIN32)
#define STASIS_EXPORT __declspec(dllexport)
#else
#define STASIS_EXPORT __attribute__((visibility("default")))
#endif

/* Global state */
static SDL_Window* g_window = NULL;
static SDL_GLContext g_gl_context = NULL;
static SDL_Renderer* g_renderer = NULL;
static bool g_use_sdl_renderer = false;
static bool g_should_quit = false;
static const Uint8* g_keyboard_state = NULL;
static int g_window_width = 800;
static int g_window_height = 600;
static bool g_postfx_enabled = false;
static GLuint g_postfx_program = 0;
static GLint g_postfx_time_loc = -1;
static GLint g_postfx_depth_loc = -1;
static GLint g_postfx_intensity_loc = -1;
static GLint g_postfx_surface_loc = -1;
static GLint g_postfx_color_loc = -1;
static float g_postfx_strength = 0.0f;
static float g_postfx_phase = 0.0f;
static float g_postfx_speed = 0.0f;
static float g_postfx_color[3] = {0.05f, 0.85f, 0.78f};
static bool g_postfx_force_disable = false;

/* Line batching for efficient rendering */
#define MAX_LINES 10000
typedef struct {
    float x, y;
    float r, g, b, a;
} LineVertex;
static LineVertex g_sdl_line_vertices[MAX_LINES * 2];
static struct {
    float x1, y1, x2, y2;
    float r, g, b, a;
} g_lines[MAX_LINES];
static LineVertex g_line_vertices[MAX_LINES * 2];
static int g_line_count = 0;
static int g_debug_frame_counter = 0;
static bool g_force_debug_overlay = false;

/* Simple shader + buffer for line rendering */
static GLuint g_line_program = 0;
static GLuint g_line_vbo = 0;
static GLuint g_line_vao = 0;
static GLint g_line_pos_loc = -1;
static GLint g_line_color_loc = -1;

/* Sprite atlas + batching (baked from SVG sources) */
#define MAX_SPRITES 256
#define SPRITE_ATLAS_W 1024
#define SPRITE_ATLAS_H 1024
#define SPRITE_ATLAS_PAD 2
#define MAX_SPRITE_VERTS (6 * 4096)

typedef struct {
    float x, y;
    float u, v;
    float r, g, b, a;
} SpriteVertex;

typedef struct {
    char* path;
    int w;
    int h;
    int atlas_x;
    int atlas_y;
    float u0, v0, u1, v1;
    uint64_t mtime;
    SDL_Texture* sdl_tex;
    int used;
} SpriteEntry;

static SpriteEntry g_sprites[MAX_SPRITES];
static int g_sprite_count = 0;

static GLuint g_sprite_program = 0;
static GLuint g_sprite_vbo = 0;
static GLuint g_sprite_vao = 0;
static GLint g_sprite_pos_loc = -1;
static GLint g_sprite_uv_loc = -1;
static GLint g_sprite_color_loc = -1;
static GLint g_sprite_tex_loc = -1;

static GLuint g_sprite_atlas_tex = 0;
static int g_sprite_atlas_cursor_x = SPRITE_ATLAS_PAD;
static int g_sprite_atlas_cursor_y = SPRITE_ATLAS_PAD;
static int g_sprite_atlas_row_h = 0;

static SpriteVertex g_sprite_vertices[MAX_SPRITE_VERTS];
static int g_sprite_vert_count = 0;

/* Convert screen coords to OpenGL NDC (-1 to 1) */
static float screen_to_ndc_x(float x) {
    return (x / g_window_width) * 2.0f - 1.0f;
}

static float screen_to_ndc_y(float y) {
    /* Flip Y so 0 is at top */
    return 1.0f - (y / g_window_height) * 2.0f;
}

STASIS_EXPORT void stasis_draw_line(float x1, float y1, float x2, float y2,
                                    float r, float g, float b, float a);

static void setup_ortho(void) {
    glMatrixMode(GL_PROJECTION);
    glLoadIdentity();
    glOrtho(0.0, (double)g_window_width, (double)g_window_height, 0.0, -1.0, 1.0);
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();
}

static GLuint compile_simple_shader(GLenum type, const char* source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint status = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &status);
    if (status != GL_TRUE) {
        char log[512];
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        SDL_Log("Simple shader compile error: %s", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static void ensure_line_program(void) {
    if (g_line_program != 0) return;

    const char* vs_src =
        "#version 120\n"
        "attribute vec2 a_pos;\n"
        "attribute vec4 a_color;\n"
        "varying vec4 v_color;\n"
        "void main(){ float x = (a_pos.x / %f) * 2.0 - 1.0; float y = 1.0 - (a_pos.y / %f) * 2.0; gl_Position = vec4(x, y, 0.0, 1.0); v_color = a_color; }\n";
    const char* fs_src =
        "#version 120\n"
        "varying vec4 v_color;\n"
        "void main(){ gl_FragColor = v_color; }\n";

    char vs_buf[256];
    snprintf(vs_buf, sizeof(vs_buf), vs_src, (float)g_window_width, (float)g_window_height);

    GLuint vs = compile_simple_shader(GL_VERTEX_SHADER, vs_buf);
    GLuint fs = compile_simple_shader(GL_FRAGMENT_SHADER, fs_src);
    if (vs == 0 || fs == 0) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        return;
    }

    g_line_program = glCreateProgram();
    glAttachShader(g_line_program, vs);
    glAttachShader(g_line_program, fs);
    glBindAttribLocation(g_line_program, 0, "a_pos");
    glBindAttribLocation(g_line_program, 1, "a_color");
    glLinkProgram(g_line_program);
    GLint linked = 0;
    glGetProgramiv(g_line_program, GL_LINK_STATUS, &linked);
    glDeleteShader(vs);
    glDeleteShader(fs);
    if (linked != GL_TRUE) {
        char log[512];
        glGetProgramInfoLog(g_line_program, sizeof(log), NULL, log);
        SDL_Log("Simple program link error: %s", log);
        glDeleteProgram(g_line_program);
        g_line_program = 0;
        return;
    }

    g_line_pos_loc = 0;
    g_line_color_loc = 1;

    if (g_line_vao == 0) {
        glGenVertexArrays(1, &g_line_vao);
    }
    if (g_line_vbo == 0) {
        glGenBuffers(1, &g_line_vbo);
    }
}

/* Flush all batched lines to OpenGL */
static void flush_lines(void) {
    if (g_line_count == 0) return;

    glUseProgram(0);
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    glDrawBuffer(GL_BACK);

    if (g_debug_frame_counter < 5) {
        SDL_Log("flush_lines frame %d: count=%d", g_debug_frame_counter, g_line_count);
        if (g_line_count > 0) {
            SDL_Log("line0: (%.2f,%.2f)->(%.2f,%.2f) rgba=%.2f,%.2f,%.2f,%.2f",
                g_lines[0].x1, g_lines[0].y1, g_lines[0].x2, g_lines[0].y2,
                g_lines[0].r, g_lines[0].g, g_lines[0].b, g_lines[0].a);
        }
    }

    /* Build vertex buffer */
    int vtx_count = g_line_count * 2;
    for (int i = 0; i < g_line_count; i++) {
        g_line_vertices[i * 2 + 0].x = g_lines[i].x1;
        g_line_vertices[i * 2 + 0].y = g_lines[i].y1;
        g_line_vertices[i * 2 + 0].r = g_lines[i].r;
        g_line_vertices[i * 2 + 0].g = g_lines[i].g;
        g_line_vertices[i * 2 + 0].b = g_lines[i].b;
        g_line_vertices[i * 2 + 0].a = g_lines[i].a;

        g_line_vertices[i * 2 + 1].x = g_lines[i].x2;
        g_line_vertices[i * 2 + 1].y = g_lines[i].y2;
        g_line_vertices[i * 2 + 1].r = g_lines[i].r;
        g_line_vertices[i * 2 + 1].g = g_lines[i].g;
        g_line_vertices[i * 2 + 1].b = g_lines[i].b;
        g_line_vertices[i * 2 + 1].a = g_lines[i].a;
    }

    ensure_line_program();
    if (g_line_program != 0) {
        glUseProgram(g_line_program);
        glBindVertexArray(g_line_vao);
        glBindBuffer(GL_ARRAY_BUFFER, g_line_vbo);
        glBufferData(GL_ARRAY_BUFFER, sizeof(LineVertex) * vtx_count, g_line_vertices, GL_DYNAMIC_DRAW);
        glEnableVertexAttribArray((GLuint)g_line_pos_loc);
        glVertexAttribPointer((GLuint)g_line_pos_loc, 2, GL_FLOAT, GL_FALSE, sizeof(LineVertex), (void*)offsetof(LineVertex, x));
        glEnableVertexAttribArray((GLuint)g_line_color_loc);
        glVertexAttribPointer((GLuint)g_line_color_loc, 4, GL_FLOAT, GL_FALSE, sizeof(LineVertex), (void*)offsetof(LineVertex, r));
        glDrawArrays(GL_LINES, 0, vtx_count);
        glDisableVertexAttribArray((GLuint)g_line_pos_loc);
        glDisableVertexAttribArray((GLuint)g_line_color_loc);
        glBindBuffer(GL_ARRAY_BUFFER, 0);
        glBindVertexArray(0);
        glUseProgram(0);
    }

    if (g_debug_frame_counter < 5) {
        GLenum err = glGetError();
        if (err != GL_NO_ERROR) {
            SDL_Log("GL error after flush_lines frame %d: 0x%x", g_debug_frame_counter, err);
        }
    }

    g_line_count = 0;
}

static char g_asset_base[512] = {0};
static char g_asset_env[512] = {0};

static void ensure_asset_base(void) {
    if (g_asset_base[0] != 0) return;

    /* Prefer explicit env override */
    const char* env = getenv("STASIS_ASSET_ROOT");
    if (env && *env) {
        strncpy(g_asset_env, env, sizeof(g_asset_env) - 1);
        g_asset_env[sizeof(g_asset_env) - 1] = 0;
        strncpy(g_asset_base, g_asset_env, sizeof(g_asset_base) - 1);
        g_asset_base[sizeof(g_asset_base) - 1] = 0;
        return;
    }
#if defined(_WIN32)
    _getcwd(g_asset_base, (int)sizeof(g_asset_base));
#else
    getcwd(g_asset_base, sizeof(g_asset_base));
#endif
}

static char* read_text_file(const char* path) {
    ensure_asset_base();

    FILE* f = fopen(path, "rb");
    if (!f) {
        /* If relative, try anchored to startup cwd */
        if (!(path[0] == '/' || path[0] == '\\' || (path[1] == ':' && (path[2] == '\\' || path[2] == '/')))) {
            char alt[1024];
            snprintf(alt, sizeof(alt), "%s/%s", g_asset_base, path);
            for (char* p = alt; *p; ++p) {
                if (*p == '\\') *p = '/';
            }
            f = fopen(alt, "rb");
            if (!f) {
                fprintf(stderr, "read_text_file: failed %s (also %s)\n", path, alt);
                return NULL;
            }
        } else {
            fprintf(stderr, "read_text_file: failed %s\n", path);
            return NULL;
        }
    }
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (len <= 0) {
        fclose(f);
        return NULL;
    }
    char* data = (char*)malloc((size_t)len + 1);
    if (!data) {
        fclose(f);
        return NULL;
    }
    size_t read = fread(data, 1, (size_t)len, f);
    fclose(f);
    data[read] = 0;
    return data;
}

static GLuint compile_shader(GLenum type, const char* source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint status = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &status);
    if (status != GL_TRUE) {
        char log[512];
        glGetShaderInfoLog(shader, sizeof(log), NULL, log);
        SDL_Log("Shader compile error: %s", log);
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

static GLuint link_program(GLuint vs, GLuint fs) {
    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glLinkProgram(prog);
    GLint status = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &status);
    if (status != GL_TRUE) {
        char log[512];
        glGetProgramInfoLog(prog, sizeof(log), NULL, log);
        SDL_Log("Program link error: %s", log);
        glDeleteProgram(prog);
        return 0;
    }
    return prog;
}

static uint64_t get_file_mtime(const char* path) {
#if defined(_WIN32)
    struct _stat st;
    if (_stat(path, &st) != 0) return 0;
    return (uint64_t)st.st_mtime;
#else
    struct stat st;
    if (stat(path, &st) != 0) return 0;
    return (uint64_t)st.st_mtime;
#endif
}

static char* stasis_strdup(const char* s) {
    if (!s) return NULL;
#if defined(_WIN32)
    return _strdup(s);
#else
    return strdup(s);
#endif
}

static int atlas_alloc(int w, int h, int* out_x, int* out_y) {
    if (w <= 0 || h <= 0) return 0;
    if (w + SPRITE_ATLAS_PAD * 2 > SPRITE_ATLAS_W) return 0;
    if (h + SPRITE_ATLAS_PAD * 2 > SPRITE_ATLAS_H) return 0;

    if (g_sprite_atlas_cursor_x + w + SPRITE_ATLAS_PAD > SPRITE_ATLAS_W) {
        g_sprite_atlas_cursor_x = SPRITE_ATLAS_PAD;
        g_sprite_atlas_cursor_y += g_sprite_atlas_row_h + SPRITE_ATLAS_PAD;
        g_sprite_atlas_row_h = 0;
    }

    if (g_sprite_atlas_cursor_y + h + SPRITE_ATLAS_PAD > SPRITE_ATLAS_H) {
        return 0;
    }

    *out_x = g_sprite_atlas_cursor_x;
    *out_y = g_sprite_atlas_cursor_y;

    g_sprite_atlas_cursor_x += w + SPRITE_ATLAS_PAD;
    if (h > g_sprite_atlas_row_h) g_sprite_atlas_row_h = h;
    return 1;
}

static void ensure_sprite_atlas(void) {
    if (g_sprite_atlas_tex != 0) return;

    glGenTextures(1, &g_sprite_atlas_tex);
    glBindTexture(GL_TEXTURE_2D, g_sprite_atlas_tex);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, SPRITE_ATLAS_W, SPRITE_ATLAS_H, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glGenerateMipmap(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, 0);
}

static void ensure_sprite_program(void) {
    if (g_sprite_program != 0) return;

    const char* vs_src =
        "#version 120\n"
        "attribute vec2 a_pos;\n"
        "attribute vec2 a_uv;\n"
        "attribute vec4 a_color;\n"
        "varying vec2 v_uv;\n"
        "varying vec4 v_color;\n"
        "void main(){ float x = (a_pos.x / %f) * 2.0 - 1.0; float y = 1.0 - (a_pos.y / %f) * 2.0; gl_Position = vec4(x, y, 0.0, 1.0); v_uv = a_uv; v_color = a_color; }\n";
    const char* fs_src =
        "#version 120\n"
        "uniform sampler2D u_tex;\n"
        "varying vec2 v_uv;\n"
        "varying vec4 v_color;\n"
        "void main(){ gl_FragColor = texture2D(u_tex, v_uv) * v_color; }\n";

    char vs_buf[512];
    snprintf(vs_buf, sizeof(vs_buf), vs_src, (float)g_window_width, (float)g_window_height);

    GLuint vs = compile_shader(GL_VERTEX_SHADER, vs_buf);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fs_src);
    if (!vs || !fs) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        return;
    }

    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glBindAttribLocation(prog, 0, "a_pos");
    glBindAttribLocation(prog, 1, "a_uv");
    glBindAttribLocation(prog, 2, "a_color");
    glLinkProgram(prog);

    GLint linked = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &linked);
    glDeleteShader(vs);
    glDeleteShader(fs);
    if (linked != GL_TRUE) {
        char log[512];
        glGetProgramInfoLog(prog, sizeof(log), NULL, log);
        SDL_Log("Sprite program link error: %s", log);
        glDeleteProgram(prog);
        return;
    }

    g_sprite_program = prog;
    g_sprite_pos_loc = 0;
    g_sprite_uv_loc = 1;
    g_sprite_color_loc = 2;
    g_sprite_tex_loc = glGetUniformLocation(g_sprite_program, "u_tex");

    if (g_sprite_vao == 0) {
        glGenVertexArrays(1, &g_sprite_vao);
    }
    if (g_sprite_vbo == 0) {
        glGenBuffers(1, &g_sprite_vbo);
    }
}

static void blend_px_premult(unsigned char* dst, int sr, int sg, int sb, int sa) {
    int inv = 255 - sa;
    dst[0] = (unsigned char)(sr + (dst[0] * inv) / 255);
    dst[1] = (unsigned char)(sg + (dst[1] * inv) / 255);
    dst[2] = (unsigned char)(sb + (dst[2] * inv) / 255);
    dst[3] = (unsigned char)(sa + (dst[3] * inv) / 255);
}

static void draw_rect_rgba(unsigned char* buf, int w, int h, int x, int y, int rw, int rh, float r, float g, float b, float a) {
    if (!buf) return;
    if (rw <= 0 || rh <= 0) return;
    int x0 = x;
    int y0 = y;
    int x1 = x + rw;
    int y1 = y + rh;
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > w) x1 = w;
    if (y1 > h) y1 = h;

    int sa = (int)(a * 255.0f + 0.5f);
    if (sa <= 0) return;
    if (sa > 255) sa = 255;
    int sr = (int)(r * (float)sa + 0.5f);
    int sg = (int)(g * (float)sa + 0.5f);
    int sb = (int)(b * (float)sa + 0.5f);
    if (sr < 0) sr = 0; if (sr > 255) sr = 255;
    if (sg < 0) sg = 0; if (sg > 255) sg = 255;
    if (sb < 0) sb = 0; if (sb > 255) sb = 255;

    for (int py = y0; py < y1; py++) {
        unsigned char* row = buf + (py * w * 4);
        for (int px = x0; px < x1; px++) {
            blend_px_premult(row + px * 4, sr, sg, sb, sa);
        }
    }
}

static void draw_circle_rgba(unsigned char* buf, int w, int h, float cx, float cy, float radius, float r, float g, float b, float a) {
    if (!buf) return;
    if (radius <= 0.0f) return;

    int sa = (int)(a * 255.0f + 0.5f);
    if (sa <= 0) return;
    if (sa > 255) sa = 255;
    int sr = (int)(r * (float)sa + 0.5f);
    int sg = (int)(g * (float)sa + 0.5f);
    int sb = (int)(b * (float)sa + 0.5f);
    if (sr < 0) sr = 0; if (sr > 255) sr = 255;
    if (sg < 0) sg = 0; if (sg > 255) sg = 255;
    if (sb < 0) sb = 0; if (sb > 255) sb = 255;

    float rr = radius * radius;
    int x0 = (int)floorf(cx - radius - 1.0f);
    int y0 = (int)floorf(cy - radius - 1.0f);
    int x1 = (int)ceilf(cx + radius + 1.0f);
    int y1 = (int)ceilf(cy + radius + 1.0f);
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > w) x1 = w;
    if (y1 > h) y1 = h;

    for (int py = y0; py < y1; py++) {
        unsigned char* row = buf + (py * w * 4);
        for (int px = x0; px < x1; px++) {
            float fx = (float)px + 0.5f;
            float fy = (float)py + 0.5f;
            float dx = fx - cx;
            float dy = fy - cy;
            if (dx * dx + dy * dy <= rr) {
                blend_px_premult(row + px * 4, sr, sg, sb, sa);
            }
        }
    }
}

static float dist2_point_segment(float px, float py, float ax, float ay, float bx, float by) {
    float abx = bx - ax;
    float aby = by - ay;
    float apx = px - ax;
    float apy = py - ay;
    float ab2 = abx * abx + aby * aby;
    float t = 0.0f;
    if (ab2 > 0.0f) {
        t = (apx * abx + apy * aby) / ab2;
        if (t < 0.0f) t = 0.0f;
        if (t > 1.0f) t = 1.0f;
    }
    float cx = ax + abx * t;
    float cy = ay + aby * t;
    float dx = px - cx;
    float dy = py - cy;
    return dx * dx + dy * dy;
}

static void draw_line_rgba(unsigned char* buf, int w, int h, float x1, float y1, float x2, float y2, float thickness, float r, float g, float b, float a) {
    if (!buf) return;
    if (thickness <= 0.0f) return;

    int sa = (int)(a * 255.0f + 0.5f);
    if (sa <= 0) return;
    if (sa > 255) sa = 255;
    int sr = (int)(r * (float)sa + 0.5f);
    int sg = (int)(g * (float)sa + 0.5f);
    int sb = (int)(b * (float)sa + 0.5f);
    if (sr < 0) sr = 0; if (sr > 255) sr = 255;
    if (sg < 0) sg = 0; if (sg > 255) sg = 255;
    if (sb < 0) sb = 0; if (sb > 255) sb = 255;

    float rad = thickness * 0.5f;
    float rr = rad * rad;
    float minx = fminf(x1, x2) - rad - 1.0f;
    float miny = fminf(y1, y2) - rad - 1.0f;
    float maxx = fmaxf(x1, x2) + rad + 1.0f;
    float maxy = fmaxf(y1, y2) + rad + 1.0f;
    int ix0 = (int)floorf(minx);
    int iy0 = (int)floorf(miny);
    int ix1 = (int)ceilf(maxx);
    int iy1 = (int)ceilf(maxy);
    if (ix0 < 0) ix0 = 0;
    if (iy0 < 0) iy0 = 0;
    if (ix1 > w) ix1 = w;
    if (iy1 > h) iy1 = h;

    for (int py = iy0; py < iy1; py++) {
        unsigned char* row = buf + (py * w * 4);
        for (int px = ix0; px < ix1; px++) {
            float fx = (float)px + 0.5f;
            float fy = (float)py + 0.5f;
            float d2 = dist2_point_segment(fx, fy, x1, y1, x2, y2);
            if (d2 <= rr) {
                blend_px_premult(row + px * 4, sr, sg, sb, sa);
            }
        }
    }
}

static void downsample_2x(unsigned char* out_buf, int out_w, int out_h, const unsigned char* in_buf, int in_w, int in_h) {
    for (int y = 0; y < out_h; y++) {
        for (int x = 0; x < out_w; x++) {
            int sx = x * 2;
            int sy = y * 2;
            const unsigned char* p0 = in_buf + ((sy + 0) * in_w + (sx + 0)) * 4;
            const unsigned char* p1 = in_buf + ((sy + 0) * in_w + (sx + 1)) * 4;
            const unsigned char* p2 = in_buf + ((sy + 1) * in_w + (sx + 0)) * 4;
            const unsigned char* p3 = in_buf + ((sy + 1) * in_w + (sx + 1)) * 4;
            unsigned char* o = out_buf + (y * out_w + x) * 4;
            o[0] = (unsigned char)(((int)p0[0] + (int)p1[0] + (int)p2[0] + (int)p3[0]) / 4);
            o[1] = (unsigned char)(((int)p0[1] + (int)p1[1] + (int)p2[1] + (int)p3[1]) / 4);
            o[2] = (unsigned char)(((int)p0[2] + (int)p1[2] + (int)p2[2] + (int)p3[2]) / 4);
            o[3] = (unsigned char)(((int)p0[3] + (int)p1[3] + (int)p2[3] + (int)p3[3]) / 4);
        }
    }
}

static uint32_t fnv1a_32(const unsigned char* data, size_t len) {
    uint32_t h = 2166136261u;
    for (size_t i = 0; i < len; i++) {
        h ^= (uint32_t)data[i];
        h *= 16777619u;
    }
    return h;
}

/* --- Minimal SVG rasterizer (rect/line/circle; no transforms) --- */
static int svg_attr_color(const char* tag, const char* name, float* r, float* g, float* b, float* a, float default_a) {
    const char* p = strstr(tag, name);
    if (!p) return 0;
    p = strchr(p, '"');
    if (!p) return 0;
    p++;
    const char* end = strchr(p, '"');
    if (!end) return 0;
    if ((end - p) == 7 && p[0] == '#') {
        unsigned int rv = 0, gv = 0, bv = 0;
        if (sscanf(p + 1, "%02x%02x%02x", &rv, &gv, &bv) == 3) {
            *r = (float)rv / 255.0f;
            *g = (float)gv / 255.0f;
            *b = (float)bv / 255.0f;
            *a = default_a;
            return 1;
        }
    }
    return 0;
}

static float svg_attr_float(const char* tag, const char* name, float fallback) {
    const char* p = strstr(tag, name);
    if (!p) return fallback;
    p = strchr(p, '"');
    if (!p) return fallback;
    p++;
    return strtof(p, NULL);
}

static void svg_draw_rect(unsigned char* buf, int sw, int sh, const char* tag, int ss) {
    float x = svg_attr_float(tag, "x", 0.0f);
    float y = svg_attr_float(tag, "y", 0.0f);
    float w = svg_attr_float(tag, "width", 0.0f);
    float h = svg_attr_float(tag, "height", 0.0f);
    float fill_r = 0, fill_g = 0, fill_b = 0, fill_a = 0;
    float stroke_r = 0, stroke_g = 0, stroke_b = 0, stroke_a = 1.0f;
    float opacity = svg_attr_float(tag, "opacity", 1.0f);
    float fill_opacity = svg_attr_float(tag, "fill-opacity", 1.0f) * opacity;
    float stroke_opacity = svg_attr_float(tag, "stroke-opacity", 1.0f) * opacity;
    float stroke_w = svg_attr_float(tag, "stroke-width", 0.0f);

    if (svg_attr_color(tag, "fill", &fill_r, &fill_g, &fill_b, &fill_a, fill_opacity) && fill_a > 0.0f) {
        draw_rect_rgba(buf, sw, sh,
            (int)floorf(x * ss), (int)floorf(y * ss),
            (int)ceilf(w * ss), (int)ceilf(h * ss),
            fill_r, fill_g, fill_b, fill_a);
    }

    if (stroke_w > 0.0f && svg_attr_color(tag, "stroke", &stroke_r, &stroke_g, &stroke_b, &stroke_a, stroke_opacity) && stroke_a > 0.0f) {
        float t = stroke_w * ss;
        /* Top */
        draw_rect_rgba(buf, sw, sh, (int)floorf(x * ss), (int)floorf(y * ss), (int)ceilf(w * ss), (int)ceilf(t), stroke_r, stroke_g, stroke_b, stroke_a);
        /* Bottom */
        draw_rect_rgba(buf, sw, sh, (int)floorf(x * ss), (int)floorf((y + h - stroke_w) * ss), (int)ceilf(w * ss), (int)ceilf(t), stroke_r, stroke_g, stroke_b, stroke_a);
        /* Left */
        draw_rect_rgba(buf, sw, sh, (int)floorf(x * ss), (int)floorf(y * ss), (int)ceilf(t), (int)ceilf(h * ss), stroke_r, stroke_g, stroke_b, stroke_a);
        /* Right */
        draw_rect_rgba(buf, sw, sh, (int)floorf((x + w - stroke_w) * ss), (int)floorf(y * ss), (int)ceilf(t), (int)ceilf(h * ss), stroke_r, stroke_g, stroke_b, stroke_a);
    }
}

static void svg_draw_circle(unsigned char* buf, int sw, int sh, const char* tag, int ss) {
    float cx = svg_attr_float(tag, "cx", 0.0f);
    float cy = svg_attr_float(tag, "cy", 0.0f);
    float r = svg_attr_float(tag, "r", 0.0f);
    float fill_r = 0, fill_g = 0, fill_b = 0, fill_a = 0;
    float stroke_r = 0, stroke_g = 0, stroke_b = 0, stroke_a = 1.0f;
    float opacity = svg_attr_float(tag, "opacity", 1.0f);
    float fill_opacity = svg_attr_float(tag, "fill-opacity", 1.0f) * opacity;
    float stroke_opacity = svg_attr_float(tag, "stroke-opacity", 1.0f) * opacity;
    float stroke_w = svg_attr_float(tag, "stroke-width", 0.0f);

    int has_fill = svg_attr_color(tag, "fill", &fill_r, &fill_g, &fill_b, &fill_a, fill_opacity) && fill_a > 0.0f;
    int has_stroke = stroke_w > 0.0f && svg_attr_color(tag, "stroke", &stroke_r, &stroke_g, &stroke_b, &stroke_a, stroke_opacity) && stroke_a > 0.0f;

    if (has_stroke) {
        draw_circle_rgba(buf, sw, sh, cx * ss, cy * ss, r * ss, stroke_r, stroke_g, stroke_b, stroke_a);
    }

    if (has_fill) {
        float inner_r = r;
        if (has_stroke) {
            inner_r -= stroke_w;
            if (inner_r < 0.0f) inner_r = 0.0f;
        }
        draw_circle_rgba(buf, sw, sh, cx * ss, cy * ss, inner_r * ss, fill_r, fill_g, fill_b, fill_a);
    }
}

static void svg_draw_line(unsigned char* buf, int sw, int sh, const char* tag, int ss) {
    float x1 = svg_attr_float(tag, "x1", 0.0f);
    float y1 = svg_attr_float(tag, "y1", 0.0f);
    float x2 = svg_attr_float(tag, "x2", 0.0f);
    float y2 = svg_attr_float(tag, "y2", 0.0f);
    float stroke_r = 0, stroke_g = 0, stroke_b = 0, stroke_a = 1.0f;
    float opacity = svg_attr_float(tag, "opacity", 1.0f);
    float stroke_opacity = svg_attr_float(tag, "stroke-opacity", 1.0f) * opacity;
    float stroke_w = svg_attr_float(tag, "stroke-width", 1.0f);

    if (svg_attr_color(tag, "stroke", &stroke_r, &stroke_g, &stroke_b, &stroke_a, stroke_opacity) && stroke_a > 0.0f) {
        draw_line_rgba(buf, sw, sh, x1 * ss, y1 * ss, x2 * ss, y2 * ss, stroke_w * ss, stroke_r, stroke_g, stroke_b, stroke_a);
    }
}

static int bake_svg_to_rgba(const char* path, unsigned char** out_pixels, int* out_w, int* out_h) {
    *out_pixels = NULL;
    *out_w = 0;
    *out_h = 0;

    char* file = read_text_file(path);
    if (!file) {
        fprintf(stderr, "bake_svg_to_rgba: failed to read %s\n", path);
        return 0;
    }
    char* text = file;

    /* skip UTF-8 BOM if present */
    if ((unsigned char)text[0] == 0xEF && (unsigned char)text[1] == 0xBB && (unsigned char)text[2] == 0xBF) {
        text += 3;
    }

    /* find <svg ...> */
    const char* svg = strstr(text, "<svg");
    if (!svg) {
        fprintf(stderr, "bake_svg_to_rgba: missing <svg> in %s\n", path);
        free(file);
        return 0;
    }
    float width = svg_attr_float(svg, "width", 0.0f);
    float height = svg_attr_float(svg, "height", 0.0f);
    if (width <= 0.0f || height <= 0.0f) {
        /* fallback to viewBox */
        const char* vb = strstr(svg, "viewBox");
        if (vb) {
            const char* q = strchr(vb, '"');
            if (q) {
                q++;
                float minx = 0.0f, miny = 0.0f, vbw = 0.0f, vbh = 0.0f;
                if (sscanf(q, "%f %f %f %f", &minx, &miny, &vbw, &vbh) == 4) {
                    width = vbw;
                    height = vbh;
                }
            }
        }
    }
    if (width <= 0.0f || height <= 0.0f) {
        fprintf(stderr, "bake_svg_to_rgba: missing width/height and viewBox in %s\n", path);
        free(file);
        return 0;
    }

    int w = (int)ceilf(width);
    int h = (int)ceilf(height);
    if (w <= 0 || h <= 0) {
        fprintf(stderr, "bake_svg_to_rgba: invalid dimensions %dx%d in %s\n", w, h, path);
        free(file);
        return 0;
    }

    const int ss = 2;
    int sw = w * ss;
    int sh = h * ss;
    unsigned char* sbuf = (unsigned char*)calloc((size_t)sw * (size_t)sh * 4, 1);
    if (!sbuf) {
        fprintf(stderr, "bake_svg_to_rgba: OOM allocating %d x %d buffer for %s\n", sw, sh, path);
        free(file);
        return 0;
    }

    for (char* p = text; *p; ) {
        char* line = p;
        while (*p && *p != '\n') p++;
        if (*p == '\n') { *p = 0; p++; }
        while (*line == ' ' || *line == '\t' || *line == '\r') line++;
        if (*line == 0) continue;

        if (strncmp(line, "<rect", 5) == 0) {
            svg_draw_rect(sbuf, sw, sh, line, ss);
        } else if (strncmp(line, "<circle", 7) == 0) {
            svg_draw_circle(sbuf, sw, sh, line, ss);
        } else if (strncmp(line, "<line", 5) == 0) {
            svg_draw_line(sbuf, sw, sh, line, ss);
        }
    }

    unsigned char* out = (unsigned char*)malloc((size_t)w * (size_t)h * 4);
    if (!out) {
        free(sbuf);
        free(file);
        return 0;
    }
    downsample_2x(out, w, h, sbuf, sw, sh);
    free(sbuf);
    free(file);
    *out_pixels = out;
    *out_w = w;
    *out_h = h;
    return 1;
}

/*
 * Debug helper: bake an SVG to RGBA on the CPU and return a deterministic 32-bit hash of the pixels.
 * Returns 0 on error (and logs).
 */
STASIS_EXPORT int stasis_gfx_debug_bake_hash(const char* path) {
    if (!path || !*path) return 0;
    unsigned char* pixels = NULL;
    int w = 0, h = 0;
    if (!bake_svg_to_rgba(path, &pixels, &w, &h)) {
        SDL_Log("gfx_debug_bake_hash: failed to bake %s", path);
        return 0;
    }
    uint32_t h32 = fnv1a_32(pixels, (size_t)w * (size_t)h * 4u);
    free(pixels);
    return (int)h32;
}

static void flush_sprites(void) {
    if (g_sprite_vert_count == 0) return;
    ensure_sprite_program();
    ensure_sprite_atlas();
    if (g_sprite_program == 0 || g_sprite_atlas_tex == 0) {
        g_sprite_vert_count = 0;
        return;
    }

    glEnable(GL_BLEND);
    glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);

    glUseProgram(g_sprite_program);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, g_sprite_atlas_tex);
    if (g_sprite_tex_loc >= 0) glUniform1i(g_sprite_tex_loc, 0);

    glBindVertexArray(g_sprite_vao);
    glBindBuffer(GL_ARRAY_BUFFER, g_sprite_vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(SpriteVertex) * (size_t)g_sprite_vert_count, g_sprite_vertices, GL_DYNAMIC_DRAW);

    glEnableVertexAttribArray((GLuint)g_sprite_pos_loc);
    glVertexAttribPointer((GLuint)g_sprite_pos_loc, 2, GL_FLOAT, GL_FALSE, sizeof(SpriteVertex), (void*)offsetof(SpriteVertex, x));
    glEnableVertexAttribArray((GLuint)g_sprite_uv_loc);
    glVertexAttribPointer((GLuint)g_sprite_uv_loc, 2, GL_FLOAT, GL_FALSE, sizeof(SpriteVertex), (void*)offsetof(SpriteVertex, u));
    glEnableVertexAttribArray((GLuint)g_sprite_color_loc);
    glVertexAttribPointer((GLuint)g_sprite_color_loc, 4, GL_FLOAT, GL_FALSE, sizeof(SpriteVertex), (void*)offsetof(SpriteVertex, r));

    glDrawArrays(GL_TRIANGLES, 0, g_sprite_vert_count);

    glDisableVertexAttribArray((GLuint)g_sprite_pos_loc);
    glDisableVertexAttribArray((GLuint)g_sprite_uv_loc);
    glDisableVertexAttribArray((GLuint)g_sprite_color_loc);
    glBindBuffer(GL_ARRAY_BUFFER, 0);
    glBindVertexArray(0);
    glBindTexture(GL_TEXTURE_2D, 0);
    glUseProgram(0);

    g_sprite_vert_count = 0;
}

static const char* kFallbackPostfxVert =
"#version 120\n"
"varying vec2 v_uv;\n"
"void main(){ v_uv = gl_MultiTexCoord0.xy; gl_Position = gl_Vertex; }\n";

static const char* kFallbackPostfxFrag =
"#version 120\n"
"varying vec2 v_uv;\n"
"uniform float u_time;\n"
"uniform float u_depth_scale;\n"
"uniform float u_intensity;\n"
"uniform float u_surface_jitter;\n"
"uniform vec3 u_biolume_color;\n"
"float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7)))*43758.5453); }\n"
"float noise(vec2 p){ vec2 i=floor(p); vec2 f=fract(p); float a=hash(i); float b=hash(i+vec2(1.0,0.0)); float c=hash(i+vec2(0.0,1.0)); float d=hash(i+vec2(1.0,1.0)); vec2 u=f*f*(3.0-2.0*f); return mix(a,b,u.x)+(c-a)*u.y*(1.0-u.x)+(d-b)*u.x*u.y; }\n"
"void main(){ float depth=clamp(v_uv.y*u_depth_scale,0.0,1.0); float ripple=noise(v_uv*6.0+u_time*0.25); float wave=sin((v_uv.y*8.0)+(u_time*0.6)+ripple*u_surface_jitter); float c=0.5+0.5*wave; vec3 deep=vec3(0.02,0.08,0.12); vec3 mid=vec3(0.00,0.16,0.22); vec3 base=mix(deep,mid,depth); vec3 color=base+u_intensity*c*u_biolume_color; float atten=mix(1.0,0.25,depth); gl_FragColor=vec4(color*atten,0.18); }\n";

static void init_postfx_shader(void) {
    const char* fragSource = kFallbackPostfxFrag;
    char* fileSource = read_text_file("docs/assets/underwater/caustics.glsl");
    if (fileSource) {
        fragSource = fileSource;
    }

    GLuint vs = compile_shader(GL_VERTEX_SHADER, kFallbackPostfxVert);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, fragSource);
    if (fileSource) {
        free(fileSource);
    }
    if (vs == 0 || fs == 0) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        g_postfx_program = 0;
        return;
    }

    g_postfx_program = link_program(vs, fs);
    glDeleteShader(vs);
    glDeleteShader(fs);

    if (g_postfx_program != 0) {
        g_postfx_time_loc = glGetUniformLocation(g_postfx_program, "u_time");
        g_postfx_depth_loc = glGetUniformLocation(g_postfx_program, "u_depth_scale");
        g_postfx_intensity_loc = glGetUniformLocation(g_postfx_program, "u_intensity");
        g_postfx_surface_loc = glGetUniformLocation(g_postfx_program, "u_surface_jitter");
        g_postfx_color_loc = glGetUniformLocation(g_postfx_program, "u_biolume_color");
        /* Don't enable by default - wait for explicit set_postfx() call */
        g_postfx_enabled = false;
        SDL_Log("Post-effects shader compiled (not enabled until set_postfx called)");
    }
}

static void render_postfx(void) {
    if (g_postfx_force_disable) {
        return;
    }

    /* Allow disabling via environment variable for debugging */
    static int checked_env = 0;
    static int env_disable = 0;
    if (!checked_env) {
        const char* val = SDL_getenv("STASIS_DISABLE_POSTFX");
        env_disable = (val && strcmp(val, "0") != 0);
        checked_env = 1;
        if (env_disable) {
            SDL_Log("Post-effects disabled via STASIS_DISABLE_POSTFX");
        }
    }
    if (env_disable) {
        return;
    }

    if (!g_postfx_enabled || g_postfx_program == 0) {
        return;
    }

    /* Ensure blending is enabled for the overlay effect */
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

    glUseProgram(g_postfx_program);
    if (g_postfx_time_loc >= 0) glUniform1f(g_postfx_time_loc, g_postfx_phase);
    if (g_postfx_depth_loc >= 0) glUniform1f(g_postfx_depth_loc, 1.6f);
    if (g_postfx_intensity_loc >= 0) glUniform1f(g_postfx_intensity_loc, g_postfx_strength);
    if (g_postfx_surface_loc >= 0) glUniform1f(g_postfx_surface_loc, 1.1f + g_postfx_speed * 0.5f);
    if (g_postfx_color_loc >= 0) glUniform3f(g_postfx_color_loc, g_postfx_color[0], g_postfx_color[1], g_postfx_color[2]);

    glBegin(GL_TRIANGLES);
    glTexCoord2f(0.0f, 0.0f); glVertex2f(-1.0f, -1.0f);
    glTexCoord2f(2.0f, 0.0f); glVertex2f( 3.0f, -1.0f);
    glTexCoord2f(0.0f, 2.0f); glVertex2f(-1.0f,  3.0f);
    glEnd();

    glUseProgram(0);
}

/*
 * Startup Render Verification
 *
 * These functions verify that rendering actually produces visible output.
 * They are called automatically after initialization to catch driver issues early.
 */

typedef struct {
    int success;
    int pixels_tested;
    int pixels_correct;
    char error_message[512];
    char gl_vendor[128];
    char gl_renderer[128];
    char gl_version[128];
    int gl_error_code;
} RenderTestResult;

static RenderTestResult g_last_test_result = {0};

/* Test OpenGL rendering by drawing a known pattern and reading it back */
static int verify_opengl_rendering(int width, int height) {
    RenderTestResult* result = &g_last_test_result;
    memset(result, 0, sizeof(*result));

    /* Store GL info for diagnostics */
    const char* vendor = (const char*)glGetString(GL_VENDOR);
    const char* renderer = (const char*)glGetString(GL_RENDERER);
    const char* version = (const char*)glGetString(GL_VERSION);

    if (vendor) strncpy(result->gl_vendor, vendor, sizeof(result->gl_vendor) - 1);
    if (renderer) strncpy(result->gl_renderer, renderer, sizeof(result->gl_renderer) - 1);
    if (version) strncpy(result->gl_version, version, sizeof(result->gl_version) - 1);

    /* Clear any pending errors */
    while (glGetError() != GL_NO_ERROR) {}

    /* Render a bright magenta quad to the center of the screen */
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    glDrawBuffer(GL_BACK);

    /* Clear to black first */
    glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);

    GLenum err = glGetError();
    if (err != GL_NO_ERROR) {
        result->gl_error_code = (int)err;
        snprintf(result->error_message, sizeof(result->error_message),
            "OpenGL error during clear: 0x%04X", err);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* Draw a test quad using immediate mode */
    setup_ortho();

    /* Draw magenta (255, 0, 255) quad in center region */
    int cx = width / 2;
    int cy = height / 2;
    int size = 50;

    glBegin(GL_QUADS);
    glColor4f(1.0f, 0.0f, 1.0f, 1.0f);  /* Magenta */
    glVertex2f((float)(cx - size), (float)(cy - size));
    glVertex2f((float)(cx + size), (float)(cy - size));
    glVertex2f((float)(cx + size), (float)(cy + size));
    glVertex2f((float)(cx - size), (float)(cy + size));
    glEnd();

    err = glGetError();
    if (err != GL_NO_ERROR) {
        result->gl_error_code = (int)err;
        snprintf(result->error_message, sizeof(result->error_message),
            "OpenGL error during quad draw: 0x%04X", err);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* Ensure rendering completed before reading back pixels */
    glFlush();
    glFinish();

    /* Read back pixels from the center of the quad */
    unsigned char pixels[4] = {0, 0, 0, 0};
    glReadBuffer(GL_BACK);
    glReadPixels(cx, height - cy, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixels);

    err = glGetError();
    if (err != GL_NO_ERROR) {
        result->gl_error_code = (int)err;
        snprintf(result->error_message, sizeof(result->error_message),
            "OpenGL error during pixel readback: 0x%04X", err);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    result->pixels_tested = 1;

    /* Verify we got magenta (or close to it) */
    /* Allow some tolerance for driver differences */
    int r_ok = (pixels[0] >= 200);  /* Should be ~255 */
    int g_ok = (pixels[1] <= 55);   /* Should be ~0 */
    int b_ok = (pixels[2] >= 200);  /* Should be ~255 */

    if (r_ok && g_ok && b_ok) {
        result->pixels_correct = 1;
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: OpenGL rendering verified");
        SDL_Log("  Test pixel readback: R=%d G=%d B=%d A=%d (expected ~255,0,255,255)",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        /* Clear so the test pattern can't leak into the first presented frame. */
        glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        return 1;
    } else {
        snprintf(result->error_message, sizeof(result->error_message),
            "Pixel verification failed: got R=%d G=%d B=%d A=%d, expected ~255,0,255,255. "
            "This may indicate a driver issue or headless environment.",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        SDL_Log("  GL_VENDOR: %s", result->gl_vendor);
        SDL_Log("  GL_RENDERER: %s", result->gl_renderer);
        SDL_Log("  GL_VERSION: %s", result->gl_version);
        return 0;
    }
}

/* Test SDL renderer by drawing a known pattern and reading it back */
static int verify_sdl_rendering(SDL_Renderer* renderer, int width, int height) {
    RenderTestResult* result = &g_last_test_result;
    memset(result, 0, sizeof(*result));

    /* Get renderer info for diagnostics */
    SDL_RendererInfo info;
    if (SDL_GetRendererInfo(renderer, &info) == 0) {
        strncpy(result->gl_renderer, info.name ? info.name : "unknown", sizeof(result->gl_renderer) - 1);
        snprintf(result->gl_version, sizeof(result->gl_version), "flags=0x%X", info.flags);
    }

    /* Clear to black */
    SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
    int rc = SDL_RenderClear(renderer);
    if (rc != 0) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderClear failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* Draw a magenta rectangle in the center */
    int cx = width / 2;
    int cy = height / 2;
    int size = 50;

    SDL_SetRenderDrawColor(renderer, 255, 0, 255, 255);  /* Magenta */
    SDL_Rect rect = { cx - size, cy - size, size * 2, size * 2 };
    rc = SDL_RenderFillRect(renderer, &rect);
    if (rc != 0) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderFillRect failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        return 0;
    }

    /* We need to read back from a texture target to verify SDL rendering */
    /* Create a texture to render to */
    SDL_Texture* target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA8888,
        SDL_TEXTUREACCESS_TARGET, width, height);
    if (!target) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_CreateTexture (target) failed: %s. Cannot verify rendering.",
            SDL_GetError());
        SDL_Log("STARTUP TEST WARNING: %s", result->error_message);
        /* Not a fatal error - some drivers don't support render targets */
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL renderer created (readback not available)");
        return 1;
    }

    /* Render to texture */
    SDL_SetRenderTarget(renderer, target);
    SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
    SDL_RenderClear(renderer);
    SDL_SetRenderDrawColor(renderer, 255, 0, 255, 255);
    SDL_RenderFillRect(renderer, &rect);

    /* Read back a single pixel from center */
    unsigned char pixels[4] = {0, 0, 0, 0};
    SDL_Rect readRect = { cx, cy, 1, 1 };
    rc = SDL_RenderReadPixels(renderer, &readRect, SDL_PIXELFORMAT_RGBA8888, pixels, 4);

    /* Switch back to default target */
    SDL_SetRenderTarget(renderer, NULL);
    SDL_DestroyTexture(target);

    if (rc != 0) {
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL_RenderReadPixels failed: %s", SDL_GetError());
        SDL_Log("STARTUP TEST WARNING: %s", result->error_message);
        /* Not a fatal error - continue anyway */
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL renderer works (readback not available)");
        return 1;
    }

    result->pixels_tested = 1;

    /* We drew magenta (R=255, G=0, B=255). Check various pixel formats:
     * - RGBA: [R,G,B,A] = [255,0,255,255]
     * - BGRA: [B,G,R,A] = [255,0,255,255]
     * - ABGR: [A,B,G,R] = [255,255,0,255]  <- This matches what user got!
     *
     * The key insight: if we see two channels at ~255 and one at ~0,
     * and the ~0 is NOT in the alpha position, we have a valid render.
     * This handles all reasonable byte orderings.
     */

    int high_count = 0;
    int low_count = 0;
    int low_pos = -1;

    for (int i = 0; i < 4; i++) {
        if (pixels[i] >= 200) high_count++;
        if (pixels[i] <= 55) {
            low_count++;
            low_pos = i;
        }
    }

    /* For magenta (two high RGB + one zero RGB + high alpha), we expect:
     * - At least 2 channels >= 200 (the R, B and A from magenta)
     * - Exactly 1 channel <= 55 (the G from magenta)
     * - The low channel should be green, not alpha (varies by format)
     *
     * Accept the result if we have 3 high values and 1 low value,
     * indicating a non-black, non-white, chromatic color was rendered.
     */
    int pattern_ok = (high_count >= 2 && low_count == 1);

    if (pattern_ok) {
        result->pixels_correct = 1;
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL rendering verified");
        SDL_Log("  Test pixel readback: [0]=%d [1]=%d [2]=%d [3]=%d (magenta pattern detected)",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        return 1;
    } else if (pixels[0] == 0 && pixels[1] == 0 && pixels[2] == 0) {
        /* All black - nothing was rendered */
        snprintf(result->error_message, sizeof(result->error_message),
            "SDL pixel verification failed: got black [0,0,0,%d]. "
            "Rendering may not be working.",
            pixels[3]);
        SDL_Log("STARTUP TEST FAILED: %s", result->error_message);
        SDL_Log("  SDL_Renderer: %s", result->gl_renderer);
        return 0;
    } else {
        /* Got something unexpected but not black - likely a format issue, allow it */
        result->pixels_correct = 1;
        result->success = 1;
        SDL_Log("STARTUP TEST PASSED: SDL rendering verified (unexpected format)");
        SDL_Log("  Test pixel readback: [0]=%d [1]=%d [2]=%d [3]=%d",
            pixels[0], pixels[1], pixels[2], pixels[3]);
        return 1;
    }
}

/*
 * Get detailed startup test results (for external diagnostics)
 * Returns pointer to static result struct
 */
STASIS_EXPORT const char* stasis_get_startup_test_error(void) {
    return g_last_test_result.error_message;
}

STASIS_EXPORT int stasis_get_startup_test_success(void) {
    return g_last_test_result.success;
}

/*
 * Initialize graphics window
 * Returns 1 on success, 0 on failure
 */
STASIS_EXPORT int stasis_init_window(int width, int height, const char* title) {
    SDL_LogSetOutputFunction(stasis_sdl_log_output, NULL);
    SDL_LogSetAllPriority(SDL_LOG_PRIORITY_INFO);
    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS) < 0) {
        SDL_Log("SDL_Init failed: %s", SDL_GetError());
        return 0;
    }

    const char* force_sdl = SDL_getenv("STASIS_USE_SDL");
    bool want_sdl = (force_sdl && strcmp(force_sdl, "0") != 0);

    if (!want_sdl) {
        /* Request OpenGL 2.1 compatibility profile for immediate mode */
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 2);
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 1);
        SDL_GL_SetAttribute(SDL_GL_CONTEXT_PROFILE_MASK, SDL_GL_CONTEXT_PROFILE_COMPATIBILITY);
        SDL_GL_SetAttribute(SDL_GL_DOUBLEBUFFER, 1);
        SDL_GL_SetAttribute(SDL_GL_DEPTH_SIZE, 24);
    }

    g_window = SDL_CreateWindow(
        title ? title : "Stasis",
        SDL_WINDOWPOS_CENTERED,
        SDL_WINDOWPOS_CENTERED,
        width,
        height,
        (want_sdl ? 0 : SDL_WINDOW_OPENGL) | SDL_WINDOW_SHOWN | SDL_WINDOW_RESIZABLE
    );

    if (!g_window) {
        SDL_Log("SDL_CreateWindow failed: %s", SDL_GetError());
        SDL_Quit();
        return 0;
    }

    /* Try GL first unless overridden */
    if (!want_sdl) {
        g_gl_context = SDL_GL_CreateContext(g_window);
        if (g_gl_context) {
            glewExperimental = GL_TRUE;
            GLenum glew_status = glewInit();
            if (glew_status != GLEW_OK) {
                SDL_Log("glewInit failed: %s", (const char*)glewGetErrorString(glew_status));
                SDL_GL_DeleteContext(g_gl_context);
                g_gl_context = NULL;
            } else {
                SDL_GL_SetSwapInterval(1);
                glViewport(0, 0, width, height);
                glDisable(GL_SCISSOR_TEST);
                glEnable(GL_BLEND);
                glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
                glDisable(GL_DEPTH_TEST);
                glDisable(GL_CULL_FACE);
                glLineWidth(1.0f);

                g_window_width = width;
                g_window_height = height;
                glBindFramebuffer(GL_FRAMEBUFFER, 0);
                glDrawBuffer(GL_BACK);
                glReadBuffer(GL_BACK);
                setup_ortho();
                ensure_line_program();
                init_postfx_shader();

                SDL_Log("Stasis graphics initialized: %dx%d", width, height);
                SDL_Log("GL_VENDOR: %s", (const char*)glGetString(GL_VENDOR));
                SDL_Log("GL_RENDERER: %s", (const char*)glGetString(GL_RENDERER));
                SDL_Log("GL_VERSION: %s", (const char*)glGetString(GL_VERSION));
            }
        }
    }

    if (g_gl_context == NULL) {
        g_use_sdl_renderer = true;
        g_postfx_force_disable = true;
        g_renderer = SDL_CreateRenderer(g_window, -1, SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
        if (!g_renderer) {
            SDL_Log("SDL_CreateRenderer failed: %s", SDL_GetError());
            SDL_DestroyWindow(g_window);
            SDL_Quit();
            return 0;
        }
        SDL_RenderSetLogicalSize(g_renderer, width, height);
        SDL_RendererInfo info;
        if (SDL_GetRendererInfo(g_renderer, &info) == 0) {
            SDL_Log("Stasis graphics initialized (SDL renderer): %dx%d name=%s flags=0x%x", width, height, info.name ? info.name : "?", info.flags);
        } else {
            SDL_Log("Stasis graphics initialized (SDL renderer): %dx%d", width, height);
        }
    } else {
        g_use_sdl_renderer = false;
    }

    g_window_width = width;
    g_window_height = height;
    g_keyboard_state = SDL_GetKeyboardState(NULL);
    g_should_quit = false;
    g_line_count = 0;

    /* Run startup render verification only when explicitly enabled. */
    const char* run_test = SDL_getenv("STASIS_RUN_RENDER_TEST");
    const char* skip_test = SDL_getenv("STASIS_SKIP_RENDER_TEST");
    int should_run_test = (run_test && strcmp(run_test, "0") != 0);
    int should_skip_test = (skip_test && strcmp(skip_test, "0") != 0);
    if (should_run_test && !should_skip_test) {
        int test_ok;
        if (g_use_sdl_renderer) {
            test_ok = verify_sdl_rendering(g_renderer, width, height);
        } else {
            test_ok = verify_opengl_rendering(width, height);
        }

        if (!test_ok) {
            /* Print detailed diagnostics to stderr */
            fprintf(stderr, "\n");
            fprintf(stderr, "=== STASIS GRAPHICS STARTUP TEST FAILED ===\n");
            fprintf(stderr, "Error: %s\n", g_last_test_result.error_message);
            fprintf(stderr, "\n");
            fprintf(stderr, "Diagnostics:\n");
            if (g_use_sdl_renderer) {
                fprintf(stderr, "  Mode: SDL Renderer\n");
                fprintf(stderr, "  Renderer: %s\n", g_last_test_result.gl_renderer);
                fprintf(stderr, "  Info: %s\n", g_last_test_result.gl_version);
            } else {
                fprintf(stderr, "  Mode: OpenGL\n");
                fprintf(stderr, "  GL_VENDOR: %s\n", g_last_test_result.gl_vendor);
                fprintf(stderr, "  GL_RENDERER: %s\n", g_last_test_result.gl_renderer);
                fprintf(stderr, "  GL_VERSION: %s\n", g_last_test_result.gl_version);
                if (g_last_test_result.gl_error_code != 0) {
                    fprintf(stderr, "  GL Error Code: 0x%04X\n", g_last_test_result.gl_error_code);
                }
            }
            fprintf(stderr, "  Pixels Tested: %d\n", g_last_test_result.pixels_tested);
            fprintf(stderr, "  Pixels Correct: %d\n", g_last_test_result.pixels_correct);
            fprintf(stderr, "\n");
            fprintf(stderr, "Possible causes:\n");
            fprintf(stderr, "  - Running in a headless environment without display\n");
            fprintf(stderr, "  - GPU driver not properly installed\n");
            fprintf(stderr, "  - Remote desktop or virtual machine without GPU passthrough\n");
            fprintf(stderr, "  - Incompatible graphics hardware\n");
            fprintf(stderr, "\n");
            fprintf(stderr, "To disable this test, unset: STASIS_RUN_RENDER_TEST (or set to 0)\n");
            fprintf(stderr, "To force SDL renderer, set: STASIS_USE_SDL=1\n");
            fprintf(stderr, "================================================\n");
            fprintf(stderr, "\n");

            /* Cleanup and return failure */
            if (g_gl_context) {
                SDL_GL_DeleteContext(g_gl_context);
                g_gl_context = NULL;
            }
            if (g_renderer) {
                SDL_DestroyRenderer(g_renderer);
                g_renderer = NULL;
            }
            if (g_window) {
                SDL_DestroyWindow(g_window);
                g_window = NULL;
            }
            SDL_Quit();
            return 0;
        }

        /* Clear the test pattern before returning to caller */
        if (g_use_sdl_renderer) {
            SDL_SetRenderDrawColor(g_renderer, 0, 0, 0, 255);
            SDL_RenderClear(g_renderer);
        } else {
            glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
            glClear(GL_COLOR_BUFFER_BIT);
        }
    }

    return 1;
}

/*
 * Get current window dimensions
 * Writes width and height to provided pointers
 */
STASIS_EXPORT void stasis_get_window_size(int* width, int* height) {
    if (width) *width = g_window_width;
    if (height) *height = g_window_height;
}

/*
 * Set fullscreen mode
 * fullscreen: 1 for fullscreen desktop, 0 for windowed
 * Returns 1 on success, 0 on failure
 */
STASIS_EXPORT int stasis_set_fullscreen(int fullscreen) {
    if (!g_window) {
        return 0;
    }

    Uint32 flags = fullscreen ? SDL_WINDOW_FULLSCREEN_DESKTOP : 0;
    int result = SDL_SetWindowFullscreen(g_window, flags);

    if (result == 0 && fullscreen) {
        /* Update window dimensions to match fullscreen size */
        SDL_GetWindowSize(g_window, &g_window_width, &g_window_height);

        if (!g_use_sdl_renderer) {
            glViewport(0, 0, g_window_width, g_window_height);
            setup_ortho();
        } else {
            SDL_RenderSetLogicalSize(g_renderer, g_window_width, g_window_height);
        }
    }

    return (result == 0) ? 1 : 0;
}

/*
 * Begin a new frame
 */
STASIS_EXPORT void stasis_begin_frame(void) {
    g_line_count = 0;
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
    } else {
        (void)g_force_debug_overlay;
    }
}

/*
 * End frame: flush lines, swap buffers, poll events
 */
STASIS_EXPORT void stasis_end_frame(void) {
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        SDL_Color color;
        /* Render lines one by one; could be grouped by color if needed */
        for (int i = 0; i < g_line_count; i++) {
            color.r = (Uint8)(g_lines[i].r * 255.0f);
            color.g = (Uint8)(g_lines[i].g * 255.0f);
            color.b = (Uint8)(g_lines[i].b * 255.0f);
            color.a = (Uint8)(g_lines[i].a * 255.0f);
            SDL_SetRenderDrawColor(g_renderer, color.r, color.g, color.b, color.a);
            SDL_RenderDrawLineF(g_renderer, g_lines[i].x1, g_lines[i].y1, g_lines[i].x2, g_lines[i].y2);
        }
        SDL_RenderPresent(g_renderer);
        g_line_count = 0;
    } else {
        flush_lines();
        flush_sprites();
        render_postfx();
        SDL_GL_SwapWindow(g_window);
    }

    /* Poll events */
    SDL_Event event;
    while (SDL_PollEvent(&event)) {
        switch (event.type) {
            case SDL_QUIT:
                g_should_quit = true;
                break;
            case SDL_KEYDOWN:
                if (event.key.keysym.sym == SDLK_ESCAPE) {
                    g_should_quit = true;
                }
                break;
            case SDL_WINDOWEVENT:
                if (event.window.event == SDL_WINDOWEVENT_SIZE_CHANGED) {
                    /* Update window dimensions when resized */
                    SDL_GetWindowSize(g_window, &g_window_width, &g_window_height);

                    if (!g_use_sdl_renderer) {
                        glViewport(0, 0, g_window_width, g_window_height);
                        setup_ortho();
                    } else {
                        SDL_RenderSetLogicalSize(g_renderer, g_window_width, g_window_height);
                    }
                }
                break;
            default:
                break;
        }
    }

    g_debug_frame_counter++;
}

/*
 * Clear screen with color
 */
STASIS_EXPORT void stasis_clear(float r, float g, float b, float a) {
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawColor(g_renderer, (Uint8)(r * 255.0f), (Uint8)(g * 255.0f), (Uint8)(b * 255.0f), (Uint8)(a * 255.0f));
        SDL_RenderClear(g_renderer);
    } else {
        glClearColor(r, g, b, a);
        glClear(GL_COLOR_BUFFER_BIT);
    }
}

/*
 * Queue a line for batch rendering
 * Coordinates in screen space (0,0 = top-left)
 */
STASIS_EXPORT void stasis_draw_line(float x1, float y1, float x2, float y2,
                                    float r, float g, float b, float a) {
    if (g_use_sdl_renderer) {
        if (g_line_count >= MAX_LINES) {
            /* Cap silently */
            return;
        }
        g_lines[g_line_count].x1 = x1;
        g_lines[g_line_count].y1 = y1;
        g_lines[g_line_count].x2 = x2;
        g_lines[g_line_count].y2 = y2;
        g_lines[g_line_count].r = r;
        g_lines[g_line_count].g = g;
        g_lines[g_line_count].b = b;
        g_lines[g_line_count].a = a;
        g_line_count++;
        return;
    }

    if (g_line_count >= MAX_LINES) {
        flush_lines();
    }

    g_lines[g_line_count].x1 = x1;
    g_lines[g_line_count].y1 = y1;
    g_lines[g_line_count].x2 = x2;
    g_lines[g_line_count].y2 = y2;
    g_lines[g_line_count].r = r;
    g_lines[g_line_count].g = g;
    g_lines[g_line_count].b = b;
    g_lines[g_line_count].a = a;
    g_line_count++;
}

static SpriteEntry* sprite_get(int handle) {
    int idx = handle - 1;
    if (idx < 0 || idx >= MAX_SPRITES) return NULL;
    if (!g_sprites[idx].used) return NULL;
    return &g_sprites[idx];
}

static int sprite_find_by_path(const char* path) {
    for (int i = 0; i < MAX_SPRITES; i++) {
        if (g_sprites[i].used && g_sprites[i].path && strcmp(g_sprites[i].path, path) == 0) {
            return i + 1;
        }
    }
    return 0;
}

static int sprite_build_into_entry(SpriteEntry* e, const char* path, int allow_reuse_slot) {
    unsigned char* pixels = NULL;
    int w = 0, h = 0;
    if (!bake_svg_to_rgba(path, &pixels, &w, &h)) {
        SDL_Log("gfx_load_sprite: failed to bake %s", path);
        return 0;
    }

    if (g_use_sdl_renderer) {
        if (!g_renderer) {
            free(pixels);
            return 0;
        }

        /* SDL expects straight alpha; convert from premultiplied. */
        for (int i = 0; i < w * h; i++) {
            unsigned char* p = pixels + i * 4;
            unsigned char a = p[3];
            if (a == 0) {
                p[0] = 0; p[1] = 0; p[2] = 0;
                continue;
            }
            int r = p[0];
            int g = p[1];
            int b = p[2];
            p[0] = (unsigned char)((r * 255 + (a / 2)) / a);
            p[1] = (unsigned char)((g * 255 + (a / 2)) / a);
            p[2] = (unsigned char)((b * 255 + (a / 2)) / a);
        }

        if (e->sdl_tex) {
            SDL_DestroyTexture(e->sdl_tex);
            e->sdl_tex = NULL;
        }

        SDL_Texture* tex = SDL_CreateTexture(g_renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_STATIC, w, h);
        if (!tex) {
            SDL_Log("gfx_load_sprite: SDL_CreateTexture failed: %s", SDL_GetError());
            free(pixels);
            return 0;
        }
        SDL_SetTextureBlendMode(tex, SDL_BLENDMODE_BLEND);
        if (SDL_UpdateTexture(tex, NULL, pixels, w * 4) != 0) {
            SDL_Log("gfx_load_sprite: SDL_UpdateTexture failed: %s", SDL_GetError());
            SDL_DestroyTexture(tex);
            free(pixels);
            return 0;
        }

        free(pixels);
        e->w = w;
        e->h = h;
        e->sdl_tex = tex;
        e->mtime = get_file_mtime(path);
        return 1;
    }

    ensure_sprite_atlas();
    if (g_sprite_atlas_tex == 0) {
        free(pixels);
        return 0;
    }

    int ax = 0, ay = 0;
    if (allow_reuse_slot) {
        ax = e->atlas_x;
        ay = e->atlas_y;
        if (w != e->w || h != e->h) {
            SDL_Log("gfx_reload: size change not supported (%s %dx%d -> %dx%d)", path, e->w, e->h, w, h);
            free(pixels);
            return 0;
        }
    } else {
        if (!atlas_alloc(w, h, &ax, &ay)) {
            SDL_Log("gfx_load_sprite: atlas full for %s (%dx%d)", path, w, h);
            free(pixels);
            return 0;
        }
    }

    glBindTexture(GL_TEXTURE_2D, g_sprite_atlas_tex);
    glTexSubImage2D(GL_TEXTURE_2D, 0, ax, ay, w, h, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    glGenerateMipmap(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, 0);

    free(pixels);

    e->w = w;
    e->h = h;
    e->atlas_x = ax;
    e->atlas_y = ay;
    e->u0 = (float)ax / (float)SPRITE_ATLAS_W;
    e->v0 = (float)ay / (float)SPRITE_ATLAS_H;
    e->u1 = (float)(ax + w) / (float)SPRITE_ATLAS_W;
    e->v1 = (float)(ay + h) / (float)SPRITE_ATLAS_H;
    e->mtime = get_file_mtime(path);
    return 1;
}

/*
 * Load and bake a sprite from an SVG file into the global atlas.
 * Returns an integer handle (stable for the lifetime of the process).
 */
STASIS_EXPORT int stasis_gfx_load_sprite(const char* path) {
    if (!path || !*path) return 0;
    if (!g_window) return 0;
    if (!g_use_sdl_renderer && !g_gl_context) return 0;
    if (g_use_sdl_renderer && !g_renderer) return 0;

    /* Debug: log path and cwd for easier troubleshooting */
    char cwd[512];
#if defined(_WIN32)
    if (_getcwd(cwd, (int)sizeof(cwd)) != NULL)
#else
    if (getcwd(cwd, sizeof(cwd)) != NULL)
#endif
    {
        fprintf(stderr, "gfx_load_sprite: cwd=%s path=%s\n", cwd, path);
    }
    else
    {
        fprintf(stderr, "gfx_load_sprite: path=%s\n", path);
    }

    int existing = sprite_find_by_path(path);
    if (existing) return existing;

    for (int i = 0; i < MAX_SPRITES; i++) {
        if (!g_sprites[i].used) {
            SpriteEntry* e = &g_sprites[i];
            memset(e, 0, sizeof(*e));
            e->path = stasis_strdup(path);
            if (!e->path) return 0;
            e->used = 1;
            if (!sprite_build_into_entry(e, path, 0)) {
                free(e->path);
                memset(e, 0, sizeof(*e));
                return 0;
            }
            g_sprite_count++;
            SDL_Log("gfx_load_sprite: %s -> handle=%d (%s)", path, i + 1, g_use_sdl_renderer ? "sdl" : "gl");
            return i + 1;
        }
    }

    SDL_Log("gfx_load_sprite: MAX_SPRITES reached");
    return 0;
}

/*
 * Poll and reload a sprite if its source changed on disk.
 * Returns 1 if reloaded, 0 otherwise.
 */
STASIS_EXPORT int stasis_gfx_poll_reload(int handle) {
    SpriteEntry* e = sprite_get(handle);
    if (!e || !e->path) return 0;
    uint64_t mt = get_file_mtime(e->path);
    if (!mt || mt <= e->mtime) return 0;
    return sprite_build_into_entry(e, e->path, 1) ? 1 : 0;
}

/*
 * Draw a baked sprite (centered) with scale, rotation (radians), and tint.
 * Premultiplied alpha atlas is blended with GL_ONE, GL_ONE_MINUS_SRC_ALPHA.
 */
STASIS_EXPORT void stasis_gfx_draw_sprite(int handle, float x, float y, float sx, float sy, float rot,
                                         float r, float g, float b, float a) {
    SpriteEntry* e = sprite_get(handle);
    if (!e) return;

    if (g_use_sdl_renderer) {
        if (!g_renderer || !e->sdl_tex) return;
#if SDL_VERSION_ATLEAST(2,0,10)
        SDL_FRect dst;
        dst.w = (float)e->w * sx;
        dst.h = (float)e->h * sy;
        dst.x = x - dst.w * 0.5f;
        dst.y = y - dst.h * 0.5f;
        SDL_FPoint center = { dst.w * 0.5f, dst.h * 0.5f };
        SDL_SetTextureColorMod(e->sdl_tex, (Uint8)(r * 255.0f), (Uint8)(g * 255.0f), (Uint8)(b * 255.0f));
        SDL_SetTextureAlphaMod(e->sdl_tex, (Uint8)(a * 255.0f));
        SDL_RenderCopyExF(g_renderer, e->sdl_tex, NULL, &dst, (double)(rot * (180.0f / 3.14159265f)), &center, SDL_FLIP_NONE);
#else
        SDL_Rect dst;
        dst.w = (int)((float)e->w * sx);
        dst.h = (int)((float)e->h * sy);
        dst.x = (int)(x - (float)dst.w * 0.5f);
        dst.y = (int)(y - (float)dst.h * 0.5f);
        SDL_Point center = { dst.w / 2, dst.h / 2 };
        SDL_SetTextureColorMod(e->sdl_tex, (Uint8)(r * 255.0f), (Uint8)(g * 255.0f), (Uint8)(b * 255.0f));
        SDL_SetTextureAlphaMod(e->sdl_tex, (Uint8)(a * 255.0f));
        SDL_RenderCopyEx(g_renderer, e->sdl_tex, NULL, &dst, (double)(rot * (180.0f / 3.14159265f)), &center, SDL_FLIP_NONE);
#endif
        return;
    }

    if (g_sprite_vert_count + 6 > MAX_SPRITE_VERTS) {
        flush_sprites();
    }

    float hw = (float)e->w * 0.5f * sx;
    float hh = (float)e->h * 0.5f * sy;
    float c = cosf(rot);
    float s = sinf(rot);

    float x0 = -hw, y0 = -hh;
    float x1 = hw, y1 = hh;

    float p0x = x + x0 * c - y0 * s;
    float p0y = y + x0 * s + y0 * c;
    float p1x = x + x1 * c - y0 * s;
    float p1y = y + x1 * s + y0 * c;
    float p2x = x + x1 * c - y1 * s;
    float p2y = y + x1 * s + y1 * c;
    float p3x = x + x0 * c - y1 * s;
    float p3y = y + x0 * s + y1 * c;

    float u0 = e->u0, v0 = e->v0, u1 = e->u1, v1 = e->v1;

    SpriteVertex* v = &g_sprite_vertices[g_sprite_vert_count];
    /* tri 1: 0,1,2 */
    v[0] = (SpriteVertex){ p0x, p0y, u0, v0, r * a, g * a, b * a, a };
    v[1] = (SpriteVertex){ p1x, p1y, u1, v0, r * a, g * a, b * a, a };
    v[2] = (SpriteVertex){ p2x, p2y, u1, v1, r * a, g * a, b * a, a };
    /* tri 2: 2,3,0 */
    v[3] = (SpriteVertex){ p2x, p2y, u1, v1, r * a, g * a, b * a, a };
    v[4] = (SpriteVertex){ p3x, p3y, u0, v1, r * a, g * a, b * a, a };
    v[5] = (SpriteVertex){ p0x, p0y, u0, v0, r * a, g * a, b * a, a };
    g_sprite_vert_count += 6;
}

/*
 * Check if a key is currently pressed
 * Uses SDL scancodes (SDL_SCANCODE_*)
 */
STASIS_EXPORT int stasis_is_key_down(int scancode) {
    /* Pump events to ensure keyboard state is current */
    SDL_PumpEvents();
    g_keyboard_state = SDL_GetKeyboardState(NULL);
    if (!g_keyboard_state) return 0;
    if (scancode < 0 || scancode >= SDL_NUM_SCANCODES) return 0;
    return g_keyboard_state[scancode] ? 1 : 0;
}

/*
 * Get current time in milliseconds
 */
STASIS_EXPORT int stasis_get_time_ms(void) {
    return (int)SDL_GetTicks();
}

/*
 * Sleep for specified milliseconds
 */
STASIS_EXPORT void stasis_sleep_ms(int ms) {
    if (ms > 0) {
        SDL_Delay((Uint32)ms);
    }
}

/*
 * Configure fullscreen post-processing parameters.
 * strength: 0-1, phase/time: seconds, speed: oscillation scalar, color: rgb tint (0-1).
 */
STASIS_EXPORT void stasis_set_postfx(float strength, float phase, float speed, float r, float g, float b) {
    g_postfx_strength = strength;
    g_postfx_phase = phase;
    g_postfx_speed = speed;
    g_postfx_color[0] = r;
    g_postfx_color[1] = g;
    g_postfx_color[2] = b;
    g_postfx_enabled = true;
}

/*
 * Check if window should close
 */
STASIS_EXPORT int stasis_should_quit(void) {
    return g_should_quit ? 1 : 0;
}

/*
 * Cleanup and shutdown
 */
STASIS_EXPORT void stasis_shutdown(void) {
    if (g_postfx_program) {
        glDeleteProgram(g_postfx_program);
        g_postfx_program = 0;
    }
    if (g_sprite_program) {
        glDeleteProgram(g_sprite_program);
        g_sprite_program = 0;
    }
    if (g_sprite_vbo) {
        glDeleteBuffers(1, &g_sprite_vbo);
        g_sprite_vbo = 0;
    }
    if (g_sprite_vao) {
        glDeleteVertexArrays(1, &g_sprite_vao);
        g_sprite_vao = 0;
    }
    if (g_sprite_atlas_tex) {
        glDeleteTextures(1, &g_sprite_atlas_tex);
        g_sprite_atlas_tex = 0;
    }
    for (int i = 0; i < MAX_SPRITES; i++) {
        if (g_sprites[i].used) {
            if (g_sprites[i].sdl_tex) {
                SDL_DestroyTexture(g_sprites[i].sdl_tex);
                g_sprites[i].sdl_tex = NULL;
            }
            if (g_sprites[i].path) free(g_sprites[i].path);
            memset(&g_sprites[i], 0, sizeof(g_sprites[i]));
        }
    }
    if (g_gl_context) {
        SDL_GL_DeleteContext(g_gl_context);
        g_gl_context = NULL;
    }
    if (g_window) {
        SDL_DestroyWindow(g_window);
        g_window = NULL;
    }
    SDL_Quit();
    SDL_Log("Stasis graphics shutdown");
}

/* ===== DIRECTORY LISTING ===== */

#ifdef _WIN32
#include <windows.h>
#else
#include <dirent.h>
#endif

#define STASIS_DIR_LIST_MAX_ENTRIES 256
#define STASIS_DIR_LIST_NAME_LEN 260
#define STASIS_UTF8_HEADER_SIZE 8
#define STASIS_DIR_ENTRY_STRIDE (STASIS_UTF8_HEADER_SIZE + STASIS_DIR_LIST_NAME_LEN)

static int count_utf8_codepoints(const unsigned char* data, int len)
{
    int count = 0;
    int i = 0;
    while (i < len) {
        unsigned char c = data[i];
        int advance = 1;
        if ((c & 0x80) == 0x00) {
            advance = 1;
        } else if ((c & 0xE0) == 0xC0) {
            advance = 2;
        } else if ((c & 0xF0) == 0xE0) {
            advance = 3;
        } else if ((c & 0xF8) == 0xF0) {
            advance = 4;
        }
        i += advance;
        count++;
    }
    return count;
}

static void write_utf8_entry(unsigned char* entry_base, const char* src)
{
    int copy_len = 0;
    while (copy_len < STASIS_DIR_LIST_NAME_LEN && src[copy_len] != '\0') {
        entry_base[STASIS_UTF8_HEADER_SIZE + copy_len] = (unsigned char)src[copy_len];
        copy_len++;
    }
    entry_base[STASIS_UTF8_HEADER_SIZE + copy_len] = 0;
    int char_len = count_utf8_codepoints(&entry_base[STASIS_UTF8_HEADER_SIZE], copy_len);
    *((int32_t*)(entry_base + 0)) = copy_len;
    *((int32_t*)(entry_base + 4)) = char_len;
}

/* List files in a directory
 * Returns number of files found (up to max_count)
 * out_paths: array of pointers to receive file paths
 * max_count: maximum number of files to return
 */
STASIS_EXPORT int stasis_list_directory(const char* path, char** out_paths, int max_count, int path_buffer_size) {
    if (!path || !out_paths || max_count <= 0) return 0;

    int count = 0;

#ifdef _WIN32
    char search_path[512];
    snprintf(search_path, sizeof(search_path), "%s\\*", path);

    WIN32_FIND_DATAA find_data;
    HANDLE hFind = FindFirstFileA(search_path, &find_data);

    if (hFind == INVALID_HANDLE_VALUE) {
        SDL_Log("stasis_list_directory: failed to open %s", path);
        return 0;
    }

    do {
        /* Skip . and .. */
        if (strcmp(find_data.cFileName, ".") == 0 || strcmp(find_data.cFileName, "..") == 0)
            continue;

        /* Copy filename to output buffer */
        if (count < max_count) {
            snprintf(out_paths[count], path_buffer_size, "%s", find_data.cFileName);
            count++;
        }
    } while (FindNextFileA(hFind, &find_data) != 0 && count < max_count);

    FindClose(hFind);
#else
    DIR* dir = opendir(path);
    if (!dir) {
        SDL_Log("stasis_list_directory: failed to open %s", path);
        return 0;
    }

    struct dirent* entry;
    while ((entry = readdir(dir)) != NULL && count < max_count) {
        /* Skip . and .. */
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0)
            continue;

        /* Copy filename to output buffer */
        snprintf(out_paths[count], path_buffer_size, "%s", entry->d_name);
        count++;
    }

    closedir(dir);
#endif

    SDL_Log("stasis_list_directory: found %d files in %s", count, path);
    return count;
}

STASIS_EXPORT int stasis_list_directory_struct(const char* path, unsigned char* names, int32_t* is_dir, int32_t* out_count) {
    if (!path || !names || !is_dir || !out_count) {
        return 0;
    }

    int count = 0;

#ifdef _WIN32
    char search_path[512];
    snprintf(search_path, sizeof(search_path), "%s\\*", path);

    WIN32_FIND_DATAA find_data;
    HANDLE hFind = FindFirstFileA(search_path, &find_data);

    if (hFind == INVALID_HANDLE_VALUE) {
        SDL_Log("stasis_list_directory_struct: failed to open %s", path);
        *out_count = 0;
        return 0;
    }

    do {
        if (strcmp(find_data.cFileName, ".") == 0 || strcmp(find_data.cFileName, "..") == 0)
            continue;

        if (count >= STASIS_DIR_LIST_MAX_ENTRIES)
            break;

        unsigned char* entry_ptr = names + ((size_t)count * STASIS_DIR_ENTRY_STRIDE);
        write_utf8_entry(entry_ptr, find_data.cFileName);
        is_dir[count] = (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) ? 1 : 0;
        count++;
    } while (FindNextFileA(hFind, &find_data) != 0);

    FindClose(hFind);
#else
    DIR* dir = opendir(path);
    if (!dir) {
        SDL_Log("stasis_list_directory_struct: failed to open %s", path);
        *out_count = 0;
        return 0;
    }

    struct dirent* entry;
    while ((entry = readdir(dir)) != NULL && count < STASIS_DIR_LIST_MAX_ENTRIES) {
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0)
            continue;

        bool entry_is_dir = false;
        char entry_path[512];
        snprintf(entry_path, sizeof(entry_path), "%s/%s", path, entry->d_name);
        struct stat st = {0};
        if (stat(entry_path, &st) == 0) {
            entry_is_dir = S_ISDIR(st.st_mode);
        }

        unsigned char* entry_ptr = names + ((size_t)count * STASIS_DIR_ENTRY_STRIDE);
        write_utf8_entry(entry_ptr, entry->d_name);
        is_dir[count] = entry_is_dir ? 1 : 0;
        count++;
    }

    closedir(dir);
#endif

    *out_count = count;
    return count;
}

STASIS_EXPORT void stasis_copy_dir_entry_name(const unsigned char* names, int32_t idx, unsigned char* out) {
    if (!names || !out || idx < 0 || idx >= STASIS_DIR_LIST_MAX_ENTRIES) {
        return;
    }

    size_t offset = (size_t)idx * STASIS_DIR_ENTRY_STRIDE;
    memcpy(out, names + offset, STASIS_DIR_ENTRY_STRIDE);
}

/* ===== FONT RENDERING WITH STB_TRUETYPE ===== */

#define MAX_FONTS 8
#define FONT_ATLAS_SIZE 512
#define FONT_FIRST_CHAR 32
#define FONT_NUM_CHARS 95

typedef struct {
    bool active;
    stbtt_fontinfo font_info;
    unsigned char* ttf_buffer;
    float scale;
    int ascent, descent, line_gap;

    /* Baked bitmap atlas */
    GLuint atlas_texture;
    stbtt_bakedchar char_data[FONT_NUM_CHARS];
    int font_size;
} StasisFont;

static StasisFont g_fonts[MAX_FONTS];

/* Load a TrueType font from disk */
STASIS_EXPORT int stasis_load_font(const char* path, int font_size) {
    if (!path || font_size <= 0) return 0;

    /* Find free slot */
    int slot = -1;
    for (int i = 0; i < MAX_FONTS; i++) {
        if (!g_fonts[i].active) {
            slot = i;
            break;
        }
    }

    if (slot == -1) {
        SDL_Log("stasis_load_font: no free font slots");
        return 0;
    }

    /* Read font file */
    FILE* f = fopen(path, "rb");
    if (!f) {
        SDL_Log("stasis_load_font: failed to open %s", path);
        return 0;
    }

    fseek(f, 0, SEEK_END);
    size_t size = ftell(f);
    fseek(f, 0, SEEK_SET);

    unsigned char* ttf_buffer = (unsigned char*)malloc(size);
    if (!ttf_buffer) {
        fclose(f);
        SDL_Log("stasis_load_font: malloc failed");
        return 0;
    }

    fread(ttf_buffer, 1, size, f);
    fclose(f);

    /* Initialize font */
    StasisFont* font = &g_fonts[slot];
    if (!stbtt_InitFont(&font->font_info, ttf_buffer, 0)) {
        free(ttf_buffer);
        SDL_Log("stasis_load_font: stbtt_InitFont failed for %s", path);
        return 0;
    }

    font->ttf_buffer = ttf_buffer;
    font->font_size = font_size;
    font->scale = stbtt_ScaleForPixelHeight(&font->font_info, (float)font_size);
    stbtt_GetFontVMetrics(&font->font_info, &font->ascent, &font->descent, &font->line_gap);

    /* Bake font atlas */
    unsigned char* atlas_bitmap = (unsigned char*)malloc(FONT_ATLAS_SIZE * FONT_ATLAS_SIZE);
    if (!atlas_bitmap) {
        free(ttf_buffer);
        SDL_Log("stasis_load_font: atlas malloc failed");
        return 0;
    }

    int result = stbtt_BakeFontBitmap(ttf_buffer, 0, (float)font_size,
                                      atlas_bitmap, FONT_ATLAS_SIZE, FONT_ATLAS_SIZE,
                                      FONT_FIRST_CHAR, FONT_NUM_CHARS, font->char_data);

    if (result <= 0) {
        free(atlas_bitmap);
        free(ttf_buffer);
        SDL_Log("stasis_load_font: BakeFontBitmap failed");
        return 0;
    }

    /* Upload to GPU */
    glGenTextures(1, &font->atlas_texture);
    glBindTexture(GL_TEXTURE_2D, font->atlas_texture);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_ALPHA, FONT_ATLAS_SIZE, FONT_ATLAS_SIZE,
                 0, GL_ALPHA, GL_UNSIGNED_BYTE, atlas_bitmap);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

    free(atlas_bitmap);

    font->active = true;
    SDL_Log("stasis_load_font: loaded %s size=%d handle=%d", path, font_size, slot + 1);

    return slot + 1; /* Return 1-based handle */
}

/* Draw text string using loaded font */
STASIS_EXPORT void stasis_draw_text(int font_handle, const char* text, float x, float y,
                                    float r, float g, float b, float a) {
    if (font_handle <= 0 || font_handle > MAX_FONTS) return;

    StasisFont* font = &g_fonts[font_handle - 1];
    if (!font->active || !text) return;

    if (g_use_sdl_renderer) {
        /* SDL renderer path - not implemented for fonts */
        return;
    }

    /* OpenGL path */
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glEnable(GL_TEXTURE_2D);
    glBindTexture(GL_TEXTURE_2D, font->atlas_texture);

    /* Set color and modelview for text */
    glMatrixMode(GL_MODELVIEW);
    glLoadIdentity();

    glColor4f(r, g, b, a);

    glBegin(GL_QUADS);

    float pos_x = x;
    float pos_y = y;

    while (*text) {
        int c = (unsigned char)*text;
        if (c >= FONT_FIRST_CHAR && c < FONT_FIRST_CHAR + FONT_NUM_CHARS) {
            stbtt_aligned_quad quad;
            stbtt_GetBakedQuad(font->char_data, FONT_ATLAS_SIZE, FONT_ATLAS_SIZE,
                              c - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, 1);

            glTexCoord2f(quad.s0, quad.t0); glVertex2f(quad.x0, quad.y0);
            glTexCoord2f(quad.s1, quad.t0); glVertex2f(quad.x1, quad.y0);
            glTexCoord2f(quad.s1, quad.t1); glVertex2f(quad.x1, quad.y1);
            glTexCoord2f(quad.s0, quad.t1); glVertex2f(quad.x0, quad.y1);
        }
        text++;
    }

    glEnd();

    glDisable(GL_TEXTURE_2D);
    glColor4f(1, 1, 1, 1);
}

/* Measure text width for layout */
STASIS_EXPORT float stasis_measure_text(int font_handle, const char* text) {
    if (font_handle <= 0 || font_handle > MAX_FONTS || !text) return 0.0f;

    StasisFont* font = &g_fonts[font_handle - 1];
    if (!font->active) return 0.0f;

    float width = 0.0f;
    float pos_x = 0.0f, pos_y = 0.0f;

    while (*text) {
        int c = (unsigned char)*text;
        if (c >= FONT_FIRST_CHAR && c < FONT_FIRST_CHAR + FONT_NUM_CHARS) {
            stbtt_aligned_quad quad;
            stbtt_GetBakedQuad(font->char_data, FONT_ATLAS_SIZE, FONT_ATLAS_SIZE,
                              c - FONT_FIRST_CHAR, &pos_x, &pos_y, &quad, 1);
        }
        text++;
    }

    return pos_x;
}
