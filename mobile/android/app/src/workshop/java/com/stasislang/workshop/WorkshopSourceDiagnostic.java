package com.stasislang.workshop;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;

final class WorkshopSourceDiagnostic {
    final String file;
    final int line;
    final int column;
    final int endLine;
    final int endColumn;
    final String symbol;
    final String message;

    WorkshopSourceDiagnostic(String file, int line, int column, int endLine, int endColumn,
            String symbol, String message) {
        this.file = normalizeProjectPath(file);
        this.line = Math.max(0, line);
        this.column = Math.max(0, column);
        this.endLine = Math.max(this.line, endLine);
        this.endColumn = Math.max(0, endColumn);
        this.symbol = symbol == null ? "" : symbol;
        this.message = message == null ? "" : message;
    }

    static WorkshopSourceDiagnostic fromCompileResult(String result) {
        if (result == null || !result.startsWith("CompileError")) return null;
        String file = field(result, "diagnostic_file");
        if (file.isEmpty()) return null;
        try {
            return new WorkshopSourceDiagnostic(file,
                    integerField(result, "diagnostic_line"),
                    integerField(result, "diagnostic_column"),
                    integerField(result, "diagnostic_end_line"),
                    integerField(result, "diagnostic_end_column"),
                    field(result, "diagnostic_symbol"),
                    field(result, "diagnostic_message"));
        } catch (IllegalArgumentException error) {
            return null;
        }
    }

    static WorkshopSourceDiagnostic fromTestFailure(
            String file, int line, int column, String symbol, String message) {
        try {
            return new WorkshopSourceDiagnostic(file, line, column, line, column, symbol, message);
        } catch (IllegalArgumentException error) {
            return null;
        }
    }

    static int sourceOffset(String source, int oneBasedLine, int oneBasedColumn) {
        if (source == null || source.isEmpty()) return 0;
        int offset = 0;
        int line = 1;
        while (line < Math.max(1, oneBasedLine) && offset < source.length()) {
            if (source.charAt(offset++) == '\n') line += 1;
        }
        int remainingColumns = Math.max(1, oneBasedColumn) - 1;
        while (remainingColumns > 0 && offset < source.length() && source.charAt(offset) != '\n') {
            offset += Character.charCount(source.codePointAt(offset));
            remainingColumns -= 1;
        }
        return offset;
    }

    String displayText(String kind) {
        StringBuilder text = new StringBuilder(kind).append("\nfile=").append(file);
        if (line > 0) {
            text.append("\nline=").append(line);
            if (column > 0) text.append(", column=").append(column);
        }
        if (!symbol.isEmpty()) text.append("\nsymbol=").append(symbol);
        if (!message.isEmpty()) text.append("\n").append(message);
        return text.toString();
    }

    private static String field(String result, String key) {
        String marker = "|" + key + "=";
        int start = result.indexOf(marker);
        if (start < 0) return "";
        start += marker.length();
        int end = result.indexOf('|', start);
        return percentDecode(result.substring(start, end < 0 ? result.length() : end));
    }

    private static int integerField(String result, String key) {
        String value = field(result, key);
        return value.isEmpty() ? 0 : Integer.parseInt(value);
    }

    private static String normalizeProjectPath(String file) {
        String normalized = file == null ? "" : file.replace('\\', '/').trim();
        if (normalized.isEmpty() || normalized.startsWith("/") || normalized.contains(":")) {
            throw new IllegalArgumentException("diagnostic path must be project-relative");
        }
        for (String segment : normalized.split("/")) {
            if (segment.isEmpty() || ".".equals(segment) || "..".equals(segment)) {
                throw new IllegalArgumentException("diagnostic path contains an unsafe segment");
            }
        }
        return normalized;
    }

    private static String percentDecode(String value) {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream(value.length());
        for (int index = 0; index < value.length();) {
            char current = value.charAt(index);
            if (current == '%' && index + 2 < value.length()) {
                int high = Character.digit(value.charAt(index + 1), 16);
                int low = Character.digit(value.charAt(index + 2), 16);
                if (high < 0 || low < 0) throw new IllegalArgumentException("invalid diagnostic escape");
                bytes.write((high << 4) | low);
                index += 3;
            } else {
                byte[] encoded = String.valueOf(current).getBytes(StandardCharsets.UTF_8);
                bytes.write(encoded, 0, encoded.length);
                index += 1;
            }
        }
        return new String(bytes.toByteArray(), StandardCharsets.UTF_8);
    }
}
