package com.stasislang.workshop;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import org.junit.Test;

public final class WorkshopGitHubSyncPolicyTest {
    @Test
    public void backupPlanUploadsCurrentFilesAndDeletesOnlyPreviouslyManagedFiles() {
        Map<String, byte[]> local = new LinkedHashMap<>();
        local.put("src/main.stasis", bytes("new"));
        local.put("assets/images/hero.png", bytes("png"));
        Map<String, String> prior = new LinkedHashMap<>();
        prior.put("src/main.stasis", sha('a'));
        prior.put("src/removed.stasis", sha('b'));

        List<WorkshopGitHubSyncPolicy.Change> plan =
                WorkshopGitHubSyncPolicy.backupPlan(local, prior);

        assertEquals(3, plan.size());
        assertEquals("assets/images/hero.png", plan.get(0).path);
        assertArrayEquals(bytes("png"), plan.get(0).content);
        assertEquals("src/main.stasis", plan.get(1).path);
        assertArrayEquals(bytes("new"), plan.get(1).content);
        assertEquals("src/removed.stasis", plan.get(2).path);
        assertNull(plan.get(2).content);
        assertFalse(plan.stream().anyMatch(change -> "README.md".equals(change.path)));
    }

    @Test
    public void remoteStateRoundTripsDeterministically() {
        Map<String, String> state = new LinkedHashMap<>();
        state.put("src/z.stasis", sha('f'));
        state.put("src/a.stasis", sha('1'));

        String encoded = WorkshopGitHubSyncPolicy.encodeRemoteState(state);

        assertEquals("src/a.stasis\t" + sha('1') + "\nsrc/z.stasis\t" + sha('f'), encoded);
        assertEquals(WorkshopGitHubSyncPolicy.decodeRemoteState(encoded),
                WorkshopGitHubSyncPolicy.decodeRemoteState(
                        WorkshopGitHubSyncPolicy.encodeRemoteState(
                                WorkshopGitHubSyncPolicy.decodeRemoteState(encoded))));
    }

    @Test(expected = IllegalArgumentException.class)
    public void remoteStateRejectsTraversal() {
        WorkshopGitHubSyncPolicy.decodeRemoteState("../secret\t" + sha('1'));
    }

    @Test
    public void fingerprintIsOrderIndependentAndContentSensitive() {
        Map<String, byte[]> first = new LinkedHashMap<>();
        first.put("b", bytes("two"));
        first.put("a", bytes("one"));
        Map<String, byte[]> reordered = new LinkedHashMap<>();
        reordered.put("a", bytes("one"));
        reordered.put("b", bytes("two"));
        Map<String, byte[]> changed = new LinkedHashMap<>(reordered);
        changed.put("b", bytes("three"));

        assertEquals(WorkshopGitHubSyncPolicy.fingerprint(first),
                WorkshopGitHubSyncPolicy.fingerprint(reordered));
        assertFalse(WorkshopGitHubSyncPolicy.fingerprint(first)
                .equals(WorkshopGitHubSyncPolicy.fingerprint(changed)));
    }

    @Test
    public void textDiffIncludesChangedAddedAndDeletedFiles() {
        Map<String, String> before = new LinkedHashMap<>();
        before.put("src/deleted.stasis", "old");
        before.put("src/main.stasis", "before");
        Map<String, String> after = new LinkedHashMap<>();
        after.put("src/main.stasis", "after");
        after.put("src/new.stasis", "new");

        Map<String, String> changed = WorkshopGitHubSyncPolicy.changedTextFiles(before, after);

        assertEquals(3, changed.size());
        assertNull(changed.get("src/deleted.stasis"));
        assertEquals("after", changed.get("src/main.stasis"));
        assertEquals("new", changed.get("src/new.stasis"));
    }

    @Test
    public void automaticSchedulingRequiresConsentChangeNetworkAndPower() {
        assertEquals(WorkshopGitHubSyncPolicy.ScheduleDecision.DISABLED,
                WorkshopGitHubSyncPolicy.automaticSchedule(false, true, true, false, false));
        assertEquals(WorkshopGitHubSyncPolicy.ScheduleDecision.UNCHANGED,
                WorkshopGitHubSyncPolicy.automaticSchedule(true, false, true, false, false));
        assertEquals(WorkshopGitHubSyncPolicy.ScheduleDecision.WAIT_FOR_NETWORK,
                WorkshopGitHubSyncPolicy.automaticSchedule(true, true, false, false, false));
        assertEquals(WorkshopGitHubSyncPolicy.ScheduleDecision.DEFER_FOR_BATTERY,
                WorkshopGitHubSyncPolicy.automaticSchedule(true, true, true, true, false));
        assertEquals(WorkshopGitHubSyncPolicy.ScheduleDecision.RUN,
                WorkshopGitHubSyncPolicy.automaticSchedule(true, true, true, true, true));
    }

    @Test
    public void interruptionRecoveryDistinguishesRecreationFromProcessDeath() {
        assertFalse(WorkshopGitHubSyncPolicy.shouldMarkInterrupted("sync", "running", true));
        assertTrue(WorkshopGitHubSyncPolicy.shouldMarkInterrupted("sync", "running", false));
        assertFalse(WorkshopGitHubSyncPolicy.shouldMarkInterrupted("", "running", false));
        assertFalse(WorkshopGitHubSyncPolicy.shouldMarkInterrupted("sync", "complete", false));
    }

    @Test
    public void offlineWorkResumesThroughTheCorrectCurrentGates() {
        assertEquals(WorkshopGitHubSyncPolicy.NetworkResumeDecision.RECHECK_AUTOMATIC_SYNC,
                WorkshopGitHubSyncPolicy.networkResume(
                        "sync", "waiting_network", true, true));
        assertEquals(WorkshopGitHubSyncPolicy.NetworkResumeDecision.RETRY_USER_SYNC,
                WorkshopGitHubSyncPolicy.networkResume(
                        "sync", "waiting_network", true, false));
        assertEquals(WorkshopGitHubSyncPolicy.NetworkResumeDecision.RETRY_PULL_REQUEST,
                WorkshopGitHubSyncPolicy.networkResume(
                        "pull_request", "deferred", true, false));
        assertEquals(WorkshopGitHubSyncPolicy.NetworkResumeDecision.NONE,
                WorkshopGitHubSyncPolicy.networkResume(
                        "sync", "waiting_network", false, true));
        assertEquals(WorkshopGitHubSyncPolicy.NetworkResumeDecision.NONE,
                WorkshopGitHubSyncPolicy.networkResume(
                        "validate", "waiting_network", true, false));
    }

    @Test(expected = IllegalStateException.class)
    public void remoteShaChangeIsAConflict() {
        WorkshopGitHubSyncPolicy.requireNoRemoteConflict(
                "src/main.stasis", sha('a'), sha('b'), false);
    }

    @Test
    public void alreadyAppliedRemoteWriteIsIdempotentAfterInterruption() {
        WorkshopGitHubSyncPolicy.requireNoRemoteConflict(
                "src/main.stasis", sha('a'), sha('b'), true);
    }

    @Test
    public void reviewFingerprintCannotConfuseEmbeddedDelimitersWithDeletion() {
        Map<String, String> embedded = new LinkedHashMap<>();
        embedded.put("a", "x\nb\n<deleted>");
        Map<String, String> deletion = new LinkedHashMap<>();
        deletion.put("a", "x");
        deletion.put("b", null);

        assertFalse(WorkshopGitHubSyncPolicy.reviewFingerprint(embedded)
                .equals(WorkshopGitHubSyncPolicy.reviewFingerprint(deletion)));
    }

    @Test
    public void targetValidationIdentityBindsRepositoryAndBranch() {
        assertEquals(WorkshopGitHubSyncPolicy.targetIdentity("owner/game", "main"),
                WorkshopGitHubSyncPolicy.targetIdentity("owner/game", "main"));
        assertFalse(WorkshopGitHubSyncPolicy.targetIdentity("owner/game", "main")
                .equals(WorkshopGitHubSyncPolicy.targetIdentity("owner/game", "release")));
    }

    @Test
    public void inFlightNetworkLossBecomesRetryable() {
        assertEquals("waiting_network", WorkshopGitHubSyncPolicy.failureState(false));
        assertEquals("error", WorkshopGitHubSyncPolicy.failureState(true));
    }

    private static byte[] bytes(String value) {
        return value.getBytes(StandardCharsets.UTF_8);
    }

    private static String sha(char value) {
        StringBuilder result = new StringBuilder();
        while (result.length() < 40) result.append(value);
        return result.toString();
    }
}
