package com.stasislang.shell;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;

public final class StasisAssetCacheTest {
    private static final String PACKAGE = "com.example.cache";
    private static final String RELEASE = "7:1.0:100";

    public static void main(String[] args) throws Exception {
        Path temp = Files.createTempDirectory("stasis-asset-cache-test");
        try {
            Path packaged = temp.resolve("packaged");
            Path files = temp.resolve("files");
            Files.createDirectories(packaged.resolve("stasis_game/assets"));
            Files.write(packaged.resolve("stasis_game/assets/token.txt"), bytes("original"));
            Files.write(packaged.resolve("stasis_game/assets/.stasis_asset_cache.v1"),
                    bytes("nested packaged marker basename"));
            writeManifest(packaged, "token.txt");
            CountingSource source = new CountingSource(packaged.toFile());

            StasisAssetCache.Result cold = cache(source, files.toFile(), RELEASE).prepare();
            check(!cold.isReused(), "first install is cold");
            check(cold.getManifestSha256().matches("[0-9a-f]{64}"),
                    "cold result exposes verified manifest identity");
            check(cold.getMetrics().getCacheWriteBytes() > 0, "cold path writes bytes");
            Path root = files.resolve("stasis_game");
            Path marker = root.resolve(".stasis_asset_cache.v1");
            check(Files.isRegularFile(marker), "cold path writes marker last");
            check(text(root.resolve("assets/.stasis_asset_cache.v1")).contains("nested packaged"),
                    "nested marker basename is retained");

            CountingSource restartedSource = new CountingSource(packaged.toFile());
            StasisAssetCache.Result recreation = cache(restartedSource, files.toFile(), RELEASE).prepare();
            check(recreation.isReused(), "ordinary recreation reuses");
            check(cold.getManifestSha256().equals(recreation.getManifestSha256()),
                    "reuse keeps the verified manifest identity");
            check(restartedSource.openCount == 1, "fresh source reads only packaged manifest");
            check(recreation.getMetrics().getPackagedReadBytes()
                    == Files.size(packaged.resolve("stasis_game/assets/manifest.json")),
                    "reuse packaged bytes are manifest-only");
            check(recreation.getMetrics().getCacheReadBytes()
                    == Files.size(marker) + Files.size(root.resolve("assets/manifest.json")),
                    "reuse cache bytes are marker and manifest only");
            check(recreation.getMetrics().getCacheWriteBytes() == 0, "reuse does not write assets");

            Path updateFiles = temp.resolve("update-time-files");
            check(!cache(source, updateFiles.toFile(), RELEASE).prepare().isReused(),
                    "same-version baseline installs");
            check(!cache(source, updateFiles.toFile(), "7:1.0:200").prepare().isReused(),
                    "same-version package update time rebuilds");

            boolean invalidName = false;
            try {
                cache(new InvalidChildSource(packaged.toFile()),
                        temp.resolve("invalid-child-files").toFile(), RELEASE).prepare();
            } catch (IOException expected) {
                invalidName = true;
            }
            check(invalidName, "dot child names are rejected before output resolution");

            Path token = root.resolve("assets/token.txt");
            long originalModified = Files.getLastModifiedTime(token).toMillis();
            Files.write(token, bytes("tampered"));
            if (Files.getLastModifiedTime(token).toMillis() == originalModified) {
                check(token.toFile().setLastModified(originalModified + 2000L),
                        "stabilize mutation timestamp");
            }
            StasisAssetCache.Result mutation = cache(source, files.toFile(), RELEASE).prepare();
            check(!mutation.isReused(), "ordinary mutation invalidates inventory");
            check(text(root.resolve("assets/token.txt")).equals("original"), "mutation rebuilds");

            Files.write(root.resolve("assets/token.txt"), bytes("x"));
            StasisAssetCache.Result truncation = cache(source, files.toFile(), RELEASE).prepare();
            check(!truncation.isReused(), "truncation invalidates inventory");
            check(text(root.resolve("assets/token.txt")).equals("original"), "truncation rebuilds");

            Files.delete(marker);
            check(!cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "missing marker rebuilds");
            Files.write(marker, bytes("corrupt marker\n"));
            check(!cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "corrupt marker rebuilds");

            Files.createDirectories(files.resolve(".stasis_game.staging"));
            Files.write(files.resolve(".stasis_game.staging/partial"), bytes("partial"));
            check(cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "partial staging is discarded before safe reuse");
            check(!Files.exists(files.resolve(".stasis_game.staging")), "staging is cleaned");

            check(!cache(source, files.toFile(), "8:2.0").prepare().isReused(),
                    "package release update rebuilds");

            Files.write(packaged.resolve("stasis_game/assets/token.txt"), bytes("updated"));
            writeManifest(packaged, "token.txt");
            check(!cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "packaged manifest change rebuilds");
            check(text(root.resolve("assets/token.txt")).equals("updated"), "manifest update publishes");

            Path backup = files.resolve(".stasis_game.previous");
            Path backupAlt = files.resolve(".stasis_game.previous.2");
            copyTree(root, backup);
            Files.write(backup.resolve(".stasis_asset_cache.v1"), bytes("stale backup"));
            check(cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "valid root wins over stale backup");
            check(!Files.exists(backup), "stale backup cleanup is deferred but retried safely");

            copyTree(root, backup);
            copyTree(root, backupAlt);
            Files.write(backup.resolve(".stasis_asset_cache.v1"), bytes("malformed candidate"));
            Files.write(root.resolve("assets/token.txt"), bytes("broken"));
            check(cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "valid backup replaces invalid root");
            check(text(root.resolve("assets/token.txt")).equals("updated"),
                    "backup recovery restores the validated tree");
            check(!Files.exists(backup), "recovered backup is no longer selected");
            check(!Files.exists(backupAlt), "alternate backup is consumed by recovery");

            copyTree(root, backup);
            check(cache(source, files.toFile(), RELEASE, StasisAssetCache.NO_INTERCEPTOR,
                    new StasisAssetCache.InventoryProbe() {
                        @Override
                        public void beforeInventory(File candidate) throws IOException {
                            if (candidate.getName().equals("stasis_game")) {
                                throw new IOException("injected malformed root inventory");
                            }
                        }
                    }, StasisAssetCache.NO_CLEANUP_PROBE).prepare().isReused(),
                    "inventory failure falls back to valid backup");

            copyTree(root, backup);
            check(cache(source, files.toFile(), RELEASE, StasisAssetCache.NO_INTERCEPTOR,
                    new StasisAssetCache.InventoryProbe() {
                        @Override
                        public void beforeInventory(File candidate) throws IOException {
                            if (candidate.getName().equals(".stasis_game.previous")) {
                                throw new IOException("injected malformed backup inventory");
                            }
                        }
                    }, StasisAssetCache.NO_CLEANUP_PROBE).prepare().isReused(),
                    "valid root survives malformed backup inventory");

            copyTree(root, backup);
            Files.write(backup.resolve(".stasis_asset_cache.v1"), bytes("stale leftover"));
            Files.write(packaged.resolve("stasis_game/assets/token.txt"), bytes("slot-update"));
            writeManifest(packaged, "token.txt");
            check(!cache(source, files.toFile(), RELEASE, StasisAssetCache.NO_INTERCEPTOR,
                    StasisAssetCache.NO_INVENTORY_PROBE, new StasisAssetCache.BackupCleanupProbe() {
                        @Override
                        public void beforeCleanup(File candidate) throws IOException {
                            if (candidate.getName().equals(".stasis_game.previous")) {
                                throw new IOException("injected stale backup cleanup failure");
                            }
                        }
                    }).prepare().isReused(),
                    "update uses alternate transaction backup when stale cleanup fails");
            check(text(root.resolve("assets/token.txt")).equals("slot-update"),
                    "alternate backup update publishes");
            check(Files.exists(backup), "failed stale cleanup leaves bounded leftover");
            check(!Files.exists(backupAlt), "alternate transaction backup is cleaned");

            Files.write(root.resolve("assets/unexpected.txt"), bytes("tamper"));
            check(!cache(source, files.toFile(), RELEASE).prepare().isReused(),
                    "unexpected extracted file invalidates inventory");

            Files.write(packaged.resolve("stasis_game/assets/token.txt"), bytes("newer"));
            writeManifest(packaged, "token.txt");
            String oldRoot = text(root.resolve("assets/token.txt"));
            boolean failed = false;
            try {
                cache(source, files.toFile(), RELEASE, new StasisAssetCache.PublicationInterceptor() {
                    @Override
                    public void beforeInstall(File staging, File destination, File backup)
                            throws IOException {
                        throw new IOException("injected publication failure");
                    }
                }).prepare();
            } catch (IOException expected) {
                failed = true;
            }
            check(failed, "publication failure is surfaced");
            check(text(root.resolve("assets/token.txt")).equals(oldRoot),
                    "publication failure preserves prior root");
            check(!Files.exists(files.resolve(".stasis_game.staging")),
                    "publication failure leaves no staging root");

            Path firstInstallWithStaleSlots = temp.resolve("first-install-stale-slots");
            Files.createDirectories(firstInstallWithStaleSlots);
            copyTree(root, firstInstallWithStaleSlots.resolve(".stasis_game.previous"));
            copyTree(root, firstInstallWithStaleSlots.resolve(".stasis_game.previous.2"));
            StasisAssetCache.Result firstInstall = cache(source,
                    firstInstallWithStaleSlots.toFile(), RELEASE,
                    StasisAssetCache.NO_INTERCEPTOR, StasisAssetCache.NO_INVENTORY_PROBE,
                    new StasisAssetCache.BackupCleanupProbe() {
                        @Override
                        public void beforeCleanup(File candidate) throws IOException {
                            throw new IOException("injected unavailable stale backup");
                        }
                    }).prepare();
            check(!firstInstall.isReused(),
                    "first install needs no rollback slot when no root exists");
            check(text(firstInstallWithStaleSlots.resolve("stasis_game/assets/token.txt"))
                            .equals("newer"),
                    "first install publishes despite unavailable stale slots");

            Files.delete(packaged.resolve("stasis_game/assets/.stasis_asset_cache.v1"));
            boolean oversized = false;
            try {
                new StasisAssetCache(source, temp.resolve("oversized-seam").toFile(),
                        PACKAGE, RELEASE, 1L).prepare();
            } catch (StasisAssetCache.VerificationException expected) {
                oversized = true;
                check(StasisAssetCache.ERROR_OVERSIZED_ASSET.equals(expected.getCode()),
                        "seam bound uses stable oversized code");
                check("assets/token.txt".equals(expected.getPath()),
                        "seam bound preserves oversized asset path");
            }
            check(oversized, "bounded seam limit rejects oversized input");

            String malformedUnicode = Character.toString((char)92) + "uZZZZ";
            String tokenHash = hex(MessageDigest.getInstance("SHA-256").digest(
                    Files.readAllBytes(packaged.resolve("stasis_game/assets/token.txt"))));
            Files.write(packaged.resolve("stasis_game/assets/manifest.json"), bytes(
                    "{\"schema\":\"stasis-assets\",\"version\":1,\"assets\":["
                            + "{\"id\":\"" + malformedUnicode
                            + "\",\"path\":\"assets/token.txt\","
                            + "\"content_sha256\":\"" + tokenHash + "\"}]}"));
            boolean malformed = false;
            Path malformedFiles = temp.resolve("malformed-files");
            try {
                cache(source, malformedFiles.toFile(), RELEASE).prepare();
            } catch (StasisAssetCache.VerificationException expected) {
                malformed = true;
                check(StasisAssetCache.ERROR_MALFORMED_MANIFEST.equals(expected.getCode()),
                        "malformed manifest has stable code");
                check("assets/manifest.json".equals(expected.getPath()),
                        "malformed manifest has stable path");
                check(expected.getMessage().contains("code=malformed_manifest path=assets/manifest.json"),
                        "malformed manifest diagnostic is machine-readable");
            }
            check(malformed, "malformed unicode is reported as IOException");
            check(!Files.exists(malformedFiles.resolve(".stasis_game.staging")),
                    "malformed manifest never leaves partial staging");
            String bounded = StasisAssetCache.boundDiagnostic("code=test path=assets/x detail="
                    + new String(new char[4096]).replace('\0', 'x'));
            check(bounded.getBytes(StandardCharsets.UTF_8).length
                    <= StasisAssetCache.MAX_DIAGNOSTIC_BYTES,
                    "diagnostic forwarding remains bounded");
            check(bounded.startsWith("code=test path=assets/x detail="),
                    "bounded diagnostic keeps the shared protocol prefix");

            String asciiDiagnostic = new StasisAssetCache.VerificationException(
                    "tampered_asset",
                    "assets/" + new String(new char[512]).replace('\0', 'p'),
                    new String(new char[512]).replace('\0', 'd'),
                    null).getDiagnostic();
            checkDiagnostic(asciiDiagnostic, "tampered_asset", "assets/");
            check(asciiDiagnostic.getBytes(StandardCharsets.UTF_8).length
                    <= StasisAssetCache.MAX_DIAGNOSTIC_BYTES,
                    "ASCII diagnostic remains bounded");

            String unicodeDiagnostic = new StasisAssetCache.VerificationException(
                    "missing_asset",
                    "assets/" + repeat("界", 256),
                    repeat("詳細", 256),
                    null).getDiagnostic();
            checkDiagnostic(unicodeDiagnostic, "missing_asset", "assets/");
            check(unicodeDiagnostic.getBytes(StandardCharsets.UTF_8).length
                    <= StasisAssetCache.MAX_DIAGNOSTIC_BYTES,
                    "Unicode diagnostic remains UTF-8 bounded");

            System.out.println("stasis asset cache JVM scenarios ok");
        } finally {
            delete(temp.toFile());
        }
    }

    private static StasisAssetCache cache(CountingSource source, File files, String release) {
        return cache(source, files, release, StasisAssetCache.NO_INTERCEPTOR);
    }

    private static StasisAssetCache cache(CountingSource source, File files, String release,
            StasisAssetCache.PublicationInterceptor interceptor) {
        return new StasisAssetCache(source, files, PACKAGE, release, interceptor);
    }

    private static StasisAssetCache cache(CountingSource source, File files, String release,
            StasisAssetCache.PublicationInterceptor publication,
            StasisAssetCache.InventoryProbe inventory,
            StasisAssetCache.BackupCleanupProbe cleanup) {
        return new StasisAssetCache(source, files, PACKAGE, release, publication,
                inventory, cleanup);
    }

    private static void writeManifest(Path packaged, String fileName) throws Exception {
        byte[] file = Files.readAllBytes(packaged.resolve("stasis_game/assets/" + fileName));
        String hash = hex(MessageDigest.getInstance("SHA-256").digest(file));
        String manifest = "{\"schema\":\"stasis-assets\",\"version\":1,\"assets\":["
                + "{\"id\":\"token\",\"path\":\"assets/" + fileName
                + "\",\"content_sha256\":\"" + hash + "\"}]}";
        Files.write(packaged.resolve("stasis_game/assets/manifest.json"),
                manifest.getBytes(StandardCharsets.UTF_8));
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

    private static byte[] bytes(String value) {
        return value.getBytes(StandardCharsets.UTF_8);
    }

    private static String text(Path path) throws IOException {
        return new String(Files.readAllBytes(path), StandardCharsets.UTF_8);
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void checkDiagnostic(String diagnostic, String code, String pathPrefix) {
        int pathMarker = diagnostic.indexOf(" path=");
        int detailMarker = diagnostic.indexOf(" detail=", pathMarker);
        check(diagnostic.startsWith("code=" + code), "diagnostic preserves code delimiter");
        check(pathMarker > 0 && detailMarker > pathMarker,
                "diagnostic preserves complete field delimiters");
        check(diagnostic.substring(pathMarker + " path=".length(), detailMarker)
                        .startsWith(pathPrefix),
                "diagnostic preserves path prefix");
    }

    private static String repeat(String value, int count) {
        StringBuilder result = new StringBuilder(value.length() * count);
        for (int index = 0; index < count; index++) result.append(value);
        return result.toString();
    }

    private static void delete(File file) throws IOException {
        if (!file.exists()) return;
        File[] children = file.listFiles();
        if (children != null) for (File child : children) delete(child);
        if (!file.delete()) throw new IOException("unable to delete " + file);
    }

    private static void copyTree(Path source, Path destination) throws IOException {
        if (Files.isDirectory(source)) {
            Files.createDirectories(destination);
            try (java.nio.file.DirectoryStream<Path> children = Files.newDirectoryStream(source)) {
                for (Path child : children) {
                    copyTree(child, destination.resolve(child.getFileName().toString()));
                }
            }
        } else {
            Files.copy(source, destination, java.nio.file.StandardCopyOption.COPY_ATTRIBUTES);
        }
    }

    private static class CountingSource implements StasisAssetCache.AssetSource {
        protected final File root;
        int openCount;

        CountingSource(File root) {
            this.root = root;
        }

        void reset() {
            openCount = 0;
        }

        @Override
        public String[] list(String path) {
            File directory = new File(root, path);
            String[] children = directory.list();
            return children == null ? new String[0] : children;
        }

        @Override
        public InputStream open(String path) throws IOException {
            openCount++;
            return new FileInputStream(new File(root, path));
        }
    }

    private static final class InvalidChildSource extends CountingSource {
        InvalidChildSource(File root) {
            super(root);
        }

        @Override
        public String[] list(String path) {
            if (path.equals("stasis_game")) return new String[] {"."};
            return super.list(path);
        }
    }
}
