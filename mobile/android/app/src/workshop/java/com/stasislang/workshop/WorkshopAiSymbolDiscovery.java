package com.stasislang.workshop;

import java.io.IOException;
import java.util.ArrayDeque;
import java.util.Locale;
import java.util.Map;
import java.util.TreeSet;

import org.json.JSONArray;
import org.json.JSONObject;

final class WorkshopAiSymbolDiscovery {
    static final String DEFAULT_ENTRY_FILE = "src/main.stasis";
    static final int DEFAULT_LIMIT = 32;
    static final int MAX_LIMIT = 200;
    static final int MAX_FILES = 16;

    private WorkshopAiSymbolDiscovery() {}

    static int boundedLimit(int requested) {
        return Math.max(1, Math.min(MAX_LIMIT, requested));
    }

    static String resolveImport(String sourceFile, String importPath) {
        String normalizedSource = normalize(sourceFile);
        int slash = normalizedSource.lastIndexOf('/');
        String parent = slash < 0 ? "" : normalizedSource.substring(0, slash + 1);
        return normalize(parent + importPath);
    }

    static TreeSet<String> defaultScope(Map<String, String> sources) throws Exception {
        TreeSet<String> files = new TreeSet<>();
        files.add(DEFAULT_ENTRY_FILE);
        JSONArray imports = directImportFiles(sources, DEFAULT_ENTRY_FILE);
        for (int index = 0; index < imports.length(); index += 1) {
            files.add(imports.getString(index));
        }
        return files;
    }

    static JSONObject importsForFiles(Map<String, String> sources, Iterable<String> files)
            throws Exception {
        JSONObject imports = new JSONObject();
        for (String file : files) imports.put(file, directImportFiles(sources, file));
        return imports;
    }

    static JSONArray directImportFiles(Map<String, String> sources, String file)
            throws Exception {
        String source = sources.get(file);
        if (source == null) return new JSONArray();
        JSONArray paths = parseImportPaths(source);
        TreeSet<String> resolved = new TreeSet<>();
        for (int index = 0; index < paths.length(); index += 1) {
            resolved.add(resolveImport(file, paths.getString(index)));
        }
        return new JSONArray(resolved);
    }

    static JSONArray parseImportPaths(String source) throws Exception {
        JSONArray imports = new JSONArray();
        String[] lines = source.split("\\r?\\n");
        for (String line : lines) {
            String trimmed = line.trim();
            if (trimmed.isEmpty()) continue;
            if (!trimmed.startsWith("import ")) break;
            String path = normalizeImportPath(trimmed);
            if (!path.isEmpty()) imports.put(path);
        }
        return imports;
    }

    static String normalizeImportPath(String value) throws IOException {
        String trimmed = value == null ? "" : value.trim();
        if (trimmed.isEmpty()) return "";
        if (trimmed.startsWith("import ")) {
            int firstQuote = trimmed.indexOf('"');
            int secondQuote = firstQuote < 0 ? -1 : trimmed.indexOf('"', firstQuote + 1);
            if (firstQuote < 0 || secondQuote <= firstQuote + 1) {
                throw new IOException("Invalid import line: " + trimmed);
            }
            return trimmed.substring(firstQuote + 1, secondQuote);
        }
        if (trimmed.indexOf('"') >= 0 || trimmed.indexOf(';') >= 0) {
            throw new IOException(
                    "Import paths should not include quotes or semicolons: " + trimmed);
        }
        return trimmed;
    }

    static boolean matches(String name, String signature, String kind, String owner,
            String queryFilter, String kindFilter, String ownerFilter) {
        String query = queryFilter == null ? "" : queryFilter.trim().toLowerCase(Locale.ROOT);
        if (!query.isEmpty()
                && !name.toLowerCase(Locale.ROOT).contains(query)
                && !signature.toLowerCase(Locale.ROOT).contains(query)) return false;
        if (kindFilter != null && !kindFilter.trim().isEmpty()
                && !kind.equals(kindFilter.trim())) return false;
        return ownerFilter == null || ownerFilter.trim().isEmpty()
                || owner.equals(ownerFilter.trim());
    }

    private static String normalize(String value) {
        ArrayDeque<String> parts = new ArrayDeque<>();
        for (String part : value.replace('\\', '/').split("/")) {
            if (part.isEmpty() || ".".equals(part)) continue;
            if ("..".equals(part)) {
                if (!parts.isEmpty()) parts.removeLast();
                continue;
            }
            parts.addLast(part);
        }
        return String.join("/", parts);
    }
}
