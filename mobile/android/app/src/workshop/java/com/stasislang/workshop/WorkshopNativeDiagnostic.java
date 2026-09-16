package com.stasislang.workshop;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.nio.charset.StandardCharsets;

/** The versioned diagnostic crossing the Rust/C/JNI boundary. */
public final class WorkshopNativeDiagnostic {
    public static final String SCHEMA = "stasis.native_diagnostic.v1";

    /** A byte range and symbol projected by the compiler-owned diagnostic. */
    public static final class Location {
        public final String file;
        public final int start;
        public final int end;
        public final int line;
        public final int column;
        public final int endLine;
        public final int endColumn;
        public final String symbol;
        public final String message;

        private Location(String file, int start, int end, int line, int column,
                int endLine, int endColumn, String symbol, String message) {
            this.file = emptyToNull(file);
            this.start = Math.max(0, start);
            this.end = Math.max(this.start, end);
            this.line = Math.max(0, line);
            this.column = Math.max(0, column);
            this.endLine = Math.max(this.line, endLine);
            this.endColumn = Math.max(0, endColumn);
            this.symbol = emptyToNull(symbol);
            this.message = emptyToNull(message);
        }

        static Location fromJson(JSONObject value) {
            if (value == null || value == JSONObject.NULL) return null;
            JSONObject range = value.optJSONObject("range");
            String file = firstNonEmpty(value.optString("file", null),
                    value.optString("path", null));
            int start = value.optInt("start", range == null ? 0 : range.optInt("start", 0));
            int end = value.optInt("end", range == null ? start : range.optInt("end", start));
            return new Location(file, start, end,
                    value.optInt("line", 0), value.optInt("column", 0),
                    value.optInt("end_line", value.optInt("endLine", 0)),
                    value.optInt("end_column", value.optInt("endColumn", 0)),
                    value.optString("symbol", null), value.optString("message", null));
        }

        JSONObject toJson() throws Exception {
            JSONObject value = new JSONObject();
            if (file != null) {
                value.put("file", file);
                value.put("path", file);
            }
            value.put("start", start).put("end", end);
            if (line > 0) value.put("line", line);
            if (column > 0) value.put("column", column);
            if (endLine > 0) value.put("end_line", endLine);
            if (endColumn > 0) value.put("end_column", endColumn);
            if (symbol != null) value.put("symbol", symbol);
            if (message != null) value.put("message", message);
            return value;
        }

        private static String firstNonEmpty(String first, String second) {
            return first != null && !first.isEmpty() ? first : second;
        }

        @Override public boolean equals(Object other) {
            if (!(other instanceof Location)) return false;
            Location value = (Location) other;
            return start == value.start && end == value.end && line == value.line
                    && column == value.column && endLine == value.endLine
                    && endColumn == value.endColumn && Objects.equals(file, value.file)
                    && Objects.equals(symbol, value.symbol)
                    && Objects.equals(message, value.message);
        }

        @Override public int hashCode() {
            return Objects.hash(file, start, end, line, column, endLine, endColumn,
                    symbol, message);
        }
    }

    public final int version;
    public final String stage;
    public final String code;
    public final String file;
    public final String symbol;
    public final String resource;
    public final String detail;
    public final List<String> causes;
    public final Location primary;
    public final List<Location> related;

    private WorkshopNativeDiagnostic(int version, String stage, String code, String file,
            String symbol, String resource, String detail, List<String> causes,
            Location primary, List<Location> related) {
        this.version = version;
        this.stage = stage;
        this.code = code;
        this.file = file;
        this.symbol = symbol;
        this.resource = resource;
        this.detail = detail;
        this.causes = Collections.unmodifiableList(new ArrayList<>(causes));
        this.primary = primary;
        this.related = Collections.unmodifiableList(new ArrayList<>(related));
    }

    public static WorkshopNativeDiagnostic fromNative(String message) {
        if (message == null) return null;
        String encoded = field(message, "diagnostic_envelope");
        try {
            if (encoded != null) {
                JSONObject envelope = new JSONObject(percentDecode(encoded));
                if (!SCHEMA.equals(envelope.optString("schema"))) return null;
                int version = envelope.optInt("version", 0);
                if (version != 1) return null;
                JSONObject context = envelope.optJSONObject("context");
                return fromJson(version, envelope.optString("stage"),
                        envelope.optString("code"), context == null ? null : context.optString("file", null),
                        context == null ? null : context.optString("symbol", null),
                        context == null ? null : context.optString("resource", null),
                        envelope.optString("detail"), envelope.optJSONArray("causes"),
                        Location.fromJson(envelope.optJSONObject("primary")),
                        locationsFromJson(envelope.optJSONArray("related")));
            }
            if (!SCHEMA.equals(field(message, "diagnostic_schema"))) return null;
            int version = parseInt(field(message, "diagnostic_version"), 0);
            if (version != 1) return null;
            return fromJson(version,
                    decode(field(message, "diagnostic_stage")), decode(field(message, "diagnostic_code")),
                    decode(field(message, "diagnostic_file")), decode(field(message, "diagnostic_symbol")),
                    decode(field(message, "diagnostic_resource")), decode(field(message, "diagnostic_detail")),
                    null, null, Collections.<Location>emptyList());
        } catch (Exception ignored) {
            return null;
        }
    }

    static WorkshopNativeDiagnostic fromJson(JSONObject value) {
        if (value == null || value == JSONObject.NULL) return null;
        try {
            if (!SCHEMA.equals(value.optString("schema")) || value.optInt("version", 0) != 1) {
                return null;
            }
            JSONObject context = value.optJSONObject("context");
            return fromJson(value.optInt("version", 0), value.optString("stage"),
                    value.optString("code"), context == null ? null : context.optString("file", null),
                    context == null ? null : context.optString("symbol", null),
                    context == null ? null : context.optString("resource", null),
                    value.optString("detail"), value.optJSONArray("causes"),
                    Location.fromJson(value.optJSONObject("primary")),
                    locationsFromJson(value.optJSONArray("related")));
        } catch (Exception ignored) {
            return null;
        }
    }

    private static WorkshopNativeDiagnostic fromJson(int version, String stage, String code,
            String file, String symbol, String resource, String detail, JSONArray causeArray,
            Location primary, List<Location> related) {
        ArrayList<String> causes = new ArrayList<>();
        if (causeArray != null) {
            for (int index = 0; index < causeArray.length(); index++) {
                causes.add(causeArray.optString(index));
            }
        }
        return new WorkshopNativeDiagnostic(version, emptyToNull(stage), emptyToNull(code),
                emptyToNull(file), emptyToNull(symbol), emptyToNull(resource), detail, causes,
                primary, related);
    }

    private static List<Location> locationsFromJson(JSONArray values) {
        ArrayList<Location> locations = new ArrayList<>();
        if (values == null) return locations;
        for (int index = 0; index < values.length(); index++) {
            Location location = Location.fromJson(values.optJSONObject(index));
            if (location != null) locations.add(location);
        }
        return locations;
    }

    public String displayText() {
        if (detail == null || detail.isEmpty()) return stage + " (" + code + ")";
        return detail;
    }

    public JSONObject toJson() throws Exception {
        JSONObject context = new JSONObject();
        if (file != null) context.put("file", file);
        if (symbol != null) context.put("symbol", symbol);
        if (resource != null) context.put("resource", resource);
        JSONObject value = new JSONObject().put("schema", SCHEMA).put("version", version)
                .put("stage", stage).put("code", code).put("context", context)
                .put("detail", detail).put("causes", new JSONArray(causes));
        if (primary != null) value.put("primary", primary.toJson());
        if (!related.isEmpty()) {
            JSONArray locations = new JSONArray();
            for (Location location : related) locations.put(location.toJson());
            value.put("related", locations);
        }
        return value;
    }

    private static String field(String message, String key) {
        String marker = "|" + key + "=";
        int start = message.indexOf(marker);
        if (start < 0) return null;
        start += marker.length();
        int end = message.indexOf('|', start);
        return end < 0 ? message.substring(start) : message.substring(start, end);
    }

    private static String decode(String value) {
        return value == null ? null : percentDecode(value);
    }

    private static int parseInt(String value, int fallback) {
        try { return Integer.parseInt(decode(value)); }
        catch (Exception ignored) { return fallback; }
    }

    private static String emptyToNull(String value) {
        return value == null || value.isEmpty() ? null : value;
    }

    private static String percentDecode(String value) {
        ByteArrayOutputStream output = new ByteArrayOutputStream(value.length());
        for (int index = 0; index < value.length(); index++) {
            char current = value.charAt(index);
            if (current == '%' && index + 2 < value.length()) {
                try {
                    output.write(Integer.parseInt(value.substring(index + 1, index + 3), 16));
                    index += 2;
                    continue;
                } catch (NumberFormatException ignored) { }
            }
            int codePoint = value.codePointAt(index);
            byte[] utf8 = new String(Character.toChars(codePoint)).getBytes(StandardCharsets.UTF_8);
            output.write(utf8, 0, utf8.length);
            index += Character.charCount(codePoint) - 1;
        }
        return new String(output.toByteArray(), StandardCharsets.UTF_8);
    }

    @Override public boolean equals(Object other) {
        if (!(other instanceof WorkshopNativeDiagnostic)) return false;
        WorkshopNativeDiagnostic value = (WorkshopNativeDiagnostic) other;
        return version == value.version && Objects.equals(stage, value.stage)
                && Objects.equals(code, value.code) && Objects.equals(file, value.file)
                && Objects.equals(symbol, value.symbol) && Objects.equals(resource, value.resource)
                && Objects.equals(detail, value.detail) && causes.equals(value.causes)
                && Objects.equals(primary, value.primary) && related.equals(value.related);
    }

    @Override public int hashCode() {
        return Objects.hash(version, stage, code, file, symbol, resource, detail, causes,
                primary, related);
    }
}
