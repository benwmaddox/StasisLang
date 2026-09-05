package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopSpriteAtlasTest {
    @Test
    public void dedicatedPageBoundaryReservesIndependentWidthAndHeightOverhead() {
        int widthCap = WorkshopSpriteAtlas.maximumRasterWidth(256);
        int heightCap = WorkshopSpriteAtlas.maximumRasterHeight(256);
        assertEquals(254, widthCap);
        assertEquals(250, heightCap);
        assertEquals(4094, WorkshopSpriteAtlas.maximumRasterWidth(4096));
        assertEquals(4090, WorkshopSpriteAtlas.maximumRasterHeight(4096));

        assertTrue(AndroidRasterPlan.exact(
                254, 250, null, 1.0f, widthCap, heightCap).supported);
        assertFalse(AndroidRasterPlan.exact(
                255, 250, null, 1.0f, widthCap, heightCap).supported);
        assertFalse(AndroidRasterPlan.exact(
                254, 251, null, 1.0f, widthCap, heightCap).supported);
    }

    @Test
    public void uploadReceiptCountsTheExactExtrudedPayload() {
        assertEquals(4L * 102L * 52L, WorkshopSpriteAtlas.uploadBytes(100, 50));
        assertEquals(0L, WorkshopSpriteAtlas.uploadBytes(0, 50));
    }

    @Test
    public void packsDifferentSizesOnOnePageWithoutUsingTheReservedHeader() {
        WorkshopSpriteAtlas atlas = new WorkshopSpriteAtlas(512);
        WorkshopSpriteAtlas.Region a = atlas.allocate(32, 17);
        WorkshopSpriteAtlas.Region b = atlas.allocate(91, 43);

        assertNotNull(a);
        assertNotNull(b);
        assertEquals(0, a.page);
        assertEquals(0, b.page);
        assertTrue(a.y >= 5);
        assertTrue(b.y >= 5);
        assertEquals(1, atlas.pageCount());
    }

    @Test
    public void createsAnotherDeterministicPageAndRejectsOversizeForDedicatedDomain() {
        WorkshopSpriteAtlas atlas = new WorkshopSpriteAtlas(256);
        WorkshopSpriteAtlas.Region first = atlas.allocate(250, 120);
        WorkshopSpriteAtlas.Region second = atlas.allocate(250, 120);
        WorkshopSpriteAtlas.Region third = atlas.allocate(250, 120);

        assertEquals(0, first.page);
        assertEquals(0, second.page);
        assertEquals(1, third.page);
        assertNull(atlas.allocate(255, 20));
    }

    @Test
    public void solidsPreferActivePageThenBoundedFollowingPage() {
        assertEquals(17, WorkshopSpriteAtlas.chooseSolidTexture(17, 23));
        assertEquals(23, WorkshopSpriteAtlas.chooseSolidTexture(0, 23));
        assertEquals(0, WorkshopSpriteAtlas.chooseSolidTexture(0, 0));
    }

    @Test
    public void painterOrderedAbabAndAbcacbCollapseOnlyWhenDomainsAreCompatible() {
        assertEquals(1, WorkshopSpriteAtlas.countMixedRuns(
                new boolean[]{false, false, false, false},
                new int[]{7, 7, 7, 7}, 4, 128));
        assertEquals(1, WorkshopSpriteAtlas.countMixedRuns(
                new boolean[]{false, false, true, false, true, false},
                new int[]{7, 7, 0, 7, 0, 7}, 6, 128));
        assertEquals(4, WorkshopSpriteAtlas.countMixedRuns(
                new boolean[]{false, false, false, false},
                new int[]{7, 9, 7, 9}, 4, 128));
        assertEquals(4, WorkshopSpriteAtlas.countMixedRuns(
                new boolean[]{false, false, true, false, true, false},
                new int[]{7, 9, 0, 7, 0, 9}, 6, 128));
        assertEquals(2, WorkshopSpriteAtlas.countMixedRuns(
                new boolean[]{false, false, false},
                new int[]{7, 7, 7}, 3, 2));
    }

    @Test
    public void differentlySizedTranslucentSolidsDoNotAffectPageCompatibility() {
        // Size and alpha are vertex attributes; only the page sequence participates
        // in batching, so differently shaped translucent solids stay in painter order.
        assertEquals(1, WorkshopSpriteAtlas.countMixedRuns(
                new boolean[]{false, true, true, false},
                new int[]{11, 0, 0, 11}, 4, 128));
    }

    @Test
    public void logicalCropMapsInsideAtlasRegionIndependentOfRasterDensity() {
        assertEquals(0.30f, WorkshopSpriteAtlas.atlasCoordinate(
                0.20f, 0.60f, 25.0f, 100.0f), 0.0001f);
        assertEquals(0.50f, WorkshopSpriteAtlas.atlasCoordinate(
                0.20f, 0.60f, 75.0f, 100.0f), 0.0001f);
    }
}
