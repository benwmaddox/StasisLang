package com.stasislang.workshop;

final class WorkshopOnboardingPolicy {
    static final int CURRENT_VERSION = 2;

    enum Step {
        WELCOME("Read the account-free workflow", "Choose Resume Tutorial to begin. AI and GitHub are optional."),
        PROJECT_OPENED("Choose or open a template", "Open the bundled Exploration Garden or create a project from a selected template."),
        PROJECT_RAN("Run the project", "Watch the selected project compile and run in the game preview."),
        CHANGE_APPLIED("Make and apply a manual change", "Open Manual Symbols and Source, edit one function, then choose Apply."),
        TESTS_PASSED("Test the change", "Choose Run Tests and continue after every runnable test passes."),
        CHANGES_REVIEWED("Review the saved change", "Choose Changes or Raw Diffs to inspect what differs from the project baseline."),
        CHANGE_REVERTED("Revert the saved change", "Select the changed baseline symbol and choose Revert Saved."),
        COMPLETE("Manual tutorial complete", "The full account-free workflow is complete. Help and Onboarding can restart it anytime.");

        final String label;
        final String instruction;

        Step(String label, String instruction) {
            this.label = label;
            this.instruction = instruction;
        }
    }

    static final class Progress {
        final int version;
        final int completedSteps;
        final boolean deferred;
        final String projectId;
        final String changeId;
        final String changeHash;

        Progress(int version, int completedSteps, boolean deferred, String projectId,
                String changeId, String changeHash) {
            this.version = version;
            this.completedSteps = Math.max(0, Math.min(completedSteps, requiredStepCount()));
            this.deferred = deferred;
            this.projectId = clean(projectId);
            this.changeId = clean(changeId);
            this.changeHash = clean(changeHash);
        }

        Step nextStep() {
            return completedSteps >= requiredStepCount() ? Step.COMPLETE : Step.values()[completedSteps];
        }

        boolean isComplete() {
            return nextStep() == Step.COMPLETE;
        }
    }

    private WorkshopOnboardingPolicy() {}

    static Progress restore(int storedVersion, int completedSteps, boolean deferred,
            boolean legacyGuideSeen, String projectId, String changeId, String changeHash) {
        if (storedVersion == 0 && legacyGuideSeen) {
            return new Progress(CURRENT_VERSION, 0, true, "", "", "");
        }
        if (storedVersion != CURRENT_VERSION) return fresh();
        int safeSteps = Math.max(0, Math.min(completedSteps, requiredStepCount()));
        if (safeSteps > Step.PROJECT_OPENED.ordinal() && clean(projectId).isEmpty()) {
            safeSteps = Step.PROJECT_OPENED.ordinal();
        }
        if (safeSteps > Step.CHANGE_APPLIED.ordinal()
                && (clean(changeId).isEmpty() || clean(changeHash).isEmpty())) {
            safeSteps = Step.CHANGE_APPLIED.ordinal();
        }
        return new Progress(CURRENT_VERSION, safeSteps, deferred, projectId, changeId, changeHash);
    }

    static Progress fresh() {
        return new Progress(CURRENT_VERSION, 0, false, "", "", "");
    }

    static Progress restart() {
        return fresh();
    }

    static Progress defer(Progress progress) {
        return copy(progress, progress.completedSteps, true);
    }

    static Progress resume(Progress progress) {
        return copy(progress, progress.completedSteps, false);
    }

    static Progress recordWelcome(Progress progress) {
        return progress.nextStep() == Step.WELCOME ? advance(progress) : progress;
    }

    static Progress recordProjectOpened(Progress progress, String projectId) {
        String cleanProjectId = clean(projectId);
        if (progress.nextStep() != Step.PROJECT_OPENED || cleanProjectId.isEmpty()) return progress;
        return new Progress(CURRENT_VERSION, progress.completedSteps + 1, false,
                cleanProjectId, "", "");
    }

    static Progress recordProjectStep(Progress progress, Step event, String projectId) {
        if (event != Step.PROJECT_RAN || event != progress.nextStep()
                || !same(progress.projectId, projectId)) {
            return progress;
        }
        return advance(progress);
    }

    static Progress recordChangeApplied(Progress progress, String projectId,
            String changeId, String changeHash) {
        String cleanChangeId = clean(changeId);
        String cleanChangeHash = clean(changeHash);
        if (progress.nextStep() != Step.CHANGE_APPLIED || !same(progress.projectId, projectId)
                || cleanChangeId.isEmpty() || cleanChangeHash.isEmpty()) {
            return progress;
        }
        return new Progress(CURRENT_VERSION, progress.completedSteps + 1, false,
                progress.projectId, cleanChangeId, cleanChangeHash);
    }

    static Progress recordChangeStep(Progress progress, Step event, String projectId,
            String changeId, String changeHash) {
        if ((event != Step.TESTS_PASSED && event != Step.CHANGES_REVIEWED
                && event != Step.CHANGE_REVERTED)
                || event != progress.nextStep() || !same(progress.projectId, projectId)
                || !same(progress.changeId, changeId) || !same(progress.changeHash, changeHash)) {
            return progress;
        }
        return advance(progress);
    }

    static String checklist(Progress progress) {
        StringBuilder text = new StringBuilder();
        Step[] steps = Step.values();
        for (int index = 0; index < requiredStepCount(); index += 1) {
            if (text.length() > 0) text.append('\n');
            text.append(index < progress.completedSteps ? "[x] " : "[ ] ")
                    .append(steps[index].label);
        }
        text.append("\n\nNext: ").append(progress.nextStep().instruction);
        text.append("\n\nOptional accounts: ChatGPT/OpenAI can propose edits and GitHub can back up or review them. ")
                .append("Neither is required for projects, manual editing, tests, Changes, or Revert. ")
                .append("Media and voice permissions are requested only when you start those features.");
        return text.toString();
    }

    private static Progress advance(Progress progress) {
        if (progress.isComplete()) return progress;
        return copy(progress, progress.completedSteps + 1, false);
    }

    private static Progress copy(Progress progress, int completedSteps, boolean deferred) {
        return new Progress(CURRENT_VERSION, completedSteps, deferred,
                progress.projectId, progress.changeId, progress.changeHash);
    }

    private static boolean same(String expected, String actual) {
        return !clean(expected).isEmpty() && clean(expected).equals(clean(actual));
    }

    private static String clean(String value) {
        return value == null ? "" : value.trim();
    }

    private static int requiredStepCount() {
        return Step.COMPLETE.ordinal();
    }
}
