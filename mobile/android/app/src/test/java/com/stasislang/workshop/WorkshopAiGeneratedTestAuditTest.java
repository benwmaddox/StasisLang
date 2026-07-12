package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.Collections;

import org.junit.Test;

public final class WorkshopAiGeneratedTestAuditTest {
    @Test
    public void geometryTestNeedsObservableBoundaryCases() {
        WorkshopAiVerificationPolicy.Decision policy = WorkshopAiVerificationPolicy.classify(
                "change ball collision size", Collections.singleton("update_ball"));
        String source = "test `bounds`(): bool { init(); update_ball(); "
                + "if (GameState.ball_x < 9) { return false; } "
                + "if (GameState.ball_x == 10) { return false; } "
                + "return GameState.ball_x > 11; }";
        WorkshopAiGeneratedTestAudit.Result result = WorkshopAiGeneratedTestAudit.audit(
                Collections.singleton(source), policy);
        assertEquals(3, result.passed);
        assertTrue(result.observableBehavior);
        assertTrue(result.boundaryCoverage);
    }

    @Test
    public void helperOnlyAssertionIsNotObservableCoverage() {
        WorkshopAiVerificationPolicy.Decision policy = WorkshopAiVerificationPolicy.classify(
                "change collision size", Collections.singleton("collision_helper"));
        WorkshopAiGeneratedTestAudit.Result result = WorkshopAiGeneratedTestAudit.audit(
                Collections.singleton("test `weak`(): bool { return helper(10); }"), policy);
        assertFalse(result.observableBehavior);
        assertFalse(result.boundaryCoverage);
    }
}
