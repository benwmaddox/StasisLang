package com.stasislang.workshop;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import java.util.UUID;

final class WorkshopProjectRegistry {
    static final int FORMAT_VERSION = 2;
    static final String LEGACY_PROJECT_DIR = "workshop_project";
    private static final String PROJECTS_DIR = "workshop_projects";
    static final String METADATA_FILE = ".stasis-workshop.json";
    private static final String V1_BACKUP_FILE = ".stasis-workshop.json.v1.bak";
    private static final String PREFS = "workshop_project_registry";
    private static final String PREF_ACTIVE_PROJECT = "active_project_directory";

    private WorkshopProjectRegistry() {}

    static ProjectInfo initialize(Context context) throws Exception {
        File legacyRoot = new File(context.getFilesDir(), LEGACY_PROJECT_DIR);
        if (!legacyRoot.isDirectory() && !legacyRoot.mkdirs()) {
            throw new IllegalStateException("unable to create legacy project directory");
        }
        File legacyMetadata = new File(legacyRoot, METADATA_FILE);
        if (!legacyMetadata.isFile()) {
            writeMetadata(legacyRoot, new ProjectInfo(
                    "bundled-workshop", "Bundled Workshop", "sample", LEGACY_PROJECT_DIR, legacyRoot));
        }
        String activeDirectory = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                .getString(PREF_ACTIVE_PROJECT, LEGACY_PROJECT_DIR);
        ProjectInfo active = findByDirectory(list(context), activeDirectory);
        if (active == null) active = readMetadata(context, legacyRoot, LEGACY_PROJECT_DIR);
        setActive(context, active);
        return active;
    }

    static List<ProjectInfo> list(Context context) throws Exception {
        ArrayList<ProjectInfo> projects = new ArrayList<>();
        File legacyRoot = new File(context.getFilesDir(), LEGACY_PROJECT_DIR);
        if (new File(legacyRoot, METADATA_FILE).isFile()) {
            projects.add(readMetadata(context, legacyRoot, LEGACY_PROJECT_DIR));
        }
        File projectsRoot = new File(context.getFilesDir(), PROJECTS_DIR);
        File[] children = projectsRoot.listFiles();
        if (children != null) {
            for (File child : children) {
                if (!child.isDirectory() || !new File(child, METADATA_FILE).isFile()) continue;
                projects.add(readMetadata(context, child, PROJECTS_DIR + "/" + child.getName()));
            }
        }
        Collections.sort(projects, new Comparator<ProjectInfo>() {
            @Override public int compare(ProjectInfo left, ProjectInfo right) {
                return left.name.compareToIgnoreCase(right.name);
            }
        });
        return projects;
    }

    static ProjectInfo createFromSample(Context context, String requestedName) throws Exception {
        return createProject(context, requestedName, "sample");
    }

    static ProjectInfo createForImport(Context context, String requestedName) throws Exception {
        return createProject(context, requestedName, "import");
    }

    private static ProjectInfo createProject(Context context, String requestedName, String origin) throws Exception {
        String name = requestedName == null ? "" : requestedName.trim();
        validateRequestedName(name);
        File projectsRoot = new File(context.getFilesDir(), PROJECTS_DIR);
        if (!projectsRoot.isDirectory() && !projectsRoot.mkdirs()) {
            throw new IllegalStateException("unable to create projects directory");
        }
        String base = slug(name);
        File root = new File(projectsRoot, base);
        for (int suffix = 2; root.exists(); suffix += 1) root = new File(projectsRoot, base + "-" + suffix);
        if (!root.mkdirs()) throw new IllegalStateException("unable to create project directory");
        String relativeDirectory = PROJECTS_DIR + "/" + root.getName();
        ProjectInfo project = new ProjectInfo(UUID.randomUUID().toString(), name, origin, relativeDirectory, root);
        try {
            writeMetadata(root, project);
        } catch (Exception error) {
            if (!root.delete() && root.exists()) error.addSuppressed(new IllegalStateException("empty project cleanup failed"));
            throw error;
        }
        return project;
    }

    static void validateRequestedName(String requestedName) {
        String name = requestedName == null ? "" : requestedName.trim();
        if (name.isEmpty() || name.length() > 80 || name.indexOf('/') >= 0 || name.indexOf('\\') >= 0) {
            throw new IllegalArgumentException("project name must be 1-80 characters without slashes");
        }
    }

    static void deleteFailedImport(Context context, ProjectInfo project) throws Exception {
        validateProjectRoot(context, project.root);
        if (LEGACY_PROJECT_DIR.equals(project.directoryName)) {
            throw new IllegalArgumentException("legacy project cannot be deleted as a failed import");
        }
        deleteTree(project.root);
        if (project.root.exists()) throw new IllegalStateException("failed import cleanup did not complete");
    }

    static void setActive(Context context, ProjectInfo project) throws Exception {
        validateProjectRoot(context, project.root);
        if (!new File(project.root, METADATA_FILE).isFile()) {
            throw new IllegalArgumentException("project metadata is missing");
        }
        SharedPreferences preferences = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        if (!preferences.edit().putString(PREF_ACTIVE_PROJECT, project.directoryName).commit()) {
            throw new IllegalStateException("active project preference commit failed");
        }
    }

    private static ProjectInfo findByDirectory(List<ProjectInfo> projects, String directory) {
        for (ProjectInfo project : projects) {
            if (project.directoryName.equals(directory)) return project;
        }
        return null;
    }

    private static ProjectInfo readMetadata(Context context, File root, String directoryName) throws Exception {
        validateProjectRoot(context, root);
        JSONObject json = new JSONObject(readFile(new File(root, METADATA_FILE)));
        int version = json.optInt("format_version", 0);
        if (version != 1 && version != FORMAT_VERSION) {
            throw new IllegalStateException("unsupported project format version " + version
                    + "; update the Workshop before opening this project");
        }
        String id = json.optString("id", "").trim();
        String name = json.optString("name", "").trim();
        boolean originMissing = !json.has("origin");
        String origin = json.optString("origin", "sample").trim();
        if (id.isEmpty() || name.isEmpty()) throw new IllegalStateException("project metadata needs id and name");
        if (!id.matches("[A-Za-z0-9][A-Za-z0-9-]{0,79}")) throw new IllegalStateException("project metadata id is invalid");
        if (!"sample".equals(origin) && !"import".equals(origin)) throw new IllegalStateException("project metadata origin is invalid");
        if (version == FORMAT_VERSION
                && !"stasis-workshop-project".equals(json.optString("schema", ""))) {
            throw new IllegalStateException("project format 2 metadata schema is invalid");
        }
        ProjectInfo project = new ProjectInfo(id, name, origin, directoryName, root);
        if (version == 1) {
            migrateV1Metadata(root, project);
        } else if (originMissing) {
            throw new IllegalStateException("project format 2 metadata origin is missing");
        }
        return project;
    }

    private static void writeMetadata(File root, ProjectInfo project) throws Exception {
        File target = new File(root, METADATA_FILE);
        if (target.exists()) throw new IllegalStateException("project metadata already exists");
        writeMetadataTemporary(root, project, target, 0);
    }

    static void deleteProject(Context context, ProjectInfo project) throws Exception {
        validateProjectRoot(context, project.root);
        if (LEGACY_PROJECT_DIR.equals(project.directoryName)) {
            throw new IllegalArgumentException("bundled project cannot be deleted");
        }
        String activeDirectory = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                .getString(PREF_ACTIVE_PROJECT, LEGACY_PROJECT_DIR);
        if (project.directoryName.equals(activeDirectory)) {
            throw new IllegalStateException("switch away from a project before deleting it");
        }
        deleteTree(project.root);
        if (project.root.exists()) throw new IllegalStateException("project directory deletion did not complete");
    }

    private static void migrateV1Metadata(File root, ProjectInfo project) throws Exception {
        File source = new File(root, METADATA_FILE);
        File backup = new File(root, V1_BACKUP_FILE);
        try {
            if (!backup.isFile()) writeSyncedFile(backup, readFile(source));
            replaceMetadata(root, project, 1);
            JSONObject migrated = new JSONObject(readFile(source));
            if (migrated.optInt("format_version", 0) != FORMAT_VERSION
                    || !project.id.equals(migrated.optString("id", ""))) {
                throw new IllegalStateException("migrated metadata verification failed");
            }
        } catch (Exception error) {
            throw new IllegalStateException("project v1 migration failed; the fsynced v1 backup was preserved", error);
        }
    }

    private static void replaceMetadata(File root, ProjectInfo project, int migratedFromVersion) throws Exception {
        File target = new File(root, METADATA_FILE);
        File temporary = writeMetadataTemporary(root, project, null, migratedFromVersion);
        try {
            Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
        } catch (AtomicMoveNotSupportedException unsupported) {
            Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.REPLACE_EXISTING);
        }
    }

    private static File writeMetadataTemporary(File root, ProjectInfo project, File publishTarget,
            int migratedFromVersion) throws Exception {
        JSONObject json = new JSONObject()
                .put("format_version", FORMAT_VERSION)
                .put("schema", "stasis-workshop-project")
                .put("id", project.id)
                .put("name", project.name)
                .put("origin", project.origin)
                .put("migrated_from_version", migratedFromVersion);
        File temporary = new File(root, METADATA_FILE + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write((json.toString(2) + "\n").getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
        if (publishTarget == null) return temporary;
        if (!temporary.renameTo(publishTarget)) {
            temporary.delete();
            throw new IllegalStateException("unable to publish project metadata");
        }
        return publishTarget;
    }

    private static void writeSyncedFile(File file, String source) throws Exception {
        File temporary = new File(file.getParentFile(), file.getName() + ".tmp");
        FileOutputStream output = new FileOutputStream(temporary);
        try {
            output.write(source.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        } finally {
            output.close();
        }
        try {
            Files.move(temporary.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE);
        } catch (AtomicMoveNotSupportedException unsupported) {
            Files.move(temporary.toPath(), file.toPath());
        }
    }

    private static void validateProjectRoot(Context context, File root) throws Exception {
        String filesPath = context.getFilesDir().getCanonicalPath();
        String rootPath = root.getCanonicalPath();
        if (!rootPath.startsWith(filesPath + File.separator)) {
            throw new IllegalArgumentException("project root must stay in app-private storage");
        }
        String relative = rootPath.substring(filesPath.length() + 1).replace(File.separatorChar, '/');
        boolean registeredChild = relative.startsWith(PROJECTS_DIR + "/")
                && relative.substring(PROJECTS_DIR.length() + 1).indexOf('/') < 0;
        if (!LEGACY_PROJECT_DIR.equals(relative) && !registeredChild) {
            throw new IllegalArgumentException("project root is outside the registry");
        }
    }

    private static String slug(String value) {
        String lower = value.toLowerCase(Locale.US);
        StringBuilder result = new StringBuilder();
        boolean separator = false;
        for (int index = 0; index < lower.length(); index += 1) {
            char character = lower.charAt(index);
            if ((character >= 'a' && character <= 'z') || (character >= '0' && character <= '9')) {
                result.append(character);
                separator = false;
            } else if (!separator && result.length() > 0) {
                result.append('-');
                separator = true;
            }
        }
        while (result.length() > 0 && result.charAt(result.length() - 1) == '-') result.setLength(result.length() - 1);
        return result.length() == 0 ? "project" : result.toString();
    }

    private static String readFile(File file) throws Exception {
        FileInputStream input = new FileInputStream(file);
        try {
            ByteArrayOutputStream output = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            int read;
            while ((read = input.read(buffer)) >= 0) output.write(buffer, 0, read);
            return output.toString("UTF-8");
        } finally {
            input.close();
        }
    }

    private static void deleteTree(File file) {
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) for (File child : children) deleteTree(child);
        }
        file.delete();
    }

    static final class ProjectInfo {
        final String id;
        final String name;
        final String origin;
        final String directoryName;
        final File root;

        ProjectInfo(String id, String name, String origin, String directoryName, File root) {
            this.id = id;
            this.name = name;
            this.origin = origin;
            this.directoryName = directoryName;
            this.root = root;
        }

        @Override public String toString() {
            return name;
        }
    }
}
