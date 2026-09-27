#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "stasis_render_contract.h"
#include "stasis_sprite_atlas_policy.h"

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_gfx_load_sprite(const char* path, int max_w, int max_h);
void stasis_gfx_release_sprite(int handle);
void stasis_gfx_set_next_sprite_atlas_policy_v3(
    int eligible, uint64_t group_id, uint32_t member_count, uint64_t logical_pixel_area,
    uint32_t max_logical_width, uint32_t max_logical_height);
void stasis_gfx_submit(int32_t* cmd_i32, const float* cmd_f32);
void stasis_gfx_notify_file_changed(const char* path);
int stasis_gfx_poll_reload(int handle);
void stasis_gfx_test_set_atlas_allocation_failure(int enabled);
void stasis_gfx_test_reset_renderer_resources(void);
int SDL_setenv_unsafe(const char* name, const char* value, int overwrite);
int SDL_unsetenv_unsafe(const char* name);

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

#define TEST_MAX_SPRITES 32u
#define TEST_MAX_PAGES 32u
#define TEST_MAX_PAIRS 128u

static const uint64_t TEST_GROUP = UINT64_C(0x7195ea1);
static uint64_t g_token;
static uint32_t g_renderer_generation;
static uint64_t g_asset_generation;
static StasisSpriteAtlasResidentV1 g_sprites[TEST_MAX_SPRITES];
static StasisSpriteAtlasPageV1 g_pages[TEST_MAX_PAGES];
static StasisSpriteAtlasPairV1 g_pairs[TEST_MAX_PAIRS];
static uint32_t g_sprite_count;
static uint32_t g_page_count;

static void set_env(const char* name, const char* value) {
    CHECK(value ? SDL_setenv_unsafe(name, value, 1) == 0 :
        SDL_unsetenv_unsafe(name) == 0);
}

static void query_native(void) {
    uint64_t token = 0;
    uint32_t renderer_generation = 0;
    uint64_t asset_generation = 0;
    uint32_t flags = 0;
    uint64_t stage_peak_cap = 0;
    uint32_t sprite_count = 0, page_count = 0, pair_count = 0;
    int result = stasis_gfx_sprite_atlas_query_v1(
        &token, &renderer_generation, &asset_generation, &flags, &stage_peak_cap,
        NULL, 0, &sprite_count, NULL, 0, &page_count, NULL, 0, &pair_count);
    CHECK(result == STASIS_SPRITE_ATLAS_QUERY_BUFFER_TOO_SMALL ||
          result == STASIS_SPRITE_ATLAS_QUERY_OK);
    CHECK(token != 0 && sprite_count <= TEST_MAX_SPRITES && page_count <= TEST_MAX_PAGES &&
          pair_count <= TEST_MAX_PAIRS);
    result = stasis_gfx_sprite_atlas_query_v1(
        &token, &renderer_generation, &asset_generation, &flags, &stage_peak_cap,
        g_sprites, TEST_MAX_SPRITES, &sprite_count,
        g_pages, TEST_MAX_PAGES, &page_count,
        g_pairs, TEST_MAX_PAIRS, &pair_count);
    CHECK(result == STASIS_SPRITE_ATLAS_QUERY_OK);
    CHECK(stage_peak_cap == STASIS_SPRITE_ATLAS_STAGE_PEAK_CAP_BYTES);
    g_token = token;
    g_renderer_generation = renderer_generation;
    g_asset_generation = asset_generation;
    g_sprite_count = sprite_count;
    g_page_count = page_count;
}

static StasisSpriteAtlasResidentV1* resident_for(int handle) {
    for (uint32_t i = 0; i < g_sprite_count; i++) {
        if (g_sprites[i].handle == handle) return &g_sprites[i];
    }
    return NULL;
}

static StasisSpriteAtlasPageV1* page_for(uint32_t page_index) {
    for (uint32_t i = 0; i < g_page_count; i++) {
        if (g_pages[i].page_index == page_index) return &g_pages[i];
    }
    return NULL;
}

static void set_group_policy(void) {
    stasis_gfx_set_next_sprite_atlas_policy_v3(
        1, TEST_GROUP, 8, UINT64_C(8) * 64u * 64u, 64u, 64u);
}

static int load_grouped(int width, int height) {
    set_group_policy();
    return stasis_gfx_load_sprite(STASIS_ATLAS_TEST_SMALL_PATH, width, height);
}

static void render_one(int handle, float draw_width, float draw_height,
                       float source_width, float source_height) {
    int32_t* cmd_i32 = (int32_t*)calloc(STASIS_RENDER_I32_COUNT, sizeof(*cmd_i32));
    float* cmd_f32 = (float*)calloc(STASIS_RENDER_F32_COUNT, sizeof(*cmd_f32));
    CHECK(cmd_i32 && cmd_f32);
    cmd_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    cmd_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    cmd_i32[STASIS_RENDER_I_FLAGS] = STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    cmd_i32[STASIS_RENDER_I_SPRITE_COUNT] = 1;
    cmd_i32[STASIS_RENDER_I_SPRITE_RUN_COUNT] = 1;
    cmd_i32[STASIS_RENDER_I_ORDER_COUNT] = 1;
    cmd_i32[STASIS_RENDER_I_SPRITE_BASE] = handle;
    cmd_i32[STASIS_RENDER_I_SPRITE_BASE + 1] = (int32_t)UINT32_C(0xffffffff);
    cmd_i32[STASIS_RENDER_I_SPRITE_RUN_BASE] = 0;
    cmd_i32[STASIS_RENDER_I_SPRITE_RUN_BASE + 1] = 1;
    cmd_i32[STASIS_RENDER_I_SPRITE_RUN_BASE + 2] = STASIS_RENDER_SPRITE_CLIP_ORDERED;
    cmd_i32[STASIS_RENDER_I_ORDER_BASE] = STASIS_RENDER_ORDER_SPRITE * STASIS_RENDER_ORDER_KIND_SCALE;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE] = 8.0f;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 1] = 8.0f;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 2] = draw_width;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 3] = draw_height;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 6] = source_width;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 7] = source_height;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 10] = 1.0f;
    cmd_f32[STASIS_RENDER_F_SPRITE_BASE + 11] = 1.0f;
    stasis_gfx_submit(cmd_i32, cmd_f32);
    free(cmd_i32);
    free(cmd_f32);
}

static uint32_t build_identity_plan(
    uint32_t page_index,
    StasisSpriteAtlasPlanPageV1* plan_page,
    StasisSpriteAtlasPlanPlacementV1 placements[TEST_MAX_SPRITES]) {
    const StasisSpriteAtlasPageV1* page = page_for(page_index);
    CHECK(page && (page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PLAN_ELIGIBLE));
    memset(plan_page, 0, sizeof(*plan_page));
    plan_page->source_page_index = page->page_index;
    plan_page->width = page->width;
    plan_page->height = page->height;
    plan_page->usable_x = page->usable_x;
    plan_page->usable_y = page->usable_y;
    plan_page->padding = page->padding;
    plan_page->reserved_header_height = page->reserved_header_height;
    plan_page->flags = page->flags;
    plan_page->compatibility_flags = page->compatibility_flags;
    plan_page->group_id = page->group_id;
    plan_page->allocation_bytes = page->allocation_bytes;

    uint32_t placement_count = 0;
    for (uint32_t i = 0; i < g_sprite_count; i++) {
        if (g_sprites[i].page_index != page_index) continue;
        CHECK(placement_count < TEST_MAX_SPRITES);
        placements[placement_count++] = (StasisSpriteAtlasPlanPlacementV1){
            g_sprites[i].handle, 0, g_sprites[i].x, g_sprites[i].y};
    }
    return placement_count;
}

int main(void) {
    set_env("STASIS_ENABLE_TEST_INPUT", "1");
    set_env("STASIS_ASSET_ROOT", STASIS_ATLAS_TEST_ASSET_ROOT);
    set_env("STASIS_GFX_WATCH_ASSETS", "0");
    CHECK(stasis_init_window(200, 120, "Atlas sealed page lifecycle") == 1);

    query_native(); /* Initializes the protected fallback page. */
    int first = load_grouped(20, 20);
    int second = load_grouped(21, 21);
    int third = load_grouped(22, 22);
    CHECK(first > 0 && second > 0 && third > 0);
    query_native();
    StasisSpriteAtlasResidentV1* first_resident = resident_for(first);
    StasisSpriteAtlasResidentV1* second_resident = resident_for(second);
    StasisSpriteAtlasResidentV1* third_resident = resident_for(third);
    CHECK(first_resident && second_resident && third_resident);
    const uint32_t sealed_page = first_resident->page_index;
    CHECK(second_resident->page_index == sealed_page && third_resident->page_index == sealed_page);

    StasisSpriteAtlasPlanPageV1 plan_page;
    StasisSpriteAtlasPlanPlacementV1 placements[TEST_MAX_SPRITES] = {{0}};
    const uint32_t placement_count = build_identity_plan(sealed_page, &plan_page, placements);
    CHECK(placement_count == 3);
    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, &plan_page, 1, placements, placement_count) == 1);
    CHECK(stasis_gfx_sprite_atlas_seal_page_v1(g_token, sealed_page) == 0);
    CHECK(stasis_gfx_sprite_atlas_commit_plan_v1(g_token ^ UINT64_C(0x8000000000000000)) == 0);

    const uint64_t before_seal_token = g_token;
    const uint64_t before_seal_generation = g_asset_generation;
    CHECK(stasis_gfx_sprite_atlas_seal_page_v1(
        g_token ^ UINT64_C(0x4000000000000000), sealed_page) == 0);
    CHECK(stasis_gfx_sprite_atlas_seal_page_v1(g_token, sealed_page) == 1);
    query_native();
    StasisSpriteAtlasPageV1* sealed = page_for(sealed_page);
    CHECK(g_token != before_seal_token && g_asset_generation == before_seal_generation + 1);
    CHECK(sealed && (sealed->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_SEALED));
    CHECK(sealed->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PROTECTED);
    CHECK((sealed->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PLAN_ELIGIBLE) == 0);
    CHECK(stasis_gfx_sprite_atlas_seal_page_v1(before_seal_token, sealed_page) == 0);
    CHECK(stasis_gfx_sprite_atlas_seal_page_v1(g_token, sealed_page) == 0);

    first_resident = resident_for(first);
    CHECK(first_resident);
    const uint32_t old_x = first_resident->x;
    const uint32_t old_y = first_resident->y;
    char changed_path[1024];
    const char* logical_path = STASIS_ATLAS_TEST_SMALL_PATH;
    CHECK(snprintf(changed_path, sizeof(changed_path), "%s/%s",
        STASIS_ATLAS_TEST_ASSET_ROOT, logical_path[0] == '/' ? logical_path + 1 : logical_path) > 0);
    stasis_gfx_notify_file_changed(changed_path);
    render_one(first, 20.0f, 20.0f, 20.0f, 20.0f);
    query_native();
    first_resident = resident_for(first);
    CHECK(first_resident && first_resident->page_index == sealed_page &&
          first_resident->x == old_x && first_resident->y == old_y &&
          first_resident->width == 20 && first_resident->height == 20);
    CHECK(stasis_gfx_poll_reload(first) == 1);
    CHECK(stasis_gfx_poll_reload(second) == 1);
    CHECK(stasis_gfx_poll_reload(third) == 1);

    stasis_gfx_test_set_atlas_allocation_failure(1);
    render_one(first, 40.0f, 40.0f, 20.0f, 20.0f);
    query_native();
    first_resident = resident_for(first);
    CHECK(first_resident && first_resident->page_index == sealed_page &&
          first_resident->x == old_x && first_resident->y == old_y &&
          first_resident->width == 20 && first_resident->height == 20);
    stasis_gfx_test_set_atlas_allocation_failure(0);
    render_one(first, 60.0f, 60.0f, 20.0f, 20.0f);
    query_native();
    first_resident = resident_for(first);
    CHECK(first_resident && first_resident->page_index != sealed_page &&
          first_resident->width == 60 && first_resident->height == 60);
    StasisSpriteAtlasPageV1* migrated_page = page_for(first_resident->page_index);
    CHECK(migrated_page &&
          (migrated_page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_SEALED) == 0);

    stasis_gfx_release_sprite(second);
    int replacement = load_grouped(21, 21);
    CHECK(replacement > 0);
    query_native();
    CHECK(resident_for(replacement) && resident_for(replacement)->page_index != sealed_page);
    stasis_gfx_release_sprite(third);
    query_native();
    sealed = page_for(sealed_page);
    CHECK(sealed && (sealed->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_SEALED));
    for (uint32_t i = 0; i < g_sprite_count; i++) {
        CHECK(g_sprites[i].page_index != sealed_page);
    }
    int empty_page_probe = load_grouped(22, 22);
    CHECK(empty_page_probe > 0);
    query_native();
    CHECK(resident_for(empty_page_probe) && resident_for(empty_page_probe)->page_index != sealed_page);

    const uint32_t before_restore_renderer_generation = g_renderer_generation;
    stasis_gfx_test_reset_renderer_resources();
    render_one(first, 60.0f, 60.0f, 20.0f, 20.0f);
    query_native();
    CHECK(g_renderer_generation != before_restore_renderer_generation);
    CHECK(resident_for(first) != NULL);
    for (uint32_t i = 0; i < g_page_count; i++) {
        CHECK((g_pages[i].flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_SEALED) == 0);
    }

    stasis_shutdown();
    puts("sprite atlas sealed page lifecycle contract passed");
    return 0;
}
