package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiRunPhaseTest {
    @Test
    public void persistedPhasesRoundTripAndTerminalStateIsExplicit() {
        for (WorkshopAiRunPhase phase : WorkshopAiRunPhase.values()) {
            assertTrue(WorkshopAiRunPhase.isWireValue(phase.wireValue()));
            assertEquals(phase, WorkshopAiRunPhase.fromWireValue(phase.wireValue()));
        }
        assertFalse(WorkshopAiRunPhase.VERIFYING.terminal());
        assertTrue(WorkshopAiRunPhase.VERIFIED.terminal());
        assertTrue(WorkshopAiRunPhase.RESTORED.terminal());
    }
}
