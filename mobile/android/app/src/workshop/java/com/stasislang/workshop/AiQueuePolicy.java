package com.stasislang.workshop;

final class AiQueuePolicy {
    private AiQueuePolicy() {}

    static boolean validProjectId(String projectId) {
        return projectId != null && projectId.matches("[A-Za-z0-9][A-Za-z0-9-]{0,79}");
    }

    static boolean validSource(String source) {
        return "text".equals(source) || "voice".equals(source);
    }

    static boolean validState(String state) {
        return "pending".equals(state) || "in_progress".equals(state)
                || "completed".equals(state) || "failed".equals(state) || "cancelled".equals(state);
    }

    static boolean canTransition(String current, String next) {
        if ("pending".equals(current)) {
            return "in_progress".equals(next) || "cancelled".equals(next);
        }
        if ("in_progress".equals(current)) {
            return "completed".equals(next) || "failed".equals(next) || "cancelled".equals(next);
        }
        return false;
    }

    static boolean terminal(String state) {
        return "completed".equals(state) || "failed".equals(state) || "cancelled".equals(state);
    }

    static int nextPendingIndex(String projectId, String[] projectIds, String[] states) {
        if (projectIds.length != states.length) throw new IllegalArgumentException("queue vectors differ in length");
        for (int index = 0; index < states.length; index += 1) {
            if (projectId.equals(projectIds[index]) && "pending".equals(states[index])) return index;
        }
        return -1;
    }

    static String recoveredState(String state, boolean resumable) {
        return "in_progress".equals(state) ? (resumable ? "pending" : "failed") : state;
    }

    static boolean retryNeedsNewPreview(boolean hadPreview) {
        return hadPreview;
    }
}
