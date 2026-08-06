package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopImageGenerationProfileTest {
    @Test
    public void profilesExposeExplicitQualitySizeAndConservativeReserve() throws Exception {
        WorkshopImageGenerationProfile draft = WorkshopImageGenerationProfile.fromId(
                WorkshopImageGenerationProfile.DRAFT_SQUARE_ID);
        WorkshopImageGenerationProfile finalLandscape = WorkshopImageGenerationProfile.fromId(
                WorkshopImageGenerationProfile.FINAL_LANDSCAPE_ID);

        assertTrue(draft.enabled());
        assertEquals("low", draft.quality);
        assertEquals("1024x1024", draft.size);
        assertEquals(0.006, draft.reserveUsd, 0.000001);
        assertEquals("high", finalLandscape.quality);
        assertEquals("1536x1024", finalLandscape.size);
        assertEquals(0.165, finalLandscape.reserveUsd, 0.000001);
        assertEquals("image_generation", finalLandscape.toolOptions().getString("type"));
        assertEquals("high", finalLandscape.toolOptions().getString("quality"));
        assertEquals("1536x1024", finalLandscape.toolOptions().getString("size"));
        assertEquals("png", finalLandscape.toolOptions().getString("output_format"));
    }

    @Test
    public void offAndLegacyQueueValuesRemainSafe() {
        assertFalse(WorkshopImageGenerationProfile.fromSelection(-1).enabled());
        assertEquals(WorkshopImageGenerationProfile.DRAFT_SQUARE_ID,
                WorkshopImageGenerationProfile.fromLegacyFlag(true).id);
        assertEquals(WorkshopImageGenerationProfile.OFF_ID,
                WorkshopImageGenerationProfile.fromLegacyFlag(false).id);
    }

    @Test(expected = IllegalArgumentException.class)
    public void unknownPersistedProfileIsRejected() {
        WorkshopImageGenerationProfile.fromId("surprise");
    }
}
