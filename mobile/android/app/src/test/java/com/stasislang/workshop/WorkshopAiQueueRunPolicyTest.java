package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopAiQueueRunPolicyTest {
    @Test
    public void offlinePendingWorkWaitsAndThenClaimsWhenConnectivityReturns() {
        assertEquals(WorkshopAiQueueRunPolicy.Decision.WAIT_FOR_NETWORK,
                decide(true, false));
        assertEquals(WorkshopAiQueueRunPolicy.Decision.CLAIM_NEXT,
                decide(true, true));
    }

    @Test
    public void queueDoesNotClaimDuringAnotherOperationOrRecoveryPause() {
        assertEquals(WorkshopAiQueueRunPolicy.Decision.IDLE,
                WorkshopAiQueueRunPolicy.decide(false, true, false,
                        false, false, true, true));
        assertEquals(WorkshopAiQueueRunPolicy.Decision.IDLE,
                WorkshopAiQueueRunPolicy.decide(false, false, true,
                        false, false, true, true));
        assertEquals(WorkshopAiQueueRunPolicy.Decision.IDLE,
                WorkshopAiQueueRunPolicy.decide(true, false, false,
                        false, false, true, true));
    }

    private static WorkshopAiQueueRunPolicy.Decision decide(
            boolean pending, boolean network) {
        return WorkshopAiQueueRunPolicy.decide(false, false, false,
                false, false, pending, network);
    }
}
