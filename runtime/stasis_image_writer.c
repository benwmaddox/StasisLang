#include "stasis_image_writer.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int write_bmp_bgra32(
    const char* path,
    int w,
    int h,
    const uint8_t* bgra,
    int is_bottom_up) {
    if (!path || !*path || w <= 0 || h <= 0 || !bgra) return 0;

    FILE* f = fopen(path, "wb");
    if (!f) {
        return 0;
    }

    /* 32bpp BI_RGB BMP (BGRA), no row padding needed. */
    const uint32_t pixel_bytes = (uint32_t)w * (uint32_t)h * 4u;
    const uint32_t file_size = 14u + 40u + pixel_bytes;

    uint8_t file_hdr[14];
    memset(file_hdr, 0, sizeof(file_hdr));
    file_hdr[0] = 'B';
    file_hdr[1] = 'M';
    file_hdr[2] = (uint8_t)(file_size & 0xFFu);
    file_hdr[3] = (uint8_t)((file_size >> 8) & 0xFFu);
    file_hdr[4] = (uint8_t)((file_size >> 16) & 0xFFu);
    file_hdr[5] = (uint8_t)((file_size >> 24) & 0xFFu);
    file_hdr[10] = 54; /* pixel data offset */

    uint8_t info_hdr[40];
    memset(info_hdr, 0, sizeof(info_hdr));
    info_hdr[0] = 40; /* BITMAPINFOHEADER size */
    info_hdr[4] = (uint8_t)(w & 0xFF);
    info_hdr[5] = (uint8_t)((w >> 8) & 0xFF);
    info_hdr[6] = (uint8_t)((w >> 16) & 0xFF);
    info_hdr[7] = (uint8_t)((w >> 24) & 0xFF);

    /* Use negative height for top-down to match the natural SDL coordinate system. */
    int32_t signed_h = is_bottom_up ? h : -h;
    info_hdr[8] = (uint8_t)(signed_h & 0xFF);
    info_hdr[9] = (uint8_t)((signed_h >> 8) & 0xFF);
    info_hdr[10] = (uint8_t)((signed_h >> 16) & 0xFF);
    info_hdr[11] = (uint8_t)((signed_h >> 24) & 0xFF);

    info_hdr[12] = 1; /* planes */
    info_hdr[14] = 32; /* bpp */
    /* biCompression=0 (BI_RGB) */
    info_hdr[20] = (uint8_t)(pixel_bytes & 0xFFu);
    info_hdr[21] = (uint8_t)((pixel_bytes >> 8) & 0xFFu);
    info_hdr[22] = (uint8_t)((pixel_bytes >> 16) & 0xFFu);
    info_hdr[23] = (uint8_t)((pixel_bytes >> 24) & 0xFFu);

    if (fwrite(file_hdr, 1, sizeof(file_hdr), f) != sizeof(file_hdr) ||
        fwrite(info_hdr, 1, sizeof(info_hdr), f) != sizeof(info_hdr)) {
        fclose(f);
        return 0;
    }

    const uint32_t row_bytes = (uint32_t)w * 4u;
    if (is_bottom_up) {
        /* Write rows bottom-up (OpenGL glReadPixels origin). */
        for (int y = 0; y < h; y++) {
            const uint8_t* row = bgra + (size_t)y * (size_t)row_bytes;
            if (fwrite(row, 1, row_bytes, f) != row_bytes) {
                fclose(f);
                return 0;
            }
        }
    } else {
        /* Write rows top-down. */
        for (int y = 0; y < h; y++) {
            const uint8_t* row = bgra + (size_t)y * (size_t)row_bytes;
            if (fwrite(row, 1, row_bytes, f) != row_bytes) {
                fclose(f);
                return 0;
            }
        }
    }

    fclose(f);
    return 1;
}

static void write_u32_be(uint8_t* out, uint32_t value) {
    out[0] = (uint8_t)(value >> 24);
    out[1] = (uint8_t)(value >> 16);
    out[2] = (uint8_t)(value >> 8);
    out[3] = (uint8_t)value;
}

static uint32_t png_crc32_update(uint32_t crc, const uint8_t* data, size_t length) {
    for (size_t i = 0; i < length; i++) {
        crc ^= data[i];
        for (int bit = 0; bit < 8; bit++) {
            uint32_t mask = (uint32_t)(-(int32_t)(crc & 1u));
            crc = (crc >> 1) ^ (0xEDB88320u & mask);
        }
    }
    return crc;
}

static int write_png_chunk(FILE* f, const char type[4], const uint8_t* data, uint32_t length) {
    uint8_t size_bytes[4];
    uint8_t crc_bytes[4];
    write_u32_be(size_bytes, length);
    uint32_t crc = png_crc32_update(0xFFFFFFFFu, (const uint8_t*)type, 4);
    if (length > 0) crc = png_crc32_update(crc, data, length);
    write_u32_be(crc_bytes, crc ^ 0xFFFFFFFFu);
    return fwrite(size_bytes, 1, 4, f) == 4 &&
           fwrite(type, 1, 4, f) == 4 &&
           (length == 0 || fwrite(data, 1, length, f) == length) &&
           fwrite(crc_bytes, 1, 4, f) == 4;
}

static uint32_t png_adler32(const uint8_t* data, size_t length) {
    uint32_t a = 1;
    uint32_t b = 0;
    for (size_t offset = 0; offset < length;) {
        size_t block = length - offset;
        if (block > 5552) block = 5552;
        for (size_t i = 0; i < block; i++) {
            a += data[offset + i];
            b += a;
        }
        a %= 65521u;
        b %= 65521u;
        offset += block;
    }
    return (b << 16) | a;
}

/* Deterministic PNG writer using zlib's uncompressed DEFLATE blocks. */
STASIS_IMAGE_WRITER_INTERNAL int stasis_image_writer_write_png_bgra32(
    const char* path,
    int w,
    int h,
    const uint8_t* bgra,
    int is_bottom_up) {
    if (!path || !*path || w <= 0 || h <= 0 || !bgra) return 0;
    if ((size_t)w > (SIZE_MAX - 1u) / 4u) return 0;
    const size_t pixel_row_bytes = (size_t)w * 4u;
    const size_t png_row_bytes = pixel_row_bytes + 1u;
    if ((size_t)h > SIZE_MAX / png_row_bytes) return 0;
    const size_t raw_size = png_row_bytes * (size_t)h;
    if (raw_size > UINT32_MAX) return 0;

    uint8_t* raw = (uint8_t*)malloc(raw_size);
    if (!raw) return 0;
    for (int y = 0; y < h; y++) {
        const int source_y = is_bottom_up ? (h - 1 - y) : y;
        const uint8_t* source = bgra + (size_t)source_y * pixel_row_bytes;
        uint8_t* target = raw + (size_t)y * png_row_bytes;
        target[0] = 0;
        for (int x = 0; x < w; x++) {
            target[1 + x * 4 + 0] = source[x * 4 + 2];
            target[1 + x * 4 + 1] = source[x * 4 + 1];
            target[1 + x * 4 + 2] = source[x * 4 + 0];
            target[1 + x * 4 + 3] = source[x * 4 + 3];
        }
    }

    const size_t block_count = (raw_size + 65534u) / 65535u;
    if (raw_size > SIZE_MAX - 6u ||
        block_count > (SIZE_MAX - raw_size - 6u) / 5u) {
        free(raw);
        return 0;
    }
    const size_t zlib_size = 2u + raw_size + block_count * 5u + 4u;
    if (zlib_size > UINT32_MAX) {
        free(raw);
        return 0;
    }
    uint8_t* zlib = (uint8_t*)malloc(zlib_size);
    if (!zlib) {
        free(raw);
        return 0;
    }

    size_t input_offset = 0;
    size_t output_offset = 0;
    zlib[output_offset++] = 0x78;
    zlib[output_offset++] = 0x01;
    while (input_offset < raw_size) {
        size_t remaining = raw_size - input_offset;
        uint16_t block_length = (uint16_t)(remaining > 65535u ? 65535u : remaining);
        uint16_t inverse_length = (uint16_t)~block_length;
        zlib[output_offset++] = remaining <= 65535u ? 1u : 0u;
        zlib[output_offset++] = (uint8_t)block_length;
        zlib[output_offset++] = (uint8_t)(block_length >> 8);
        zlib[output_offset++] = (uint8_t)inverse_length;
        zlib[output_offset++] = (uint8_t)(inverse_length >> 8);
        memcpy(zlib + output_offset, raw + input_offset, block_length);
        output_offset += block_length;
        input_offset += block_length;
    }
    write_u32_be(zlib + output_offset, png_adler32(raw, raw_size));
    output_offset += 4;

    uint8_t ihdr[13];
    write_u32_be(ihdr, (uint32_t)w);
    write_u32_be(ihdr + 4, (uint32_t)h);
    ihdr[8] = 8;
    ihdr[9] = 6;
    ihdr[10] = 0;
    ihdr[11] = 0;
    ihdr[12] = 0;
    static const uint8_t signature[8] = {137, 80, 78, 71, 13, 10, 26, 10};

    FILE* f = fopen(path, "wb");
    int ok = f && fwrite(signature, 1, sizeof(signature), f) == sizeof(signature) &&
             write_png_chunk(f, "IHDR", ihdr, sizeof(ihdr)) &&
             write_png_chunk(f, "IDAT", zlib, (uint32_t)output_offset) &&
             write_png_chunk(f, "IEND", NULL, 0);
    if (f && fclose(f) != 0) ok = 0;
    if (!ok) remove(path);
    free(zlib);
    free(raw);
    return ok;
}

STASIS_IMAGE_WRITER_INTERNAL int stasis_image_writer_write_bmp_bgra32(
    const char* path,
    int w,
    int h,
    const uint8_t* bgra,
    int is_bottom_up) {
    return write_bmp_bgra32(path, w, h, bgra, is_bottom_up);
}
