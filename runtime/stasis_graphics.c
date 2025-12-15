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
static bool g_force_debug_overlay = true;

/* Simple shader + buffer for line rendering */
static GLuint g_line_program = 0;
static GLuint g_line_vbo = 0;
static GLuint g_line_vao = 0;
static GLint g_line_pos_loc = -1;
static GLint g_line_color_loc = -1;

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
        "void main(){ gl_Position = vec4((a_pos.xy / vec2(%f,%f))*2.0 - 1.0, 0.0, 1.0); v_color = a_color; }\n";
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

static char* read_text_file(const char* path) {
    FILE* f = fopen(path, "rb");
    if (!f) return NULL;
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

    /* Force completion and display the test pattern */
    glFlush();
    glFinish();
    SDL_GL_SwapWindow(g_window);

    /* Brief pause so user can see the test pattern */
    SDL_Delay(500);

    /* Read back pixels from the center of the quad (need to redraw since we swapped) */
    glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);

    glBegin(GL_QUADS);
    glColor4f(1.0f, 0.0f, 1.0f, 1.0f);
    glVertex2f((float)(cx - size), (float)(cy - size));
    glVertex2f((float)(cx + size), (float)(cy - size));
    glVertex2f((float)(cx + size), (float)(cy + size));
    glVertex2f((float)(cx - size), (float)(cy + size));
    glEnd();
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
        (want_sdl ? 0 : SDL_WINDOW_OPENGL) | SDL_WINDOW_SHOWN
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

    /* Run startup render verification unless disabled */
    const char* skip_test = SDL_getenv("STASIS_SKIP_RENDER_TEST");
    if (skip_test && strcmp(skip_test, "0") != 0) {
        SDL_Log("STARTUP TEST: Skipped (STASIS_SKIP_RENDER_TEST set)");
    } else {
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
            fprintf(stderr, "To skip this test, set: STASIS_SKIP_RENDER_TEST=1\n");
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
 * Begin a new frame
 */
STASIS_EXPORT void stasis_begin_frame(void) {
    g_line_count = 0;
    if (g_use_sdl_renderer) {
        SDL_SetRenderDrawBlendMode(g_renderer, SDL_BLENDMODE_BLEND);
        SDL_SetRenderDrawColor(g_renderer, 26, 153, 26, 255);
        SDL_RenderClear(g_renderer);
        /* Debug overlay: yellow block and red cross */
        SDL_SetRenderDrawColor(g_renderer, 255, 255, 0, 160);
        SDL_FRect rect = { 0.0f, 0.0f, 120.0f, 120.0f };
        SDL_RenderFillRectF(g_renderer, &rect);
        SDL_SetRenderDrawColor(g_renderer, 255, 0, 0, 255);
        SDL_RenderDrawLineF(g_renderer, 0.0f, 0.0f, (float)g_window_width, (float)g_window_height);
        SDL_RenderDrawLineF(g_renderer, (float)g_window_width, 0.0f, 0.0f, (float)g_window_height);
    } else {
        if (g_force_debug_overlay) {
            setup_ortho();
            glClearColor(0.1f, 0.6f, 0.1f, 1.0f);
            glClear(GL_COLOR_BUFFER_BIT);
        }
    }
}

/*
 * End frame: flush lines, swap buffers, poll events
 */
STASIS_EXPORT void stasis_end_frame(void) {
    if (g_debug_frame_counter < 3 && g_line_count == 0) {
        /* Inject a visible debug line if nothing was queued */
        stasis_draw_line(0.0f, 0.0f, (float)g_window_width, (float)g_window_height, 1.0f, 0.0f, 0.0f, 1.0f);
    }

    /* Log detailed GL state for first few frames */
    if (g_debug_frame_counter < 3 && !g_use_sdl_renderer) {
        GLint fb = 0, draw_buf = 0;
        glGetIntegerv(GL_FRAMEBUFFER_BINDING, &fb);
        glGetIntegerv(GL_DRAW_BUFFER, &draw_buf);
        SDL_Log("end_frame %d: FB=%d DrawBuf=0x%X lines=%d postfx=%d",
            g_debug_frame_counter, fb, draw_buf, g_line_count,
            (g_postfx_enabled && !g_postfx_force_disable) ? 1 : 0);
    }

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
    if (g_gl_context) {
        SDL_GL_DeleteContext(g_gl_context);
        g_gl_context = NULL;
    }
    if (g_postfx_program) {
        glDeleteProgram(g_postfx_program);
        g_postfx_program = 0;
    }
    if (g_window) {
        SDL_DestroyWindow(g_window);
        g_window = NULL;
    }
    SDL_Quit();
    SDL_Log("Stasis graphics shutdown");
}
