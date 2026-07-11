package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopCodexLimitsTest {
    @Test
    public void formatsFiveHourAndWeeklyRemainingPercent() {
        assertEquals("5h 75% left", WorkshopCodexLimits.formatWindow(300L, 25.0));
        assertEquals("weekly 58% left", WorkshopCodexLimits.formatWindow(10_080L, 42.4));
    }

    @Test
    public void clampsRemainingPercent() {
        assertEquals("5h 100% left", WorkshopCodexLimits.formatWindow(300L, -4.0));
        assertEquals("5h 0% left", WorkshopCodexLimits.formatWindow(300L, 110.0));
    }
}
