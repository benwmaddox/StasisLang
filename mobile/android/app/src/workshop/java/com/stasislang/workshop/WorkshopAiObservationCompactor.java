package com.stasislang.workshop;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;

import org.json.JSONObject;

final class WorkshopAiObservationCompactor {
    static final class SourceMetadata {
        final int characters;
        final String sha256;

        SourceMetadata(int characters, String sha256) {
            this.characters = characters;
            this.sha256 = sha256;
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
        args.put(key + "_sha256", metadata.sha256);
    }

    static SourceMetadata describe(String source) throws Exception {
        String value = source == null ? "" : source;
        return new SourceMetadata(value.length(), sha256(value));
    }

    private static String sha256(String source) throws Exception {
        byte[] digest = MessageDigest.getInstance("SHA-256")
                .digest(source.getBytes(StandardCharsets.UTF_8));
        StringBuilder hex = new StringBuilder(digest.length * 2);
        for (byte value : digest) hex.append(String.format("%02x", value & 0xff));
        return hex.toString();
    }
}
