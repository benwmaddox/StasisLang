package com.stasislang.workshop;

import android.content.Context;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.os.Handler;
import android.os.Looper;

final class AndroidAudioFocus {
    interface Listener {
        void onFocusChanged(boolean focused);
    }

    private final AudioManager manager;
    private final AudioFocusRequest request;
    private final Listener listener;
    private boolean requested;

    AndroidAudioFocus(Context context, Listener listener) {
        this.listener = listener;
        manager = (AudioManager)context.getSystemService(Context.AUDIO_SERVICE);
        AudioAttributes attributes = new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_GAME)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build();
        request = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(attributes)
                .setAcceptsDelayedFocusGain(false)
                .setOnAudioFocusChangeListener(this::handleChange,
                        new Handler(Looper.getMainLooper()))
                .build();
    }

    void resume() {
        if (requested || manager == null) return;
        requested = manager.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
        listener.onFocusChanged(requested);
    }

    void pause() {
        if (manager != null && requested) manager.abandonAudioFocusRequest(request);
        requested = false;
        listener.onFocusChanged(false);
    }

    private void handleChange(int change) {
        if (change == AudioManager.AUDIOFOCUS_LOSS) requested = false;
        listener.onFocusChanged(requested && change == AudioManager.AUDIOFOCUS_GAIN);
    }
}
