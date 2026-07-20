package com.stasislang.workshop;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;

final class WorkshopGitHubSyncPolicy {
    enum ScheduleDecision {
        DISABLED,
        UNCHANGED,
        WAIT_FOR_NETWORK,
        DEFER_FOR_BATTERY,
        RUN
    }

    static final class Change {
        final String path;
        final byte[] content;

        Change(String path, byte[] content) {
            this.path = path;
            this.content = content;
        }

        boolean deletesRemoteFile() {
            return content == null;
        }
    }

    private WorkshopGitHubSyncPolicy() {}

    static List<Change> backupPlan(Map<String, byte[]> localFiles,
            Map<String, String> priorRemoteState) {
        validatePaths(localFiles.keySet());
        validatePaths(priorRemoteState.keySet());
        TreeSet<String> paths = new TreeSet<>();
        paths.addAll(localFiles.keySet());
        paths.addAll(priorRemoteState.keySet());
        ArrayList<Change> changes = new ArrayList<>();
        for (String path : paths) {
            byte[] content = localFiles.get(path);
            if (content != null) changes.add(new Change(path, Arrays.copyOf(content, content.length)));
            else if (priorRemoteState.containsKey(path)) changes.add(new Change(path, null));
        }
        return Collections.unmodifiableList(changes);
    }

    static Map<String, String> changedTextFiles(Map<String, String> before,
            Map<String, String> after) {
        validatePaths(before.keySet());
        validatePaths(after.keySet());
        LinkedHashMap<String, String> changed = new LinkedHashMap<>();
        TreeSet<String> paths = new TreeSet<>();
        paths.addAll(before.keySet());
        paths.addAll(after.keySet());
        for (String path : paths) {
            String oldContent = before.get(path);
            String newContent = after.get(path);
            if (!java.util.Objects.equals(oldContent, newContent)) changed.put(path, newContent);
        }
        return changed;
    }

    static ScheduleDecision automaticSchedule(boolean enabled, boolean changed,
            boolean hasUsableNetwork, boolean batterySaverEnabled, boolean charging) {
        if (!enabled) return ScheduleDecision.DISABLED;
        if (!changed) return ScheduleDecision.UNCHANGED;
        WorkshopBackgroundWorkPolicy.Decision background = WorkshopBackgroundWorkPolicy.decide(
                false, hasUsableNetwork, batterySaverEnabled, charging);
        if (background == WorkshopBackgroundWorkPolicy.Decision.WAIT_FOR_NETWORK) {
            return ScheduleDecision.WAIT_FOR_NETWORK;
        }
        if (background == WorkshopBackgroundWorkPolicy.Decision.DEFER_FOR_BATTERY) {
            return ScheduleDecision.DEFER_FOR_BATTERY;
        }
        return ScheduleDecision.RUN;
    }

    static boolean shouldMarkInterrupted(String operation, String state,
            boolean inProcessOperationActive) {
        return operation != null && !operation.isEmpty()
                && ("queued".equals(state) || "running".equals(state))
                && !inProcessOperationActive;
    }

    static boolean shouldResumeAfterNetwork(String operation, String state,
            boolean hasUsableNetwork) {
        return hasUsableNetwork && ("sync".equals(operation) || "pull_request".equals(operation))
                && ("waiting_network".equals(state) || "deferred".equals(state));
    }

    static void requireNoRemoteConflict(String path, String expectedRemoteSha,
            String actualRemoteSha, boolean desiredContentAlreadyPresent) {
        if (desiredContentAlreadyPresent || expectedRemoteSha == null || expectedRemoteSha.isEmpty()) return;
        if (!expectedRemoteSha.equals(actualRemoteSha)) {
            throw new IllegalStateException(
                    "conflict: " + path + " changed remotely since the last backup");
        }
    }

    static String failureState(boolean hasUsableNetwork) {
        return hasUsableNetwork ? "error" : "waiting_network";
    }

    static String targetIdentity(String repository, String branch) {
        LinkedHashMap<String, byte[]> target = new LinkedHashMap<>();
        target.put("repository", repository.getBytes(StandardCharsets.UTF_8));
        target.put("branch", branch.getBytes(StandardCharsets.UTF_8));
        return fingerprint(target);
    }

    static String reviewFingerprint(Map<String, String> changes) {
        validatePaths(changes.keySet());
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            for (String path : new TreeSet<String>(changes.keySet())) {
                byte[] pathBytes = path.getBytes(StandardCharsets.UTF_8);
                String content = changes.get(path);
                digest.update(intBytes(pathBytes.length));
                digest.update(pathBytes);
                digest.update((byte)(content == null ? 0 : 1));
                if (content != null) {
                    byte[] contentBytes = content.getBytes(StandardCharsets.UTF_8);
                    digest.update(intBytes(contentBytes.length));
                    digest.update(contentBytes);
                }
            }
            return hex(digest.digest());
        } catch (Exception unavailable) {
            throw new IllegalStateException("SHA-256 unavailable", unavailable);
        }
    }

    static String fingerprint(Map<String, byte[]> files) {
        validatePaths(files.keySet());
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            for (String path : new TreeSet<String>(files.keySet())) {
                byte[] pathBytes = path.getBytes(StandardCharsets.UTF_8);
                byte[] content = files.get(path);
                digest.update(intBytes(pathBytes.length));
                digest.update(pathBytes);
                digest.update(intBytes(content.length));
                digest.update(content);
            }
            return hex(digest.digest());
        } catch (Exception unavailable) {
            throw new IllegalStateException("SHA-256 unavailable", unavailable);
        }
    }

    static String encodeRemoteState(Map<String, String> remoteState) {
        validatePaths(remoteState.keySet());
        StringBuilder encoded = new StringBuilder();
        for (String path : new TreeSet<String>(remoteState.keySet())) {
            String sha = remoteState.get(path);
            if (sha == null || !sha.matches("[0-9a-fA-F]{40,64}")) {
                throw new IllegalArgumentException("invalid remote SHA for " + path);
            }
            if (encoded.length() > 0) encoded.append('\n');
            encoded.append(path).append('\t').append(sha.toLowerCase());
        }
        return encoded.toString();
    }

    static Map<String, String> decodeRemoteState(String encoded) {
        LinkedHashMap<String, String> state = new LinkedHashMap<>();
        if (encoded == null || encoded.isEmpty()) return state;
        for (String line : encoded.split("\\n", -1)) {
            int separator = line.lastIndexOf('\t');
            if (separator <= 0 || separator == line.length() - 1) {
                throw new IllegalArgumentException("invalid GitHub remote state");
            }
            String path = line.substring(0, separator);
            String sha = line.substring(separator + 1);
            validatePath(path);
            if (!sha.matches("[0-9a-fA-F]{40,64}") || state.put(path, sha.toLowerCase()) != null) {
                throw new IllegalArgumentException("invalid GitHub remote state entry");
            }
        }
        return state;
    }

    private static void validatePaths(Iterable<String> paths) {
        for (String path : paths) validatePath(path);
    }

    private static void validatePath(String path) {
        if (path == null || path.isEmpty() || path.startsWith("/") || path.startsWith("\\")
                || path.contains("\\") || path.contains("\n") || path.contains("\r")
                || path.contains("\t")) {
            throw new IllegalArgumentException("invalid project path");
        }
        for (String part : path.split("/", -1)) {
            if (part.isEmpty() || ".".equals(part) || "..".equals(part)) {
                throw new IllegalArgumentException("invalid project path");
            }
        }
    }

    private static byte[] intBytes(int value) {
        return new byte[] {
                (byte)(value >>> 24), (byte)(value >>> 16),
                (byte)(value >>> 8), (byte)value
        };
    }

    private static String hex(byte[] bytes) {
        char[] digits = "0123456789abcdef".toCharArray();
        StringBuilder value = new StringBuilder(bytes.length * 2);
        for (byte item : bytes) {
            int unsigned = item & 0xff;
            value.append(digits[unsigned >>> 4]).append(digits[unsigned & 0x0f]);
        }
        return value.toString();
    }
}
