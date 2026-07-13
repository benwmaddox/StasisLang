package com.stasislang.workshop;

final class WorkshopAiOverlayPolicy {
    private WorkshopAiOverlayPolicy() { }

    static boolean shouldShow(boolean panelOpen, boolean runActive, int queueCount) {
        return !panelOpen && (runActive || queueCount > 0);
    }

    static String queueLabel(boolean runActive, int queueCount) {
        if (runActive) return queueCount > 1 ? "AI working +" + (queueCount - 1) : "AI working";
        return queueCount == 1 ? "AI queued" : "AI queue " + queueCount;
    }

    static String contentDescription(boolean runActive, int queueCount, int step,
                                     int maxSteps, int actions, String phase, String elapsed) {
        return queueLabel(runActive, queueCount) + ", step " + step + " of " + maxSteps
                + ", " + actions + " actions, " + phase + ", time " + elapsed
                + "; tap to open Workshop";
    }
}
