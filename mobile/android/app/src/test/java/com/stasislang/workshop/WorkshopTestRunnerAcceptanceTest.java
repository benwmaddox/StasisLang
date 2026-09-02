package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import org.json.JSONObject;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class WorkshopTestRunnerAcceptanceTest {
    @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();

    @Test public void sourceAndTestFixturesProduceOneDeterministicRevisionSeam() {
        String original = "const IT028_TICK_REVISION: i32 = 1;\n"
                + "function tick(): i32 { return IT028_TICK_REVISION; }\n";
        String accepted = WorkshopTestRunnerAcceptance.acceptedSource(original);
        String failing = WorkshopTestRunnerAcceptance.failingSource(accepted);
        assertTrue(accepted.contains("const IT028_TICK_REVISION: i32 = 3;"));
        assertTrue(failing.contains("const IT028_TICK_REVISION: i32 = 4;"));
        assertFalse(failing.contains("const IT028_TICK_REVISION: i32 = 3;"));
        assertTrue(failing.contains("return IT028_TICK_REVISION;"));
        assertEquals("import \"../src/main.stasis\";\n\n"
                + "test `IT-030 Workshop JNI rollback`(): bool {\n"
                + "    return IT028_TICK_REVISION == 3;\n}\n",
                WorkshopTestRunnerAcceptance.testSource());
        assertThrows(IllegalStateException.class,
                () -> WorkshopTestRunnerAcceptance.acceptedSource(accepted));
        assertThrows(IllegalStateException.class, () ->
                WorkshopTestRunnerAcceptance.acceptedSource(original + original));
    }

    @Test public void renderParityFixtureOwnsOneReachableTaggedRevision() throws Exception {
        File repository = new File(System.getProperty("user.dir")).getCanonicalFile();
        while (repository != null && !new File(repository, "Cargo.toml").isFile()) {
            repository = repository.getParentFile();
        }
        assertTrue("repository root was not found", repository != null);
        File fixture = new File(repository,
                "samples/render_parity/main.stasis").getCanonicalFile();
        String source = new String(Files.readAllBytes(fixture.toPath()),
                StandardCharsets.UTF_8);
        String tag = "const IT028_TICK_REVISION: i32 = 1;";
        assertEquals(source.indexOf(tag), source.lastIndexOf(tag));
        assertTrue(source.indexOf(tag) >= 0);
        assertTrue(source.contains("seam_it028_tick_marker = IT028_TICK_REVISION;"));
        String accepted = WorkshopTestRunnerAcceptance.acceptedSource(source);
        assertFalse(accepted.contains(tag));
        assertTrue(accepted.contains("const IT028_TICK_REVISION: i32 = 3;"));
    }

    @Test public void structuredResultRequiresExactCountsAndKeepsLocation() throws Exception {
        String result = "{\"kind\":\"stasis_test_run\",\"passed\":1,\"failed\":0,"
                + "\"all_passed\":true,\"results\":[{\"file\":\"tests/it030_workshop_jni.test.stasis\","
                + "\"line\":3,\"column\":1,\"name\":\"IT-030 Workshop JNI rollback\","
                + "\"passed\":true,\"status\":\"passed\"}]}";
        JSONObject run = WorkshopTestRunnerAcceptance.parseRun(result);
        JSONObject named = WorkshopTestRunnerAcceptance.validateNamedResultForTest(run);
        assertEquals(3, named.getInt("line"));
        assertEquals(1, named.getInt("column"));
        assertThrows(IllegalStateException.class, () ->
                WorkshopTestRunnerAcceptance.parseRun(result.replace(
                        "\"passed\":1", "\"passed\":2")));
    }

    @Test public void productionTransactionRestoresSourceAndRemovesTemporaryTest()
            throws Exception {
        File project = temporaryFolder.newFolder("it030-project");
        File source = new File(project, "src/main.stasis");
        assertTrue(source.getParentFile().mkdirs());
        Files.write(source.toPath(), ("const IT028_TICK_REVISION: i32 = 1;\n"
                + "function tick(): i32 { return IT028_TICK_REVISION; }\n")
                .getBytes(StandardCharsets.UTF_8));
        WorkshopAiProjectTransaction.Snapshot packaged =
                WorkshopAiProjectTransaction.capture(project);
        String packagedFingerprint = WorkshopAiProjectTransaction.fingerprint(packaged);

        Files.write(source.toPath(), WorkshopTestRunnerAcceptance.acceptedSource(
                new String(Files.readAllBytes(source.toPath()), StandardCharsets.UTF_8))
                .getBytes(StandardCharsets.UTF_8));
        File test = new File(project, WorkshopTestRunnerAcceptance.TEST_PATH);
        assertTrue(test.getParentFile().mkdirs());
        Files.write(test.toPath(), WorkshopTestRunnerAcceptance.testSource()
                .getBytes(StandardCharsets.UTF_8));
        assertTrue(test.isFile());

        WorkshopAiProjectTransaction.restore(project, packaged);
        assertFalse(test.exists());
        assertEquals(packagedFingerprint, WorkshopAiProjectTransaction.fingerprint(
                WorkshopAiProjectTransaction.capture(project)));
    }
}
