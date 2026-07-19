package com.stasislang.workshop;

final class WorkshopExplorationLessonPolicy {
    static final int TAPPED_DESTINATION = 1;
    static final int COLLECTED_KEEPSAKE = 1 << 1;
    static final int OPENED_EDITOR = 1 << 2;
    static final int APPLIED_EDIT = 1 << 3;
    static final int PASSED_TESTS = 1 << 4;
    static final int COMPLETE = TAPPED_DESTINATION | COLLECTED_KEEPSAKE | OPENED_EDITOR
            | APPLIED_EDIT | PASSED_TESTS;

    private WorkshopExplorationLessonPolicy() {}

    static int observeGame(int progress, int acceptedTapCount, int collectedCount) {
        int updated = progress;
        if (acceptedTapCount > 0) updated |= TAPPED_DESTINATION;
        if (collectedCount > 0) updated |= COLLECTED_KEEPSAKE;
        return updated;
    }

    static int record(int progress, int event) {
        return progress | (event & COMPLETE);
    }

    static String prompt(int progress) {
        if ((progress & TAPPED_DESTINATION) == 0) return "tap a destination";
        if ((progress & COLLECTED_KEEPSAKE) == 0) return "collect a keepsake";
        if ((progress & OPENED_EDITOR) == 0) return "open Workshop menu";
        if ((progress & APPLIED_EDIT) == 0) return "edit MOVE_SPEED, then Apply";
        if ((progress & PASSED_TESTS) == 0) return "Run Tests";
        return "tutorial complete";
    }

    static boolean isComplete(int progress) {
        return (progress & COMPLETE) == COMPLETE;
    }
}
