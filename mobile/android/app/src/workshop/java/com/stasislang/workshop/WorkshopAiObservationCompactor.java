package com.stasislang.workshop;

import org.json.JSONObject;

final class WorkshopAiObservationCompactor {
    static final class SourceMetadata {
        final int characters;

        SourceMetadata(int characters) {
            this.characters = characters;
        }
    }

    private WorkshopAiObservationCompactor() {}

    static JSONObject compactSuccessfulWrite(JSONObject observation) throws Exception {
        JSONObject retained = new JSONObject(observation.toString());
        JSONObject args = retained.optJSONObject("args");
        if (args == null) return retained;
        compact(args, "new_source");
        compact(args, "source");
        return retained;
    }

    private static void compact(JSONObject args, String key) throws Exception {
        if (!args.has(key)) return;
        String source = args.optString(key, "");
        SourceMetadata metadata = describe(source);
        args.remove(key);
        args.put(key + "_chars", metadata.characters);
    }

    static SourceMetadata describe(String source) throws Exception {
        String value = source == null ? "" : source;
        return new SourceMetadata(value.length());
    }
}
