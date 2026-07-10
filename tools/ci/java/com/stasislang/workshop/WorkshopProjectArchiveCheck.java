package com.stasislang.workshop;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.RandomAccessFile;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import java.util.zip.ZipOutputStream;

public final class WorkshopProjectArchiveCheck {
    public static void main(String[] args) throws Exception {
        File root = Files.createTempDirectory("stasis-archive-check").toFile();
        try {
            write(new File(root, ".stasis-workshop.json"), "{\"format_version\":1}\n");
            write(new File(root, "src/main.stasis"), "function main(): void {}\n");
            write(new File(root, "build/runtime_state.txt"), "generated\n");
            write(new File(root, "src/ignored.tmp"), "temporary\n");

            ByteArrayOutputStream output = new ByteArrayOutputStream();
            WorkshopProjectArchive.ExportSummary summary = WorkshopProjectArchive.exportProject(root, output);
            require(summary.fileCount == 2, "expected metadata and source only");
            require(summary.totalBytes == new File(root, ".stasis-workshop.json").length()
                    + new File(root, "src/main.stasis").length(), "unexpected exported byte count");
            List<String> entries = zipEntries(output.toByteArray());
            require(entries.equals(Arrays.asList(".stasis-workshop.json", "src/main.stasis")),
                    "archive entries were not deterministic: " + entries);

            File imported = new File(root.getParentFile(), root.getName() + "-imported");
            require(imported.mkdirs(), "import target mkdir failed");
            try {
                write(new File(imported, ".stasis-workshop.json"), "{\"format_version\":1,\"id\":\"fresh\"}\n");
                WorkshopProjectArchive.ImportSummary importedSummary = WorkshopProjectArchive.importProject(
                        new ByteArrayInputStream(output.toByteArray()), imported);
                require(importedSummary.fileCount == 2, "unexpected imported file count");
                require(new File(imported, "src/main.stasis").isFile(), "main source was not restored");
                String freshMetadata = new String(Files.readAllBytes(
                        new File(imported, ".stasis-workshop.json").toPath()), StandardCharsets.UTF_8);
                require(freshMetadata.contains("fresh"), "archive overwrote fresh project identity");

                boolean traversalRejected = false;
                try {
                    WorkshopProjectArchive.importProject(new ByteArrayInputStream(traversalArchive()), imported);
                } catch (Exception expected) {
                    traversalRejected = expected.getMessage().contains("path is invalid");
                }
                require(traversalRejected, "archive traversal was not rejected");
                require(!new File(imported.getParentFile(), "escaped.stasis").exists(), "archive escaped target root");
            } finally {
                deleteTree(imported);
            }

            File oversized = new File(root, "src/oversized.bin");
            RandomAccessFile random = new RandomAccessFile(oversized, "rw");
            try {
                random.setLength(WorkshopProjectArchive.MAX_ENTRY_BYTES + 1L);
            } finally {
                random.close();
            }
            boolean rejected = false;
            try {
                WorkshopProjectArchive.exportProject(root, new ByteArrayOutputStream());
            } catch (Exception expected) {
                rejected = expected.getMessage().contains("exceeds archive limit");
            }
            require(rejected, "oversized archive entry was not rejected");
            System.out.println("android project archive check ok");
        } finally {
            deleteTree(root);
        }
    }

    private static List<String> zipEntries(byte[] archive) throws Exception {
        ArrayList<String> entries = new ArrayList<>();
        ZipInputStream input = new ZipInputStream(new ByteArrayInputStream(archive));
        try {
            ZipEntry entry;
            while ((entry = input.getNextEntry()) != null) entries.add(entry.getName());
        } finally {
            input.close();
        }
        return entries;
    }

    private static byte[] traversalArchive() throws Exception {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        ZipOutputStream zip = new ZipOutputStream(bytes);
        try {
            zip.putNextEntry(new ZipEntry("../escaped.stasis"));
            zip.write("escape".getBytes(StandardCharsets.UTF_8));
            zip.closeEntry();
            zip.finish();
        } finally {
            zip.close();
        }
        return bytes.toByteArray();
    }

    private static void write(File file, String text) throws Exception {
        File parent = file.getParentFile();
        if (!parent.isDirectory() && !parent.mkdirs()) throw new IllegalStateException("mkdir failed");
        FileOutputStream output = new FileOutputStream(file);
        try {
            output.write(text.getBytes(StandardCharsets.UTF_8));
        } finally {
            output.close();
        }
    }

    private static void deleteTree(File file) {
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) for (File child : children) deleteTree(child);
        }
        file.delete();
    }

    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
