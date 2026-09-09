#include "../stasis_svg.h"

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <limits>
#include <sstream>
#include <string>
#include <vector>

namespace {

constexpr int kMaxDimension = 8192;
constexpr std::uint64_t kMaxPixels = 24'000'000u;
constexpr int kMaxRepetitions = 20;

int fail(const std::string& message)
{
    std::cerr << "stasis_svg_bench: " << message << '\n';
    return 1;
}

bool parse_bounded_int(
    const char* text,
    const char* name,
    int minimum,
    int maximum,
    int* value,
    std::string& error
)
{
    if (!text || !*text) {
        error = std::string(name) + " must be a decimal integer";
        return false;
    }

    char* end = nullptr;
    const unsigned long long parsed = std::strtoull(text, &end, 10);
    if (end == text || *end != '\0' || parsed > static_cast<unsigned long long>(maximum)) {
        error = std::string(name) + " must be in [" + std::to_string(minimum) + ", " +
            std::to_string(maximum) + "]";
        return false;
    }
    if (parsed < static_cast<unsigned long long>(minimum)) {
        error = std::string(name) + " must be in [" + std::to_string(minimum) + ", " +
            std::to_string(maximum) + "]";
        return false;
    }

    *value = static_cast<int>(parsed);
    return true;
}

std::string json_escape(const std::string& value)
{
    std::ostringstream escaped;
    escaped << '"';
    for (unsigned char character : value) {
        switch (character) {
        case '"': escaped << "\\\""; break;
        case '\\': escaped << "\\\\"; break;
        case '\b': escaped << "\\b"; break;
        case '\f': escaped << "\\f"; break;
        case '\n': escaped << "\\n"; break;
        case '\r': escaped << "\\r"; break;
        case '\t': escaped << "\\t"; break;
        default:
            if (character < 0x20u) {
                escaped << "\\u00" << std::hex << std::setw(2) << std::setfill('0')
                    << static_cast<unsigned int>(character) << std::dec << std::setfill('0');
            } else {
                escaped << static_cast<char>(character);
            }
            break;
        }
    }
    escaped << '"';
    return escaped.str();
}

std::size_t first_difference(
    const std::vector<unsigned char>& expected,
    const unsigned char* actual
)
{
    for (std::size_t index = 0; index < expected.size(); ++index) {
        if (expected[index] != actual[index]) return index;
    }
    return expected.size();
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc != 6) {
        return fail(
            "usage: stasis_svg_bench input.svg width height output.rgba repetitions");
    }

    const std::string input_path = argv[1] ? argv[1] : "";
    const std::string output_path = argv[4] ? argv[4] : "";
    if (input_path.empty()) return fail("input.svg must not be empty");
    if (output_path.empty()) return fail("output.rgba must not be empty");

    int width = 0;
    int height = 0;
    int repetitions = 0;
    std::string parse_error;
    if (!parse_bounded_int(argv[2], "width", 1, kMaxDimension, &width, parse_error) ||
        !parse_bounded_int(argv[3], "height", 1, kMaxDimension, &height, parse_error) ||
        !parse_bounded_int(argv[5], "repetitions", 1, kMaxRepetitions, &repetitions, parse_error)) {
        return fail(parse_error);
    }

    const std::uint64_t pixels = static_cast<std::uint64_t>(width) *
        static_cast<std::uint64_t>(height);
    if (pixels > kMaxPixels) {
        return fail(
            "width * height must be at most " + std::to_string(kMaxPixels) + " pixels");
    }
    const std::size_t byte_count = static_cast<std::size_t>(pixels) * 4u;

    std::vector<double> samples_ms;
    samples_ms.reserve(static_cast<std::size_t>(repetitions));
    std::vector<unsigned char> reference;
    reference.reserve(byte_count);

    for (int repetition = 0; repetition < repetitions; ++repetition) {
        unsigned char* pixels_data = nullptr;
        int actual_width = 0;
        int actual_height = 0;
        const auto start = std::chrono::steady_clock::now();
        const int ok = stasis_svg_rasterize_file(
            input_path.c_str(), width, height, &pixels_data, &actual_width, &actual_height);
        const auto end = std::chrono::steady_clock::now();
        samples_ms.push_back(
            std::chrono::duration<double, std::milli>(end - start).count());

        if (!ok) return fail("stasis_svg_rasterize_file failed on repetition " +
            std::to_string(repetition + 1));
        if (!pixels_data) return fail("stasis_svg_rasterize_file returned null pixels");
        if (actual_width != width || actual_height != height) {
            std::free(pixels_data);
            return fail("rasterized dimensions did not match the requested dimensions");
        }

        if (repetition == 0) {
            reference.assign(pixels_data, pixels_data + byte_count);
        } else if (std::memcmp(reference.data(), pixels_data, byte_count) != 0) {
            const std::size_t mismatch = first_difference(reference, pixels_data);
            std::free(pixels_data);
            return fail("raster output changed between repetitions at byte " +
                std::to_string(mismatch));
        }
        std::free(pixels_data);
    }

    std::ofstream output(output_path.c_str(), std::ios::binary | std::ios::trunc);
    if (!output) return fail("could not open output.rgba for writing");
    output.write(
        reinterpret_cast<const char*>(reference.data()),
        static_cast<std::streamsize>(reference.size()));
    if (!output) return fail("could not write output.rgba");
    output.close();
    if (!output) return fail("could not finalize output.rgba");

    double total_ms = 0.0;
    double minimum_ms = std::numeric_limits<double>::max();
    double maximum_ms = 0.0;
    for (const double sample_ms : samples_ms) {
        total_ms += sample_ms;
        if (sample_ms < minimum_ms) minimum_ms = sample_ms;
        if (sample_ms > maximum_ms) maximum_ms = sample_ms;
    }
    const double mean_ms = total_ms / static_cast<double>(samples_ms.size());

    std::cout << std::fixed << std::setprecision(6);
    std::cout << "{\"schema\":\"stasis_svg_bench.v1\""
        << ",\"renderer\":" << json_escape(stasis_svg_renderer_name())
        << ",\"input\":" << json_escape(input_path)
        << ",\"output\":" << json_escape(output_path)
        << ",\"width\":" << width
        << ",\"height\":" << height
        << ",\"repetitions\":" << repetitions
        << ",\"output_bytes\":" << byte_count
        << ",\"timing\":{\"label\":\"stasis_svg_rasterize_file_end_to_end\""
        << ",\"includes_load\":true"
        << ",\"scope\":\"one bridge call including file load, sizing, contain fit, draw, sync, and allocation\""
        << ",\"load_submission\":\"not measured separately because ThorVG load is asynchronous; "
           "total bridge timing is authoritative\""
        << ",\"samples_ms\":[";
    for (std::size_t index = 0; index < samples_ms.size(); ++index) {
        if (index != 0) std::cout << ',';
        std::cout << samples_ms[index];
    }
    std::cout << "]"
        << ",\"total_ms\":" << total_ms
        << ",\"mean_ms\":" << mean_ms
        << ",\"min_ms\":" << minimum_ms
        << ",\"max_ms\":" << maximum_ms
        << "}}\n";
    return std::cout ? 0 : fail("could not write timing JSON");
}
