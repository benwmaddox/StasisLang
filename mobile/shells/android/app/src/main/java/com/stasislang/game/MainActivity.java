package @STASIS_PACKAGE_ID@;

import android.content.res.AssetManager;
import android.os.Bundle;
import org.libsdl.app.SDLActivity;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;

public final class MainActivity extends SDLActivity {
    private static native void nativeSetAssetRoot(String path);

    @Override
    protected void onCreate(Bundle state) {
        System.loadLibrary("SDL2");
        System.loadLibrary("SDL2_image");
        System.loadLibrary("main");
        File root = new File(getFilesDir(), "stasis_game");
        File staging = new File(getFilesDir(), ".stasis_game.staging");
        try {
            deleteTree(staging);
            copyAssetTree(getAssets(), "stasis_game", staging);
            deleteTree(root);
            if (!staging.renameTo(root)) {
                throw new IOException("Unable to publish " + root);
            }
        } catch (IOException error) {
            throw new IllegalStateException("Unable to install bundled Stasis assets", error);
        }
        nativeSetAssetRoot(root.getAbsolutePath());
        super.onCreate(state);
    }

    private static void copyAssetTree(AssetManager assets, String assetPath, File output)
            throws IOException {
        String[] children = assets.list(assetPath);
        if (children != null && children.length > 0) {
            if (!output.isDirectory() && !output.mkdirs()) {
                throw new IOException("Unable to create " + output);
            }
            for (String child : children) {
                copyAssetTree(assets, assetPath + "/" + child, new File(output, child));
            }
            return;
        }
        File parent = output.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("Unable to create " + parent);
        }
        try (InputStream input = assets.open(assetPath);
                FileOutputStream stream = new FileOutputStream(output)) {
            byte[] buffer = new byte[16384];
            int count;
            while ((count = input.read(buffer)) != -1) {
                stream.write(buffer, 0, count);
            }
        }
    }

    private static void deleteTree(File path) throws IOException {
        if (!path.exists()) {
            return;
        }
        File[] children = path.listFiles();
        if (children != null) {
            for (File child : children) {
                deleteTree(child);
            }
        }
        if (!path.delete()) {
            throw new IOException("Unable to remove " + path);
        }
    }

    @Override
    protected String[] getLibraries() {
        return new String[] {"SDL2", "SDL2_image", "main"};
    }
}
