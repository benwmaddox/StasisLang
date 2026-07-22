package com.stasislang.workshop;

final class WorkshopFrameBudget {
    private static final double FRAME_BUDGET_MILLIS = 1000.0 / 60.0;

    private WorkshopFrameBudget() {}

    static int percent(double millis) {
        return Math.max(0, (int)((millis * 100.0 / FRAME_BUDGET_MILLIS) + 0.5));
    }
}
