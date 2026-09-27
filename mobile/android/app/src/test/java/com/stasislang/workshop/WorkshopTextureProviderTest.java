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

    @Test
    public void fontMetadataCacheHitsUntilCatalogGenerationChanges() throws Exception {
        WorkshopTextureProvider.MetadataCache<String> cache =
                new WorkshopTextureProvider.MetadataCache<>();
        int[] resolverCalls = {0};

        WorkshopTextureProvider.MetadataLookup<String> first =
                WorkshopTextureProvider.resolveImmutableMetadata(cache, 17, 5L, () -> {
                    resolverCalls[0] += 1;
                    return "font metadata";
                });
        WorkshopTextureProvider.MetadataLookup<String> hit =
                WorkshopTextureProvider.resolveImmutableMetadata(cache, 17, 5L, () -> {
                    resolverCalls[0] += 1;
                    return "unexpected";
                });
        WorkshopTextureProvider.MetadataLookup<String> refreshed =
                WorkshopTextureProvider.resolveImmutableMetadata(cache, 17, 6L, () -> {
                    resolverCalls[0] += 1;
                    return "new catalog metadata";
                });

        assertFalse(first.cacheHit);
        assertTrue(hit.cacheHit);
        assertFalse(refreshed.cacheHit);
        assertEquals("new catalog metadata", refreshed.value);
        assertEquals(2, resolverCalls[0]);
    }

    @Test
    public void cachedTextMetadataCachesImmutableRunsButAlwaysResolvesReplaceableRuns()
            throws Exception {
        WorkshopTextureProvider.MetadataCache<WorkshopTextureProvider.CachedTextMetadata> cache =
                new WorkshopTextureProvider.MetadataCache<>();
        int[] immutableCalls = {0};
        int[] replaceableCalls = {0};

        WorkshopTextureProvider.MetadataLookup<WorkshopTextureProvider.CachedTextMetadata> fixed =
                WorkshopTextureProvider.resolveCachedTextMetadata(cache, 9, 12L, () -> {
                    immutableCalls[0] += 1;
                    return new WorkshopTextureProvider.CachedTextMetadata(
                            "/fonts/fixed.ttf", "Fixed", 18, false);
                });
        WorkshopTextureProvider.MetadataLookup<WorkshopTextureProvider.CachedTextMetadata> fixedHit =
                WorkshopTextureProvider.resolveCachedTextMetadata(cache, 9, 12L, () -> {
                    immutableCalls[0] += 1;
                    return new WorkshopTextureProvider.CachedTextMetadata(
                            "/fonts/fixed.ttf", "unexpected", 18, false);
                });
        WorkshopTextureProvider.MetadataLookup<WorkshopTextureProvider.CachedTextMetadata> dynamicOne =
                WorkshopTextureProvider.resolveCachedTextMetadata(cache, 10, 12L, () -> {
                    replaceableCalls[0] += 1;
                    return new WorkshopTextureProvider.CachedTextMetadata(
                            "/fonts/fixed.ttf", "score 1", 18, true);
                });
        WorkshopTextureProvider.MetadataLookup<WorkshopTextureProvider.CachedTextMetadata> dynamicTwo =
                WorkshopTextureProvider.resolveCachedTextMetadata(cache, 10, 12L, () -> {
                    replaceableCalls[0] += 1;
                    return new WorkshopTextureProvider.CachedTextMetadata(
                            "/fonts/fixed.ttf", "score 2", 18, true);
                });

        assertFalse(fixed.cacheHit);
        assertTrue(fixedHit.cacheHit);
        assertEquals(1, immutableCalls[0]);
        assertFalse(dynamicOne.cacheHit);
        assertFalse(dynamicTwo.cacheHit);
        assertEquals("score 1", dynamicOne.value.text);
        assertEquals("score 2", dynamicTwo.value.text);
        assertEquals(2, replaceableCalls[0]);
    }

    @Test
    public void catalogGenerationInvalidatesCachedTextMetadata() throws Exception {
        WorkshopTextureProvider.MetadataCache<WorkshopTextureProvider.CachedTextMetadata> cache =
                new WorkshopTextureProvider.MetadataCache<>();
        int[] resolverCalls = {0};

        WorkshopTextureProvider.resolveCachedTextMetadata(cache, 23, 31L, () -> {
            resolverCalls[0] += 1;
            return new WorkshopTextureProvider.CachedTextMetadata(
                    "/fonts/old.ttf", "label", 18, false);
        });
        WorkshopTextureProvider.MetadataLookup<WorkshopTextureProvider.CachedTextMetadata> nextGeneration =
                WorkshopTextureProvider.resolveCachedTextMetadata(cache, 23, 32L, () -> {
                    resolverCalls[0] += 1;
                    return new WorkshopTextureProvider.CachedTextMetadata(
                            "/fonts/new.ttf", "label", 18, false);
                });

        assertFalse(nextGeneration.cacheHit);
        assertEquals("/fonts/new.ttf", nextGeneration.value.fontPath);
        assertEquals(2, resolverCalls[0]);
    }
}
