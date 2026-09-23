package @STASIS_PACKAGE_ID@;

import android.app.Activity;
import android.app.AlertDialog;
import android.graphics.Color;
import android.os.Bundle;
import android.widget.TextView;

/** Keeps the terminal native replay receipt visible after SDL closes the game activity. */
public final class ReplayResultActivity extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        String receipt = getIntent().getStringExtra(MainActivity.REPLAY_RESULT_EXTRA);
        if (receipt == null || receipt.isEmpty()) {
            finish();
            return;
        }

        boolean complete = MainActivity.isVerifiedReplayCompletionReceipt(receipt);
        TextView message = new TextView(this);
        message.setText(MainActivity.formatReplayReceiptForDisplay(receipt));
        message.setTextColor(complete ? Color.rgb(20, 100, 50) : Color.rgb(145, 25, 25));
        message.setTextSize(16.0f);
        message.setPadding(24, 12, 24, 12);
        message.setTextIsSelectable(true);
        new AlertDialog.Builder(this)
                .setTitle(complete ? "Replay complete" : "Replay failed")
                .setView(message)
                .setPositiveButton("Close", (dialog, which) -> finish())
                .setOnCancelListener(dialog -> finish())
                .show();
    }
}
