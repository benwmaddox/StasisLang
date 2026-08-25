package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopPongRendererMigrationTest {
    @Test
    public void migrationPreservesGameBodiesAndRenamesOnlyLifecycleDeclarations() {
        String legacy = "global GameState { player_y: i32; }\n"
                + "function main(): void { GameState.player_y = 72; }\n"
                + "function tick(): void { GameState.player_y += 3; }\n"
                + "function render(): void { GameState.player_y += 5; }\n"
                + "function on_code_swap(): void { render(); }\n";

        String migrated = WorkshopPongRendererMigration.migrateSource(legacy);

        assertTrue(migrated.startsWith(WorkshopPongRendererMigration.ADAPTER_IMPORT));
        assertTrue(migrated.contains("function pong_game_main(): void { GameState.player_y = 72; }"));
        assertTrue(migrated.contains("function pong_game_tick(): void { GameState.player_y += 3; }"));
        assertTrue(migrated.contains("function pong_game_render(): void { GameState.player_y += 5; }"));
        assertTrue(migrated.contains("function pong_game_on_code_swap(): void { render(); }"));
        assertFalse(migrated.contains("function main(): void"));
        assertEquals(migrated, WorkshopPongRendererMigration.migrateSource(migrated));
    }

    @Test
    public void migrationRejectsAmbiguousOrIncompleteLifecycleSets() {
        String duplicateMain = "function main(): void {}\nfunction main(): void {}\n"
                + "function tick(): void {}\nfunction render(): void {}\n"
                + "function on_code_swap(): void {}\n";
        assertThrows(IllegalArgumentException.class,
                () -> WorkshopPongRendererMigration.migrateSource(duplicateMain));

        String missingRender = "function main(): void {}\nfunction tick(): void {}\n"
                + "function on_code_swap(): void {}\n";
        assertThrows(IllegalArgumentException.class,
                () -> WorkshopPongRendererMigration.migrateSource(missingRender));
    }
}
