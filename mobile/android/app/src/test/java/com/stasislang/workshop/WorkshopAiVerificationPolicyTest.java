package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.Arrays;
import java.util.Collections;

import org.junit.Test;

public final class WorkshopAiVerificationPolicyTest {
    @Test
    public void gameplayGeometryRequiresIndependentBoundaryReview() {
        WorkshopAiVerificationPolicy.Decision decision = WorkshopAiVerificationPolicy.classify(
                "make the ball 20 pixels square and update collision", Arrays.asList("render", "update_ball"));
        assertEquals(WorkshopAiVerificationPolicy.Risk.GAMEPLAY, decision.risk);
        assertTrue(decision.requiresBehaviorTest);
        assertTrue(decision.requiresBoundaryCoverage);
        assertTrue(decision.requiresIndependentReview);
    }

    @Test
    public void structuralChangesTakePriority() {
        WorkshopAiVerificationPolicy.Decision decision = WorkshopAiVerificationPolicy.classify(
                "add health", Arrays.asList("GameState", "init"));
        assertEquals(WorkshopAiVerificationPolicy.Risk.STRUCTURAL, decision.risk);
        assertTrue(decision.requiresIndependentReview);
    }

    @Test
    public void plainCopyChangeStaysLowRisk() {
        WorkshopAiVerificationPolicy.Decision decision = WorkshopAiVerificationPolicy.classify(
                "rename the help caption", Collections.<String>emptyList());
        assertEquals(WorkshopAiVerificationPolicy.Risk.LOW, decision.risk);
        assertFalse(decision.requiresIndependentReview);
    }
}
