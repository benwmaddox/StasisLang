package com.stasislang.workshop;

final class WorkshopProjectFormatPolicy {
    static final int CURRENT_VERSION = 3;
    private WorkshopProjectFormatPolicy() {}

    static boolean supported(int version) {
        return version == 1 || version == 2 || version == CURRENT_VERSION;
    }

    static String templateId(int version, String origin, String declaredTemplateId) {
        if ("import".equals(origin)) return "";
        if (!"sample".equals(origin)) throw new IllegalArgumentException("project origin is invalid");
        return version < CURRENT_VERSION
                ? WorkshopTemplateCatalog.LEGACY_TEMPLATE_ID
                : (declaredTemplateId == null ? "" : declaredTemplateId.trim());
    }

    static String backupFileName(int sourceVersion) {
        if (sourceVersion == 1) return ".stasis-workshop.json.v1.bak";
        if (sourceVersion == 2) return ".stasis-workshop.json.v2.bak";
        throw new IllegalArgumentException("current or unknown project format does not need migration backup");
    }
}
