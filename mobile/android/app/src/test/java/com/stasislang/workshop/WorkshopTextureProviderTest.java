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
}
