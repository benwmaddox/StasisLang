#ifndef STASIS_MIXED_QUAD_PLANNER_H
#define STASIS_MIXED_QUAD_PLANNER_H

#include <stdint.h>
#include <math.h>

#define STASIS_MIXED_SOLID_DOMAIN (-1)

typedef struct {
    uint32_t submissions;
    uint32_t domain_transitions;
} StasisMixedQuadPlan;

/* Domain ids are host-private atlas page indices. Solids are page-neutral:
 * they inherit an active page or look ahead to the next sprite page. */
static inline StasisMixedQuadPlan stasis_mixed_quad_plan_domains(
    const int32_t* domains, int32_t count, int32_t capacity, int32_t fallback_domain
) {
    StasisMixedQuadPlan plan = {0, 0};
    if (!domains || count <= 0 || capacity <= 0 || fallback_domain < 0) return plan;
    int32_t active = -1;
    int32_t used = 0;
    for (int32_t i = 0; i < count; i++) {
        int32_t domain = domains[i];
        if (domain == STASIS_MIXED_SOLID_DOMAIN) {
            domain = active;
            if (domain < 0) {
                for (int32_t next = i + 1; next < count && next - i <= capacity; next++) {
                    if (domains[next] >= 0) { domain = domains[next]; break; }
                }
            }
            if (domain < 0) domain = fallback_domain;
        }
        if (active >= 0 && domain != active) {
            if (used > 0) plan.submissions++;
            plan.domain_transitions++;
            used = 0;
        }
        active = domain;
        used++;
        if (used == capacity) {
            plan.submissions++;
            used = 0;
        }
    }
    if (used > 0) plan.submissions++;
    return plan;
}

static inline void stasis_mixed_quad_transform(
    float x, float y, float w, float h, float pivot_x, float pivot_y,
    float scale_x, float scale_y, float rotation_degrees, float out_xy[8]
) {
    const float radians = rotation_degrees * (3.14159265f / 180.0f);
    const float c = cosf(radians);
    const float s = sinf(radians);
    const float lx[4] = {0, w, w, 0};
    const float ly[4] = {0, 0, h, h};
    for (int i = 0; i < 4; i++) {
        const float dx = (lx[i] - pivot_x) * scale_x;
        const float dy = (ly[i] - pivot_y) * scale_y;
        out_xy[i * 2] = x + pivot_x + dx * c - dy * s;
        out_xy[i * 2 + 1] = y + pivot_y + dx * s + dy * c;
    }
}

#endif
