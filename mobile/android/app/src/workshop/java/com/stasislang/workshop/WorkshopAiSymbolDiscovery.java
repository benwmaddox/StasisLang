package com.stasislang.workshop;

import java.util.ArrayDeque;
import java.util.Locale;

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
