package com.stasislang.workshop;

import org.json.JSONObject;

final class WorkshopImageGenerationProfile {
    // GPT Image 2 output estimates documented by OpenAI on 2026-08-06.
    static final String OFF_ID = "off";
    static final String DRAFT_SQUARE_ID = "draft_square";
    static final String FINAL_SQUARE_ID = "final_square";
    static final String FINAL_LANDSCAPE_ID = "final_landscape";
    static final String FINAL_PORTRAIT_ID = "final_portrait";

    private static final WorkshopImageGenerationProfile[] PROFILES = {
            new WorkshopImageGenerationProfile(OFF_ID, "No generated image", "", "", 0.0),
            new WorkshopImageGenerationProfile(DRAFT_SQUARE_ID,
                    "Draft - low 1024x1024 (~$0.006)", "low", "1024x1024", 0.006),
            new WorkshopImageGenerationProfile(FINAL_SQUARE_ID,
                    "Final square - high 1024x1024 (~$0.211)", "high", "1024x1024", 0.211),
            new WorkshopImageGenerationProfile(FINAL_LANDSCAPE_ID,
                    "Final landscape - high 1536x1024 (~$0.165)", "high", "1536x1024", 0.165),
            new WorkshopImageGenerationProfile(FINAL_PORTRAIT_ID,
                    "Final portrait - high 1024x1536 (~$0.165)", "high", "1024x1536", 0.165),
    };

    final String id;
    final String label;
    final String quality;
    final String size;
    final double reserveUsd;

    private WorkshopImageGenerationProfile(
            String id, String label, String quality, String size, double reserveUsd) {
        this.id = id;
        this.label = label;
        this.quality = quality;
        this.size = size;
        this.reserveUsd = reserveUsd;
    }

    boolean enabled() {
        return !OFF_ID.equals(id);
    }

    JSONObject toolOptions() throws Exception {
        if (!enabled()) throw new IllegalStateException("off profile has no ImageGen tool options");
        return new JSONObject()
                .put("type", "image_generation")
                .put("action", "auto")
                .put("quality", quality)
                .put("size", size)
                .put("output_format", "png");
    }

    static WorkshopImageGenerationProfile fromId(String id) {
        for (WorkshopImageGenerationProfile profile : PROFILES) {
            if (profile.id.equals(id)) return profile;
        }
        throw new IllegalArgumentException("unknown image generation profile: " + id);
    }

    static WorkshopImageGenerationProfile fromSelection(int position) {
        if (position < 0 || position >= PROFILES.length) return PROFILES[0];
        return PROFILES[position];
    }

    static WorkshopImageGenerationProfile fromLegacyFlag(boolean enabled) {
        return fromId(enabled ? DRAFT_SQUARE_ID : OFF_ID);
    }

    static String[] labels() {
        String[] labels = new String[PROFILES.length];
        for (int index = 0; index < PROFILES.length; index += 1) {
            labels[index] = PROFILES[index].label;
        }
        return labels;
    }
}
