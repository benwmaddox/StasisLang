package com.stasislang.workshop;

import android.app.Activity;
import android.os.Bundle;
import android.view.Gravity;
import android.widget.TextView;

public final class MainActivity extends Activity {
    static {
        System.loadLibrary("stasis_mobile_smoke");
    }

    private static native String nativeStatus();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        TextView status = new TextView(this);
        status.setGravity(Gravity.CENTER);
        status.setTextSize(20.0f);
        status.setText(nativeStatus());
        setContentView(status);
    }
}