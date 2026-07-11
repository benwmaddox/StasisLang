package com.stasislang.workshop;

final class WorkshopAiPricing {
    static final long LONG_CONTEXT_INPUT_TOKENS = 272_000L;

    static final class Rates {
        final double inputUsdPerMillion;
        final double cachedInputUsdPerMillion;
        final double cacheWriteUsdPerMillion;
        final double outputUsdPerMillion;

        Rates(double input, double cachedInput, double cacheWrite, double output) {
            inputUsdPerMillion = input;
            cachedInputUsdPerMillion = cachedInput;
            cacheWriteUsdPerMillion = cacheWrite;
            outputUsdPerMillion = output;
        }

        double estimate(long inputTokens, long cachedInputTokens, long cacheWriteInputTokens,
                long outputTokens) {
            long uncached = Math.max(0L, inputTokens - cachedInputTokens - cacheWriteInputTokens);
            double inputMultiplier = inputTokens > LONG_CONTEXT_INPUT_TOKENS ? 2.0 : 1.0;
            double outputMultiplier = inputTokens > LONG_CONTEXT_INPUT_TOKENS ? 1.5 : 1.0;
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
            double multiplier = inputTokens > LONG_CONTEXT_INPUT_TOKENS ? 2.0 : 1.0;
            return inputTokens * conservativeInputUsdPerMillion() * multiplier / 1_000_000.0;
        }

        double effectiveOutputUsdPerMillion(long inputTokens) {
            return outputUsdPerMillion * (inputTokens > LONG_CONTEXT_INPUT_TOKENS ? 1.5 : 1.0);
        }
    }

    private static final Rates SOL = new Rates(5.00, 0.50, 6.25, 30.00);
    private static final Rates TERRA = new Rates(2.50, 0.25, 3.125, 15.00);
    private static final Rates LUNA = new Rates(1.00, 0.10, 1.25, 6.00);

    private WorkshopAiPricing() {}

    static Rates forModel(String model) {
        if ("gpt-5.6".equals(model) || "gpt-5.6-sol".equals(model)) return SOL;
        if ("gpt-5.6-terra".equals(model)) return TERRA;
        if ("gpt-5.6-luna".equals(model)) return LUNA;
        return null;
    }

    static boolean isKnown(String model) {
        return forModel(model) != null;
    }
}
