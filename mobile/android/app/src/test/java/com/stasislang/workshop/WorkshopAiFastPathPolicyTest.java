package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiFastPathPolicyTest {
    @Test
    public void recognizesShortTuningRequests() {
        assertTrue(WorkshopAiFastPathPolicy.isSimpleTuningPrompt("paddles should be double height"));
        assertTrue(WorkshopAiFastPathPolicy.isSimpleTuningPrompt("increase.paddle size"));
        assertTrue(WorkshopAiFastPathPolicy.isSimpleTuningPrompt("make the ball slower"));
    }

    @Test
    public void rejectsShortButStructuralRequests() {
        assertFalse(WorkshopAiFastPathPolicy.isSimpleTuningPrompt("add a new multiplayer system"));
        assertFalse(WorkshopAiFastPathPolicy.isSimpleTuningPrompt("create a new level editor"));
    }

    @Test
    public void boundsInitialSourceContext() {
        assertTrue(WorkshopAiFastPathPolicy.canAppendSource(100, 200, 3));
        assertFalse(WorkshopAiFastPathPolicy.canAppendSource(
                WorkshopAiFastPathPolicy.MAX_SOURCE_CHARS - 2, 2, 1));
        assertFalse(WorkshopAiFastPathPolicy.canAppendSource(
                0, 1, WorkshopAiFastPathPolicy.MAX_SOURCE_SYMBOLS));
    }

    @Test
    public void finalizesOnlyTestedCompiledWrites() {
        assertTrue(WorkshopAiFastPathPolicy.canAutoFinalize(true, 2, true, true));
        assertFalse(WorkshopAiFastPathPolicy.canAutoFinalize(false, 2, true, true));
        assertFalse(WorkshopAiFastPathPolicy.canAutoFinalize(true, 0, true, true));
        assertFalse(WorkshopAiFastPathPolicy.canAutoFinalize(true, 2, false, true));
        assertFalse(WorkshopAiFastPathPolicy.canAutoFinalize(true, 2, true, false));
    }
}
