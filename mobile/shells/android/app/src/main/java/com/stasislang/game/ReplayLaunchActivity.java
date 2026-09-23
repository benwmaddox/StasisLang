package @STASIS_PACKAGE_ID@;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.widget.Toast;

/** Offers replay selection before the native game activity starts. */
public final class ReplayLaunchActivity extends Activity {
    private static final int REQUEST_REPLAY_IMPORT = 4801;
    private static final String REPLAY_URI_EXTRA = "stasis.replay_uri";

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        showStartOptions();
    }

    private void showStartOptions() {
        new AlertDialog.Builder(this)
                .setTitle(getApplicationInfo().loadLabel(getPackageManager()))
                .setMessage("Choose how to start the game.")
                .setPositiveButton("Play", (dialog, which) -> startGame(null, 0))
                .setNeutralButton("Import replay", (dialog, which) -> openReplayPicker())
                .setNegativeButton("Exit", (dialog, which) -> finish())
                .setOnCancelListener(dialog -> finish())
                .show();
    }

    private void openReplayPicker() {
        Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        picker.addCategory(Intent.CATEGORY_OPENABLE);
        picker.setType("*/*");
        picker.putExtra(Intent.EXTRA_MIME_TYPES,
                new String[] {"application/json", "application/octet-stream"});
        try {
            startActivityForResult(picker, REQUEST_REPLAY_IMPORT);
        } catch (ActivityNotFoundException | SecurityException error) {
            Toast.makeText(this, "Replay import is unavailable", Toast.LENGTH_LONG).show();
            showStartOptions();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_REPLAY_IMPORT) return;
        if (resultCode == RESULT_OK && data != null && data.getData() != null) {
            startGame(data.getData(), data.getFlags());
        } else {
            showStartOptions();
        }
    }

    private void startGame(Uri replay, int grantFlags) {
        Intent game = new Intent(this, MainActivity.class);
        if (replay != null) {
            game.setAction(Intent.ACTION_VIEW);
            game.setData(replay);
            game.putExtra(REPLAY_URI_EXTRA, replay.toString());
            game.addFlags(grantFlags & Intent.FLAG_GRANT_READ_URI_PERMISSION);
        }
        try {
            startActivity(game);
            finish();
        } catch (ActivityNotFoundException | SecurityException error) {
            Toast.makeText(this, "The game could not be started", Toast.LENGTH_LONG).show();
            showStartOptions();
        }
    }
}
