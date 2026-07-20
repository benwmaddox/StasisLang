package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.content.SharedPreferences;

import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

import org.junit.Test;

public final class WorkshopOnboardingStoreTest {
    @Test
    public void uiActionSequenceSurvivesPreferenceBackedProcessRecreation() {
        MemoryPreferences preferences = new MemoryPreferences();
        WorkshopOnboardingPolicy.Progress progress = WorkshopOnboardingStore.load(preferences);

        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.recordWelcome(progress));
        progress = saveAndReload(preferences,
                WorkshopOnboardingPolicy.recordProjectOpened(progress, "project-a"));
        assertSame(progress, WorkshopOnboardingPolicy.recordProjectStep(
                progress, WorkshopOnboardingPolicy.Step.PROJECT_RAN, "project-b"));
        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.recordProjectStep(
                progress, WorkshopOnboardingPolicy.Step.PROJECT_RAN, "project-a"));
        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.recordChangeApplied(
                progress, "project-a", "function|src/main.stasis|Root|tick", "source-hash"));

        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.defer(progress));
        assertTrue(progress.deferred);
        assertEquals(WorkshopOnboardingPolicy.Step.TESTS_PASSED, progress.nextStep());
        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.resume(progress));
        assertFalse(progress.deferred);

        assertSame(progress, WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.TESTS_PASSED,
                "project-a", "function|src/main.stasis|Root|tick", "wrong-hash"));
        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.TESTS_PASSED,
                "project-a", "function|src/main.stasis|Root|tick", "source-hash"));
        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED,
                "project-a", "function|src/main.stasis|Root|tick", "source-hash"));
        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.CHANGE_REVERTED,
                "project-a", "function|src/main.stasis|Root|tick", "source-hash"));

        assertTrue(progress.isComplete());
        assertEquals("project-a", progress.projectId);
        assertEquals("source-hash", progress.changeHash);
        assertFalse(preferences.contains(WorkshopOnboardingStore.LEGACY_SEEN));

        progress = saveAndReload(preferences, WorkshopOnboardingPolicy.restart());
        assertEquals(WorkshopOnboardingPolicy.Step.WELCOME, progress.nextStep());
        assertEquals("", progress.projectId);
        assertEquals("", progress.changeId);
        assertEquals("", progress.changeHash);
    }

    private static WorkshopOnboardingPolicy.Progress saveAndReload(
            MemoryPreferences preferences, WorkshopOnboardingPolicy.Progress progress) {
        assertTrue(WorkshopOnboardingStore.save(preferences, progress));
        return WorkshopOnboardingStore.load(preferences);
    }

    private static final class MemoryPreferences implements SharedPreferences {
        private final Map<String, Object> values = new HashMap<>();

        @Override public Map<String, ?> getAll() { return new HashMap<>(values); }
        @Override public String getString(String key, String fallback) {
            Object value = values.get(key);
            return value instanceof String ? (String)value : fallback;
        }
        @SuppressWarnings("unchecked")
        @Override public Set<String> getStringSet(String key, Set<String> fallback) {
            Object value = values.get(key);
            return value instanceof Set ? new HashSet<>((Set<String>)value) : fallback;
        }
        @Override public int getInt(String key, int fallback) {
            Object value = values.get(key);
            return value instanceof Integer ? (Integer)value : fallback;
        }
        @Override public long getLong(String key, long fallback) {
            Object value = values.get(key);
            return value instanceof Long ? (Long)value : fallback;
        }
        @Override public float getFloat(String key, float fallback) {
            Object value = values.get(key);
            return value instanceof Float ? (Float)value : fallback;
        }
        @Override public boolean getBoolean(String key, boolean fallback) {
            Object value = values.get(key);
            return value instanceof Boolean ? (Boolean)value : fallback;
        }
        @Override public boolean contains(String key) { return values.containsKey(key); }
        @Override public Editor edit() { return new MemoryEditor(); }
        @Override public void registerOnSharedPreferenceChangeListener(
                OnSharedPreferenceChangeListener listener) {}
        @Override public void unregisterOnSharedPreferenceChangeListener(
                OnSharedPreferenceChangeListener listener) {}

        private final class MemoryEditor implements Editor {
            private final Map<String, Object> pending = new HashMap<>();
            private final Set<String> removed = new HashSet<>();
            private boolean clear;

            @Override public Editor putString(String key, String value) {
                pending.put(key, value);
                return this;
            }
            @Override public Editor putStringSet(String key, Set<String> value) {
                pending.put(key, value == null ? null : new HashSet<>(value));
                return this;
            }
            @Override public Editor putInt(String key, int value) {
                pending.put(key, value);
                return this;
            }
            @Override public Editor putLong(String key, long value) {
                pending.put(key, value);
                return this;
            }
            @Override public Editor putFloat(String key, float value) {
                pending.put(key, value);
                return this;
            }
            @Override public Editor putBoolean(String key, boolean value) {
                pending.put(key, value);
                return this;
            }
            @Override public Editor remove(String key) {
                removed.add(key);
                return this;
            }
            @Override public Editor clear() {
                clear = true;
                return this;
            }
            @Override public boolean commit() {
                if (clear) values.clear();
                for (String key : removed) values.remove(key);
                for (Map.Entry<String, Object> entry : pending.entrySet()) {
                    if (entry.getValue() == null) values.remove(entry.getKey());
                    else values.put(entry.getKey(), entry.getValue());
                }
                return true;
            }
            @Override public void apply() { commit(); }
        }
    }
}
