package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class RendererResourceLifecycleTest {
    @Test
    public void restoreFailureRetriesWithoutPresentingStaleResources() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();
        lifecycle.onSurfaceChanged();
        assertEquals(2, lifecycle.surfaceGeneration());
        assertEquals(1, lifecycle.rendererGeneration());
        assertEquals(2, lifecycle.presentationGeneration());
        assertFalse(lifecycle.canPresent());

        assertTrue(lifecycle.beginRestore());
        lifecycle.finishRestore(false);
        assertFalse(lifecycle.canPresent());
        assertEquals(3, lifecycle.presentationGeneration());
        assertTrue(lifecycle.beginRestore());
        lifecycle.finishRestore(true);
        assertTrue(lifecycle.canPresent());
        assertEquals(2, lifecycle.restoreAttempts());
        assertEquals(1, lifecycle.restoreFailures());
    }

    @Test
    public void pauseResumePreservesResourcesAndRecreationAdvancesGenerations() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();
        assertTrue(lifecycle.beginRestore());
        lifecycle.finishRestore(true);
        lifecycle.onPause();
        assertEquals(RendererResourceLifecycle.State.PAUSED, lifecycle.state());
        lifecycle.onResume();
        assertEquals(RendererResourceLifecycle.State.READY, lifecycle.state());
        assertEquals(1, lifecycle.surfaceGeneration());
        assertEquals(1, lifecycle.rendererGeneration());
        assertEquals(2, lifecycle.presentationGeneration());
        assertEquals("foreground", lifecycle.reason());
        lifecycle.onRendererCreated();
        assertEquals(2, lifecycle.surfaceGeneration());
        assertEquals(2, lifecycle.rendererGeneration());
        assertEquals(3, lifecycle.presentationGeneration());
        assertFalse(lifecycle.canPresent());
    }

    @Test
    public void surfaceResizeKeepsRendererReadyAndGpuResourcesValid() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();
        assertTrue(lifecycle.beginRestore());
        lifecycle.finishRestore(true);

        lifecycle.onSurfaceChanged();

        assertEquals(RendererResourceLifecycle.State.READY, lifecycle.state());
        assertEquals(2, lifecycle.surfaceGeneration());
        assertEquals(1, lifecycle.rendererGeneration());
        assertEquals(2, lifecycle.presentationGeneration());
        assertTrue(lifecycle.canPresent());
        assertEquals("surface_changed", lifecycle.reason());
    }

    @Test
    public void explicitRedrawOnlyAdvancesPresentationGeneration() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();
        int surface = lifecycle.surfaceGeneration();
        int renderer = lifecycle.rendererGeneration();

        lifecycle.requestRedraw();

        assertEquals(surface, lifecycle.surfaceGeneration());
        assertEquals(renderer, lifecycle.rendererGeneration());
        assertEquals(2, lifecycle.presentationGeneration());
    }

    @Test
    public void presentationGenerationWrapsAcrossSignedRangeWithoutZero() {
        assertEquals(Integer.MIN_VALUE,
                RendererResourceLifecycle.nextGeneration(Integer.MAX_VALUE));
        assertEquals(1, RendererResourceLifecycle.nextGeneration(0));
    }

    @Test
    public void pauseBeforeInitialRestoreStillRequiresRestoreAfterResume() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();

        lifecycle.onPause();
        lifecycle.onResume();

        assertEquals(RendererResourceLifecycle.State.RESTORE_PENDING, lifecycle.state());
        assertFalse(lifecycle.canPresent());
        assertTrue(lifecycle.beginRestore());
    }

    @Test
    public void deferredRestoreKeepsWithholdingGameFramesWithoutCountingFailure() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();
        assertTrue(lifecycle.beginRestore());

        lifecycle.deferRestore();

        assertEquals(RendererResourceLifecycle.State.RESTORE_PENDING, lifecycle.state());
        assertFalse(lifecycle.canPresent());
        assertEquals(0, lifecycle.restoreFailures());
        assertEquals(2, lifecycle.presentationGeneration());
        assertTrue(lifecycle.beginRestore());
    }
}
