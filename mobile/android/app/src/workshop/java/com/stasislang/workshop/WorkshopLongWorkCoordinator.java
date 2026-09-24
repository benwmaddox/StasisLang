package com.stasislang.workshop;

import android.content.Context;
import android.content.Intent;
import android.os.Build;

final class WorkshopLongWorkCoordinator {
    static final String KIND_GITHUB = "github";
    static final String KIND_PROJECT_IO = "project_io";
    private static final Object LOCK = new Object();
    private static String activeKind = "";

    private WorkshopLongWorkCoordinator() {}

    static boolean beginGitHub(Context context, String detail) {
        return begin(context, KIND_GITHUB, detail);
    }

    static boolean beginProjectIo(Context context, String detail) {
        return begin(context, KIND_PROJECT_IO, detail);
    }

    private static boolean begin(Context context, String kind, String detail) {
        synchronized (LOCK) {
            if (!activeKind.isEmpty()) return false;
            activeKind = kind;
        }
        Intent intent = new Intent(context, WorkshopLongWorkService.class)
                .setAction(WorkshopLongWorkService.ACTION_START)
                .putExtra(WorkshopLongWorkService.EXTRA_KIND, kind)
                .putExtra(WorkshopLongWorkService.EXTRA_DETAIL, boundedDetail(detail));
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent);
            } else {
                context.startService(intent);
            }
            return true;
        } catch (RuntimeException error) {
            synchronized (LOCK) {
                activeKind = "";
            }
            return false;
        }
    }

    static void finishGitHub(Context context) {
        finish(context, KIND_GITHUB);
    }

    static void finishProjectIo(Context context) {
        finish(context, KIND_PROJECT_IO);
    }

    private static void finish(Context context, String kind) {
        synchronized (LOCK) {
            if (!kind.equals(activeKind)) return;
            activeKind = "";
        }
        context.stopService(new Intent(context, WorkshopLongWorkService.class));
    }

    static boolean isGitHubActive() {
        return isActive(KIND_GITHUB);
    }

    static boolean isProjectIoActive() {
        return isActive(KIND_PROJECT_IO);
    }

    static boolean isAnyActive() {
        synchronized (LOCK) {
            return !activeKind.isEmpty();
        }
    }

    private static boolean isActive(String kind) {
        synchronized (LOCK) {
            return kind.equals(activeKind);
        }
    }

    private static String boundedDetail(String detail) {
        if (detail == null || detail.trim().isEmpty()) return "Processing project files";
        String normalized = detail.replace('\n', ' ').replace('\r', ' ').trim();
        return normalized.length() <= 120 ? normalized : normalized.substring(0, 120);
    }
}
