package com.stasislang.workshop;

final class WorkshopBackgroundWorkPolicy {
    enum Decision {
        RUN,
        WAIT_FOR_NETWORK,
        DEFER_FOR_BATTERY
    }

    private WorkshopBackgroundWorkPolicy() {}

    static Decision decide(boolean userInitiated, boolean hasUsableNetwork,
            boolean batterySaverEnabled, boolean charging) {
        if (!hasUsableNetwork) return Decision.WAIT_FOR_NETWORK;
        if (!userInitiated && batterySaverEnabled && !charging) {
            return Decision.DEFER_FOR_BATTERY;
        }
        return Decision.RUN;
    }
}
