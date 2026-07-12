package com.stasislang.workshop;

final class WorkshopCodexLimits {
    private WorkshopCodexLimits() {}

    static String formatWindow(long windowMinutes, double usedPercent) {
        long remaining = Math.round(Math.max(0.0, Math.min(100.0, 100.0 - usedPercent)));
        return windowLabel(windowMinutes) + " " + remaining + "% left";
    }

    static String windowLabel(long windowMinutes) {
        if (windowMinutes == 300L) return "5h";
        if (windowMinutes >= 7L * 24L * 60L) return "weekly";
        if (windowMinutes > 0L && windowMinutes % 60L == 0L) return (windowMinutes / 60L) + "h";
        return windowMinutes > 0L ? windowMinutes + "m" : "window";
    }
}
