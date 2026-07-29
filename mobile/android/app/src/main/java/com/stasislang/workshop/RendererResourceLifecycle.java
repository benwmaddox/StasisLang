package com.stasislang.workshop;

final class RendererResourceLifecycle {
    enum State { UNAVAILABLE, READY, PAUSED, RESTORE_PENDING, RESTORING, RESTORE_FAILED }

    private int surfaceGeneration;
    private int rendererGeneration;
    private int restoreAttempts;
    private int restoreFailures;
    private State state = State.UNAVAILABLE;
    private State stateBeforePause = State.UNAVAILABLE;
    private String reason = "none";

    void onRendererCreated() {
        surfaceGeneration = nextGeneration(surfaceGeneration);
        rendererGeneration = nextGeneration(rendererGeneration);
        state = State.RESTORE_PENDING;
        reason = "renderer_created";
    }

    void onSurfaceChanged() {
        if (state == State.UNAVAILABLE) return;
        surfaceGeneration = nextGeneration(surfaceGeneration);
        reason = "surface_changed";
    }

    void onPause() {
        if (state == State.UNAVAILABLE) return;
        stateBeforePause = state;
        state = State.PAUSED;
        reason = "background";
    }

    void onResume() {
        if (state != State.PAUSED) return;
        state = stateBeforePause == State.READY ? State.READY : State.RESTORE_PENDING;
        reason = "foreground";
    }

    boolean beginRestore() {
        if (state != State.RESTORE_PENDING && state != State.RESTORE_FAILED) return false;
        restoreAttempts = nextCounter(restoreAttempts);
        state = State.RESTORING;
        return true;
    }

    void finishRestore(boolean succeeded) {
        if (state != State.RESTORING) return;
        if (succeeded) {
            state = State.READY;
        } else {
            restoreFailures = nextCounter(restoreFailures);
            state = State.RESTORE_FAILED;
        }
    }

    void resourceFailed() {
        if (state == State.UNAVAILABLE || state == State.PAUSED) return;
        restoreFailures = nextCounter(restoreFailures);
        state = State.RESTORE_FAILED;
    }

    boolean canPresent() { return state == State.READY; }
    int surfaceGeneration() { return surfaceGeneration; }
    int rendererGeneration() { return rendererGeneration; }
    int restoreAttempts() { return restoreAttempts; }
    int restoreFailures() { return restoreFailures; }
    State state() { return state; }
    String reason() { return reason; }

    private static int nextGeneration(int value) {
        value += 1;
        return value == 0 ? 1 : value;
    }

    private static int nextCounter(int value) {
        return value == Integer.MAX_VALUE ? Integer.MAX_VALUE : value + 1;
    }
}
