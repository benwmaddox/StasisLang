package com.stasislang.workshop;

import android.util.Log;

import java.io.File;

import org.json.JSONArray;
import org.json.JSONObject;

/** Acceptance-only proof of real Workshop JNI test execution and rollback. */
final class WorkshopTestRunnerAcceptance {
    static final String SCHEMA = "stasis.workshop_test_runner.v1";
    static final String TEST_PATH = "tests/it030_workshop_jni.test.stasis";
    static final String TEST_NAME = "IT-030 Workshop JNI rollback";
    private static final String LOG_TAG = "StasisWorkshop";
    private static final String PACKAGED_REVISION =
            "const IT028_TICK_REVISION: i32 = 1;";
    private static final String ACCEPTED_REVISION =
            "const IT028_TICK_REVISION: i32 = 3;";
    private static final String FAILING_REVISION =
            "const IT028_TICK_REVISION: i32 = 4;";

    private WorkshopTestRunnerAcceptance() {}

    static String run(MainActivity activity, String projectRoot) {
        WorkshopAiProjectTransaction.Snapshot packaged = null;
        String packagedFingerprint = null;
        JSONObject cleanup = new JSONObject();
        try {
            packaged = activity.acceptanceCaptureProject(projectRoot);
            packagedFingerprint = activity.acceptanceProjectFingerprint(packaged);
            String original = activity.acceptanceReadSource(projectRoot);
            String accepted = acceptedSource(original);
            activity.acceptanceReplaceSource(projectRoot, accepted);
            activity.acceptanceWriteTest(projectRoot, TEST_PATH, testSource());
            String acceptedCompile = activity.acceptanceCompile(projectRoot);
            requireCompileReady(acceptedCompile, "accepted revision");
            WorkshopAiProjectTransaction.Snapshot acceptedSnapshot =
                    activity.acceptanceCaptureProject(projectRoot);
            String acceptedSha = activity.acceptanceProjectFingerprint(acceptedSnapshot);
            JSONObject acceptedRuntime = activity.acceptanceRuntimeState(projectRoot);
            JSONObject pass = caseRecord("pass", 1,
                    parseRun(activity.acceptanceRunTests(projectRoot)), acceptedSha,
                    acceptedRuntime, true);
            requireCase(pass, "passed", 0);
            logCase(pass);

            activity.acceptanceReplaceSource(projectRoot, failingSource(accepted));
            String failingCompile = activity.acceptanceCompile(projectRoot);
            requireCompileReady(failingCompile, "test-failing revision");
            String failingSha = activity.acceptanceProjectFingerprint(
                    activity.acceptanceCaptureProject(projectRoot));
            JSONObject failingRuntime = activity.acceptanceRuntimeState(projectRoot);
            JSONObject failure = caseRecord("fail", 2,
                    parseRun(activity.acceptanceRunTests(projectRoot)), failingSha,
                    failingRuntime, true);
            requireCase(failure, "failed", 1);
            logCase(failure);

            activity.acceptanceRestoreProject(projectRoot, acceptedSnapshot);
            String rollbackCompile = activity.acceptanceCompile(projectRoot);
            requireCompileReady(rollbackCompile, "accepted rollback");
            String restoredSha = activity.acceptanceProjectFingerprint(
                    activity.acceptanceCaptureProject(projectRoot));
            JSONObject restoredRuntime = activity.acceptanceRuntimeState(projectRoot);
            requireRollback(acceptedSha, restoredSha, acceptedRuntime, failingRuntime,
                    restoredRuntime);
            JSONObject subsequent = caseRecord("subsequent_pass", 3,
                    parseRun(activity.acceptanceRunTests(projectRoot)), restoredSha,
                    restoredRuntime, true);
            requireCase(subsequent, "passed", 0);
            logCase(subsequent);

            cleanup = restorePackaged(activity, projectRoot, packaged, packagedFingerprint);
            packaged = null;
            JSONObject summary = new JSONObject().put("schema", SCHEMA)
                    .put("test_id", "IT-030").put("event", "test_runner")
                    .put("status", "passed").put("ordered", true)
                    .put("case_count", 3)
                    .put("case_phases", new JSONArray()
                            .put("pass").put("fail").put("subsequent_pass"))
                    .put("transport", "rust_owned_json")
                    .put("accepted_source_sha256", acceptedSha)
                    .put("failing_source_sha256", failingSha)
                    .put("rollback_source_sha256", restoredSha)
                    .put("accepted_runtime", runtimeIdentity(acceptedRuntime))
                    .put("failing_runtime", runtimeIdentity(failingRuntime))
                    .put("rollback_runtime", runtimeIdentity(restoredRuntime))
                    .put("temporary_test", new JSONObject().put("path", TEST_PATH)
                            .put("created", true).put("removed", true))
                    .put("cleanup_receipt", cleanup);
            Log.i(LOG_TAG, "Stasis Workshop IT-030: " + summary);
            return summary.toString();
        } catch (Exception error) {
            if (packaged != null) {
                cleanup = bestEffortRestore(activity, projectRoot, packaged, packagedFingerprint);
            }
            String reason = error.getMessage() == null ? error.getClass().getSimpleName()
                    : error.getMessage();
            JSONObject failed = new JSONObject();
            try {
                failed.put("schema", SCHEMA).put("test_id", "IT-030")
                        .put("event", "test_runner").put("status", "failed")
                        .put("error", reason).put("cleanup_receipt", cleanup);
            } catch (Exception ignored) {
                // Scalar JSONObject construction cannot fail.
            }
            Log.e(LOG_TAG, "Stasis Workshop IT-030: " + failed);
            return failed.toString();
        }
    }

    static String acceptedSource(String source) {
        int first = source.indexOf(PACKAGED_REVISION);
        if (first < 0 || first != source.lastIndexOf(PACKAGED_REVISION)) {
            throw new IllegalStateException("IT-030 packaged revision tag is not unique");
        }
        return source.replace(PACKAGED_REVISION, ACCEPTED_REVISION);
    }

    static String failingSource(String accepted) {
        int first = accepted.indexOf(ACCEPTED_REVISION);
        if (first < 0 || first != accepted.lastIndexOf(ACCEPTED_REVISION)) {
            throw new IllegalStateException("IT-030 accepted revision tag is not unique");
        }
        return accepted.replace(ACCEPTED_REVISION, FAILING_REVISION);
    }

    static String testSource() {
        return "import \"../src/main.stasis\";\n\n"
                + "test `" + TEST_NAME + "`(): bool {\n"
                + "    return IT028_TICK_REVISION == 3;\n}\n";
    }

    static JSONObject parseRun(String raw) throws Exception {
        JSONObject run = new JSONObject(raw);
        if (!"stasis_test_run".equals(run.optString("kind"))) {
            throw new IllegalStateException("nativeRunTests returned the wrong result kind");
        }
        JSONArray results = run.optJSONArray("results");
        if (results == null || run.optInt("passed", -1) + run.optInt("failed", -1)
                != results.length()) {
            throw new IllegalStateException("nativeRunTests counts do not match results");
        }
        return run;
    }

    static JSONObject validateNamedResultForTest(JSONObject run) throws Exception {
        JSONObject value = caseRecord("pass", 1, run,
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                new JSONObject().put("source", "live_session")
                        .put("source_fingerprint", "runtime-source").put("generation", 1), true);
        requireCase(value, "passed", 0);
        return value.getJSONObject("result");
    }

    private static JSONObject caseRecord(String phase, int sequence, JSONObject run,
            String sourceSha, JSONObject runtime, boolean testExists) throws Exception {
        JSONObject result = namedResult(run);
        return new JSONObject().put("schema", SCHEMA).put("test_id", "IT-030")
                .put("event", "case").put("phase", phase).put("sequence", sequence)
                .put("status", "passed").put("passed", run.getInt("passed"))
                .put("failed", run.getInt("failed"))
                .put("all_passed", run.getBoolean("all_passed"))
                .put("result", result).put("source_sha256", sourceSha)
                .put("runtime", runtimeIdentity(runtime))
                .put("test_file", new JSONObject().put("path", TEST_PATH)
                        .put("exists", testExists));
    }

    private static JSONObject namedResult(JSONObject run) throws Exception {
        JSONArray results = run.getJSONArray("results");
        for (int index = 0; index < results.length(); index += 1) {
            JSONObject result = results.getJSONObject(index);
            if (TEST_NAME.equals(result.optString("name"))) return result;
        }
        throw new IllegalStateException("nativeRunTests omitted IT-030 result");
    }

    private static void requireCase(JSONObject value, String resultStatus,
            int expectedFailed) throws Exception {
        JSONObject result = value.getJSONObject("result");
        if (value.getInt("failed") != expectedFailed
                || value.getBoolean("all_passed") != (expectedFailed == 0)
                || !TEST_PATH.equals(result.optString("file"))
                || result.optInt("line", 0) != 3 || result.optInt("column", 0) != 1
                || !TEST_NAME.equals(result.optString("name"))
                || !resultStatus.equals(result.optString("status"))
                || result.optBoolean("passed") != "passed".equals(resultStatus)) {
            throw new IllegalStateException("IT-030 result lost counts or source location: " + value);
        }
    }

    private static void requireRollback(String acceptedSha, String restoredSha,
            JSONObject accepted, JSONObject failing, JSONObject restored) throws Exception {
        JSONObject acceptedId = runtimeIdentity(accepted);
        JSONObject failingId = runtimeIdentity(failing);
        JSONObject restoredId = runtimeIdentity(restored);
        if (!acceptedSha.equals(restoredSha)
                || acceptedId.getString("fingerprint").equals(failingId.getString("fingerprint"))
                || !acceptedId.getString("fingerprint").equals(
                        restoredId.getString("fingerprint"))
                || failingId.getLong("generation") != acceptedId.getLong("generation") + 1
                || restoredId.getLong("generation") != failingId.getLong("generation") + 1) {
            throw new IllegalStateException("IT-030 rollback did not restore accepted runtime");
        }
    }

    private static JSONObject runtimeIdentity(JSONObject runtime) throws Exception {
        String fingerprint = runtime.optString("source_fingerprint", "");
        long generation = runtime.optLong("generation", 0);
        if (!"live_session".equals(runtime.optString("source"))
                || fingerprint.isEmpty() || generation <= 0) {
            throw new IllegalStateException("IT-030 runtime identity is incomplete: " + runtime);
        }
        return new JSONObject().put("fingerprint", fingerprint).put("generation", generation);
    }

    private static JSONObject restorePackaged(MainActivity activity, String projectRoot,
            WorkshopAiProjectTransaction.Snapshot packaged, String packagedFingerprint)
            throws Exception {
        activity.acceptanceRestoreProject(projectRoot, packaged);
        String actual = activity.acceptanceProjectFingerprint(
                activity.acceptanceCaptureProject(projectRoot));
        String compile = activity.acceptanceCompile(projectRoot);
        requireCompileReady(compile, "packaged cleanup");
        JSONObject runtime = activity.acceptanceRuntimeState(projectRoot);
        boolean removed = !new File(projectRoot, TEST_PATH).exists();
        if (!packagedFingerprint.equals(actual) || !removed) {
            throw new IllegalStateException("IT-030 packaged cleanup was not exact");
        }
        return new JSONObject().put("status", "Restored")
                .put("packaged_source_sha256", actual).put("test_removed", true)
                .put("compile", compile).put("runtime", runtimeIdentity(runtime));
    }

    private static JSONObject bestEffortRestore(MainActivity activity, String projectRoot,
            WorkshopAiProjectTransaction.Snapshot packaged, String packagedFingerprint) {
        try {
            return restorePackaged(activity, projectRoot, packaged, packagedFingerprint);
        } catch (Exception error) {
            try {
                return new JSONObject().put("status", "failed").put("error",
                        error.getMessage() == null ? error.getClass().getSimpleName()
                                : error.getMessage());
            } catch (Exception ignored) {
                return new JSONObject();
            }
        }
    }

    private static void logCase(JSONObject value) {
        Log.i(LOG_TAG, "Stasis Workshop IT-030 case: " + value);
    }

    private static void requireCompileReady(String value, String phase) {
        if (value == null || !value.startsWith("CompileReady") || !value.contains("status=0")) {
            throw new IllegalStateException(phase + " did not compile: " + value);
        }
    }
}
