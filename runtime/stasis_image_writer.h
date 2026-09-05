#ifndef STASIS_IMAGE_WRITER_H
#define STASIS_IMAGE_WRITER_H

#if defined(__GNUC__) || defined(__clang__)
#define STASIS_IMAGE_WRITER_INTERNAL __attribute__((visibility("hidden")))
#else
#define STASIS_IMAGE_WRITER_INTERNAL
#endif

STASIS_IMAGE_WRITER_INTERNAL int stasis_image_writer_write_bmp_bgra32(
    const char* path,
    int w,
    int h,
    const unsigned char* bgra,
    int is_bottom_up);
STASIS_IMAGE_WRITER_INTERNAL int stasis_image_writer_write_png_bgra32(
    const char* path,
    int w,
    int h,
    const unsigned char* bgra,
    int is_bottom_up);

#endif
