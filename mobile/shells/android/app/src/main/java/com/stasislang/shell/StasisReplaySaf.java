package com.stasislang.shell;

import android.content.ContentResolver;
import android.net.Uri;
import android.os.ParcelFileDescriptor;

import java.io.File;
import java.io.FileDescriptor;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/** Bounded replay import/export for the packaged native runtime. */
public final class StasisReplaySaf {
    public static final long MAX_REPLAY_BYTES = 256L * 1024L * 1024L;
    private static final int COPY_BUFFER_BYTES = 64 * 1024;

    private StasisReplaySaf() {}

    public static String boundDiagnostic(IOException error) {
        String detail = error == null || error.getMessage() == null
                ? "unknown replay storage failure" : error.getMessage();
        if (detail.length() > 320) detail = detail.substring(0, 320);
        return "code=replay_saf_failure path=stasis_replay.json detail=" + detail;
    }

    /**
     * Copies a user-selected SAF document into app-private storage and publishes
     * it with a same-directory rename. Native startup only receives the published
     * path after this method returns.
     */
    public static File importReplay(
            ContentResolver resolver, Uri source, File filesDir) throws IOException {
        if (resolver == null || source == null || filesDir == null) {
            throw new IOException("replay import requires a resolver, URI, and files directory");
        }
        if (!filesDir.isDirectory() && !filesDir.mkdirs() && !filesDir.isDirectory()) {
            throw new IOException("replay import directory is unavailable");
        }
        File destination = new File(filesDir, "stasis_replay.json");
        File staging = new File(filesDir,
                ".stasis_replay.json.part." + Long.toHexString(System.nanoTime()));
        try (InputStream input = resolver.openInputStream(source)) {
            if (input == null) throw new IOException("SAF replay document could not be opened");
            copyBounded(input, staging, MAX_REPLAY_BYTES);
        } catch (IOException error) {
            deleteQuietly(staging);
            throw error;
        }
        publishAtomically(staging, destination);
        return destination;
    }

    /**
     * Copies a previously published replay to a SAF destination with the same
     * bounded byte contract. The provider owns the final document transaction.
     */
    public static void exportReplay(
            ContentResolver resolver, Uri destination, File source) throws IOException {
        if (resolver == null || destination == null || source == null || !source.isFile()) {
            throw new IOException("replay export source or destination is unavailable");
        }
        long length = source.length();
        if (length <= 0L || length > MAX_REPLAY_BYTES) {
            throw new IOException("replay export exceeds its bounded file limit");
        }
        ParcelFileDescriptor descriptor = resolver.openFileDescriptor(destination, "w");
        if (descriptor == null) throw new IOException("SAF replay destination could not be opened");
        try (InputStream input = new FileInputStream(source);
             ParcelFileDescriptor.AutoCloseOutputStream output =
                     new ParcelFileDescriptor.AutoCloseOutputStream(descriptor)) {
            copyBounded(input, output, MAX_REPLAY_BYTES);
            output.flush();
            FileDescriptor file = output.getFD();
            file.sync();
        }
    }

    private static void copyBounded(InputStream input, File destination, long maximum)
            throws IOException {
        try (FileOutputStream output = new FileOutputStream(destination)) {
            copyBounded(input, output, maximum);
            output.flush();
            output.getFD().sync();
        }
    }

    private static void copyBounded(InputStream input, OutputStream output, long maximum)
            throws IOException {
        byte[] buffer = new byte[COPY_BUFFER_BYTES];
        long total = 0L;
        while (true) {
            int read = input.read(buffer);
            if (read < 0) break;
            if (read == 0) continue;
            total += read;
            if (total > maximum) throw new IOException("replay document exceeds its bounded file limit");
            output.write(buffer, 0, read);
        }
        if (total == 0L) throw new IOException("replay document is empty");
    }

    private static void publishAtomically(File staging, File destination) throws IOException {
        File backup = new File(destination.getParentFile(),
                ".stasis_replay.json.previous." + Long.toHexString(System.nanoTime()));
        boolean hadPrevious = destination.isFile();
        if (hadPrevious && !destination.renameTo(backup)) {
            deleteQuietly(staging);
            throw new IOException("replay import could not stage the previous document");
        }
        if (!staging.renameTo(destination)) {
            if (hadPrevious) backup.renameTo(destination);
            deleteQuietly(staging);
            throw new IOException("replay import could not publish the document");
        }
        if (hadPrevious) deleteQuietly(backup);
    }

    private static void deleteQuietly(File file) {
        if (file != null && file.exists()) file.delete();
    }
}
