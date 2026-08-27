#include "stasis_svg.h"

#include "thorvg_capi.h"

#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <limits>
#include <mutex>

namespace {

constexpr unsigned kThorvgWorkers = 4;

class ThorvgLifetime {
public:
    ThorvgLifetime() : ready_(tvg_engine_init(kThorvgWorkers) == TVG_RESULT_SUCCESS) {}
    ~ThorvgLifetime()
    {
        if (ready_) tvg_engine_term();
    }

    bool ready() const { return ready_; }

private:
    bool ready_;
};

ThorvgLifetime& thorvg_lifetime()
{
    static ThorvgLifetime lifetime;
    return lifetime;
}

std::mutex& raster_mutex()
{
    static std::mutex mutex;
    return mutex;
}

bool success(Tvg_Result result)
{
    return result == TVG_RESULT_SUCCESS;
}

bool valid_target(int target_w, int target_h)
{
    return (target_w == 0 && target_h == 0) || (target_w > 0 && target_h > 0);
}

bool allocate_pixels(int width, int height, unsigned char** out_pixels)
{
    if (width <= 0 || height <= 0) return false;
    const size_t w = static_cast<size_t>(width);
    const size_t h = static_cast<size_t>(height);
    if (w > std::numeric_limits<size_t>::max() / h ||
        w * h > std::numeric_limits<size_t>::max() / 4u) {
        return false;
    }
    *out_pixels = static_cast<unsigned char*>(std::calloc(w * h, 4u));
    return *out_pixels != nullptr;
}

template <typename Loader>
int rasterize(
    Loader load,
    int target_w,
    int target_h,
    unsigned char** out_pixels,
    int* out_w,
    int* out_h
)
{
    if (!out_pixels || !out_w || !out_h) return 0;
    *out_pixels = nullptr;
    *out_w = 0;
    *out_h = 0;
    if (!valid_target(target_w, target_h)) return 0;

    std::lock_guard<std::mutex> lock(raster_mutex());
    if (!thorvg_lifetime().ready()) return 0;

    Tvg_Canvas canvas = tvg_swcanvas_create(TVG_ENGINE_OPTION_DEFAULT);
    Tvg_Paint picture = tvg_picture_new();
    bool picture_owned_by_canvas = false;
    unsigned char* pixels = nullptr;
    int width = target_w;
    int height = target_h;
    int ok = 0;
    float natural_w = 0.0f;
    float natural_h = 0.0f;
    float content_w = 0.0f;
    float content_h = 0.0f;
    float translate_x = 0.0f;
    float translate_y = 0.0f;

    if (!canvas || !picture || !load(picture)) goto cleanup;

    if (!success(tvg_picture_get_size(picture, &natural_w, &natural_h)) ||
        !std::isfinite(natural_w) || !std::isfinite(natural_h) ||
        natural_w <= 0.0f || natural_h <= 0.0f) {
        goto cleanup;
    }

    if (width == 0) {
        if (natural_w > static_cast<float>(std::numeric_limits<int>::max()) ||
            natural_h > static_cast<float>(std::numeric_limits<int>::max())) {
            goto cleanup;
        }
        width = static_cast<int>(std::ceil(natural_w));
        height = static_cast<int>(std::ceil(natural_h));
    }

    if (!allocate_pixels(width, height, &pixels)) goto cleanup;

    content_w = static_cast<float>(width);
    content_h = static_cast<float>(height);
    if (target_w > 0) {
        const float scale_x = static_cast<float>(width) / natural_w;
        const float scale_y = static_cast<float>(height) / natural_h;
        const float scale = scale_x < scale_y ? scale_x : scale_y;
        int contained_w = static_cast<int>(std::ceil(natural_w * scale));
        int contained_h = static_cast<int>(std::ceil(natural_h * scale));
        if (contained_w < 1) contained_w = 1;
        if (contained_h < 1) contained_h = 1;
        if (contained_w > width) contained_w = width;
        if (contained_h > height) contained_h = height;
        content_w = static_cast<float>(contained_w);
        content_h = static_cast<float>(contained_h);
        translate_x = static_cast<float>(width - contained_w) * 0.5f;
        translate_y = static_cast<float>(height - contained_h) * 0.5f;
    }

    if (!success(tvg_swcanvas_set_target(
            canvas,
            reinterpret_cast<uint32_t*>(pixels),
            static_cast<uint32_t>(width),
            static_cast<uint32_t>(width),
            static_cast<uint32_t>(height),
            TVG_COLORSPACE_ABGR8888S)) ||
        !success(tvg_picture_set_size(picture, content_w, content_h)) ||
        !success(tvg_paint_translate(picture, translate_x, translate_y)) ||
        !success(tvg_canvas_add(canvas, picture))) {
        goto cleanup;
    }
    picture_owned_by_canvas = true;

    if (!success(tvg_canvas_draw(canvas, true)) || !success(tvg_canvas_sync(canvas))) {
        goto cleanup;
    }

    *out_pixels = pixels;
    *out_w = width;
    *out_h = height;
    pixels = nullptr;
    ok = 1;

cleanup:
    std::free(pixels);
    if (!picture_owned_by_canvas && picture) tvg_paint_unref(picture, true);
    if (canvas) tvg_canvas_destroy(canvas);
    return ok;
}

}  // namespace

extern "C" int stasis_svg_rasterize_file(
    const char* path,
    int target_w,
    int target_h,
    unsigned char** out_pixels,
    int* out_w,
    int* out_h
)
{
    if (!path || !*path) return 0;
    return rasterize(
        [path](Tvg_Paint picture) {
            return success(tvg_picture_load(picture, path));
        },
        target_w,
        target_h,
        out_pixels,
        out_w,
        out_h
    );
}

extern "C" int stasis_svg_rasterize_memory(
    const void* data,
    size_t size,
    int target_w,
    int target_h,
    unsigned char** out_pixels,
    int* out_w,
    int* out_h
)
{
    if (!data || size == 0 || size > std::numeric_limits<uint32_t>::max()) return 0;
    return rasterize(
        [data, size](Tvg_Paint picture) {
            return success(tvg_picture_load_data(
                picture,
                static_cast<const char*>(data),
                static_cast<uint32_t>(size),
                "svg",
                nullptr,
                true));
        },
        target_w,
        target_h,
        out_pixels,
        out_w,
        out_h
    );
}

extern "C" const char* stasis_svg_renderer_name(void)
{
    return "ThorVG 1.2.0 (5654bbbb13c518c93ce159569838b329c0af85a7)";
}
