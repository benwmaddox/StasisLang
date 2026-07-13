package com.stasislang.workshop;

import java.util.Collection;
import java.util.Locale;

final class WorkshopAiVerificationPolicy {
    enum Risk {
        LOW,
        GAMEPLAY,
        STRUCTURAL,
        VISUAL
    }

    static final class Decision {
        final Risk risk;
        final boolean requiresBehaviorTest;
        final boolean requiresBoundaryCoverage;
        final boolean requiresLogicalSnapshot;
        final boolean requiresIndependentReview;

        Decision(Risk risk, boolean requiresBehaviorTest, boolean requiresBoundaryCoverage,
                boolean requiresLogicalSnapshot, boolean requiresIndependentReview) {
            this.risk = risk;
            this.requiresBehaviorTest = requiresBehaviorTest;
            this.requiresBoundaryCoverage = requiresBoundaryCoverage;
            this.requiresLogicalSnapshot = requiresLogicalSnapshot;
            this.requiresIndependentReview = requiresIndependentReview;
        }
    }

    private WorkshopAiVerificationPolicy() {}

    static Decision classify(String prompt, Collection<String> changedSymbols) {
        StringBuilder text = new StringBuilder(prompt == null ? "" : prompt.toLowerCase(Locale.ROOT));
        if (changedSymbols != null) {
            for (String symbol : changedSymbols) {
                text.append(' ').append(symbol == null ? "" : symbol.toLowerCase(Locale.ROOT));
            }
        }
        String value = text.toString();
        boolean structural = containsAny(value, "struct", "global", "init", "reset", "spawn",
                "lifecycle", "on_code_swap", "migration");
        boolean gameplay = containsAny(value, "collision", "collide", "movement", "move", "speed",
                "velocity", "score", "health", "damage", "timer", "input", "touch", "paddle",
                "ball", "enemy", "projectile", "physics", "range", "bounds", "size", "width",
                "height", "position");
        boolean visual = containsAny(value, "render", "sprite", "camera", "layout", "visual",
                "button", "font", "text", "image", "color");
        if (structural) {
            return new Decision(Risk.STRUCTURAL, true, gameplay, visual, true);
        }
        if (gameplay) {
            return new Decision(Risk.GAMEPLAY, true, true, visual, true);
        }
        if (visual) {
            return new Decision(Risk.VISUAL, true, false, true, true);
        }
        return new Decision(Risk.LOW, false, false, false, false);
    }

    private static boolean containsAny(String value, String... needles) {
        for (String needle : needles) {
            if (value.contains(needle)) return true;
        }
        return false;
    }
}
