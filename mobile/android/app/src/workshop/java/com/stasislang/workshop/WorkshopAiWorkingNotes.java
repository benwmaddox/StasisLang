package com.stasislang.workshop;

final class WorkshopAiWorkingNotes {
    static final int MAX_CHARS = 2_000;
    private static final int MAX_DISPLAY_CHARS = 320;

    private WorkshopAiWorkingNotes() {}

    static boolean isValid(String notes) {
        return notes != null && !notes.trim().isEmpty() && notes.length() <= MAX_CHARS;
    }

    static String normalize(String notes) {
        return notes == null ? "" : notes.trim();
    }

    static String compactForDisplay(String notes) {
        String compact = normalize(notes).replaceAll("\\s+", " ");
        if (compact.length() <= MAX_DISPLAY_CHARS) return compact;
        return compact.substring(0, MAX_DISPLAY_CHARS - 3) + "...";
    }
}
