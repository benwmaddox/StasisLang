package com.stasislang.workshop;

import java.util.Locale;

final class WorkshopAiFastPathPolicy {
    static final int MAX_SOURCE_SYMBOLS = 16;
    static final int MAX_SOURCE_CHARS = 16 * 1024;

    private static final String[] TUNING_TERMS = {
            "size", "width", "wide", "wider", "height", "tall", "taller",
            "bigger", "larger", "smaller", "shorter", "double", "half",
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

    static int relevanceScore(String prompt, String symbolName, String kind) {
        if (prompt == null || symbolName == null) return 0;
        String normalizedPrompt = prompt.toLowerCase(Locale.ROOT).replaceAll("[^a-z0-9]+", " ");
        String normalizedName = symbolName.toLowerCase(Locale.ROOT).replace('_', ' ');
        int score = 0;
        for (String raw : normalizedPrompt.split(" +")) {
            String token = raw.endsWith("s") && raw.length() > 4
                    ? raw.substring(0, raw.length() - 1) : raw;
            if (token.length() < 4 || isTuningWord(token)) continue;
            if (normalizedName.contains(token)) score += 4;
        }
        if ("render".equals(normalizedName) && containsVisualTuningTerm(normalizedPrompt)) score += 3;
        if ("test".equals(kind) && score > 0) score += 2;
        return score;
    }

    private static boolean isTuningWord(String token) {
        for (String term : TUNING_TERMS) {
            if (term.equals(token)) return true;
        }
        return "make".equals(token) || "both".equals(token) || "pixel".equals(token)
                || "instead".equals(token) || "should".equals(token);
    }

    private static boolean containsVisualTuningTerm(String prompt) {
        return prompt.contains("size") || prompt.contains("width") || prompt.contains("wide")
                || prompt.contains("height") || prompt.contains("tall")
                || prompt.contains("color") || prompt.contains("position")
                || prompt.contains("bigger") || prompt.contains("larger")
                || prompt.contains("smaller") || prompt.contains("shorter");
    }
}
