package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.junit.Test;

public final class WorkshopSourceDiagnosticTest {
    @Test
    public void compileDiagnosticParsesProjectFileSymbolAndSpan() {
        WorkshopSourceDiagnostic diagnostic = WorkshopSourceDiagnostic.fromCompileResult(
                "CompileError: bad|diagnostic_file=src/systems/collision.stasis"
                        + "|diagnostic_line=7|diagnostic_column=5|diagnostic_end_line=7"
                        + "|diagnostic_end_column=6|diagnostic_symbol=resolve_hit"
                        + "|diagnostic_message=missing%20closing%20brace");

        assertEquals("src/systems/collision.stasis", diagnostic.file);
        assertEquals(7, diagnostic.line);
        assertEquals(5, diagnostic.column);
        assertEquals("resolve_hit", diagnostic.symbol);
        assertEquals("missing closing brace", diagnostic.message);
    }

    @Test
    public void diagnosticRejectsEscapingOrAbsolutePaths() {
        assertNull(WorkshopSourceDiagnostic.fromCompileResult(
                "CompileError|diagnostic_file=../outside.stasis|diagnostic_line=1"));
        assertNull(WorkshopSourceDiagnostic.fromTestFailure("C:/outside.stasis", 1, "bad", ""));
    }

    @Test
    public void sourceOffsetUsesLineAndUnicodeColumn() {
        assertEquals(6, WorkshopSourceDiagnostic.sourceOffset("one\ntw😀o\nthree", 2, 3));
        assertEquals(10, WorkshopSourceDiagnostic.sourceOffset("one\ntw😀o\nthree", 3, 1));
    }
}
