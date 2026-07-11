package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopAiPricingTest {
    @Test
    public void recognizesSupportedGpt5AndLaterModelsAndSnapshots() {
        String[] models = {
                "gpt-5", "gpt-5-2025-08-07", "gpt-5-mini", "gpt-5-mini-2025-08-07",
                "gpt-5-nano", "gpt-5-nano-2025-08-07", "gpt-5-pro", "gpt-5-pro-2025-10-06",
                "gpt-5.1", "gpt-5.1-2025-11-13", "gpt-5.2", "gpt-5.2-2025-12-11",
                "gpt-5.2-pro", "gpt-5.2-pro-2025-12-11", "gpt-5.3-codex",
                "gpt-5.4", "gpt-5.4-2026-03-05", "gpt-5.4-mini", "gpt-5.4-mini-2026-03-17",
                "gpt-5.4-nano", "gpt-5.4-nano-2026-03-17", "gpt-5.4-pro", "gpt-5.4-pro-2026-03-05",
                "gpt-5.5", "gpt-5.5-2026-04-23", "gpt-5.5-pro", "gpt-5.5-pro-2026-04-23",
                "gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"
        };
        for (String model : models) assertTrue(model, WorkshopAiPricing.isKnown(model));
        assertTrue(WorkshopAiPricing.isKnown("gpt-5.6"));
        assertFalse(WorkshopAiPricing.isKnown("gpt-5.2-chat-latest"));
        assertFalse(WorkshopAiPricing.isKnown("gpt-5.2-codex"));
        assertFalse(WorkshopAiPricing.isKnown("gpt-5.3-codex-spark"));
        assertFalse(WorkshopAiPricing.isKnown("gpt-5.6-unknown"));
    }

    @Test
    public void exposesPublishedStandardAndCacheWriteRates() {
        assertRates("gpt-5", 1.25, 0.125, 1.25, 10.00);
        assertRates("gpt-5-mini", 0.25, 0.025, 0.25, 2.00);
        assertRates("gpt-5-nano", 0.05, 0.005, 0.05, 0.40);
        assertRates("gpt-5-pro", 15.00, 15.00, 15.00, 120.00);
        assertRates("gpt-5.2", 1.75, 0.175, 1.75, 14.00);
        assertRates("gpt-5.2-pro", 21.00, 21.00, 21.00, 168.00);
        assertRates("gpt-5.4", 2.50, 0.25, 2.50, 15.00);
        assertRates("gpt-5.4-mini", 0.75, 0.075, 0.75, 4.50);
        assertRates("gpt-5.4-nano", 0.20, 0.02, 0.20, 1.25);
        assertRates("gpt-5.4-pro", 30.00, 30.00, 30.00, 180.00);
        assertRates("gpt-5.5", 5.00, 0.50, 5.00, 30.00);
        assertRates("gpt-5.5-pro", 30.00, 30.00, 30.00, 180.00);
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
        WorkshopAiPricing.Rates legacy = WorkshopAiPricing.forModel("gpt-5.2");
        assertNotNull(legacy);
        assertEquals(legacy.outputUsdPerMillion, legacy.effectiveOutputUsdPerMillion(300_000), 0.0);
    }

    @Test
    public void exposesModelSpecificRequestCompatibility() {
        assertFalse(WorkshopAiPricing.forModel("gpt-5.5").explicitCacheBreakpoints);
        assertTrue(WorkshopAiPricing.forModel("gpt-5.6-sol").explicitCacheBreakpoints);
        assertEquals("high", WorkshopAiPricing.forModel("gpt-5-pro").reasoningEffort);
        assertFalse(WorkshopAiPricing.forModel("gpt-5.2-pro").structuredOutputs);
        assertFalse(WorkshopAiPricing.forModel("gpt-5.4-pro").structuredOutputs);
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
