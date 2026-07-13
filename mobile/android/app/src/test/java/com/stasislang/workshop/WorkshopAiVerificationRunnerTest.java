package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.Collections;

import org.junit.Test;

public final class WorkshopAiVerificationRunnerTest {
    @Test
    public void lowRiskCompiledTestedWriteCanApply() {
        WorkshopAiVerificationPolicy.Decision policy = WorkshopAiVerificationPolicy.classify(
                "rename caption", Collections.<String>emptyList());
        WorkshopAiVerificationResult result = WorkshopAiVerificationRunner.verify(
                policy, true, true, 1, Collections.<String>emptyList(), false);
        assertEquals(WorkshopAiVerificationResult.Status.VERIFIED, result.status);
        assertTrue(result.canApplyAutomatically());
    }

    @Test
    public void gameplaySelfTestsRemainInconclusiveWithoutIndependentChecks() {
        WorkshopAiVerificationPolicy.Decision policy = WorkshopAiVerificationPolicy.classify(
                "change collision size", Collections.singleton("update_ball"));
        WorkshopAiVerificationResult result = WorkshopAiVerificationRunner.verify(
                policy, true, true, 2, Collections.singleton("tests/ball.test.stasis"), false);
        assertEquals(WorkshopAiVerificationResult.Status.INCONCLUSIVE, result.status);
        assertFalse(result.canApplyAutomatically());
    }

    @Test
    public void failedGeneratedTestsCannotApply() {
        WorkshopAiVerificationPolicy.Decision policy = WorkshopAiVerificationPolicy.classify(
                "rename caption", Collections.<String>emptyList());
        WorkshopAiVerificationResult result = WorkshopAiVerificationRunner.verify(
                policy, true, false, 1, Collections.<String>emptyList(), false);
        assertEquals(WorkshopAiVerificationResult.Status.FAILED, result.status);
    }

    @Test
    public void independentReviewerFillsExistingCheckSlot() {
        WorkshopAiVerificationResult preliminary = new WorkshopAiVerificationResult(
                WorkshopAiVerificationResult.Status.INCONCLUSIVE,
                WorkshopAiVerificationPolicy.Risk.GAMEPLAY, 4, 5,
                "independent verification is required", 0L);
        WorkshopAiVerificationResult verified = WorkshopAiVerificationRunner.completeIndependent(
                preliminary, WorkshopAiVerificationResult.Status.VERIFIED, "passed", 12L);
        assertEquals(5, verified.passedChecks);
        assertEquals(5, verified.totalChecks);
    }
}
