package com.stasislang.workshop;

import android.content.Context;
import android.content.Intent;
import android.os.Build;

final class WorkshopLongWorkCoordinator {
    private static final Object LOCK = new Object();
    private static boolean aiActive;
    private static Runnable aiCancel;

    private WorkshopLongWorkCoordinator() {}

    static boolean beginAi(Context context, String detail, Runnable cancel) {
        synchronized (LOCK) {
            if (aiActive) return false;
            aiActive = true;
            aiCancel = cancel;
        }
        Intent intent = new Intent(context, WorkshopLongWorkService.class)
                .setAction(WorkshopLongWorkService.ACTION_START_AI)
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
                aiActive = false;
                aiCancel = null;
            }
            return false;
        }
    }

    static void finishAi(Context context) {
        synchronized (LOCK) {
            aiActive = false;
            aiCancel = null;
        }
        context.stopService(new Intent(context, WorkshopLongWorkService.class));
    }

    static boolean isAiActive() {
        synchronized (LOCK) {
            return aiActive;
        }
    }

    static void requestAiCancellation() {
        Runnable cancel;
        synchronized (LOCK) {
            cancel = aiCancel;
        }
        if (cancel != null) cancel.run();
    }

    private static String boundedDetail(String detail) {
        if (detail == null || detail.trim().isEmpty()) return "Working on a queued game change";
        String normalized = detail.replace('\n', ' ').replace('\r', ' ').trim();
        return normalized.length() <= 120 ? normalized : normalized.substring(0, 120);
    }
}
