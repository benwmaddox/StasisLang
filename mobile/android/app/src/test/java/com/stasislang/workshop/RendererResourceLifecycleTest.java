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
        assertFalse(lifecycle.canPresent());

        assertTrue(lifecycle.beginRestore());
        lifecycle.finishRestore(false);
        assertFalse(lifecycle.canPresent());
        assertTrue(lifecycle.beginRestore());
        lifecycle.finishRestore(true);
        assertTrue(lifecycle.canPresent());
        assertEquals(2, lifecycle.restoreAttempts());
        assertEquals(1, lifecycle.restoreFailures());
    }

    @Test
    public void repeatedPauseResumeAndRecreationAdvanceGenerations() {
        RendererResourceLifecycle lifecycle = new RendererResourceLifecycle();
        lifecycle.onRendererCreated();
        lifecycle.onPause();
        assertEquals(RendererResourceLifecycle.State.PAUSED, lifecycle.state());
        lifecycle.onResume();
        assertEquals(2, lifecycle.surfaceGeneration());
        assertEquals(2, lifecycle.rendererGeneration());
        assertEquals("foreground", lifecycle.reason());
        lifecycle.onRendererCreated();
        assertEquals(3, lifecycle.surfaceGeneration());
        assertEquals(3, lifecycle.rendererGeneration());
        assertFalse(lifecycle.canPresent());
    }
}
