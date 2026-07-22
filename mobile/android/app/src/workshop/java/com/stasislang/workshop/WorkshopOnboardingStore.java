package com.stasislang.workshop;

import android.content.SharedPreferences;

final class WorkshopOnboardingStore {
    static final String LEGACY_SEEN = "manual_tutorial_seen_v1";
    static final String VERSION = "manual_tutorial_version";
    static final String COMPLETED_STEPS = "manual_tutorial_completed_steps";
    static final String DEFERRED = "manual_tutorial_deferred";
    static final String PROJECT_ID = "manual_tutorial_project_id";
    static final String CHANGE_ID = "manual_tutorial_change_id";
    static final String CHANGE_HASH = "manual_tutorial_change_hash";

    private WorkshopOnboardingStore() {}

    static WorkshopOnboardingPolicy.Progress load(SharedPreferences preferences) {
        return WorkshopOnboardingPolicy.restore(
                preferences.getInt(VERSION, 0),
                preferences.getInt(COMPLETED_STEPS, 0),
                preferences.getBoolean(DEFERRED, false),
                preferences.getBoolean(LEGACY_SEEN, false),
                preferences.getString(PROJECT_ID, ""),
                preferences.getString(CHANGE_ID, ""),
                preferences.getString(CHANGE_HASH, ""));
    }

    static boolean save(SharedPreferences preferences, WorkshopOnboardingPolicy.Progress progress) {
        return preferences.edit()
                .putInt(VERSION, progress.version)
                .putInt(COMPLETED_STEPS, progress.completedSteps)
                .putBoolean(DEFERRED, progress.deferred)
                .putString(PROJECT_ID, progress.projectId)
                .putString(CHANGE_ID, progress.changeId)
                .putString(CHANGE_HASH, progress.changeHash)
                .remove(LEGACY_SEEN)
                .commit();
    }
}
