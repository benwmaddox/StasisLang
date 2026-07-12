package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopRestartLoopPolicyTest {
    @Test
    public void threeEarlyCrashesDetectRestartLoop() {
        WorkshopRestartLoopPolicy.Result result = WorkshopRestartLoopPolicy.noteCrash(
                2_000L, 2, 3_000L, 4_000L);
        assertEquals(3, result.consecutiveEarlyCrashes);
        assertTrue(result.restartLoopDetected);
    }

    @Test
    public void oldCrashStartsNewSequence() {
        WorkshopRestartLoopPolicy.Result result = WorkshopRestartLoopPolicy.noteCrash(
                1_000L, 2, 1_000L + WorkshopRestartLoopPolicy.LOOP_WINDOW_MS + 1L,
                1_000L + WorkshopRestartLoopPolicy.LOOP_WINDOW_MS + 2L);
        assertEquals(1, result.consecutiveEarlyCrashes);
        assertFalse(result.restartLoopDetected);
    }

    @Test
    public void sameCrashRecordIsNotCountedTwice() {
        WorkshopRestartLoopPolicy.Result result = WorkshopRestartLoopPolicy.noteCrash(
                3_000L, 2, 3_000L, 4_000L);
        assertEquals(2, result.consecutiveEarlyCrashes);
        assertFalse(result.restartLoopDetected);
    }
}
