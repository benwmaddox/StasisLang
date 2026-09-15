package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.json.JSONArray;
import org.json.JSONObject;
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
        assertNull(WorkshopSourceDiagnostic.fromTestFailure("C:/outside.stasis", 1, 1, "bad", ""));
    }

    @Test
    public void testFailurePreservesReportedColumn() {
        WorkshopSourceDiagnostic diagnostic = WorkshopSourceDiagnostic.fromTestFailure(
                "tests/failing.test.stasis", 4, 12, "failing", "unknown call");

        assertEquals(4, diagnostic.line);
        assertEquals(12, diagnostic.column);
        assertEquals(12, diagnostic.endColumn);
    }

    @Test
    public void sourceOffsetUsesLineAndUnicodeColumn() {
        assertEquals(6, WorkshopSourceDiagnostic.sourceOffset("one\ntw😀o\nthree", 2, 3));
        assertEquals(10, WorkshopSourceDiagnostic.sourceOffset("one\ntw😀o\nthree", 3, 1));
    }

    @Test
    public void compileDiagnosticProjectsTypedPrimaryAndRelatedLocations() throws Exception {
        JSONObject envelope = new JSONObject()
                .put("schema", WorkshopNativeDiagnostic.SCHEMA)
                .put("version", 1)
                .put("stage", "compile")
                .put("code", "stasis.explicitGenericCall")
                .put("context", new JSONObject().put("file", "src/main.stasis"))
                .put("detail", "explicit generic calls are unsupported")
                .put("causes", new JSONArray())
                .put("primary", new JSONObject()
                        .put("path", "src/main.stasis")
                        .put("start", 42)
                        .put("end", 57)
                        .put("symbol", "capacity"))
                .put("related", new JSONArray().put(new JSONObject()
                        .put("file", "src/types.stasis")
                        .put("start", 7)
                        .put("end", 61)
                        .put("symbol", "capacity")
                        .put("message", "generic template declared here")));
        WorkshopSourceDiagnostic diagnostic = WorkshopSourceDiagnostic.fromCompileResult(
                "CompileError: detail|diagnostic_envelope=" + envelope);

        assertEquals("stasis.explicitGenericCall", diagnostic.code);
        assertEquals(42, diagnostic.start);
        assertEquals(57, diagnostic.end);
        assertEquals("capacity", diagnostic.primary.symbol);
        assertEquals("src/types.stasis", diagnostic.related.get(0).file);
        assertEquals("generic template declared here", diagnostic.related.get(0).message);
    }
}
