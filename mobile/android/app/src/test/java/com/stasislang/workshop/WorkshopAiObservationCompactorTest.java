package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import org.junit.Test;

public final class WorkshopAiObservationCompactorTest {
    @Test
    public void successfulWriteRetainsIdentityButNotFullSource() throws Exception {
        org.json.JSONObject compact = WorkshopAiObservationCompactor.compactSuccessfulWrite(
                new org.json.JSONObject()
                        .put("tool", "write_symbol")
                        .put("args", new org.json.JSONObject()
                                .put("name", "tick")
                                .put("new_source", "function tick(): void {}")));
        org.json.JSONObject args = compact.getJSONObject("args");
        assertEquals(24, args.getInt("new_source_chars"));
        assertFalse(args.has("new_source"));
        assertFalse(args.has("new_source_sha256"));
    }
}
