package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiToolLoopPolicyTest {
    @Test
    public void boundedInspectionTransitionsToWriteInsteadOfExecutingMoreReads() {
        WorkshopAiToolLoopPolicy policy = new WorkshopAiToolLoopPolicy(2);

        assertTrue(policy.shouldExecute(false));
        policy.recordBatch(false);
        assertTrue(policy.shouldExecute(false));
        policy.recordBatch(false);
        assertTrue(policy.requiresWriteOrDone());
        assertFalse(policy.shouldExecute(false));

        assertTrue(policy.shouldExecute(true));
        policy.recordBatch(true);
        assertFalse(policy.requiresWriteOrDone());
    }

    @Test
    public void restoresReadOnlyProgressWithoutReplayingAcceptedBatches() {
        WorkshopAiToolLoopPolicy policy = new WorkshopAiToolLoopPolicy(2);
        policy.restoreConsecutiveReadOnlyBatches(2);

        assertTrue(policy.requiresWriteOrDone());
        assertFalse(policy.shouldExecute(false));
        assertTrue(policy.shouldExecute(true));
    }
}
