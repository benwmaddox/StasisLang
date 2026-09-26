package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopRenderSurfaceReadinessTest {
    @Test
    public void waitsForLayoutAndDrawableAfterInitialOneByOneSurface() {
        assertFalse(WorkshopRenderSurfaceReadiness.isReady(1, 1, 1, 1));
        assertFalse(WorkshopRenderSurfaceReadiness.isReady(1080, 2201, 1, 1));
        assertFalse(WorkshopRenderSurfaceReadiness.isReady(1, 1, 1080, 2201));
        assertTrue(WorkshopRenderSurfaceReadiness.isReady(1080, 2201, 1080, 2201));

        StasisPreviewRenderer.DisplayViewport drawable =
                StasisPreviewRenderer.fitViewport(640, 360, 1080, 2201);
        assertEquals(0, drawable.x);
        assertEquals(796, drawable.y);
        assertEquals(1080, drawable.width);
        assertEquals(608, drawable.height);
    }

    @Test
    public void renderSurfaceWaitHasAnExactBoundedDeadline() {
        assertFalse(WorkshopRenderSurfaceReadiness.hasTimedOut(1_000L, 30_999L, 30_000L));
        assertTrue(WorkshopRenderSurfaceReadiness.hasTimedOut(1_000L, 31_000L, 30_000L));
        assertFalse(WorkshopRenderSurfaceReadiness.hasTimedOut(1_000L, 31_000L, 0L));
        assertFalse(WorkshopRenderSurfaceReadiness.hasTimedOut(2_000L, 1_000L, 30_000L));
    }
}
