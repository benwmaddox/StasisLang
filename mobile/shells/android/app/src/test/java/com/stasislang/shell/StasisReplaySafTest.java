package com.stasislang.shell;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public final class StasisReplaySafTest {
    @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();

    @Test
    public void oversizedImportKeepsPreviousReplayAndExactLimitPublishes() throws Exception {
        File files = temporaryFolder.newFolder("files");
        File destination = importBytes(files, "previous", 32L);

        expectImportFailure(files, "too large", 8L);
        assertEquals("previous", read(destination));
        assertNoStagingFiles(files);

        importBytes(files, "12345678", 8L);
        assertEquals("12345678", read(destination));
        assertNoStagingFiles(files);
    }

    @Test
    public void failedReadKeepsPreviousReplayAndRemovesPartialStaging() throws Exception {
        File files = temporaryFolder.newFolder("files");
        File destination = importBytes(files, "previous", 32L);
        InputStream broken = new InputStream() {
            private boolean sentPartial;

            @Override
            public int read() throws IOException {
                throw new IOException("source read failed");
            }

            @Override
            public int read(byte[] buffer, int offset, int length) throws IOException {
                if (sentPartial) throw new IOException("source read failed");
                sentPartial = true;
                byte[] partial = bytes("partial");
                System.arraycopy(partial, 0, buffer, offset, partial.length);
                return partial.length;
            }
        };
        try {
            StasisReplaySaf.importReplay(broken, files, 32L);
            fail("expected source read failure");
        } catch (IOException expected) {
            assertTrue(expected.getMessage().contains("source read failed"));
        }

        assertEquals("previous", read(destination));
        assertNoStagingFiles(files);
    }

    @Test
    public void queuedWarmImportCanBeConsumedOnce() throws Exception {
        File files = temporaryFolder.newFolder("files");
        assertFalse(StasisReplaySaf.isReplayPending(files));
        StasisReplaySaf.markReplayPending(files);
        assertTrue(StasisReplaySaf.isReplayPending(files));
        assertTrue(StasisReplaySaf.clearReplayPending(files));
        assertFalse(StasisReplaySaf.isReplayPending(files));
    }

    @Test
    public void emptyImportIsRejectedWithoutPublishing() throws Exception {
        File files = temporaryFolder.newFolder("files");
        expectImportFailure(files, "", 32L);
        assertFalse(new File(files, "stasis_replay.json").exists());
        assertNoStagingFiles(files);
    }

    private static File importBytes(File files, String contents, long maximum) throws IOException {
        return StasisReplaySaf.importReplay(
                new ByteArrayInputStream(bytes(contents)), files, maximum);
    }

    private static void expectImportFailure(File files, String contents, long maximum)
            throws IOException {
        try {
            importBytes(files, contents, maximum);
            fail("expected bounded replay import failure");
        } catch (IOException expected) {
            // The caller checks that the previous publication remains readable.
        }
    }

    private static String read(File file) throws IOException {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }

    private static byte[] bytes(String value) {
        return value.getBytes(StandardCharsets.UTF_8);
    }

    private static void assertNoStagingFiles(File directory) {
        File[] staging = directory.listFiles((parent, name) ->
                name.startsWith(".stasis_replay.json.part."));
        assertTrue("temporary replay stages are cleaned", staging == null || staging.length == 0);
    }
}
