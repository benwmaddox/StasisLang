package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import org.junit.Test;

public final class WorkshopAiProjectTransactionTest {
    @Test
    public void restoreRevertsChangesDeletesAddedTestsAndRecreatesDeletedTests() throws Exception {
        File root = Files.createTempDirectory("workshop-ai-transaction").toFile();
        File source = write(root, "src/main.stasis", "function main(): void {}\n");
        File originalTest = write(root, "tests/original.test.stasis",
                "test `original`(): bool { return true; }\n");
        WorkshopAiProjectTransaction.Snapshot snapshot = WorkshopAiProjectTransaction.capture(root);

        Files.write(source.toPath(), "broken\n".getBytes(StandardCharsets.UTF_8));
        originalTest.delete();
        File added = write(root, "tests/generated.test.stasis",
                "test `generated`(): bool { return true; }\n");

        WorkshopAiProjectTransaction.restore(root, snapshot);

        assertEquals("function main(): void {}\n", read(source));
        assertEquals("test `original`(): bool { return true; }\n", read(originalTest));
        assertFalse(added.exists());
    }

    @Test
    public void fingerprintIsStableAndDetectsAcceptedWriteBoundaries() throws Exception {
        File root = Files.createTempDirectory("workshop-ai-fingerprint").toFile();
        File source = write(root, "src/main.stasis", "function main(): void {}\n");
        WorkshopAiProjectTransaction.Snapshot before = WorkshopAiProjectTransaction.capture(root);

        assertEquals(WorkshopAiProjectTransaction.fingerprint(before),
                WorkshopAiProjectTransaction.fingerprint(
                        WorkshopAiProjectTransaction.capture(root)));
        Files.write(source.toPath(),
                "function main(): void { print(1); }\n".getBytes(StandardCharsets.UTF_8));
        assertNotEquals(WorkshopAiProjectTransaction.fingerprint(before),
                WorkshopAiProjectTransaction.fingerprint(
                        WorkshopAiProjectTransaction.capture(root)));
    }

    private static File write(File root, String relative, String source) throws Exception {
        File file = new File(root, relative.replace('/', File.separatorChar));
        file.getParentFile().mkdirs();
        Files.write(file.toPath(), source.getBytes(StandardCharsets.UTF_8));
        return file;
    }

    private static String read(File file) throws Exception {
        return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
    }
}
