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

/* Line batching for efficient rendering */
#define MAX_LINES 10000
static struct {
    float x1, y1, x2, y2;
    float r, g, b, a;
} g_lines[MAX_LINES];
static int g_line_count = 0;

/* Convert screen coords to OpenGL NDC (-1 to 1) */
static float screen_to_ndc_x(float x) {
    return (x / g_window_width) * 2.0f - 1.0f;
}

static float screen_to_ndc_y(float y) {
    /* Flip Y so 0 is at top */
    return 1.0f - (y / g_window_height) * 2.0f;
}

/* Flush all batched lines to OpenGL */
static void flush_lines(void) {
    if (g_line_count == 0) return;

    glBegin(GL_LINES);
    for (int i = 0; i < g_line_count; i++) {
        glColor4f(g_lines[i].r, g_lines[i].g, g_lines[i].b, g_lines[i].a);
        glVertex2f(screen_to_ndc_x(g_lines[i].x1), screen_to_ndc_y(g_lines[i].y1));
        glVertex2f(screen_to_ndc_x(g_lines[i].x2), screen_to_ndc_y(g_lines[i].y2));
    }
    glEnd();

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
"void main(){ float depth=clamp(v_uv.y*u_depth_scale,0.0,1.0); float ripple=noise(v_uv*6.0+u_time*0.25); float wave=sin((v_uv.y*8.0)+(u_time*0.6)+ripple*u_surface_jitter); float c=0.5+0.5*wave; vec3 deep=vec3(0.02,0.08,0.12); vec3 mid=vec3(0.00,0.16,0.22); vec3 base=mix(deep,mid,depth); vec3 color=base+u_intensity*c*u_biolume_color; float atten=mix(1.0,0.25,depth); gl_FragColor=vec4(color*atten,0.45); }\n";

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
        g_postfx_enabled = true;
    }
}

static void render_postfx(void) {
    if (!g_postfx_enabled || g_postfx_program == 0) {
        return;
    }

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
 * Initialize graphics window
 * Returns 1 on success, 0 on failure
 */
STASIS_EXPORT int stasis_init_window(int width, int height, const char* title) {
    SDL_LogSetAllPriority(SDL_LOG_PRIORITY_INFO);
    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS) < 0) {
        SDL_Log("SDL_Init failed: %s", SDL_GetError());
        return 0;
    }

    /* Request OpenGL 2.1 compatibility profile for immediate mode */
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MAJOR_VERSION, 2);
    SDL_GL_SetAttribute(SDL_GL_CONTEXT_MINOR_VERSION, 1);
    SDL_GL_SetAttribute(SDL_GL_DOUBLEBUFFER, 1);
    SDL_GL_SetAttribute(SDL_GL_DEPTH_SIZE, 24);

    g_window = SDL_CreateWindow(
        title ? title : "Stasis",
        SDL_WINDOWPOS_CENTERED,
        SDL_WINDOWPOS_CENTERED,
        width,
        height,
        SDL_WINDOW_OPENGL | SDL_WINDOW_SHOWN
    );

    if (!g_window) {
        SDL_Log("SDL_CreateWindow failed: %s", SDL_GetError());
        SDL_Quit();
        return 0;
    }

    g_gl_context = SDL_GL_CreateContext(g_window);
    if (!g_gl_context) {
        SDL_Log("SDL_GL_CreateContext failed: %s", SDL_GetError());
        SDL_DestroyWindow(g_window);
        SDL_Quit();
        return 0;
    }

    /* Load OpenGL functions (glew) */
    glewExperimental = GL_TRUE;
    GLenum glew_status = glewInit();
    if (glew_status != GLEW_OK) {
        SDL_Log("glewInit failed: %s", (const char*)glewGetErrorString(glew_status));
        SDL_GL_DeleteContext(g_gl_context);
        SDL_DestroyWindow(g_window);
        SDL_Quit();
        return 0;
    }

    /* Enable vsync */
    SDL_GL_SetSwapInterval(1);

    /* Setup OpenGL state */
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    glDisable(GL_DEPTH_TEST);
    glLineWidth(2.0f);

    g_window_width = width;
    g_window_height = height;
    g_keyboard_state = SDL_GetKeyboardState(NULL);
    g_should_quit = false;
    g_line_count = 0;
    init_postfx_shader();

    SDL_Log("Stasis graphics initialized: %dx%d", width, height);
    return 1;
}

/*
 * Begin a new frame
 */
STASIS_EXPORT void stasis_begin_frame(void) {
    g_line_count = 0;
}

/*
 * End frame: flush lines, swap buffers, poll events
 */
STASIS_EXPORT void stasis_end_frame(void) {
    flush_lines();
    render_postfx();

    SDL_GL_SwapWindow(g_window);

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
}

/*
 * Clear screen with color
 */
STASIS_EXPORT void stasis_clear(float r, float g, float b, float a) {
    glClearColor(r, g, b, a);
    glClear(GL_COLOR_BUFFER_BIT);
}

/*
 * Queue a line for batch rendering
 * Coordinates in screen space (0,0 = top-left)
 */
STASIS_EXPORT void stasis_draw_line(float x1, float y1, float x2, float y2,
                                     float r, float g, float b, float a) {
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
