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
}
