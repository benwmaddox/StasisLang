package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopTextureProviderTest {
    @Test
    public void projectIdentityInvalidatesTextureCache() {
        assertTrue(WorkshopTextureProvider.projectChanged(null, "projects/one"));
        assertFalse(WorkshopTextureProvider.projectChanged("projects/one", "projects/one"));
        assertTrue(WorkshopTextureProvider.projectChanged("projects/one", "projects/two"));
    }

    @Test
    public void zeroSpriteHandleUsesFallbackWithoutRejectingStableSignedHandles() {
        assertTrue(WorkshopTextureProvider.usesFallbackSprite(0));
        assertFalse(WorkshopTextureProvider.usesFallbackSprite(17));
        assertFalse(WorkshopTextureProvider.usesFallbackSprite(-17));
    }

    @Test
    public void resourceIdentityIncludesProjectEvenWhenNumericHandleCollides() {
        String alpha = WorkshopTextureProvider.acceptanceIdentity(
                "sprite", 17, "/projects/alpha", "abc");
        String beta = WorkshopTextureProvider.acceptanceIdentity(
                "sprite", 17, "/projects/beta", "abc");

        assertFalse(alpha.equals(beta));
        assertEquals("sprite:17:/projects/alpha:abc", alpha);
    }

    @Test
    public void underprovisionDiagnosticNamesSourceRequiredPreparedAndMemory() {
        assertEquals(
                "sprite_source_resolution status=source-underprovisioned path=/project/sheet.png"
                        + " content_sha256=abc source=64x32 required=192x96 prepared=192x96"
                        + " decoded_bytes=8192 prepared_bytes=73728",
                WorkshopTextureProvider.formatSourceResolutionDiagnostic(
                        "/project/sheet.png", "abc", 64, 32, 192, 96, 192, 96,
                        8192L, 73728L));
    }

    @Test
    public void generationMatchRejectsEitherStaleGenerationDimension() {
        assertTrue(WorkshopTextureProvider.generationMatches(4, 7, 4, 7));
        assertFalse(WorkshopTextureProvider.generationMatches(3, 7, 4, 7));
        assertFalse(WorkshopTextureProvider.generationMatches(4, 6, 4, 7));
    }

    @Test
    public void textRasterMemoryLimitIsExactAndOverflowSafe() {
        assertTrue(WorkshopTextureProvider.textRasterSupported(2048, 2048));
        assertFalse(WorkshopTextureProvider.textRasterSupported(2049, 2048));
        assertFalse(WorkshopTextureProvider.textRasterSupported(Integer.MAX_VALUE, 2));
        assertFalse(WorkshopTextureProvider.textRasterSupported(0, 1));
    }

    @Test
    public void textRasterPlanPreservesLogicalBoundsThroughPhysicalRounding() {
        WorkshopTextureProvider.TextRasterPlan plan =
                WorkshopTextureProvider.textRasterPlan(10.1f, -7.2f, 2.9f, 1.501f, 4096);
        assertTrue(plan.supported);
        assertEquals(11, plan.logicalWidth);
        assertEquals(11, plan.logicalHeight);
        assertEquals(17, plan.rasterWidth);
        assertEquals(17, plan.rasterHeight);
        assertEquals(17.0f / 11.0f, plan.scaleX(), 0.0001f);
        assertEquals(17L * 17L * 4L, plan.byteLength());
    }

    @Test
    public void textRasterPlanRejectsDeviceAxisLimitBeforeAllocation() {
        WorkshopTextureProvider.TextRasterPlan tooWide =
                WorkshopTextureProvider.textRasterPlan(3000.0f, -8.0f, 3.0f, 2.0f, 4096);
        assertFalse(tooWide.supported);
        assertEquals(6000, tooWide.rasterWidth);

        WorkshopTextureProvider.TextRasterPlan aboveSpriteCap =
                WorkshopTextureProvider.textRasterPlan(20.0f, -8.0f, 3.0f, 9.0f, 4096);
        assertTrue(aboveSpriteCap.supported);
        assertEquals(180, aboveSpriteCap.rasterWidth);
    }

    @Test
    public void textCacheIdentityIncludesFontAndExactPhysicalScale() {
        String base = WorkshopTextureProvider.textIdentity("font-a", "label", 2.0f);
        assertFalse(base.equals(WorkshopTextureProvider.textIdentity(
                "font-b", "label", 2.0f)));
        assertFalse(base.equals(WorkshopTextureProvider.textIdentity(
                "font-a", "label", 2.0005f)));
        assertEquals(base, WorkshopTextureProvider.textIdentity("font-a", "label", 2.0f));
    }
}
