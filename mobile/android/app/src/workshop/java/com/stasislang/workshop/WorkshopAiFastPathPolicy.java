package com.stasislang.workshop;

import java.util.Locale;

final class WorkshopAiFastPathPolicy {
    static final int MAX_SOURCE_SYMBOLS = 32;
    static final int MAX_SOURCE_CHARS = 24 * 1024;

    private static final String[] TUNING_TERMS = {
            "size", "width", "height", "bigger", "smaller", "double", "half",
            "increase", "decrease", "faster", "slower", "speed", "color", "position"
    };
    private static final String[] COMPLEX_TERMS = {
            "add a new", "create a new", "multiplayer", "network", "system", "level editor"
    };

    private WorkshopAiFastPathPolicy() {}

    static boolean isSimpleTuningPrompt(String prompt) {
        if (prompt == null) return false;
        String normalized = prompt.trim().toLowerCase(Locale.ROOT);
        if (normalized.isEmpty() || normalized.length() > 160 || normalized.indexOf('\n') >= 0) {
            return false;
        }
        for (String term : COMPLEX_TERMS) {
            if (normalized.contains(term)) return false;
        }
        for (String term : TUNING_TERMS) {
            if (normalized.contains(term)) return true;
        }
        return false;
    }

    static boolean canAppendSource(int currentChars, int candidateChars, int includedCount) {
        if (currentChars < 0 || candidateChars < 0 || includedCount < 0) return false;
        if (includedCount >= MAX_SOURCE_SYMBOLS) return false;
        int separatorChars = includedCount == 0 ? 0 : 1;
        return (long) currentChars + separatorChars + candidateChars <= MAX_SOURCE_CHARS;
    }

    static boolean canAutoFinalize(boolean wroteTest, int successfulWrites,
            boolean compileReady, boolean runnableTestsPassed) {
        return wroteTest && successfulWrites > 0 && compileReady && runnableTestsPassed;
    }
}
