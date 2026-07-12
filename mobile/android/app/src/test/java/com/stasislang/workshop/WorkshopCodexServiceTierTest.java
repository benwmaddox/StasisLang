package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class WorkshopCodexServiceTierTest {
    @Test
    public void standardModeUsesTheDefaultTier() {
        assertEquals("", WorkshopCodexServiceTier.requestTier(false));
    }

    @Test
    public void fastModeRequestsTheCatalogPriorityTier() {
        assertEquals("priority", WorkshopCodexServiceTier.requestTier(true));
    }
}
