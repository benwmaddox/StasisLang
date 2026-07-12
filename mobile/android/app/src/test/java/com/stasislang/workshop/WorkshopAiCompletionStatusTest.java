package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiCompletionStatusTest {
    @Test
    public void identicalFinalEditsRemainAppliedAfterToolWrites() {
        assertEquals("applied", WorkshopAiCompletionStatus.afterEdits("no change"));
    }

    @Test
    public void preservesActualReloadStatus() {
        assertEquals("hot swapped", WorkshopAiCompletionStatus.afterEdits("hot swapped"));
        assertEquals("reset reload", WorkshopAiCompletionStatus.afterEdits("reset reload"));
        assertEquals("compiled", WorkshopAiCompletionStatus.afterEdits("compiled"));
    }

    @Test
    public void finalizesOnlyTestedCompiledWrites() {
        assertTrue(WorkshopAiCompletionStatus.canFinalizeTestedWrites(true, 2, true, true));
        assertFalse(WorkshopAiCompletionStatus.canFinalizeTestedWrites(false, 2, true, true));
        assertFalse(WorkshopAiCompletionStatus.canFinalizeTestedWrites(true, 0, true, true));
        assertFalse(WorkshopAiCompletionStatus.canFinalizeTestedWrites(true, 2, false, true));
        assertFalse(WorkshopAiCompletionStatus.canFinalizeTestedWrites(true, 2, true, false));
    }
}
