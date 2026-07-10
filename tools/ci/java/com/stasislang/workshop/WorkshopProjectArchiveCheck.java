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
