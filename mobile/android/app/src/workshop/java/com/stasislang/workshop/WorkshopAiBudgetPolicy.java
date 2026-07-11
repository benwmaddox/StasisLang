package com.stasislang.workshop;

final class WorkshopAiBudgetPolicy {
    private WorkshopAiBudgetPolicy() {}

    static boolean canStart(double monthlyLimitUsd, double monthlySpendUsd) {
        return monthlyLimitUsd > 0.0 && remainingUsd(monthlyLimitUsd, monthlySpendUsd) > 0.0;
    }

    static double remainingUsd(double monthlyLimitUsd, double monthlySpendUsd) {
        return Math.max(0.0, monthlyLimitUsd - Math.max(0.0, monthlySpendUsd));
    }
}
