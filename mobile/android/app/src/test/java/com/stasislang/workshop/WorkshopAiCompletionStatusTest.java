package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

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
}
