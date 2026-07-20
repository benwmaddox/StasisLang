package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiResumePolicyTest {
    @Test
    public void safeBoundariesResumeBeforeOrAfterToolsWithoutFreshCalls() {
        assertTrue(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.READY, true, true, true, false).resumable);
        assertTrue(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.RESPONSE_READY, true, true, true, false).resumable);
    }

    @Test
    public void uncertainPaidCallFallsBackWithoutReplay() {
        WorkshopAiResumePolicy.Decision decision = WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.PROVIDER_IN_FLIGHT, true, true, true, false);

        assertFalse(decision.resumable);
        assertTrue(decision.detail.contains("paid provider call may have completed"));
        assertTrue(decision.detail.contains("will not be replayed"));
        assertTrue(decision.detail.contains("Fresh Retry"));
    }

    @Test
    public void changedSnapshotsProviderAndCancellationRequireFreshRetry() {
        assertFresh(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.READY, false, true, true, false), "Project files changed");
        assertFresh(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.READY, true, false, true, false), "attachments changed");
        assertFresh(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.READY, true, true, false, false), "provider or model changed");
        assertFresh(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.READY, true, true, true, true), "cancelled");
        assertFresh(WorkshopAiResumePolicy.decide(
                WorkshopAiResumePolicy.CANCEL_REQUESTED, true, true, true, false), "cancelled");
    }

    @Test
    public void interruptedQueueItemOnlyReturnsToPendingWithSafeCheckpoint() {
        assertTrue("pending".equals(AiQueuePolicy.recoveredState("in_progress", true, false)));
        assertTrue("failed".equals(AiQueuePolicy.recoveredState("in_progress", false, false)));
        assertTrue("cancelled".equals(AiQueuePolicy.recoveredState("in_progress", false, true)));
        assertTrue("completed".equals(AiQueuePolicy.recoveredState("completed", true, true)));
    }

    private static void assertFresh(WorkshopAiResumePolicy.Decision decision, String detail) {
        assertFalse(decision.resumable);
        assertTrue(decision.detail.contains(detail));
        assertTrue(decision.detail.contains("Fresh Retry"));
    }
}
