#include "stasis_image_writer.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "image writer check failed at %s:%d: %s\n", \
            __FILE__, __LINE__, #condition); \
        return 1; \
    } \
} while (0)

static uint32_t read_u32_be(const uint8_t* bytes) {
    return ((uint32_t)bytes[0] << 24) |
           ((uint32_t)bytes[1] << 16) |
           ((uint32_t)bytes[2] << 8) |
           (uint32_t)bytes[3];
}

static uint32_t read_u32_le(const uint8_t* bytes) {
    return (uint32_t)bytes[0] |
           ((uint32_t)bytes[1] << 8) |
           ((uint32_t)bytes[2] << 16) |
           ((uint32_t)bytes[3] << 24);
}

static size_t read_file(const char* path, uint8_t** output) {
    FILE* file = fopen(path, "rb");
    if (!file) return 0;
    if (fseek(file, 0, SEEK_END) != 0) {
        fclose(file);
        return 0;
    }
    long size = ftell(file);
    if (size <= 0 || fseek(file, 0, SEEK_SET) != 0) {
        fclose(file);
        return 0;
    }
    uint8_t* bytes = (uint8_t*)malloc((size_t)size);
    if (!bytes || fread(bytes, 1, (size_t)size, file) != (size_t)size) {
        free(bytes);
        fclose(file);
        return 0;
    }
    fclose(file);
    *output = bytes;
    return (size_t)size;
}

static int check_bmp(const char* path, int expected_height, const uint8_t* pixels) {
    uint8_t* bytes = NULL;
    size_t size = read_file(path, &bytes);
    CHECK(size == 70);
    CHECK(bytes[0] == 'B' && bytes[1] == 'M');
    CHECK(read_u32_le(bytes + 2) == 70);
    CHECK(read_u32_le(bytes + 10) == 54);
    CHECK(read_u32_le(bytes + 14) == 40);
    CHECK(read_u32_le(bytes + 18) == 2);
    CHECK((int32_t)read_u32_le(bytes + 22) == expected_height);
    CHECK(bytes[26] == 1 && bytes[28] == 32);
    CHECK(read_u32_le(bytes + 34) == 16);
    CHECK(memcmp(bytes + 54, pixels, 16) == 0);
    free(bytes);
    return 0;
}

static int check_png(const char* path, int bottom_up, const uint8_t* top_pixels) {
    static const uint8_t signature[8] = {137, 80, 78, 71, 13, 10, 26, 10};
    uint8_t* bytes = NULL;
    size_t size = read_file(path, &bytes);
    CHECK(size >= 8 + 25 + 12);
    CHECK(memcmp(bytes, signature, sizeof(signature)) == 0);

    size_t offset = 8;
    const uint8_t* idat = NULL;
    uint32_t idat_size = 0;
    while (offset + 12 <= size) {
        uint32_t length = read_u32_be(bytes + offset);
        CHECK(length <= size - offset - 12);
        const uint8_t* type = bytes + offset + 4;
        const uint8_t* data = bytes + offset + 8;
        CHECK(offset + 12 + length <= size);
        if (memcmp(type, "IHDR", 4) == 0) {
            CHECK(length == 13);
            CHECK(read_u32_be(data) == 2 && read_u32_be(data + 4) == 2);
            CHECK(data[8] == 8 && data[9] == 6);
        } else if (memcmp(type, "IDAT", 4) == 0) {
            CHECK(idat == NULL);
            idat = data;
            idat_size = length;
        } else if (memcmp(type, "IEND", 4) == 0) {
            CHECK(length == 0);
            break;
        }
        offset += (size_t)length + 12;
    }
    CHECK(idat != NULL);
    CHECK(idat_size == 2 + 5 + 18 + 4);
    CHECK(idat[0] == 0x78 && idat[1] == 0x01);
    CHECK(idat[2] == 1);
    CHECK(idat[3] == 18 && idat[4] == 0);
    CHECK(idat[5] == (uint8_t)~18u && idat[6] == (uint8_t)(~18u >> 8));
    uint8_t expected[18];
    for (int y = 0; y < 2; y++) {
        int source_y = bottom_up ? 1 - y : y;
        expected[y * 9] = 0;
        for (int x = 0; x < 2; x++) {
            const uint8_t* source = top_pixels + (source_y * 2 + x) * 4;
            uint8_t* target = expected + y * 9 + 1 + x * 4;
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
            target[3] = source[3];
        }
    }
    CHECK(memcmp(idat + 7, expected, sizeof(expected)) == 0);
    free(bytes);
    return 0;
}

int main(void) {
    static const uint8_t pixels[] = {
        0, 0, 255, 255, 0, 255, 0, 255,
        255, 0, 0, 255, 255, 255, 255, 255,
    };
    const char* top_bmp = "stasis_image_writer_top.bmp";
    const char* bottom_bmp = "stasis_image_writer_bottom.bmp";
    const char* top_png = "stasis_image_writer_top.png";
    const char* bottom_png = "stasis_image_writer_bottom.png";
    remove(top_bmp);
    remove(bottom_bmp);
    remove(top_png);
    remove(bottom_png);

    CHECK(stasis_image_writer_write_bmp_bgra32(top_bmp, 2, 2, pixels, 0));
    CHECK(stasis_image_writer_write_bmp_bgra32(bottom_bmp, 2, 2, pixels, 1));
    CHECK(check_bmp(top_bmp, -2, pixels) == 0);
    CHECK(check_bmp(bottom_bmp, 2, pixels) == 0);
    CHECK(stasis_image_writer_write_png_bgra32(top_png, 2, 2, pixels, 0));
    CHECK(stasis_image_writer_write_png_bgra32(bottom_png, 2, 2, pixels, 1));
    CHECK(check_png(top_png, 0, pixels) == 0);
    CHECK(check_png(bottom_png, 1, pixels) == 0);

    remove(top_bmp);
    remove(bottom_bmp);
    remove(top_png);
    remove(bottom_png);
    puts("stasis_image_writer_test: ok");
    return 0;
}
