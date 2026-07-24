package com.stasislang.workshop;

import java.util.Arrays;
import java.util.Collections;
import java.util.List;

final class WorkshopTemplateCatalog {
    static final String DEFAULT_TEMPLATE_ID = "exploration";
    static final String LEGACY_TEMPLATE_ID = "pong";

    private static final Template PONG = new Template(
            "pong",
            "Pong",
            "workshop_sample/",
            new String[] {
                    "src/main.stasis",
                    "src/preview_adapter.stasis",
                    "src/root.stasis",
                    "src/game_state.stasis",
                    "src/player.stasis",
                    "src/enemy.stasis",
                    "src/input.stasis",
                    "src/assets.stasis",
                    "src/systems/collision.stasis"
            },
            new String[] { "tests/enemy_paddle_speed_schedule.test.stasis" },
            new String[] {
                    "AGENTS.md",
                    "assets/manifest.json",
                    "assets/ball.svg",
                    "assets/paddle.svg",
                    "assets/center_line.svg"
            });

    private static final Template EXPLORATION = new Template(
            "exploration",
            "Exploration Garden",
            "exploration_sample/",
            new String[] {
                    "src/main.stasis",
                    "src/host.stasis",
                    "src/host_aot.stasis",
                    "src/host_game.stasis",
                    "src/host_runtime.stasis",
                    "src/config.stasis",
                    "src/components.stasis",
                    "src/world_data.stasis",
                    "src/input.stasis",
                    "src/assets.stasis",
                    "src/systems/movement.stasis",
                    "src/systems/collection.stasis",
                    "src/systems/inventory.stasis",
                    "src/systems/camera.stasis",
                    "src/systems/tutorial.stasis",
                    "src/systems/audio.stasis",
                    "src/systems/render_extract.stasis",
                    "src/systems/schedule.stasis"
            },
            new String[] { "tests/exploration_gameplay.test.stasis" },
            new String[] {
                    "AGENTS.md",
                    "assets/manifest.json",
                    "assets/player.svg",
                    "assets/sun_keepsake.svg",
                    "assets/moon_keepsake.svg",
                    "assets/destination.svg",
                    "stasis.json",
                    "README.md",
                    "qa/first_keepsake.json"
            });

    private WorkshopTemplateCatalog() {}

    static Template require(String id) {
        if (PONG.id.equals(id)) return PONG;
        if (EXPLORATION.id.equals(id)) return EXPLORATION;
        throw new IllegalArgumentException("unknown Workshop template: " + id);
    }

    static boolean isKnown(String id) {
        return PONG.id.equals(id) || EXPLORATION.id.equals(id);
    }

    static List<Template> list() {
        return Collections.unmodifiableList(Arrays.asList(EXPLORATION, PONG));
    }

    static final class Template {
        final String id;
        final String name;
        final String assetRoot;
        final String[] sourceFiles;
        final String[] testFiles;
        final String[] auxiliaryFiles;

        Template(String id, String name, String assetRoot, String[] sourceFiles, String[] testFiles,
                String[] auxiliaryFiles) {
            this.id = id;
            this.name = name;
            this.assetRoot = assetRoot;
            this.sourceFiles = sourceFiles.clone();
            this.testFiles = testFiles.clone();
            this.auxiliaryFiles = auxiliaryFiles.clone();
        }

        @Override public String toString() {
            return name;
        }
    }
}
