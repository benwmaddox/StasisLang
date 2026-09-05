package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class AndroidRasterPlanTest {
    @Test public void fullSpriteUsesSmallestAspectPreservingPhysicalRaster() {
        AndroidRasterPlan.Requirement request = new AndroidRasterPlan.Requirement();
        request.include(100, 50, 0, 0, 1, 1);
        AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(400, 200, request, 2.625f, 8192);
        assertEquals(263, plan.width);
        assertEquals(132, plan.height);
        assertTrue(plan.supported);
    }

    @Test public void cropAndNegativeNonUniformScaleUseMaximumSamplingAxis() {
        AndroidRasterPlan.Requirement request = new AndroidRasterPlan.Requirement();
        request.include(40, 20, 10, 20, -2, 3);
        AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(100, 50, request, 1.25f, 8192);
        assertEquals(1000, plan.width);
        assertEquals(500, plan.height);
    }

    @Test public void requirementsAggregateBeforePlanning() {
        AndroidRasterPlan.Requirement request = new AndroidRasterPlan.Requirement();
        request.include(64, 32, 0, 0, 1, 1);
        request.include(20, 20, 5, 5, 2, 2);
        AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(80, 40, request, 1.5f, 8192);
        assertEquals(960, plan.width);
        assertEquals(480, plan.height);
    }

    @Test public void limitsRejectRatherThanSilentlyUndersample() {
        AndroidRasterPlan.Requirement request = new AndroidRasterPlan.Requirement();
        request.include(10_000, 10_000, 0, 0, 2, 2);
        AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(100, 100, request, 3.0f, 4096);
        assertFalse(plan.supported);
        assertEquals(4096, plan.width);
        assertEquals(4096, plan.height);
    }

    @Test public void zeroUploadableCapacityIsNeverReportedSupported() {
        AndroidRasterPlan.Result plan = AndroidRasterPlan.exact(
                1, 1, new AndroidRasterPlan.Requirement(), 1.0f, 0);
        assertFalse(plan.supported);
        assertEquals(1, plan.width);
        assertEquals(1, plan.height);
    }

    @Test public void cacheIdentityChangesForDensityAndEitherGeneration() {
        AndroidRasterPlan.Result plan = new AndroidRasterPlan.Result(300, 150, true);
        String baseline = plan.identity(2.0f, 4, 7);
        assertFalse(baseline.equals(plan.identity(3.0f, 4, 7)));
        assertFalse(baseline.equals(plan.identity(2.0f, 5, 7)));
        assertFalse(baseline.equals(plan.identity(2.0f, 4, 8)));
        assertEquals(baseline, plan.identity(2.0f, 4, 7));
    }
}
