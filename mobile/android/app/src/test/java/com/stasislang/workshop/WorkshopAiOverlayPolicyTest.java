package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiOverlayPolicyTest {
    @Test
    public void overlayOnlyShowsOutsideThePanelWhileWorkExists() {
        assertFalse(WorkshopAiOverlayPolicy.shouldShow(false, false, 0));
        assertTrue(WorkshopAiOverlayPolicy.shouldShow(false, true, 0));
        assertTrue(WorkshopAiOverlayPolicy.shouldShow(false, false, 2));
        assertFalse(WorkshopAiOverlayPolicy.shouldShow(true, true, 2));
    }

    @Test
    public void labelsDistinguishActiveAndQueuedWork() {
        assertEquals("AI working", WorkshopAiOverlayPolicy.queueLabel(true, 1));
        assertEquals("AI working +2", WorkshopAiOverlayPolicy.queueLabel(true, 3));
        assertEquals("AI queued", WorkshopAiOverlayPolicy.queueLabel(false, 1));
        assertEquals("AI queue 3", WorkshopAiOverlayPolicy.queueLabel(false, 3));
    }

    @Test
    public void contentDescriptionIncludesProgressAndOpenAction() {
        assertEquals("AI working +1, step 3 of 15, 7 actions, calling AI, time 8.2s; tap to open Workshop",
                WorkshopAiOverlayPolicy.contentDescription(true, 2, 3, 15, 7,
                        "calling AI", "8.2s"));
    }
}
