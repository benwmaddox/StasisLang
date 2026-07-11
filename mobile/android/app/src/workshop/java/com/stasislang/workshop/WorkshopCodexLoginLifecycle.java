package com.stasislang.workshop;

final class WorkshopCodexLoginLifecycle {
    private boolean resumed;
    private boolean awaitingUser;
    private boolean statusRequestInFlight;
    private boolean pollScheduled;
    private boolean completionHandled;

    void onResume() {
        resumed = true;
    }

    void onPause() {
        resumed = false;
        pollScheduled = false;
    }

    void onLoginStarted() {
        awaitingUser = true;
        completionHandled = false;
    }

    boolean beginStatusRequest() {
        if (statusRequestInFlight) return false;
        statusRequestInFlight = true;
        pollScheduled = false;
        return true;
    }

    void finishStatusRequest() {
        statusRequestInFlight = false;
    }

    void onAwaitingUser() {
        awaitingUser = true;
    }

    boolean shouldPresentDialog(boolean dialogShowing, boolean hasValidCode) {
        return resumed && awaitingUser && !dialogShowing && hasValidCode;
    }

    boolean schedulePoll() {
        if (!resumed || !awaitingUser || statusRequestInFlight || pollScheduled) return false;
        pollScheduled = true;
        return true;
    }

    boolean onSignedIn() {
        awaitingUser = false;
        pollScheduled = false;
        if (completionHandled) return false;
        completionHandled = true;
        return true;
    }

    void onTerminalFailure() {
        awaitingUser = false;
        pollScheduled = false;
    }
}
