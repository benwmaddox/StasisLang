package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiBudgetPolicyTest {
    @Test
    public void usesOnlyTheDeviceMonthlyLimit() {
        assertTrue(WorkshopAiBudgetPolicy.canStart(5.00, 0.09));
        assertEquals(4.91, WorkshopAiBudgetPolicy.remainingUsd(5.00, 0.09), 0.0000001);
        assertFalse(WorkshopAiBudgetPolicy.canStart(5.00, 5.00));
        assertFalse(WorkshopAiBudgetPolicy.canStart(0.00, 0.00));
    }

    @Test
    public void neverReturnsNegativeRemainingBudget() {
        assertEquals(0.0, WorkshopAiBudgetPolicy.remainingUsd(5.00, 5.25), 0.0);
        assertEquals(5.0, WorkshopAiBudgetPolicy.remainingUsd(5.00, -1.00), 0.0);
    }
}
