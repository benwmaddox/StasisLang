#ifndef STASIS_SVG_H
#define STASIS_SVG_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Rasterize an SVG into straight-alpha RGBA8 pixels allocated with malloc().
 * A zero target size uses the SVG's natural dimensions. Otherwise both target
 * dimensions must be positive and the image is contained and centered.
 */
int stasis_svg_rasterize_file(
    const char* path,
    int target_w,
    int target_h,
    unsigned char** out_pixels,
    int* out_w,
    int* out_h
);

int stasis_svg_rasterize_memory(
    const void* data,
    size_t size,
    int target_w,
    int target_h,
    unsigned char** out_pixels,
    int* out_w,
    int* out_h
);

const char* stasis_svg_renderer_name(void);

#ifdef __cplusplus
}
#endif

#endif
