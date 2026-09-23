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
        Notification.Builder builder = new Notification.Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle(notificationTitle(kind))
                .setContentText(detail == null || detail.isEmpty()
                        ? "Processing project files" : detail)
                .setContentIntent(openPending)
                .setOngoing(true)
                .setOnlyAlertOnce(true);
        return builder.build();
    }

    private static String notificationTitle(String kind) {
        if (WorkshopLongWorkCoordinator.KIND_GITHUB.equals(kind)) {
            return "Stasis Workshop is syncing GitHub";
        }
        if (WorkshopLongWorkCoordinator.KIND_PROJECT_IO.equals(kind)) {
            return "Stasis Workshop is processing project files";
        }
        return "Stasis Workshop is processing files";
    }
}
