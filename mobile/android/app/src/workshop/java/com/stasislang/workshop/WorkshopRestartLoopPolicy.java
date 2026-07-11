package com.stasislang.workshop;

final class WorkshopRestartLoopPolicy {
    static final long LOOP_WINDOW_MS = 10L * 60L * 1000L;
    static final int LOOP_CRASH_THRESHOLD = 3;

    static final class Result {
        final long lastSeenCrashTimestampMs;
        final int consecutiveEarlyCrashes;
        final boolean restartLoopDetected;

        Result(long timestamp, int count) {
            lastSeenCrashTimestampMs = timestamp;
            consecutiveEarlyCrashes = count;
            restartLoopDetected = count >= LOOP_CRASH_THRESHOLD;
        }
    }

    private WorkshopRestartLoopPolicy() {}

    static Result noteCrash(long priorTimestampMs, int priorCount,
            long crashTimestampMs, long nowMs) {
        if (crashTimestampMs <= 0L || crashTimestampMs <= priorTimestampMs
                || crashTimestampMs > nowMs) {
            return new Result(priorTimestampMs, Math.max(0, priorCount));
        }
        boolean continuesLoop = priorTimestampMs > 0L
                && crashTimestampMs - priorTimestampMs <= LOOP_WINDOW_MS;
        return new Result(crashTimestampMs, continuesLoop ? priorCount + 1 : 1);
    }
}
