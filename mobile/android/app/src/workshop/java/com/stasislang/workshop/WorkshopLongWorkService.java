package com.stasislang.workshop;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.os.IBinder;

public final class WorkshopLongWorkService extends Service {
    static final String ACTION_START = "com.stasislang.workshop.action.START";
    static final String ACTION_CANCEL_AI = "com.stasislang.workshop.action.CANCEL_AI";
    static final String EXTRA_KIND = "kind";
    static final String EXTRA_DETAIL = "detail";
    private static final String CHANNEL_ID = "stasis_workshop_long_work";
    private static final int NOTIFICATION_ID = 4101;

    @Override public void onCreate() {
        super.onCreate();
        NotificationManager manager = getSystemService(NotificationManager.class);
        if (manager != null) {
            manager.createNotificationChannel(new NotificationChannel(
                    CHANNEL_ID, "Workshop background work", NotificationManager.IMPORTANCE_LOW));
        }
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null ? "" : intent.getAction();
        if (ACTION_CANCEL_AI.equals(action)) {
            WorkshopLongWorkCoordinator.requestAiCancellation();
            startForeground(NOTIFICATION_ID, notification(
                    WorkshopLongWorkCoordinator.KIND_AI, "Cancellation requested"));
            return START_NOT_STICKY;
        }
        String kind = intent == null ? "" : intent.getStringExtra(EXTRA_KIND);
        String detail = intent == null ? "" : intent.getStringExtra(EXTRA_DETAIL);
        startForeground(NOTIFICATION_ID, notification(kind, detail));
        return START_NOT_STICKY;
    }

    @Override public IBinder onBind(Intent intent) {
        return null;
    }

    private Notification notification(String kind, String detail) {
        Intent open = new Intent(this, MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent openPending = PendingIntent.getActivity(this, 0, open,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Intent cancel = new Intent(this, WorkshopLongWorkService.class).setAction(ACTION_CANCEL_AI);
        PendingIntent cancelPending = PendingIntent.getService(this, 1, cancel,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Notification.Builder builder = new Notification.Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle(notificationTitle(kind))
                .setContentText(detail == null || detail.isEmpty()
                        ? "Working on a queued game change" : detail)
                .setContentIntent(openPending)
                .setOngoing(true)
                .setOnlyAlertOnce(true);
        if (WorkshopLongWorkCoordinator.KIND_AI.equals(kind)) {
            builder.addAction(new Notification.Action.Builder(
                    android.R.drawable.ic_menu_close_clear_cancel, "Stop", cancelPending).build());
        }
        return builder.build();
    }

    private static String notificationTitle(String kind) {
        if (WorkshopLongWorkCoordinator.KIND_GITHUB.equals(kind)) {
            return "Stasis Workshop is syncing GitHub";
        }
        if (WorkshopLongWorkCoordinator.KIND_PROJECT_IO.equals(kind)) {
            return "Stasis Workshop is processing project files";
        }
        return "Stasis Workshop is running AI work";
    }
}
