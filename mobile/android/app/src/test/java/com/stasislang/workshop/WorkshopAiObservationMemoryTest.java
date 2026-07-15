package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.util.List;

import org.junit.Test;

public final class WorkshopAiObservationMemoryTest {
    @Test
    public void retainsEarlierBatchesAndMovesUpdatedTargetsToNewest() {
        WorkshopAiObservationMemory memory = new WorkshopAiObservationMemory();
        memory.remember("read_symbol:render", "{\"name\":\"render\",\"source\":\"old\"}");
        memory.remember("read_symbol:tick", "{\"name\":\"tick\"}");
        memory.remember("read_symbol:render", "{\"name\":\"render\",\"source\":\"current\"}");

        List<String> snapshot = memory.snapshotNewestFirst();
        assertEquals(2, snapshot.size());
        assertTrue(snapshot.get(0).contains("current"));
        assertFalse(snapshot.toString().contains("old"));
        assertTrue(snapshot.get(1).contains("tick"));
    }

    @Test
    public void boundsRetainedTargetsAndSnapshotCharacters() {
        WorkshopAiObservationMemory memory = new WorkshopAiObservationMemory();
        for (int index = 0; index < 30; index += 1) {
            memory.remember("target-" + index, "target-" + index + ":" + "x".repeat(10_000));
        }

        assertEquals(16, memory.size());
        List<String> snapshot = memory.snapshotNewestFirst();
        assertTrue(snapshot.size() <= 10);
        assertTrue(snapshot.get(0).startsWith("target-29:"));
        assertFalse(snapshot.toString().contains("target-0"));
    }

    @Test
    public void restoresBoundedCheckpointWithoutDuplicatingObservations() {
        WorkshopAiObservationMemory memory = new WorkshopAiObservationMemory();
        memory.restoreNewestFirst(List.of(
                "{\"tool\":\"write_symbol\",\"args\":{\"name\":\"tick\"},\"result\":\"old\"}",
                "{\"tool\":\"read_symbol\",\"args\":{\"name\":\"render\"}}"));
        memory.remember("write_symbol|{\"name\":\"tick\"}",
                "{\"tool\":\"write_symbol\",\"args\":{\"name\":\"tick\"},\"result\":\"current\"}");

        assertEquals(2, memory.size());
        assertTrue(memory.snapshotNewestFirst().get(0).contains("write_symbol"));
        assertTrue(memory.snapshotNewestFirst().get(0).contains("current"));
        assertFalse(memory.snapshotNewestFirst().toString().contains("old"));
    }
}
