package com.stasislang.workshop;

import java.io.File;

final class WorkshopBlockingErrorPolicy {
    private static final int MAX_SUMMARY_LENGTH = 1200;

    private WorkshopBlockingErrorPolicy() {}

    static boolean shouldShow(boolean gameRunning, String status) {
        return !gameRunning && status != null
                && (status.startsWith("CompileError") || status.startsWith("RunError"));
    }

    static String summary(String status, String projectRoot) {
        if (status == null) return "Unknown game error";
        int metadata = status.indexOf('|');
        String summary = status.substring(0, metadata < 0 ? status.length() : metadata);
        if (projectRoot != null && !projectRoot.isEmpty()) {
            summary = summary.replace(projectRoot + File.separator, "")
                    .replace(projectRoot.replace('\\', '/') + "/", "");
            String projectMarker = "/files/" + new File(projectRoot).getName() + "/";
            int marker = summary.indexOf(projectMarker);
            int prefix = summary.indexOf(": ");
            if (marker >= 0 && prefix >= 0 && prefix + 2 < marker) {
                summary = summary.substring(0, prefix + 2)
                        + summary.substring(marker + projectMarker.length());
            }
        }
        WorkshopSourceDiagnostic diagnostic = WorkshopSourceDiagnostic.fromCompileResult(status);
        if (diagnostic != null && diagnostic.line > 0) {
            summary += "\n\n" + diagnostic.file + ":" + diagnostic.line
                    + (diagnostic.column > 0 ? ":" + diagnostic.column : "");
        }
        return summary.length() <= MAX_SUMMARY_LENGTH
                ? summary : summary.substring(0, MAX_SUMMARY_LENGTH - 3) + "...";
    }
}
