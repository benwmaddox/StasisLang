#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "stasis_render_contract.h"
#include "stasis_sprite_atlas_policy.h"

int stasis_init_window(int width, int height, const char* title);
void stasis_shutdown(void);
int stasis_gfx_load_sprite(const char* path, int max_w, int max_h);
void stasis_gfx_set_next_sprite_atlas_policy_v3(
    int eligible, uint64_t group_id, uint32_t member_count, uint64_t logical_pixel_area,
    uint32_t max_logical_width, uint32_t max_logical_height);
void stasis_gfx_submit(int32_t* cmd_i32, const float* cmd_f32);
int stasis_gfx_dump_png(const char* path);
void stasis_gfx_notify_file_changed(const char* path);
int stasis_gfx_poll_reload(int handle);
int stasis_test_get_sprite_state(int32_t handle, int32_t* out_i32, int32_t capacity);
void stasis_gfx_test_advance_renderer_generation(void);
void stasis_gfx_test_set_atlas_stage_failpoint(int failpoint);
int SDL_setenv_unsafe(const char* name, const char* value, int overwrite);
int SDL_unsetenv_unsafe(const char* name);

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "check failed at %s:%d: %s\n", __FILE__, __LINE__, #condition); \
        exit(1); \
    } \
} while (0)

#define TEST_MAX_SPRITES 16u
#define TEST_MAX_PAGES 16u
#define TEST_MAX_PAIRS 128u

static uint64_t g_token;
static uint32_t g_renderer_generation;
static uint64_t g_asset_generation;
static uint32_t g_query_flags;
static uint64_t g_stage_peak_cap;
static StasisSpriteAtlasResidentV1 g_sprites[TEST_MAX_SPRITES];
static StasisSpriteAtlasPageV1 g_pages[TEST_MAX_PAGES];
static StasisSpriteAtlasPairV1 g_pairs[TEST_MAX_PAIRS];
static uint32_t g_sprite_count;
static uint32_t g_page_count;
static uint32_t g_pair_count;

static void set_env(const char* name, const char* value) {
    CHECK(value ? SDL_setenv_unsafe(name, value, 1) == 0 :
        SDL_unsetenv_unsafe(name) == 0);
}

static int query_native(void) {
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
    CHECK(token != 0 && stage_peak_cap == STASIS_SPRITE_ATLAS_STAGE_PEAK_CAP_BYTES);
    g_token = token;
    g_renderer_generation = renderer_generation;
    g_asset_generation = asset_generation;
    g_query_flags = flags;
    g_stage_peak_cap = stage_peak_cap;
    g_sprite_count = sprite_count;
    g_page_count = page_count;
    g_pair_count = pair_count;
    return 1;
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

static uint64_t fnv1a_path(const char* path) {
    uint64_t hash = UINT64_C(14695981039346656037);
    for (const unsigned char* p = (const unsigned char*)path; *p; p++) {
        hash ^= *p;
        hash *= UINT64_C(1099511628211);
    }
    return hash;
}

static void save_native_snapshot(void) {
    FILE* file = fopen(STASIS_ATLAS_TEST_SNAPSHOT_PATH, "wb");
    CHECK(file != NULL);
    fprintf(file,
        "{\"schema\":\"atlas-affinity-native-query/v1\",\"snapshot_token\":%llu,"
        "\"renderer_generation\":%u,\"asset_generation\":%llu,\"flags\":%u,"
        "\"stage_peak_cap_bytes\":%llu,\"evidence_provenance\":\"bounded-runtime-histogram\",\"pages\":[",
        (unsigned long long)g_token, g_renderer_generation,
        (unsigned long long)g_asset_generation, g_query_flags,
        (unsigned long long)g_stage_peak_cap);
    for (uint32_t i = 0; i < g_page_count; i++) {
        const StasisSpriteAtlasPageV1* page = &g_pages[i];
        if (i) fputc(',', file);
        fprintf(file,
            "{\"page_index\":%u,\"width\":%u,\"height\":%u,\"usable_x\":%u,"
            "\"usable_y\":%u,\"padding\":%u,\"reserved_header_height\":%u,"
            "\"flags\":%u,\"compatibility_flags\":%u,\"group_id\":%llu,"
            "\"allocation_bytes\":%llu}",
            page->page_index, page->width, page->height, page->usable_x, page->usable_y,
            page->padding, page->reserved_header_height, page->flags,
            page->compatibility_flags, (unsigned long long)page->group_id,
            (unsigned long long)page->allocation_bytes);
    }
    fputs("],\"sprites\":[", file);
    for (uint32_t i = 0; i < g_sprite_count; i++) {
        const StasisSpriteAtlasResidentV1* sprite = &g_sprites[i];
        if (i) fputc(',', file);
        fprintf(file,
            "{\"handle\":%d,\"width\":%u,\"height\":%u,\"logical_width\":%u,"
            "\"logical_height\":%u,\"page_index\":%u,\"x\":%u,\"y\":%u,"
            "\"allocation_width\":%u,\"allocation_height\":%u,\"padding\":%u,"
            "\"flags\":%u,\"group_id\":%llu,\"normalized_path_hash\":%llu}",
            sprite->handle, sprite->width, sprite->height, sprite->logical_width,
            sprite->logical_height, sprite->page_index, sprite->x, sprite->y,
            sprite->allocation_width, sprite->allocation_height, sprite->padding,
            sprite->flags, (unsigned long long)sprite->group_id,
            (unsigned long long)sprite->normalized_path_hash);
    }
    fputs("],\"pairs\":[", file);
    for (uint32_t i = 0; i < g_pair_count; i++) {
        const StasisSpriteAtlasPairV1* pair = &g_pairs[i];
        if (i) fputc(',', file);
        fprintf(file, "{\"from_handle\":%d,\"to_handle\":%d,\"weight\":%llu}",
            pair->from_handle, pair->to_handle, (unsigned long long)pair->weight);
    }
    fputs("]}\n", file);
    CHECK(fclose(file) == 0);
}

static void build_plan(
    StasisSpriteAtlasPlanPageV1 pages[TEST_MAX_PAGES], uint32_t* page_count,
    StasisSpriteAtlasPlanPlacementV1 placements[TEST_MAX_SPRITES], uint32_t* placement_count,
    uint32_t* out_large_page_array_index
) {
    *page_count = 0;
    *placement_count = 0;
    *out_large_page_array_index = UINT32_MAX;
    uint64_t largest_allocation = 0;
    for (uint32_t i = 0; i < g_page_count; i++) {
        const StasisSpriteAtlasPageV1* source = &g_pages[i];
        if ((source->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PLAN_ELIGIBLE) == 0) continue;
        CHECK((source->flags & (STASIS_SPRITE_ATLAS_PAGE_FLAG_DEDICATED |
            STASIS_SPRITE_ATLAS_PAGE_FLAG_COLD | STASIS_SPRITE_ATLAS_PAGE_FLAG_FALLBACK |
            STASIS_SPRITE_ATLAS_PAGE_FLAG_PROTECTED)) == 0);
        StasisSpriteAtlasPlanPageV1* target = &pages[*page_count];
        target->source_page_index = source->page_index;
        target->width = source->width;
        target->height = source->height;
        target->usable_x = source->usable_x;
        target->usable_y = source->usable_y;
        target->padding = source->padding;
        target->reserved_header_height = source->reserved_header_height;
        target->flags = source->flags;
        target->compatibility_flags = source->compatibility_flags;
        target->group_id = source->group_id;
        target->allocation_bytes = source->allocation_bytes;
        if (source->allocation_bytes > largest_allocation) {
            largest_allocation = source->allocation_bytes;
            *out_large_page_array_index = *page_count;
        }
        (*page_count)++;
    }
    CHECK(*page_count >= 1);
    CHECK(*out_large_page_array_index != UINT32_MAX);
    if (*page_count == 2) {
        CHECK(pages[0].source_page_index > 0 && pages[1].source_page_index > 0);
    }
    const StasisSpriteAtlasPlanPageV1* destination = &pages[*out_large_page_array_index];
    uint32_t cursor_x = destination->usable_x;
    uint32_t cursor_y = destination->usable_y;
    uint32_t row_height = 0;
    for (uint32_t i = 0; i < g_sprite_count; i++) {
        const StasisSpriteAtlasResidentV1* sprite = &g_sprites[i];
        if ((sprite->flags & STASIS_SPRITE_ATLAS_SPRITE_FLAG_AFFINITY_ELIGIBLE) == 0) continue;
        CHECK(*placement_count < TEST_MAX_SPRITES);
        const uint32_t allocation_width = sprite->width + 2u * destination->padding;
        const uint32_t allocation_height = sprite->height + 2u * destination->padding;
        if (cursor_x + allocation_width > destination->width) {
            cursor_x = destination->usable_x;
            cursor_y += row_height;
            row_height = 0;
        }
        CHECK(cursor_y + allocation_height <= destination->height);
        StasisSpriteAtlasPlanPlacementV1* placement = &placements[(*placement_count)++];
        placement->handle = sprite->handle;
        placement->page_index = *out_large_page_array_index;
        placement->x = cursor_x + destination->padding;
        placement->y = cursor_y + destination->padding;
        cursor_x += allocation_width;
        if (allocation_height > row_height) row_height = allocation_height;
    }
    CHECK(*placement_count == 3);
}

static void render_scene(
    const int handles[3], const StasisSpriteAtlasResidentV1* residents[3],
    int clip_barriers, int overlap_sprites, int32_t* cmd_i32, float* cmd_f32
) {
    memset(cmd_i32, 0, STASIS_RENDER_I32_COUNT * sizeof(*cmd_i32));
    memset(cmd_f32, 0, STASIS_RENDER_F32_COUNT * sizeof(*cmd_f32));
    cmd_i32[STASIS_RENDER_I_MAGIC] = STASIS_RENDER_MAGIC;
    cmd_i32[STASIS_RENDER_I_VERSION] = STASIS_RENDER_VERSION;
    cmd_i32[STASIS_RENDER_I_FLAGS] = STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT;
    cmd_i32[STASIS_RENDER_I_SPRITE_COUNT] = 3;
    cmd_i32[STASIS_RENDER_I_SPRITE_RUN_COUNT] = 3;
    cmd_i32[STASIS_RENDER_I_CLIP_COUNT] = clip_barriers ? 3 : 0;
    cmd_i32[STASIS_RENDER_I_ORDER_COUNT] = clip_barriers ? 9 : 3;
    cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 0] = 0.04f;
    cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 1] = 0.08f;
    cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 2] = 0.12f;
    cmd_f32[STASIS_RENDER_F_CLEAR_BASE + 3] = 1.0f;
    static const uint32_t normal_tint[3] = {
        UINT32_C(0xffffffff), UINT32_C(0xffff8080), UINT32_C(0x80ffffff)};
    static const uint32_t overlap_tint[3] = {
        UINT32_C(0x80ffffff), UINT32_C(0x80ff8080), UINT32_C(0x80ffffff)};
    for (int i = 0; i < 3; i++) {
        const int32_t ibase = STASIS_RENDER_I_SPRITE_BASE +
            i * STASIS_RENDER_SPRITE_I32_STRIDE;
        const int32_t rbase = STASIS_RENDER_I_SPRITE_RUN_BASE +
            i * STASIS_RENDER_SPRITE_RUN_I32_STRIDE;
        const int32_t fbase = STASIS_RENDER_F_SPRITE_BASE +
            i * STASIS_RENDER_SPRITE_F32_STRIDE;
        cmd_i32[ibase + 0] = handles[i];
        cmd_i32[ibase + 1] = (int32_t)(overlap_sprites ? overlap_tint[i] : normal_tint[i]);
        cmd_i32[rbase + 0] = i;
        cmd_i32[rbase + 1] = 1;
        cmd_i32[rbase + 2] = STASIS_RENDER_SPRITE_CLIP_ORDERED;
        cmd_f32[fbase + 0] = overlap_sprites ? 42.0f :
            8.0f + (float)i * (clip_barriers ? 64.0f : 40.0f);
        cmd_f32[fbase + 1] = overlap_sprites ? 24.0f : 8.0f;
        cmd_f32[fbase + 2] = 50.0f;
        cmd_f32[fbase + 3] = 72.0f;
        cmd_f32[fbase + 4] = 0.0f;
        cmd_f32[fbase + 5] = 0.0f;
        cmd_f32[fbase + 6] = (float)residents[i]->logical_width * 0.5f;
        cmd_f32[fbase + 7] = (float)residents[i]->logical_height * 0.5f;
        cmd_f32[fbase + 8] = 0.0f;
        cmd_f32[fbase + 9] = 0.0f;
        cmd_f32[fbase + 10] = 1.0f;
        cmd_f32[fbase + 11] = 1.0f;
        cmd_f32[fbase + 12] = 0.0f;
        if (clip_barriers) {
            const int32_t cbase = STASIS_RENDER_F_CLIP_BASE +
                i * STASIS_RENDER_CLIP_F32_STRIDE;
            cmd_f32[cbase + 0] = overlap_sprites ? 0.0f : (float)i * 64.0f;
            cmd_f32[cbase + 1] = 0.0f;
            cmd_f32[cbase + 2] = overlap_sprites ? 200.0f : 64.0f;
            cmd_f32[cbase + 3] = 120.0f;
            cmd_i32[STASIS_RENDER_I_ORDER_BASE + i * 3 + 0] =
                STASIS_RENDER_ORDER_CLIP_PUSH * STASIS_RENDER_ORDER_KIND_SCALE + i;
            cmd_i32[STASIS_RENDER_I_ORDER_BASE + i * 3 + 1] =
                STASIS_RENDER_ORDER_SPRITE * STASIS_RENDER_ORDER_KIND_SCALE + i;
            cmd_i32[STASIS_RENDER_I_ORDER_BASE + i * 3 + 2] =
                STASIS_RENDER_ORDER_CLIP_POP * STASIS_RENDER_ORDER_KIND_SCALE;
        } else {
            cmd_i32[STASIS_RENDER_I_ORDER_BASE + i] =
                STASIS_RENDER_ORDER_SPRITE * STASIS_RENDER_ORDER_KIND_SCALE + i;
        }
    }
    stasis_gfx_submit(cmd_i32, cmd_f32);
}

static void read_file(const char* path, unsigned char** out_bytes, size_t* out_size) {
    FILE* file = fopen(path, "rb");
    CHECK(file != NULL);
    CHECK(fseek(file, 0, SEEK_END) == 0);
    long length = ftell(file);
    CHECK(length > 0 && fseek(file, 0, SEEK_SET) == 0);
    unsigned char* bytes = (unsigned char*)malloc((size_t)length);
    CHECK(bytes != NULL);
    CHECK(fread(bytes, 1, (size_t)length, file) == (size_t)length);
    CHECK(fclose(file) == 0);
    *out_bytes = bytes;
    *out_size = (size_t)length;
}

static void assert_pngs_equal(void) {
    unsigned char *before = NULL, *after = NULL;
    size_t before_size = 0, after_size = 0;
    read_file(STASIS_ATLAS_TEST_BEFORE_PNG, &before, &before_size);
    read_file(STASIS_ATLAS_TEST_AFTER_PNG, &after, &after_size);
    CHECK(before_size == after_size && memcmp(before, after, before_size) == 0);
    free(before);
    free(after);
}

static void assert_handle_is_live(int handle) {
    int32_t state[18] = {0};
    CHECK(stasis_test_get_sprite_state(handle, state, 18) == 1);
    CHECK(state[0] == 1 && state[1] > 0);
}

int main(void) {
    set_env("STASIS_ENABLE_TEST_INPUT", "1");
    set_env("STASIS_ASSET_ROOT", STASIS_ATLAS_TEST_ASSET_ROOT);
    set_env("STASIS_GFX_WATCH_ASSETS", "0");
    CHECK(stasis_init_window(200, 120, "Atlas staging transaction") == 1);

    /* Querying initializes the procedural fallback in its protected page. */
    CHECK(query_native());
    CHECK(g_page_count >= 1);
    int handles[3] = {0};
    stasis_gfx_set_next_sprite_atlas_policy_v3(
        1, UINT64_C(0x623a71), 2, 24u * 36u, 24u, 36u);
    handles[0] = stasis_gfx_load_sprite(
        STASIS_ATLAS_TEST_SMALL_PATH, STASIS_ATLAS_TEST_SMALL_WIDTH,
        STASIS_ATLAS_TEST_SMALL_HEIGHT);
    CHECK(handles[0] > 0);
    stasis_gfx_set_next_sprite_atlas_policy_v3(
        1, UINT64_C(0x623a71), 3, 12000u, 40u, 60u);
    handles[1] = stasis_gfx_load_sprite(
        STASIS_ATLAS_TEST_DOG_PATH, STASIS_ATLAS_TEST_DOG_WIDTH,
        STASIS_ATLAS_TEST_DOG_HEIGHT);
    CHECK(handles[1] > 0);
    stasis_gfx_set_next_sprite_atlas_policy_v3(
        1, UINT64_C(0x623a71), 3, 12000u, 40u, 60u);
    handles[2] = stasis_gfx_load_sprite(
        STASIS_ATLAS_TEST_DOG_RUN_PATH, STASIS_ATLAS_TEST_DOG_RUN_WIDTH,
        STASIS_ATLAS_TEST_DOG_RUN_HEIGHT);
    CHECK(handles[2] > 0);

    CHECK(query_native());
    CHECK(g_sprite_count == 3 && g_page_count >= 3);
    StasisSpriteAtlasResidentV1* residents[3];
    for (int i = 0; i < 3; i++) {
        residents[i] = resident_for(handles[i]);
        CHECK(residents[i] != NULL);
        CHECK(residents[i]->flags & STASIS_SPRITE_ATLAS_SPRITE_FLAG_AFFINITY_ELIGIBLE);
        CHECK(residents[i]->group_id == UINT64_C(0x623a71));
        CHECK(residents[i]->normalized_path_hash == fnv1a_path(i == 0
            ? STASIS_ATLAS_TEST_SMALL_PATH
            : (i == 1 ? STASIS_ATLAS_TEST_DOG_PATH : STASIS_ATLAS_TEST_DOG_RUN_PATH)));
    }
    uint32_t eligible_page_count = 0;
    int fallback_protected = 0;
    uint32_t first_source_page = UINT32_MAX;
    for (uint32_t i = 0; i < g_page_count; i++) {
        const StasisSpriteAtlasPageV1* page = &g_pages[i];
        if (page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_FALLBACK) {
            CHECK(page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PROTECTED);
            fallback_protected = 1;
        }
        if (page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PLAN_ELIGIBLE) {
            if (first_source_page == UINT32_MAX) first_source_page = page->page_index;
            eligible_page_count++;
        }
    }
    CHECK(fallback_protected && eligible_page_count == 2);
    CHECK(first_source_page > 0); /* Native plan page indices are sparse source page IDs. */
    CHECK(residents[0]->page_index != residents[1]->page_index);
    CHECK(residents[1]->page_index == residents[2]->page_index);

    int32_t* cmd_i32 = (int32_t*)calloc(STASIS_RENDER_I32_COUNT, sizeof(*cmd_i32));
    float* cmd_f32 = (float*)calloc(STASIS_RENDER_F32_COUNT, sizeof(*cmd_f32));
    CHECK(cmd_i32 && cmd_f32);
    render_scene(handles, (const StasisSpriteAtlasResidentV1**)residents, 1, 0, cmd_i32, cmd_f32);
    StasisSpriteAtlasFrameStatsV1 frame_stats;
    CHECK(stasis_gfx_sprite_atlas_last_frame_stats_v1(&frame_stats) == 1);
    CHECK(frame_stats.accepted_frame_serial == 1);
    CHECK(frame_stats.ordered_sprite_segments == 3);
    CHECK(frame_stats.ordered_page_runs == 3);
    CHECK(frame_stats.draw_submissions == 3);
    CHECK(query_native());
    CHECK((g_query_flags & STASIS_SPRITE_ATLAS_QUERY_FLAG_PAIR_VALID) == 0);
    CHECK(g_pair_count == 0); /* Clip push/pop barriers break affinity adjacency. */
    printf("barrier frame: segments=%u page_runs=%u draw_submissions=%u\n",
        frame_stats.ordered_sprite_segments, frame_stats.ordered_page_runs,
        frame_stats.draw_submissions);

    for (int frame = 2; frame <= (int)STASIS_SPRITE_ATLAS_PAIR_WINDOW_FRAMES; frame++) {
        render_scene(handles, (const StasisSpriteAtlasResidentV1**)residents, 0, 0, cmd_i32, cmd_f32);
    }
    CHECK(stasis_gfx_sprite_atlas_last_frame_stats_v1(&frame_stats) == 1);
    CHECK(frame_stats.accepted_frame_serial == STASIS_SPRITE_ATLAS_PAIR_WINDOW_FRAMES);
    CHECK(frame_stats.ordered_sprite_segments == 1);
    CHECK(frame_stats.ordered_page_runs == 2);
    CHECK(frame_stats.draw_submissions == 2);
    CHECK(frame_stats.page_transitions == 1);
    CHECK(query_native());
    CHECK(g_query_flags & STASIS_SPRITE_ATLAS_QUERY_FLAG_PAIR_VALID);
    CHECK(g_pair_count == 2);
    for (uint32_t i = 0; i < g_pair_count; i++) CHECK(g_pairs[i].weight == 63u);
    save_native_snapshot();

    printf("baseline frame: segments=%u page_runs=%u draw_submissions=%u page_transitions=%u\n",
        frame_stats.ordered_sprite_segments, frame_stats.ordered_page_runs,
        frame_stats.draw_submissions, frame_stats.page_transitions);

    /* Capture overlapping translucent sprites under one shared clip region before staging. */
    render_scene(handles, (const StasisSpriteAtlasResidentV1**)residents, 1, 1, cmd_i32, cmd_f32);
    CHECK(stasis_gfx_dump_png(STASIS_ATLAS_TEST_BEFORE_PNG) == 1);
    StasisSpriteAtlasPlanPageV1 plan_pages[TEST_MAX_PAGES] = {{0}};
    StasisSpriteAtlasPlanPlacementV1 placements[TEST_MAX_SPRITES] = {{0}};
    uint32_t plan_page_count = 0, placement_count = 0, destination_index = UINT32_MAX;
    build_plan(plan_pages, &plan_page_count, placements, &placement_count, &destination_index);

    /* Overlapping full-plan coordinates are rejected without changing live pages or handles. */
    StasisSpriteAtlasPlanPlacementV1 bad_placements[TEST_MAX_SPRITES];
    memcpy(bad_placements, placements, placement_count * sizeof(*placements));
    bad_placements[1].x = bad_placements[0].x;
    bad_placements[1].y = bad_placements[0].y;
    const uint64_t original_token = g_token;
    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, plan_pages, plan_page_count, bad_placements, placement_count) == 0);
    CHECK(query_native() && g_token == original_token);
    for (int i = 0; i < 3; i++) assert_handle_is_live(handles[i]);

    /* Exercise cleanup after staged page allocation and after one upload. */
    stasis_gfx_test_set_atlas_stage_failpoint(1);
    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, plan_pages, plan_page_count, placements, placement_count) == 0);
    stasis_gfx_test_set_atlas_stage_failpoint(2);
    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, plan_pages, plan_page_count, placements, placement_count) == 0);
    stasis_gfx_test_set_atlas_stage_failpoint(0);
    CHECK(query_native() && g_token == original_token);
    for (int i = 0; i < 3; i++) assert_handle_is_live(handles[i]);

    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, plan_pages, plan_page_count, placements, placement_count) == 1);
    CHECK(stasis_gfx_sprite_atlas_commit_plan_v1(g_token) == 1);
    CHECK(query_native());
    for (int i = 0; i < 3; i++) {
        StasisSpriteAtlasResidentV1* moved = resident_for(handles[i]);
        CHECK(moved && moved->page_index == plan_pages[destination_index].source_page_index);
        CHECK(moved->x == placements[i].x && moved->y == placements[i].y);
        assert_handle_is_live(handles[i]);
        residents[i] = moved;
    }
    render_scene(handles, (const StasisSpriteAtlasResidentV1**)residents, 0, 0, cmd_i32, cmd_f32);
    CHECK(stasis_gfx_sprite_atlas_last_frame_stats_v1(&frame_stats) == 1);
    CHECK(frame_stats.ordered_sprite_segments == 1);
    CHECK(frame_stats.ordered_page_runs == 1);
    CHECK(frame_stats.draw_submissions == 1);
    CHECK(frame_stats.page_transitions == 0);
    /* Repeat the same overlapping translucent clip-barrier frame after publication. */
    render_scene(handles, (const StasisSpriteAtlasResidentV1**)residents, 1, 1, cmd_i32, cmd_f32);
    CHECK(stasis_gfx_dump_png(STASIS_ATLAS_TEST_AFTER_PNG) == 1);
    assert_pngs_equal();
    printf("optimized frame: segments=%u page_runs=%u draw_submissions=%u page_transitions=%u\n",
        frame_stats.ordered_sprite_segments, frame_stats.ordered_page_runs,
        frame_stats.draw_submissions, frame_stats.page_transitions);

    /* A file-change notification invalidates a prepared plan immediately. */
    CHECK(query_native());
    StasisSpriteAtlasPlanPageV1 reload_pages[TEST_MAX_PAGES] = {{0}};
    StasisSpriteAtlasPlanPlacementV1 reload_placements[TEST_MAX_SPRITES] = {{0}};
    uint32_t reload_page_count = 0, reload_placement_count = 0, reload_dest = UINT32_MAX;
    build_plan(reload_pages, &reload_page_count, reload_placements,
        &reload_placement_count, &reload_dest);
    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, reload_pages, reload_page_count, reload_placements, reload_placement_count) == 1);
    char changed_path[1024];
    const char* logical_path = STASIS_ATLAS_TEST_DOG_PATH;
    CHECK(snprintf(changed_path, sizeof(changed_path), "%s/%s",
        STASIS_ATLAS_TEST_ASSET_ROOT, logical_path[0] == '/' ? logical_path + 1 : logical_path) > 0);
    stasis_gfx_notify_file_changed(changed_path);
    CHECK(stasis_gfx_sprite_atlas_commit_plan_v1(g_token) == 0);
    CHECK(query_native() && g_token != original_token);

    /* Apply the reload and verify the pending generation protects its whole page. */
    render_scene(handles, (const StasisSpriteAtlasResidentV1**)residents, 0, 0, cmd_i32, cmd_f32);
    CHECK(query_native());
    StasisSpriteAtlasResidentV1* reloaded = resident_for(handles[1]);
    CHECK(reloaded && (reloaded->flags & STASIS_SPRITE_ATLAS_SPRITE_FLAG_PROTECTED));
    StasisSpriteAtlasPageV1* reloading_page = page_for(reloaded->page_index);
    CHECK(reloading_page && (reloading_page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PROTECTED));
    CHECK((reloading_page->flags & STASIS_SPRITE_ATLAS_PAGE_FLAG_PLAN_ELIGIBLE) == 0);
    int completed_reloads = 0;
    for (int i = 0; i < 3; i++) completed_reloads += stasis_gfx_poll_reload(handles[i]);
    CHECK(completed_reloads >= 1);
    CHECK(query_native());

    /* A renderer generation bump between stage and commit must discard staged GPU pages. */
    StasisSpriteAtlasPlanPageV1 generation_pages[TEST_MAX_PAGES] = {{0}};
    StasisSpriteAtlasPlanPlacementV1 generation_placements[TEST_MAX_SPRITES] = {{0}};
    uint32_t generation_page_count = 0, generation_placement_count = 0;
    uint32_t generation_dest = UINT32_MAX;
    build_plan(generation_pages, &generation_page_count, generation_placements,
        &generation_placement_count, &generation_dest);
    CHECK(stasis_gfx_sprite_atlas_stage_plan_v1(
        g_token, generation_pages, generation_page_count,
        generation_placements, generation_placement_count) == 1);
    const uint64_t staged_generation_token = g_token;
    stasis_gfx_test_advance_renderer_generation();
    CHECK(stasis_gfx_sprite_atlas_commit_plan_v1(staged_generation_token) == 0);
    for (int i = 0; i < 3; i++) assert_handle_is_live(handles[i]);

    free(cmd_i32);
    free(cmd_f32);
    stasis_shutdown();
    puts("sprite atlas query/stage/commit transaction contract passed");
    return 0;
}
