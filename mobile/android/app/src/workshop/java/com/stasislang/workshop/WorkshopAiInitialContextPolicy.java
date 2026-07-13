package com.stasislang.workshop;

final class WorkshopAiInitialContextPolicy {
    static final int MAX_SYMBOLS = 256;
    static final int MAX_SYMBOL_INDEX_CHARS = 16 * 1024;

    private WorkshopAiInitialContextPolicy() {}

    static boolean canAppend(int currentChars, int candidateChars, int includedCount) {
        if (currentChars < 0 || candidateChars < 0 || includedCount < 0) return false;
        if (includedCount >= MAX_SYMBOLS) return false;
        int separatorChars = includedCount == 0 ? 0 : 1;
        return (long)currentChars + separatorChars + candidateChars <= MAX_SYMBOL_INDEX_CHARS;
    }
}
