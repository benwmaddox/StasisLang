#include "../stasis_svg.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace {

bool read_file(const char* path, std::vector<unsigned char>& bytes)
{
    FILE* file = std::fopen(path, "rb");
    if (!file) return false;
    std::fseek(file, 0, SEEK_END);
    const long size = std::ftell(file);
    std::fseek(file, 0, SEEK_SET);
    if (size <= 0) {
        std::fclose(file);
        return false;
    }
    bytes.resize(static_cast<size_t>(size));
    const bool ok = std::fread(bytes.data(), 1, bytes.size(), file) == bytes.size();
    std::fclose(file);
    return ok;
}

bool pixel_is(
    const unsigned char* pixels,
    int width,
    int x,
    int y,
    unsigned char r,
    unsigned char g,
    unsigned char b,
    unsigned char a
)
{
    const unsigned char* pixel = pixels + (static_cast<size_t>(y) * width + x) * 4u;
    return pixel[0] == r && pixel[1] == g && pixel[2] == b && pixel[3] == a;
}

bool pixel_near(
    const unsigned char* pixels,
    int width,
    int x,
    int y,
    unsigned char r,
    unsigned char g,
    unsigned char b,
    unsigned char a
)
{
    const unsigned char* pixel = pixels + (static_cast<size_t>(y) * width + x) * 4u;
    return std::abs(static_cast<int>(pixel[0]) - r) <= 1 &&
        std::abs(static_cast<int>(pixel[1]) - g) <= 1 &&
        std::abs(static_cast<int>(pixel[2]) - b) <= 1 &&
        std::abs(static_cast<int>(pixel[3]) - a) <= 1;
}

int fail(const char* message)
{
    std::fprintf(stderr, "stasis_svg_smoke: %s\n", message);
    return 1;
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc != 2) return fail("expected fixture path");
    if (std::strstr(stasis_svg_renderer_name(), "ThorVG 1.2.0") == nullptr) {
        return fail("renderer identity mismatch");
    }

    std::vector<unsigned char> source;
    if (!read_file(argv[1], source)) return fail("could not read fixture");

    unsigned char* memory_pixels = nullptr;
    unsigned char* file_pixels = nullptr;
    unsigned char* repeated_pixels = nullptr;
    int memory_w = 0;
    int memory_h = 0;
    int file_w = 0;
    int file_h = 0;
    int repeated_w = 0;
    int repeated_h = 0;

    if (!stasis_svg_rasterize_memory(
            source.data(), source.size(), 40, 20, &memory_pixels, &memory_w, &memory_h)) {
        return fail("memory raster failed");
    }
    if (!stasis_svg_rasterize_file(argv[1], 40, 20, &file_pixels, &file_w, &file_h)) {
        std::free(memory_pixels);
        return fail("file raster failed");
    }
    if (!stasis_svg_rasterize_memory(
            source.data(), source.size(), 40, 20,
            &repeated_pixels, &repeated_w, &repeated_h)) {
        std::free(file_pixels);
        std::free(memory_pixels);
        return fail("repeat raster failed");
    }

    const size_t byte_count = 40u * 20u * 4u;
    const bool dimensions_ok = memory_w == 40 && memory_h == 20 &&
        file_w == 40 && file_h == 20 && repeated_w == 40 && repeated_h == 20;
    const bool outputs_match = std::memcmp(memory_pixels, file_pixels, byte_count) == 0 &&
        std::memcmp(memory_pixels, repeated_pixels, byte_count) == 0;
    const bool clipping_ok =
        pixel_is(memory_pixels, 40, 5, 10, 0, 0, 0, 0) &&
        pixel_is(memory_pixels, 40, 15, 10, 0xf0, 0x20, 0x10, 0xff) &&
        pixel_near(memory_pixels, 40, 25, 10, 0x10, 0x60, 0xe0, 0x80) &&
        pixel_is(memory_pixels, 40, 35, 10, 0, 0, 0, 0);

    if (!clipping_ok) {
        for (int x : {5, 15, 25, 35}) {
            const unsigned char* pixel = memory_pixels + (10u * 40u + (unsigned)x) * 4u;
            std::fprintf(stderr, "x=%d rgba=%u,%u,%u,%u\n", x,
                pixel[0], pixel[1], pixel[2], pixel[3]);
        }
    }

    std::free(repeated_pixels);
    std::free(file_pixels);
    std::free(memory_pixels);

    if (!dimensions_ok) return fail("target dimensions mismatch");
    if (!outputs_match) return fail("file/memory/repeat pixels differ");
    if (!clipping_ok) return fail("viewport clipping or straight-alpha RGBA differs");

    unsigned char* natural_pixels = nullptr;
    int natural_w = 0;
    int natural_h = 0;
    if (!stasis_svg_rasterize_memory(
            source.data(), source.size(), 0, 0,
            &natural_pixels, &natural_w, &natural_h)) {
        return fail("natural-size raster failed");
    }
    std::free(natural_pixels);
    if (natural_w != 10 || natural_h != 10) return fail("natural dimensions mismatch");

    const char invalid[] = "not an svg document";
    unsigned char* invalid_pixels = nullptr;
    int invalid_w = 0;
    int invalid_h = 0;
    if (stasis_svg_rasterize_memory(
            invalid, sizeof(invalid) - 1u, 16, 16,
            &invalid_pixels, &invalid_w, &invalid_h)) {
        std::free(invalid_pixels);
        return fail("invalid SVG unexpectedly rasterized");
    }

    std::puts("stasis svg ThorVG smoke ok");
    return 0;
}
