package com.stasislang.workshop;

final class WorkshopCodexServiceTier {
    private WorkshopCodexServiceTier() { }

    static String requestTier(boolean fastMode) {
        return fastMode ? "priority" : "";
    }
}
