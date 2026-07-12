package com.stasislang.workshop;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.UUID;

final class WorkshopAiTemporaryVerification {
    interface Executor {
        Result execute(File testFile) throws Exception;
    }

    static final class Result {
        final boolean passed;
        final String evidence;

        Result(boolean passed, String evidence) {
            this.passed = passed;
            this.evidence = evidence == null ? "" : evidence;
        }
    }

    private WorkshopAiTemporaryVerification() {}

    static Result run(File projectRoot, String testSource, Executor executor) throws Exception {
        if (projectRoot == null || testSource == null || testSource.trim().isEmpty()
                || executor == null) {
            throw new IllegalArgumentException("temporary verification requires project, test source, and executor");
        }
        File tests = new File(projectRoot, "tests");
        if (!tests.isDirectory() && !tests.mkdirs()) {
            throw new IllegalStateException("temporary verification tests directory could not be created");
        }
        File testFile = new File(tests, ".workshop_verification_" + UUID.randomUUID()
                + ".test.stasis");
        String testsRoot = tests.getCanonicalPath() + File.separator;
        if (!testFile.getCanonicalPath().startsWith(testsRoot)) {
            throw new IllegalStateException("temporary verification path escaped tests directory");
        }
        try {
            writeAtomic(testFile, testSource.trim() + "\n");
            return executor.execute(testFile);
        } finally {
            File temporary = new File(testFile.getParentFile(), testFile.getName() + ".tmp");
            if (!temporary.delete() && temporary.exists()) {
                throw new IllegalStateException("temporary verification staging file cleanup failed");
            }
            if (!testFile.delete() && testFile.exists()) {
                throw new IllegalStateException("temporary verification test cleanup failed");
            }
        }
    }

    static boolean acceptedRun(int baselinePassed, int verifierRunPassed, int verifierRunFailed) {
        return baselinePassed >= 0 && verifierRunFailed == 0
                && verifierRunPassed >= baselinePassed + 1;
    }

    private static void writeAtomic(File file, String source) throws Exception {
        File temporary = new File(file.getParentFile(), file.getName() + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(source.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
        try {
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING);
        } catch (Exception error) {
            temporary.delete();
            throw error;
        }
    }
}
