package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiPricingTest {
    @Test
    public void recognizesEverySupportedGpt56ModelAndAlias() {
        assertTrue(WorkshopAiPricing.isKnown("gpt-5.6"));
        assertTrue(WorkshopAiPricing.isKnown("gpt-5.6-sol"));
        assertTrue(WorkshopAiPricing.isKnown("gpt-5.6-terra"));
        assertTrue(WorkshopAiPricing.isKnown("gpt-5.6-luna"));
        assertFalse(WorkshopAiPricing.isKnown("gpt-5.6-unknown"));
    }

    @Test
    public void exposesPublishedStandardAndCacheWriteRates() {
        assertRates("gpt-5.6-sol", 5.00, 0.50, 6.25, 30.00);
        assertRates("gpt-5.6-terra", 2.50, 0.25, 3.125, 15.00);
        assertRates("gpt-5.6-luna", 1.00, 0.10, 1.25, 6.00);
    }

    @Test
    public void estimatesUsageWithCacheAndLongContextMultipliers() {
        WorkshopAiPricing.Rates rates = WorkshopAiPricing.forModel("gpt-5.6-terra");
        assertNotNull(rates);
        assertEquals(0.0191875, rates.estimate(2_000, 500, 500, 1_000), 0.0000001);
        assertEquals(24.0, rates.estimate(300_000, 0, 0, 1_000_000), 0.0000001);
        assertEquals(1.875, rates.conservativeInputCostUsd(300_000), 0.0000001);
        assertEquals(22.5, rates.effectiveOutputUsdPerMillion(300_000), 0.0000001);
    }

    private static void assertRates(String model, double input, double cached, double cacheWrite,
            double output) {
        WorkshopAiPricing.Rates rates = WorkshopAiPricing.forModel(model);
        assertNotNull(rates);
        assertEquals(input, rates.inputUsdPerMillion, 0.0);
        assertEquals(cached, rates.cachedInputUsdPerMillion, 0.0);
        assertEquals(cacheWrite, rates.cacheWriteUsdPerMillion, 0.0);
        assertEquals(output, rates.outputUsdPerMillion, 0.0);
    }
}
