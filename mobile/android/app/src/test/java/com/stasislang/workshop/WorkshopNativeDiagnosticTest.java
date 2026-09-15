package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;

public class WorkshopNativeDiagnosticTest {
    @Test public void parsesVersionedEnvelopeAndPreservesCauseOrder() {
        String message = "RunError: detail|diagnostic_envelope=%7B%22schema%22%3A%22stasis.native_diagnostic.v1%22%2C%22version%22%3A1%2C%22stage%22%3A%22resource%22%2C%22code%22%3A%22stasis.missingResource%22%2C%22context%22%3A%7B%22resource%22%3A%22assets%2Fmissing.svg%22%7D%2C%22detail%22%3A%22outer%20detail%22%2C%22causes%22%3A%5B%22outer%22%2C%22inner%22%5D%7D";
        WorkshopNativeDiagnostic diagnostic = WorkshopNativeDiagnostic.fromNative(message);
        assertEquals(1, diagnostic.version);
        assertEquals("resource", diagnostic.stage);
        assertEquals("stasis.missingResource", diagnostic.code);
        assertEquals("assets/missing.svg", diagnostic.resource);
        assertEquals("outer detail", diagnostic.detail);
        assertEquals("outer", diagnostic.causes.get(0));
        assertEquals("inner", diagnostic.causes.get(1));
        assertEquals("outer detail", diagnostic.displayText());
    }

    @Test public void ignoresUnversionedOrUnrelatedMessages() {
        assertNull(WorkshopNativeDiagnostic.fromNative("RunError: native preview frame failed"));
        assertTrue(WorkshopNativeDiagnostic.fromNative(
                "CompileError: x|diagnostic_schema=stasis.native_diagnostic.v1"
                        + "|diagnostic_version=1|diagnostic_stage=parse|diagnostic_code=stasis.parse"
                        + "|diagnostic_detail=detail") != null);
    }

    @Test public void decodesUtf8DetailContextAndCauses() {
        String message = "RunError: detail|diagnostic_envelope="
                + "%7B%22schema%22%3A%22stasis.native_diagnostic.v1%22%2C%22version%22%3A1%2C"
                + "%22stage%22%3A%22resource%22%2C%22code%22%3A%22stasis.missingResource%22%2C"
                + "%22context%22%3A%7B%22resource%22%3A%22assets%2F%E4%B8%96%E7%95%8C.svg%22%7D%2C"
                + "%22detail%22%3A%22%E8%B5%84%E6%BA%90%20%E2%9C%93%22%2C%22causes%22%3A%5B"
                + "%22resource%20phase%22%2C%22%E8%B5%84%E6%BA%90%20%E2%9C%93%22%5D%7D";
        WorkshopNativeDiagnostic diagnostic = WorkshopNativeDiagnostic.fromNative(message);
        assertEquals("assets/世界.svg", diagnostic.resource);
        assertEquals("资源 ✓", diagnostic.detail);
        assertEquals("resource phase", diagnostic.causes.get(0));
        assertEquals("资源 ✓", diagnostic.causes.get(1));

        String emoji = "RunError: detail|diagnostic_envelope="
                + "%7B%22schema%22%3A%22stasis.native_diagnostic.v1%22%2C%22version%22%3A1%2C"
                + "%22stage%22%3A%22resource%22%2C%22code%22%3A%22stasis.missingResource%22%2C"
                + "%22context%22%3A%7B%22resource%22%3A%22assets%2F%F0%9F%8C%8D.svg%22%7D%2C"
                + "%22detail%22%3A%22%F0%9F%8C%8D%22%2C%22causes%22%3A%5B%22resource%22%5D%7D";
        assertEquals("\uD83C\uDF0D", WorkshopNativeDiagnostic.fromNative(emoji).detail);
    }

    @Test public void parsesPrimaryAndRelatedSourceLocationsWithoutBreakingV1Context() throws Exception {
        JSONObject envelope = new JSONObject()
                .put("schema", WorkshopNativeDiagnostic.SCHEMA)
                .put("version", 1)
                .put("stage", "compile")
                .put("code", "stasis.explicitGenericCall")
                .put("context", new JSONObject()
                        .put("file", "src/use.stasis")
                        .put("symbol", "capacity"))
                .put("detail", "explicit generic calls are unsupported")
                .put("causes", new JSONArray().put("compile phase").put("unsupported call"))
                .put("primary", new JSONObject()
                        .put("path", "src/use.stasis")
                        .put("start", 42)
                        .put("end", 57)
                        .put("symbol", "capacity")
                        .put("message", "explicit generic calls are unsupported"))
                .put("related", new JSONArray().put(new JSONObject()
                        .put("file", "src/types.stasis")
                        .put("start", 7)
                        .put("end", 61)
                        .put("symbol", "capacity")
                        .put("message", "generic template declared here")));
        WorkshopNativeDiagnostic diagnostic = WorkshopNativeDiagnostic.fromNative(
                "CompileError: detail|diagnostic_envelope=" + envelope);

        assertNotNull(diagnostic);
        assertEquals("src/use.stasis", diagnostic.primary.file);
        assertEquals(42, diagnostic.primary.start);
        assertEquals(57, diagnostic.primary.end);
        assertEquals("capacity", diagnostic.primary.symbol);
        assertEquals(1, diagnostic.related.size());
        assertEquals("src/types.stasis", diagnostic.related.get(0).file);
        assertEquals("generic template declared here", diagnostic.related.get(0).message);
        assertEquals(diagnostic, WorkshopNativeDiagnostic.fromJson(diagnostic.toJson()));
    }
}
