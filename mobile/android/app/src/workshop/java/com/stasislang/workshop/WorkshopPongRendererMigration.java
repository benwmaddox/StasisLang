package com.stasislang.workshop;

final class WorkshopPongRendererMigration {
    static final String ADAPTER_IMPORT = "import \"preview_adapter.stasis\";";

    private WorkshopPongRendererMigration() {}

    static boolean isProductionSource(String source) {
        return source != null && (source.contains(ADAPTER_IMPORT)
                || source.contains("global gfx_cmd_i32:"));
    }

    static String migrateSource(String source) {
        if (source == null || source.isEmpty()) {
            throw new IllegalArgumentException("Pong source is empty");
        }
        if (isProductionSource(source)) return source;
        String migrated = renameDeclaration(source, "main", "pong_game_main");
        migrated = renameDeclaration(migrated, "tick", "pong_game_tick");
        migrated = renameDeclaration(migrated, "render", "pong_game_render");
        migrated = renameDeclaration(migrated, "on_code_swap", "pong_game_on_code_swap");
        return ADAPTER_IMPORT + "\n\n" + migrated;
    }

    private static String renameDeclaration(String source, String from, String to) {
        String needle = "function " + from + "(): void";
        int first = source.indexOf(needle);
        if (first < 0 || source.indexOf(needle, first + needle.length()) >= 0) {
            throw new IllegalArgumentException("expected exactly one " + from + " lifecycle");
        }
        return source.substring(0, first) + "function " + to + "(): void"
                + source.substring(first + needle.length());
    }
}
