package com.stasislang.workshop;

import org.json.JSONObject;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

final class WorkshopAiObservationMemory {
    private static final int MAX_ENTRIES = 16;
    private static final int MAX_SNAPSHOT_CHARS = 96 * 1024;

    private final LinkedHashMap<String, String> observations = new LinkedHashMap<>();

    void remember(String key, String observationJson) {
        if (key == null || key.isEmpty() || observationJson == null || observationJson.isEmpty()) return;
        observations.remove(key);
        observations.put(key, observationJson);
        while (observations.size() > MAX_ENTRIES) {
            String oldest = observations.keySet().iterator().next();
            observations.remove(oldest);
        }
    }

    List<String> snapshotNewestFirst() {
        ArrayList<Map.Entry<String, String>> entries = new ArrayList<>(observations.entrySet());
        ArrayList<String> retained = new ArrayList<>();
        int chars = 0;
        for (int index = entries.size() - 1; index >= 0; index -= 1) {
            String value = entries.get(index).getValue();
            if (value.length() > MAX_SNAPSHOT_CHARS) continue;
            if (chars + value.length() > MAX_SNAPSHOT_CHARS) break;
            retained.add(value);
            chars += value.length();
        }
        return retained;
    }

    int size() {
        return observations.size();
    }

    void restoreNewestFirst(List<String> snapshot) {
        observations.clear();
        if (snapshot == null) return;
        for (int index = snapshot.size() - 1; index >= 0; index -= 1) {
            String value = snapshot.get(index);
            try {
                JSONObject observation = new JSONObject(value);
                JSONObject args = observation.optJSONObject("args");
                remember(observation.optString("tool", "observation") + "|"
                        + (args == null ? "{}" : args.toString()), value);
            } catch (Exception error) {
                remember("checkpoint-" + index, value);
            }
        }
    }
}
