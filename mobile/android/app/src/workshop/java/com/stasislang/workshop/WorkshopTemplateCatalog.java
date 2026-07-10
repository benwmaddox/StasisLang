package com.stasislang.workshop;

import java.util.Arrays;
import java.util.Collections;
import java.util.List;

final class WorkshopTemplateCatalog {
    static final String DEFAULT_TEMPLATE_ID = "pong";
    static final String LEGACY_TEMPLATE_ID = "pong";

    private static final Template PONG = new Template(
            "pong",
            "Pong",
            "workshop_sample/",
            new String[] {
                    "src/main.stasis",
                    "src/root.stasis",
                    "src/game_state.stasis",
                    "src/player.stasis",
                    "src/enemy.stasis",
                    "src/input.stasis",
                    "src/assets.stasis",
                    "src/systems/collision.stasis"
            },
            new String[] { "tests/enemy_paddle_speed_schedule.test.stasis" });

    private WorkshopTemplateCatalog() {}

    static Template require(String id) {
        if (PONG.id.equals(id)) return PONG;
        throw new IllegalArgumentException("unknown Workshop template: " + id);
    }

    static boolean isKnown(String id) {
        return PONG.id.equals(id);
    }

    static List<Template> list() {
        return Collections.unmodifiableList(Arrays.asList(PONG));
    }

    static final class Template {
        final String id;
        final String name;
        final String assetRoot;
        final String[] sourceFiles;
        final String[] testFiles;

        Template(String id, String name, String assetRoot, String[] sourceFiles, String[] testFiles) {
            this.id = id;
            this.name = name;
            this.assetRoot = assetRoot;
            this.sourceFiles = sourceFiles.clone();
            this.testFiles = testFiles.clone();
        }

        @Override public String toString() {
            return name;
        }
    }
}
