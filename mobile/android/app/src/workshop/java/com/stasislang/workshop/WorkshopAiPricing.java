package com.stasislang.workshop;

final class WorkshopAiPricing {
    static final long LONG_CONTEXT_INPUT_TOKENS = 272_000L;

    static final class Rates {
        final double inputUsdPerMillion;
        final double cachedInputUsdPerMillion;
        final double cacheWriteUsdPerMillion;
        final double outputUsdPerMillion;
        final boolean longContextPricing;
        final boolean explicitCacheBreakpoints;
        final boolean structuredOutputs;
        final String reasoningEffort;

        Rates(double input, double cachedInput, double cacheWrite, double output,
                boolean longContext, boolean explicitCache, boolean structured, String effort) {
            inputUsdPerMillion = input;
            cachedInputUsdPerMillion = cachedInput;
            cacheWriteUsdPerMillion = cacheWrite;
            outputUsdPerMillion = output;
            longContextPricing = longContext;
            explicitCacheBreakpoints = explicitCache;
            structuredOutputs = structured;
            reasoningEffort = effort;
        }

        double estimate(long inputTokens, long cachedInputTokens, long cacheWriteInputTokens,
                long outputTokens) {
            long uncached = Math.max(0L, inputTokens - cachedInputTokens - cacheWriteInputTokens);
            double inputMultiplier = usesLongContextPricing(inputTokens) ? 2.0 : 1.0;
            double outputMultiplier = usesLongContextPricing(inputTokens) ? 1.5 : 1.0;
            double inputCost = uncached * inputUsdPerMillion * inputMultiplier;
            double cachedCost = cachedInputTokens * cachedInputUsdPerMillion * inputMultiplier;
            double cacheWriteCost = cacheWriteInputTokens * cacheWriteUsdPerMillion * inputMultiplier;
            double outputCost = outputTokens * outputUsdPerMillion * outputMultiplier;
            return (inputCost + cachedCost + cacheWriteCost + outputCost) / 1_000_000.0;
        }

        double conservativeInputUsdPerMillion() {
            return Math.max(inputUsdPerMillion, cacheWriteUsdPerMillion);
        }

        double conservativeInputCostUsd(long inputTokens) {
            double multiplier = usesLongContextPricing(inputTokens) ? 2.0 : 1.0;
            return inputTokens * conservativeInputUsdPerMillion() * multiplier / 1_000_000.0;
        }

        double effectiveOutputUsdPerMillion(long inputTokens) {
            return outputUsdPerMillion * (usesLongContextPricing(inputTokens) ? 1.5 : 1.0);
        }

        private boolean usesLongContextPricing(long inputTokens) {
            return longContextPricing && inputTokens > LONG_CONTEXT_INPUT_TOKENS;
        }
    }

    private static final Rates GPT_5 = standard(1.25, 0.125, 10.00);
    private static final Rates GPT_5_MINI = standard(0.25, 0.025, 2.00);
    private static final Rates GPT_5_NANO = standard(0.05, 0.005, 0.40);
    private static final Rates GPT_5_PRO = pro(15.00, 120.00, false, true, "high");
    private static final Rates GPT_5_2 = standard(1.75, 0.175, 14.00);
    private static final Rates GPT_5_2_PRO = pro(21.00, 168.00, false, false, "medium");
    private static final Rates GPT_5_4 = longContext(2.50, 0.25, 15.00);
    private static final Rates GPT_5_4_MINI = standard(0.75, 0.075, 4.50);
    private static final Rates GPT_5_4_NANO = standard(0.20, 0.02, 1.25);
    private static final Rates GPT_5_4_PRO = pro(30.00, 180.00, true, false, "medium");
    private static final Rates GPT_5_5 = longContext(5.00, 0.50, 30.00);
    private static final Rates GPT_5_5_PRO = pro(30.00, 180.00, false, true, "medium");
    private static final Rates SOL = gpt56(5.00, 0.50, 6.25, 30.00);
    private static final Rates TERRA = gpt56(2.50, 0.25, 3.125, 15.00);
    private static final Rates LUNA = gpt56(1.00, 0.10, 1.25, 6.00);

    private WorkshopAiPricing() {}

    static Rates forModel(String model) {
        if ("gpt-5".equals(model) || "gpt-5-2025-08-07".equals(model)
                || "gpt-5.1".equals(model) || "gpt-5.1-2025-11-13".equals(model)) return GPT_5;
        if ("gpt-5-mini".equals(model) || "gpt-5-mini-2025-08-07".equals(model)) return GPT_5_MINI;
        if ("gpt-5-nano".equals(model) || "gpt-5-nano-2025-08-07".equals(model)) return GPT_5_NANO;
        if ("gpt-5-pro".equals(model) || "gpt-5-pro-2025-10-06".equals(model)) return GPT_5_PRO;
        if ("gpt-5.2".equals(model) || "gpt-5.2-2025-12-11".equals(model)
                || "gpt-5.3-codex".equals(model)) return GPT_5_2;
        if ("gpt-5.2-pro".equals(model) || "gpt-5.2-pro-2025-12-11".equals(model)) return GPT_5_2_PRO;
        if ("gpt-5.4".equals(model) || "gpt-5.4-2026-03-05".equals(model)) return GPT_5_4;
        if ("gpt-5.4-mini".equals(model) || "gpt-5.4-mini-2026-03-17".equals(model)) return GPT_5_4_MINI;
        if ("gpt-5.4-nano".equals(model) || "gpt-5.4-nano-2026-03-17".equals(model)) return GPT_5_4_NANO;
        if ("gpt-5.4-pro".equals(model) || "gpt-5.4-pro-2026-03-05".equals(model)) return GPT_5_4_PRO;
        if ("gpt-5.5".equals(model) || "gpt-5.5-2026-04-23".equals(model)) return GPT_5_5;
        if ("gpt-5.5-pro".equals(model) || "gpt-5.5-pro-2026-04-23".equals(model)) return GPT_5_5_PRO;
        if ("gpt-5.6".equals(model) || "gpt-5.6-sol".equals(model)) return SOL;
        if ("gpt-5.6-terra".equals(model)) return TERRA;
        if ("gpt-5.6-luna".equals(model)) return LUNA;
        return null;
    }

    static boolean isKnown(String model) {
        return forModel(model) != null;
    }

    private static Rates standard(double input, double cached, double output) {
        return new Rates(input, cached, input, output, false, false, true, "medium");
    }

    private static Rates longContext(double input, double cached, double output) {
        return new Rates(input, cached, input, output, true, false, true, "medium");
    }

    private static Rates pro(double input, double output, boolean longContext,
            boolean structured, String effort) {
        return new Rates(input, input, input, output, longContext, false, structured, effort);
    }

    private static Rates gpt56(double input, double cached, double cacheWrite, double output) {
        return new Rates(input, cached, cacheWrite, output, true, true, true, "medium");
    }
}
