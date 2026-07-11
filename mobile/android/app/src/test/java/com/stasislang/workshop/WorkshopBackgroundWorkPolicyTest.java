package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopBackgroundWorkPolicyTest {
    @Test
    public void explicitWorkRunsDuringBatterySaver() {
        assertEquals(WorkshopBackgroundWorkPolicy.Decision.RUN,
                WorkshopBackgroundWorkPolicy.decide(true, true, true, false));
    }

    @Test
    public void automaticWorkDefersWhenUnpluggedInBatterySaver() {
        assertEquals(WorkshopBackgroundWorkPolicy.Decision.DEFER_FOR_BATTERY,
                WorkshopBackgroundWorkPolicy.decide(false, true, true, false));
    }

    @Test
    public void chargingAllowsAutomaticWork() {
        assertEquals(WorkshopBackgroundWorkPolicy.Decision.RUN,
                WorkshopBackgroundWorkPolicy.decide(false, true, true, true));
    }

    @Test
    public void networkGatePrecedesBatteryPolicy() {
        assertEquals(WorkshopBackgroundWorkPolicy.Decision.WAIT_FOR_NETWORK,
                WorkshopBackgroundWorkPolicy.decide(true, false, false, true));
    }
}
