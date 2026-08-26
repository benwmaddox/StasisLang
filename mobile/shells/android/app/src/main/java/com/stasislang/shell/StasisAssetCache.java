package com.stasislang.shell;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.Base64;

/** App-private, verified extraction cache for the packaged release asset tree. */
public final class StasisAssetCache {
    public static final String CACHE_SCHEMA = "stasis.android.asset-cache";
    public static final int CACHE_VERSION = 1;
    public static final int MAX_MANIFEST_BYTES = 1024 * 1024;
    public static final int MAX_MARKER_BYTES = 512 * 1024;
    public static final int MAX_MANIFEST_ASSETS = 4096;
    public static final int MAX_TREE_FILES = 8192;
    public static final long MAX_ASSET_BYTES = 128L * 1024L * 1024L;
    public static final long MAX_TOTAL_ASSET_BYTES = 150L * 1024L * 1024L;
    public static final long MAX_COPIED_TREE_BYTES = 150L * 1024L * 1024L;
    public static final int MAX_DIAGNOSTIC_BYTES = 384;
    private static final int MAX_DIAGNOSTIC_CODE_BYTES = 64;
    private static final int MAX_DIAGNOSTIC_PATH_BYTES = 160;

    /** Stable IT-022 failure identifiers shared by Java, native, and seam logs. */
    public static final String ERROR_MALFORMED_MANIFEST = "malformed_manifest";
    public static final String ERROR_MISSING_ASSET = "missing_asset";
    public static final String ERROR_TAMPERED_ASSET = "tampered_asset";
    public static final String ERROR_TRAVERSAL_PATH = "traversal_path";
    public static final String ERROR_DUPLICATE_ASSET = "duplicate_asset";
    public static final String ERROR_OVERSIZED_ASSET = "oversized_asset";
    public static final String ERROR_INVALID_TREE = "invalid_asset_tree";

    private static final String PACKAGED_ROOT = "stasis_game";
    private static final String MANIFEST_PATH = "assets/manifest.json";
    private static final String MARKER_NAME = ".stasis_asset_cache.v1";
    private static final String STAGING_NAME = ".stasis_game.staging";
    private static final String BACKUP_NAME = ".stasis_game.previous";
    private static final String BACKUP_ALT_NAME = ".stasis_game.previous.2";

    public interface AssetSource {
        String[] list(String path) throws IOException;

        InputStream open(String path) throws IOException;
    }

    /** A bounded, machine-readable preparation failure with a stable asset path. */
    public static final class VerificationException extends IOException {
        private final String code;
        private final String path;

        VerificationException(String code, String path, String detail, Throwable cause) {
            super(formatDiagnostic(code, path, detail), cause);
            this.code = code;
            this.path = path;
        }

        public String getCode() {
            return code;
        }

        public String getPath() {
            return path;
        }

        public String getDiagnostic() {
            return getMessage();
        }
    }

    private static String formatDiagnostic(String code, String path, String detail) {
        String safeCode = code == null || code.isEmpty() ? "unknown" : code;
        String safePath = path == null || path.isEmpty() ? MANIFEST_PATH : path;
        String safeDetail = detail == null || detail.isEmpty() ? "rejected" : detail;
        String boundedCode = truncateUtf8(safeCode, MAX_DIAGNOSTIC_CODE_BYTES);
        String boundedPath = truncateUtf8(safePath, MAX_DIAGNOSTIC_PATH_BYTES);
        int grammarBytes = "code= path= detail=".getBytes(StandardCharsets.UTF_8).length;
        int detailBudget = Math.max(0, MAX_DIAGNOSTIC_BYTES - grammarBytes
                - boundedCode.getBytes(StandardCharsets.UTF_8).length
                - boundedPath.getBytes(StandardCharsets.UTF_8).length);
        String boundedDetail = truncateUtf8(safeDetail, detailBudget);
        return "code=" + boundedCode + " path=" + boundedPath + " detail=" + boundedDetail;
    }

    /** Keep externally forwarded diagnostics bounded without dropping the field grammar. */
    public static String boundDiagnostic(String diagnostic) {
        if (diagnostic == null || diagnostic.getBytes(StandardCharsets.UTF_8).length
                <= MAX_DIAGNOSTIC_BYTES) {
            return diagnostic;
        }
        int pathMarker = diagnostic.indexOf(" path=");
        int detailMarker = pathMarker < 0 ? -1 : diagnostic.indexOf(" detail=", pathMarker);
        if (diagnostic.startsWith("code=") && pathMarker >= 0 && detailMarker >= 0) {
            return formatDiagnostic(
                    diagnostic.substring("code=".length(), pathMarker),
                    diagnostic.substring(pathMarker + " path=".length(), detailMarker),
                    diagnostic.substring(detailMarker + " detail=".length()));
        }
        return truncateUtf8(diagnostic, MAX_DIAGNOSTIC_BYTES);
    }

    private static String truncateUtf8(String value, int maxBytes) {
        if (maxBytes <= 0 || value.isEmpty()) return "";
        StringBuilder bounded = new StringBuilder();
        int usedBytes = 0;
        for (int index = 0; index < value.length();) {
            int codePoint = value.codePointAt(index);
            String character = new String(Character.toChars(codePoint));
            int characterBytes = character.getBytes(StandardCharsets.UTF_8).length;
            if (usedBytes + characterBytes > maxBytes) break;
            bounded.append(character);
            usedBytes += characterBytes;
            index += Character.charCount(codePoint);
        }
        return bounded.toString();
    }

    private static VerificationException verificationFailure(
            String code, String path, String detail) {
        return new VerificationException(code, path, detail, null);
    }

    private static VerificationException verificationFailure(
            String code, String path, String detail, Throwable cause) {
        return new VerificationException(code, path, detail, cause);
    }

    interface PublicationInterceptor {
        void beforeInstall(File staging, File root, File backup) throws IOException;
    }

    interface InventoryProbe {
        void beforeInventory(File candidate) throws IOException;
    }

    interface BackupCleanupProbe {
        void beforeCleanup(File backup) throws IOException;
    }

    static final PublicationInterceptor NO_INTERCEPTOR =
            new PublicationInterceptor() {
                @Override
                public void beforeInstall(File staging, File root, File backup) {
                }
            };
    static final InventoryProbe NO_INVENTORY_PROBE =
            new InventoryProbe() {
                @Override
                public void beforeInventory(File candidate) {
                }
            };
    static final BackupCleanupProbe NO_CLEANUP_PROBE =
            new BackupCleanupProbe() {
                @Override
                public void beforeCleanup(File backup) {
                }
            };

    public static final class Metrics {
        private long packagedReadBytes;
        private long cacheReadBytes;
        private long cacheWriteBytes;

        public long getPackagedReadBytes() {
            return packagedReadBytes;
        }

        public long getCacheReadBytes() {
            return cacheReadBytes;
        }

        public long getCacheWriteBytes() {
            return cacheWriteBytes;
        }

        public long getTotalReadBytes() {
            return packagedReadBytes + cacheReadBytes;
        }

        public long getTotalWriteBytes() {
            return cacheWriteBytes;
        }
    }

    public static final class Result {
        private final File root;
        private final boolean reused;
        private final Metrics metrics;
        private final String manifestSha256;

        private Result(File root, boolean reused, Metrics metrics, String manifestSha256) {
            this.root = root;
            this.reused = reused;
            this.metrics = metrics;
            this.manifestSha256 = manifestSha256;
        }

        public File getRoot() {
            return root;
        }

        public boolean isReused() {
            return reused;
        }

        public Metrics getMetrics() {
            return metrics;
        }

        /** SHA-256 already verified against the packaged and extracted manifests. */
        public String getManifestSha256() {
            return manifestSha256;
        }
    }

    private final AssetSource source;
    private final File filesDir;
    private final String packageName;
    private final String releaseIdentity;
    private final long maxAssetBytes;
    private final PublicationInterceptor publicationInterceptor;
    private final InventoryProbe inventoryProbe;
    private final BackupCleanupProbe backupCleanupProbe;

    public StasisAssetCache(AssetSource source, File filesDir, String packageName,
            String releaseIdentity) {
        this(source, filesDir, packageName, releaseIdentity, NO_INTERCEPTOR,
                NO_INVENTORY_PROBE, NO_CLEANUP_PROBE, MAX_ASSET_BYTES);
    }

    /** Test-only bound override; production callers always use MAX_ASSET_BYTES. */
    public StasisAssetCache(AssetSource source, File filesDir, String packageName,
            String releaseIdentity, long maxAssetBytes) {
        this(source, filesDir, packageName, releaseIdentity, NO_INTERCEPTOR,
                NO_INVENTORY_PROBE, NO_CLEANUP_PROBE, maxAssetBytes);
    }

    StasisAssetCache(AssetSource source, File filesDir, String packageName,
            String releaseIdentity, PublicationInterceptor publicationInterceptor) {
        this(source, filesDir, packageName, releaseIdentity, publicationInterceptor,
                NO_INVENTORY_PROBE, NO_CLEANUP_PROBE, MAX_ASSET_BYTES);
    }

    StasisAssetCache(AssetSource source, File filesDir, String packageName,
            String releaseIdentity, PublicationInterceptor publicationInterceptor,
            InventoryProbe inventoryProbe, BackupCleanupProbe backupCleanupProbe) {
        this(source, filesDir, packageName, releaseIdentity, publicationInterceptor,
                inventoryProbe, backupCleanupProbe, MAX_ASSET_BYTES);
    }

    StasisAssetCache(AssetSource source, File filesDir, String packageName,
            String releaseIdentity, PublicationInterceptor publicationInterceptor,
            InventoryProbe inventoryProbe, BackupCleanupProbe backupCleanupProbe,
            long maxAssetBytes) {
        if (source == null || filesDir == null || packageName == null || releaseIdentity == null
                || publicationInterceptor == null || inventoryProbe == null
                || backupCleanupProbe == null || maxAssetBytes <= 0) {
            throw new IllegalArgumentException("asset cache arguments must be non-null");
        }
        this.source = source;
        this.filesDir = filesDir;
        this.packageName = packageName;
        this.releaseIdentity = releaseIdentity;
        this.publicationInterceptor = publicationInterceptor;
        this.inventoryProbe = inventoryProbe;
        this.backupCleanupProbe = backupCleanupProbe;
        this.maxAssetBytes = maxAssetBytes;
    }

    public Result prepare() throws IOException {
        Metrics metrics = new Metrics();
        File root = new File(filesDir, PACKAGED_ROOT);
        File staging = new File(filesDir, STAGING_NAME);
        File[] backups = backupSlots();
        deleteTree(staging);

        byte[] packagedManifest;
        try {
            packagedManifest = readSourceBounded(MANIFEST_PATH, MAX_MANIFEST_BYTES, metrics);
        } catch (IOException error) {
            throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                    error.getMessage(), error);
        }
        Manifest manifest;
        try {
            manifest = Manifest.parse(packagedManifest);
        } catch (VerificationException error) {
            throw error;
        } catch (IOException error) {
            throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                    error.getMessage(), error);
        }
        String manifestHash = sha256(packagedManifest);
        if (recoverPublication(root, backups, packagedManifest, manifestHash, metrics)) {
            return new Result(root, true, metrics, manifestHash);
        }

        try {
            copyAssetTree(PACKAGED_ROOT, staging, metrics, 0, new CopyState());
            verifyExtractedTree(staging, manifest, packagedManifest, metrics);
            writeMarker(staging, manifestHash, metrics);
            publish(staging, root, backups);
            return new Result(root, false, metrics, manifestHash);
        } catch (IOException error) {
            deleteTree(staging);
            throw error;
        }
    }

    private boolean isReusable(File root, byte[] packagedManifest, String manifestHash,
            Metrics metrics) throws IOException {
        if (!root.isDirectory()) return false;
        File markerFile = new File(root, MARKER_NAME);
        if (!markerFile.isFile()) return false;
        Marker marker;
        try {
            marker = Marker.parse(readFileBounded(markerFile, MAX_MARKER_BYTES, metrics));
        } catch (IOException error) {
            return false;
        }
        if (!CACHE_SCHEMA.equals(marker.schema) || marker.version != CACHE_VERSION
                || !packageName.equals(marker.packageName)
                || !releaseIdentity.equals(marker.releaseIdentity)
                || !manifestHash.equals(marker.manifestHash)) {
            return false;
        }
        File extractedManifest = new File(root, MANIFEST_PATH);
        byte[] extractedBytes;
        try {
            extractedBytes = readFileBounded(extractedManifest, MAX_MANIFEST_BYTES, metrics);
        } catch (IOException error) {
            return false;
        }
        if (!MessageDigest.isEqual(packagedManifest, extractedBytes)
                || !manifestHash.equals(sha256(extractedBytes))) {
            return false;
        }
        try {
            inventoryProbe.beforeInventory(root);
            return inventoryMatches(root, marker.entries);
        } catch (IOException error) {
            // A broken candidate is invalid; another transaction candidate may recover it.
            return false;
        }
    }

    private void verifyExtractedTree(File root, Manifest manifest, byte[] packagedManifest,
            Metrics metrics) throws IOException {
        byte[] extractedManifest = readFileBounded(new File(root, MANIFEST_PATH),
                MAX_MANIFEST_BYTES, metrics);
        if (!MessageDigest.isEqual(packagedManifest, extractedManifest)) {
            throw verificationFailure(ERROR_TAMPERED_ASSET, MANIFEST_PATH,
                    "extracted manifest differs from packaged manifest");
        }
        long totalBytes = 0;
        for (AssetEntry entry : manifest.assets) {
            File file = safeFile(root, entry.path);
            if (!file.isFile()) {
                throw verificationFailure(ERROR_MISSING_ASSET, entry.path, "asset is missing");
            }
            if (file.length() > maxAssetBytes) {
                throw verificationFailure(ERROR_OVERSIZED_ASSET, entry.path,
                        "asset exceeds the per-file byte limit");
            }
            totalBytes += file.length();
            if (totalBytes > MAX_TOTAL_ASSET_BYTES) {
                throw verificationFailure(ERROR_OVERSIZED_ASSET, entry.path,
                        "assets exceed the total byte limit");
            }
            if (!entry.sha256.equals(sha256(file, metrics))) {
                throw verificationFailure(ERROR_TAMPERED_ASSET, entry.path,
                        "asset hash does not match the manifest");
            }
        }
        List<InventoryEntry> inventory = collectInventory(root);
        if (inventory.size() > MAX_TREE_FILES) {
            throw verificationFailure(ERROR_INVALID_TREE, MANIFEST_PATH,
                    "extracted asset tree exceeds the file limit");
        }
    }

    private void writeMarker(File root, String manifestHash, Metrics metrics) throws IOException {
        List<InventoryEntry> entries = collectInventory(root);
        StringBuilder marker = new StringBuilder(256 + entries.size() * 48);
        marker.append("schema=").append(CACHE_SCHEMA).append('\n');
        marker.append("version=").append(CACHE_VERSION).append('\n');
        marker.append("package_name=").append(encode(packageName)).append('\n');
        marker.append("release_identity=").append(encode(releaseIdentity)).append('\n');
        marker.append("manifest_sha256=").append(manifestHash).append('\n');
        marker.append("entry_count=").append(entries.size()).append('\n');
        for (InventoryEntry entry : entries) {
            marker.append("entry=").append(encode(entry.path)).append('\t')
                    .append(entry.length).append('\t').append(entry.modified).append('\n');
        }
        byte[] bytes = marker.toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_MARKER_BYTES) throw new IOException("asset cache marker is oversized");
        File markerFile = new File(root, MARKER_NAME);
        try (FileOutputStream output = new FileOutputStream(markerFile)) {
            output.write(bytes);
            output.getFD().sync();
            metrics.cacheWriteBytes += bytes.length;
        }
    }

    private File[] backupSlots() {
        return new File[] {
                new File(filesDir, BACKUP_NAME),
                new File(filesDir, BACKUP_ALT_NAME)
        };
    }

    private void publish(File staging, File root, File[] backups) throws IOException {
        File backup = null;
        boolean movedOldRoot = false;
        try {
            if (root.exists()) {
                backup = chooseBackupSlot(backups);
                rename(root, backup);
                movedOldRoot = true;
            }
            publicationInterceptor.beforeInstall(staging, root, backup);
            rename(staging, root);
        } catch (IOException error) {
            if (root.exists() && movedOldRoot) deleteTree(root);
            if (movedOldRoot && backup.exists()) {
                try {
                    rename(backup, root);
                } catch (IOException rollbackError) {
                    error.addSuppressed(rollbackError);
                }
            }
            throw error;
        }
        // The staging-to-root rename is the commit. Cleanup is deferred so an
        // old backup cannot damage or invalidate the newly committed tree.
        if (backup != null) tryCleanupBackup(backup);
    }

    private boolean recoverPublication(File root, File[] backups, byte[] packagedManifest,
            String manifestHash, Metrics metrics) throws IOException {
        boolean rootValid = isReusable(root, packagedManifest, manifestHash, metrics);
        File validBackup = null;
        for (File backup : backups) {
            if (backup.exists() && isReusable(backup, packagedManifest, manifestHash, metrics)) {
                validBackup = backup;
                break;
            }
        }
        if (rootValid) {
            for (File backup : backups) tryCleanupBackup(backup);
            return true;
        }
        if (validBackup != null) {
            if (root.exists()) deleteTree(root);
            rename(validBackup, root);
            for (File backup : backups) {
                if (!backup.equals(validBackup)) tryCleanupBackup(backup);
            }
            return true;
        }
        // Keep an invalid prior root until publication can move it to backup;
        // this preserves rollback if staging or publication fails.
        for (File backup : backups) tryCleanupBackup(backup);
        return false;
    }

    private File chooseBackupSlot(File[] backups) throws IOException {
        for (File backup : backups) {
            if (!backup.exists()) return backup;
        }
        for (File backup : backups) tryCleanupBackup(backup);
        for (File backup : backups) {
            if (!backup.exists()) return backup;
        }
        throw new IOException("no transaction backup slot is available");
    }

    private static void rename(File source, File target) throws IOException {
        if (!source.renameTo(target)) {
            throw new IOException("unable to atomically publish " + target);
        }
    }

    private void copyAssetTree(String sourcePath, File output, Metrics metrics, int depth,
            CopyState state) throws IOException {
        if (depth > 64) {
            throw verificationFailure(ERROR_INVALID_TREE, assetPath(sourcePath),
                    "packaged asset tree is too deep");
        }
        String[] children = source.list(sourcePath);
        if (children != null && children.length > 0) {
            if (children.length > MAX_TREE_FILES) {
                throw verificationFailure(ERROR_INVALID_TREE, assetPath(sourcePath),
                        "packaged asset directory is too large");
            }
            if (!output.isDirectory() && !output.mkdirs()) {
                throw new IOException("unable to create " + output);
            }
            Set<String> childNames = new HashSet<>();
            for (String child : children) {
                if (child == null || child.isEmpty() || child.equals(".") || child.equals("..")
                        || child.indexOf('/') >= 0
                        || child.indexOf('\\') >= 0) {
                    throw verificationFailure(ERROR_INVALID_TREE, assetPath(sourcePath),
                            "invalid packaged asset name");
                }
                if (!childNames.add(child)) {
                    throw verificationFailure(ERROR_DUPLICATE_ASSET,
                            assetPath(sourcePath + "/" + child),
                            "packaged asset name is duplicated");
                }
                copyAssetTree(sourcePath + "/" + child, new File(output, child), metrics,
                        depth + 1, state);
            }
            return;
        }
        if (++state.fileCount > MAX_TREE_FILES) {
            throw verificationFailure(ERROR_INVALID_TREE, assetPath(sourcePath),
                    "packaged asset tree is too large");
        }
        File parent = output.getParentFile();
        if (parent != null && !parent.isDirectory() && !parent.mkdirs()) {
            throw new IOException("unable to create " + parent);
        }
        try (InputStream input = source.open(sourcePath); FileOutputStream outputStream =
                new FileOutputStream(output)) {
            long perFileLimit = sourcePath.equals(PACKAGED_ROOT + "/" + MANIFEST_PATH)
                    ? MAX_MANIFEST_BYTES : maxAssetBytes;
            copy(input, outputStream, metrics, true, perFileLimit, state, assetPath(sourcePath));
        } catch (VerificationException error) {
            throw error;
        } catch (IOException error) {
            throw verificationFailure(ERROR_MISSING_ASSET, assetPath(sourcePath),
                    error.getMessage(), error);
        }
    }

    private static String assetPath(String sourcePath) {
        String prefix = PACKAGED_ROOT + "/";
        return sourcePath.startsWith(prefix) ? sourcePath.substring(prefix.length()) : sourcePath;
    }

    private static void copy(InputStream input, OutputStream output, Metrics metrics,
            boolean packaged, long perFileLimit, CopyState state, String path) throws IOException {
        byte[] buffer = new byte[16 * 1024];
        long total = 0;
        int count;
        while ((count = input.read(buffer)) != -1) {
            total += count;
            state.totalBytes += count;
            if (total > perFileLimit || state.totalBytes > MAX_COPIED_TREE_BYTES) {
                throw verificationFailure(ERROR_OVERSIZED_ASSET, path,
                        "packaged asset tree exceeds its byte limit");
            }
            if (packaged) metrics.packagedReadBytes += count;
            else metrics.cacheReadBytes += count;
            output.write(buffer, 0, count);
            metrics.cacheWriteBytes += count;
        }
    }

    private byte[] readSourceBounded(String path, int limit, Metrics metrics) throws IOException {
        try (InputStream input = source.open(PACKAGED_ROOT + "/" + path)) {
            return readBounded(input, limit, metrics, true);
        }
    }

    private static byte[] readFileBounded(File file, int limit, Metrics metrics) throws IOException {
        try (FileInputStream input = new FileInputStream(file)) {
            return readBounded(input, limit, metrics, false);
        }
    }

    private static byte[] readBounded(InputStream input, int limit, Metrics metrics,
            boolean packaged) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream(Math.min(limit, 16 * 1024));
        byte[] buffer = new byte[16 * 1024];
        int total = 0;
        int count;
        while ((count = input.read(buffer)) != -1) {
            total += count;
            if (total > limit) throw new IOException("bounded asset read exceeded its limit");
            if (packaged) metrics.packagedReadBytes += count;
            else metrics.cacheReadBytes += count;
            output.write(buffer, 0, count);
        }
        return output.toByteArray();
    }

    private static String sha256(byte[] bytes) throws IOException {
        try {
            return hex(MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (NoSuchAlgorithmException error) {
            throw new IOException("SHA-256 is unavailable", error);
        }
    }

    private static String sha256(File file, Metrics metrics) throws IOException {
        try (FileInputStream input = new FileInputStream(file)) {
            MessageDigest digest;
            try {
                digest = MessageDigest.getInstance("SHA-256");
            } catch (NoSuchAlgorithmException error) {
                throw new IOException("SHA-256 is unavailable", error);
            }
            byte[] buffer = new byte[16 * 1024];
            int count;
            while ((count = input.read(buffer)) != -1) {
                metrics.cacheReadBytes += count;
                digest.update(buffer, 0, count);
            }
            return hex(digest.digest());
        }
    }

    private static String hex(byte[] bytes) {
        StringBuilder result = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) {
            int unsigned = value & 0xff;
            if (unsigned < 16) result.append('0');
            result.append(Integer.toHexString(unsigned));
        }
        return result.toString();
    }

    private static File safeFile(File root, String path) throws IOException {
        if (!isSafeAssetPath(path)) {
            throw verificationFailure(ERROR_TRAVERSAL_PATH, path,
                    "asset path is outside the assets directory");
        }
        File file = new File(root, path);
        String rootPath = root.getCanonicalPath() + File.separator;
        if (!file.getCanonicalPath().startsWith(rootPath)) {
            throw verificationFailure(ERROR_TRAVERSAL_PATH, path,
                    "asset path escapes extraction root");
        }
        return file;
    }

    private static boolean isSafeAssetPath(String path) {
        if (path == null || !path.startsWith("assets/") || path.endsWith("/")
                || path.indexOf('\\') >= 0 || path.indexOf('\0') >= 0 || path.contains("//")
                || path.contains("/../") || path.endsWith("/..") || path.contains("/./")
                || path.endsWith("/.")) {
            return false;
        }
        for (int index = 0; index < path.length(); index++) {
            if (path.charAt(index) < 0x20 || path.charAt(index) == 0x7f) return false;
        }
        return true;
    }

    private static List<InventoryEntry> collectInventory(File root) throws IOException {
        List<InventoryEntry> entries = new ArrayList<>();
        collectInventory(root, root, entries);
        Collections.sort(entries);
        return entries;
    }

    private static void collectInventory(File root, File current, List<InventoryEntry> entries)
            throws IOException {
        File[] children = current.listFiles();
        if (children == null) {
            if (!current.isDirectory()) throw new IOException("unable to enumerate " + current);
            return;
        }
        for (File child : children) {
            if (current.equals(root) && child.getName().equals(MARKER_NAME)) continue;
            String canonical = child.getCanonicalPath();
            String rootPath = root.getCanonicalPath() + File.separator;
            if (!canonical.startsWith(rootPath)) throw new IOException("asset tree escapes root");
            if (child.isDirectory()) collectInventory(root, child, entries);
            else if (child.isFile()) {
                String relative = root.toPath().relativize(child.toPath()).toString()
                        .replace(File.separatorChar, '/');
                entries.add(new InventoryEntry(relative, child.length(), child.lastModified()));
                if (entries.size() > MAX_TREE_FILES) throw new IOException("asset tree is too large");
            } else throw new IOException("unsupported extraction entry: " + child);
        }
    }

    private static boolean inventoryMatches(File root, List<InventoryEntry> expected)
            throws IOException {
        List<InventoryEntry> actual = collectInventory(root);
        if (actual.size() != expected.size()) return false;
        for (int index = 0; index < expected.size(); index++) {
            InventoryEntry left = expected.get(index);
            InventoryEntry right = actual.get(index);
            if (!left.path.equals(right.path) || left.length != right.length
                    || left.modified != right.modified) return false;
        }
        return true;
    }

    private static void deleteTree(File path) throws IOException {
        if (!path.exists()) return;
        File[] children = path.listFiles();
        if (children != null) {
            for (File child : children) deleteTree(child);
        }
        if (!path.delete()) throw new IOException("unable to remove " + path);
    }

    private void tryCleanupBackup(File path) {
        try {
            backupCleanupProbe.beforeCleanup(path);
            deleteTree(path);
        } catch (IOException ignored) {
            // A committed root is valid even when an old backup remains.
        }
    }

    private static final class CopyState {
        int fileCount;
        long totalBytes;
    }

    private static String encode(String value) {
        return Base64.getEncoder().withoutPadding()
                .encodeToString(value.getBytes(StandardCharsets.UTF_8));
    }

    private static String decode(String value) throws IOException {
        if (value == null || value.isEmpty()) throw new IOException("missing cache marker value");
        try {
            return new String(Base64.getDecoder().decode(value), StandardCharsets.UTF_8);
        } catch (IllegalArgumentException error) {
            throw new IOException("invalid cache marker encoding", error);
        }
    }

    private static final class InventoryEntry implements Comparable<InventoryEntry> {
        final String path;
        final long length;
        final long modified;

        InventoryEntry(String path, long length, long modified) {
            this.path = path;
            this.length = length;
            this.modified = modified;
        }

        @Override
        public int compareTo(InventoryEntry other) {
            return path.compareTo(other.path);
        }
    }

    private static final class Marker {
        String schema;
        int version;
        String packageName;
        String releaseIdentity;
        String manifestHash;
        List<InventoryEntry> entries;

        static Marker parse(byte[] bytes) throws IOException {
            Marker marker = new Marker();
            Map<String, String> values = new HashMap<>();
            List<InventoryEntry> entries = new ArrayList<>();
            String text = new String(bytes, StandardCharsets.UTF_8);
            for (String line : text.split("\\n", -1)) {
                if (line.isEmpty()) continue;
                if (line.startsWith("entry=")) {
                    String[] fields = line.substring(6).split("\\t", -1);
                    if (fields.length != 3) throw new IOException("invalid asset cache entry");
                    long length = parseNonNegative(fields[1]);
                    long modified = parseNonNegative(fields[2]);
                    entries.add(new InventoryEntry(decode(fields[0]), length, modified));
                } else {
                    int split = line.indexOf('=');
                    if (split <= 0) throw new IOException("invalid asset cache marker");
                    values.put(line.substring(0, split), line.substring(split + 1));
                }
            }
            marker.schema = values.get("schema");
            marker.version = parseInt(values.get("version"));
            marker.packageName = decode(values.get("package_name"));
            marker.releaseIdentity = decode(values.get("release_identity"));
            marker.manifestHash = values.get("manifest_sha256");
            int entryCount = parseInt(values.get("entry_count"));
            if (entryCount < 0 || entryCount != entries.size() || entries.size() > MAX_TREE_FILES) {
                throw new IOException("invalid asset cache inventory count");
            }
            Collections.sort(entries);
            marker.entries = entries;
            return marker;
        }

        private static int parseInt(String value) throws IOException {
            try {
                return Integer.parseInt(value);
            } catch (Exception error) {
                throw new IOException("invalid asset cache marker number", error);
            }
        }

        private static long parseNonNegative(String value) throws IOException {
            try {
                long result = Long.parseLong(value);
                if (result < 0) throw new NumberFormatException();
                return result;
            } catch (Exception error) {
                throw new IOException("invalid asset cache inventory value", error);
            }
        }
    }

    private static final class AssetEntry {
        final String path;
        final String sha256;

        AssetEntry(String path, String sha256) {
            this.path = path;
            this.sha256 = sha256;
        }
    }

    private static final class Manifest {
        final List<AssetEntry> assets;

        private Manifest(List<AssetEntry> assets) {
            this.assets = assets;
        }

        static Manifest parse(byte[] bytes) throws IOException {
            Object value = new JsonParser(new String(bytes, StandardCharsets.UTF_8)).parse();
            if (!(value instanceof Map)) {
                throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                        "manifest is not an object");
            }
            Map<?, ?> object = (Map<?, ?>) value;
            if (!(object.get("schema") instanceof String)
                    || !"stasis-assets".equals(object.get("schema"))) {
                throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                        "unsupported manifest schema");
            }
            Object version = object.get("version");
            if (!(version instanceof Long)
                    || (((Long) version).longValue() != 1L && ((Long) version).longValue() != 2L)) {
                throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                        "unsupported manifest version");
            }
            Object rawAssets = object.get("assets");
            if (!(rawAssets instanceof List) || ((List<?>) rawAssets).size() > MAX_MANIFEST_ASSETS) {
                throw verificationFailure(ERROR_OVERSIZED_ASSET, MANIFEST_PATH,
                        "manifest exceeds the entry limit");
            }
            Set<String> ids = new HashSet<>();
            Set<String> paths = new HashSet<>();
            List<AssetEntry> assets = new ArrayList<>();
            for (Object raw : (List<?>) rawAssets) {
                if (!(raw instanceof Map)) {
                    throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                            "manifest entry is not an object");
                }
                Map<?, ?> entry = (Map<?, ?>) raw;
                Object id = entry.get("id");
                Object path = entry.get("path");
                Object hash = entry.get("content_sha256");
                if (!(id instanceof String) || ((String) id).isEmpty()) {
                    throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                            "manifest entry id is invalid");
                }
                if (!(path instanceof String)) {
                    throw verificationFailure(ERROR_MALFORMED_MANIFEST, MANIFEST_PATH,
                            "manifest entry path is invalid");
                }
                if (!isSafeAssetPath((String) path)) {
                    throw verificationFailure(ERROR_TRAVERSAL_PATH, (String) path,
                            "manifest entry path is outside the assets directory");
                }
                if (!paths.add((String) path)) {
                    throw verificationFailure(ERROR_DUPLICATE_ASSET, (String) path,
                            "manifest entry path is duplicated");
                }
                if (!ids.add((String) id)) {
                    throw verificationFailure(ERROR_DUPLICATE_ASSET, (String) path,
                            "manifest entry id is duplicated");
                }
                if (!(hash instanceof String) || !((String) hash).matches("[0-9a-f]{64}")) {
                    throw verificationFailure(ERROR_MALFORMED_MANIFEST, (String) path,
                            "manifest entry hash is invalid");
                }
                assets.add(new AssetEntry((String) path, (String) hash));
            }
            return new Manifest(assets);
        }
    }

    private static final class JsonParser {
        private final String text;
        private int index;

        JsonParser(String text) {
            this.text = text;
        }

        Object parse() throws IOException {
            Object value = value(0);
            whitespace();
            if (index != text.length()) throw error("trailing JSON");
            return value;
        }

        private Object value(int depth) throws IOException {
            if (depth > 32) throw error("JSON nesting limit exceeded");
            whitespace();
            if (index >= text.length()) throw error("unexpected end of JSON");
            char current = text.charAt(index);
            if (current == '{') return object(depth + 1);
            if (current == '[') return array(depth + 1);
            if (current == '"') return string();
            if (text.startsWith("true", index)) { index += 4; return Boolean.TRUE; }
            if (text.startsWith("false", index)) { index += 5; return Boolean.FALSE; }
            if (text.startsWith("null", index)) { index += 4; return null; }
            return number();
        }

        private Map<String, Object> object(int depth) throws IOException {
            Map<String, Object> result = new HashMap<>();
            index++;
            whitespace();
            if (take('}')) return result;
            while (true) {
                whitespace();
                if (index >= text.length() || text.charAt(index) != '"') {
                    throw error("object key is not a string");
                }
                String key = string();
                whitespace();
                require(':');
                if (result.containsKey(key)) throw error("duplicate JSON object key");
                result.put(key, value(depth));
                whitespace();
                if (take('}')) return result;
                require(',');
            }
        }

        private List<Object> array(int depth) throws IOException {
            List<Object> result = new ArrayList<>();
            index++;
            whitespace();
            if (take(']')) return result;
            while (true) {
                result.add(value(depth));
                whitespace();
                if (take(']')) return result;
                require(',');
            }
        }

        private String string() throws IOException {
            require('"');
            StringBuilder result = new StringBuilder();
            while (index < text.length()) {
                char current = text.charAt(index++);
                if (current == '"') return result.toString();
                if (current == '\\') {
                    if (index >= text.length()) throw error("unterminated escape");
                    char escaped = text.charAt(index++);
                    switch (escaped) {
                        case '"': case '\\': case '/': result.append(escaped); break;
                        case 'b': result.append('\b'); break;
                        case 'f': result.append('\f'); break;
                        case 'n': result.append('\n'); break;
                        case 'r': result.append('\r'); break;
                        case 't': result.append('\t'); break;
                        case 'u':
                            String digits = hexDigits(4);
                            try {
                                result.append((char)Integer.parseInt(digits, 16));
                            } catch (NumberFormatException error) {
                                throw error("invalid JSON unicode escape");
                            }
                            break;
                        default: throw error("invalid JSON escape");
                    }
                } else {
                    if (current < 0x20) throw error("control character in JSON string");
                    result.append(current);
                }
            }
            throw error("unterminated JSON string");
        }

        private String hexDigits(int count) throws IOException {
            if (index + count > text.length()) throw error("short JSON unicode escape");
            String result = text.substring(index, index + count);
            index += count;
            return result;
        }

        private Number number() throws IOException {
            int start = index;
            while (index < text.length() && "-+0123456789.eE".indexOf(text.charAt(index)) >= 0) {
                index++;
            }
            try {
                String value = text.substring(start, index);
                if (!value.matches("-?(0|[1-9][0-9]*)(\\.[0-9]+)?([eE][+-]?[0-9]+)?")) {
                    throw new NumberFormatException();
                }
                if (value.indexOf('.') >= 0 || value.indexOf('e') >= 0
                        || value.indexOf('E') >= 0) {
                    return Double.valueOf(value);
                }
                return Long.valueOf(value);
            } catch (Exception parseError) {
                throw error("invalid JSON number");
            }
        }

        private void whitespace() {
            while (index < text.length() && Character.isWhitespace(text.charAt(index))) index++;
        }

        private boolean take(char expected) {
            if (index < text.length() && text.charAt(index) == expected) { index++; return true; }
            return false;
        }

        private void require(char expected) throws IOException {
            if (!take(expected)) throw error("expected '" + expected + "'");
        }

        private IOException error(String message) {
            return new IOException(message + " at byte " + index);
        }
    }
}
