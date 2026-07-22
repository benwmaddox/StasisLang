package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopFrameBudgetTest {
    @Test
    public void reportsTickRenderAndCombinedSharesOfSixtyFpsFrame() {
        assertEquals(5, WorkshopFrameBudget.percent(0.91));
        assertEquals(3, WorkshopFrameBudget.percent(0.55));
        assertEquals(9, WorkshopFrameBudget.percent(0.91 + 0.55));
        assertEquals(0, WorkshopFrameBudget.percent(-1.0));
    }
}
