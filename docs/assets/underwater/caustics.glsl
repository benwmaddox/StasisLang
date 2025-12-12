#version 330 core
// Fullscreen caustics fragment shader for the underwater automation sample.
// Inputs: v_uv from a passthrough quad (0..1). Host sets uniforms from Stasis globals.

in vec2 v_uv;
out vec4 fragColor;

uniform float u_time;           // seconds
uniform float u_depth_scale;    // scales v_uv.y into deeper bands
uniform float u_intensity;      // effect strength (0-1)
uniform float u_surface_jitter; // subtle surface ripple intensity
uniform vec3  u_biolume_color;  // glow tint

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    float a = hash(i);
    float b = hash(i + vec2(1.0, 0.0));
    float c = hash(i + vec2(0.0, 1.0));
    float d = hash(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(a, b, u.x) + (c - a) * u.y * (1.0 - u.x) + (d - b) * u.x * u.y;
}

void main() {
    float depth = clamp(v_uv.y * u_depth_scale, 0.0, 1.0);
    float ripple = noise(v_uv * 6.0 + u_time * 0.25);
    float wave = sin((v_uv.y * 8.0) + (u_time * 0.6) + ripple * u_surface_jitter);
    float caustics = 0.5 + 0.5 * wave;

    vec3 deep = vec3(0.02, 0.08, 0.12);
    vec3 mid = vec3(0.00, 0.16, 0.22);
    vec3 base = mix(deep, mid, depth);

    vec3 color = base + u_intensity * caustics * u_biolume_color;
    float atten = mix(1.0, 0.25, depth);
    fragColor = vec4(color * atten, 1.0);
}
