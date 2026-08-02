package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class CanonicalSymbolIdentityTest {
    @Test
    public void semanticOwnerDoesNotAffectCanonicalMapping() {
        assertTrue(CanonicalSymbolIdentity.matchesRustItem(
                "function", "src/main.stasis", "tick", "tick(): void", 40, 70,
                "function", "src/main.stasis", "tick", "tick(): void",
                new int[] { 35 }, new int[] { 71 }));
    }

    @Test
    public void overloadsRequireMatchingSignatureAndSpan() {
        assertFalse(CanonicalSymbolIdentity.matchesRustItem(
                "function", "src/actions.stasis", "damage", "damage(i32): void", 80, 120,
                "function", "src/actions.stasis", "damage", "damage(f32): void",
                new int[] { 80 }, new int[] { 121 }));
        assertFalse(CanonicalSymbolIdentity.matchesRustItem(
                "function", "src/actions.stasis", "damage", "damage(i32): void", 80, 120,
                "function", "src/actions.stasis", "damage", "damage(i32): void",
                new int[] { 130 }, new int[] { 170 }));
    }

    @Test
    public void canonicalIdentitySurvivesRecreationAndDoesNotFallBack() {
        String symbolId = "v1|function|src/main.stasis|tick|()";
        assertEquals(symbolId, CanonicalSymbolIdentity.identityKey(
                symbolId, "function", "src/main.stasis", "Systems", "tick"));
        assertTrue(CanonicalSymbolIdentity.sameIdentity(
                symbolId, "function", "src/main.stasis", "Systems", "tick",
                symbolId, "function", "src/main.stasis", "Lifecycle", "tick"));
        assertFalse(CanonicalSymbolIdentity.sameIdentity(
                symbolId, "function", "src/main.stasis", "Systems", "tick",
                "", "function", "src/main.stasis", "Systems", "tick"));
    }

    @Test
    public void schemaV1FallbackRequiresBothSidesToLackCanonicalIds() {
        assertTrue(CanonicalSymbolIdentity.sameIdentity(
                "", "function", "src/main.stasis", "Systems", "tick",
                "", "function", "src/main.stasis", "Systems", "tick"));
    }
}
