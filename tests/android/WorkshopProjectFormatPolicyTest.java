package com.stasislang.workshop;

public final class WorkshopProjectFormatPolicyTest {
    public static void main(String[] args) {
        require("exploration".equals(WorkshopTemplateCatalog.DEFAULT_TEMPLATE_ID),
                "new installs default to exploration");
        require("pong".equals(WorkshopTemplateCatalog.LEGACY_TEMPLATE_ID),
                "legacy sample identity remains Pong");
        require(WorkshopTemplateCatalog.list().size() == 2
                && "exploration".equals(WorkshopTemplateCatalog.list().get(0).id)
                && WorkshopTemplateCatalog.isKnown("pong"), "catalog exposes exploration and Pong");
        require(WorkshopProjectFormatPolicy.supported(1), "v1 supported");
        require(WorkshopProjectFormatPolicy.supported(2), "v2 supported");
        require(WorkshopProjectFormatPolicy.supported(3), "v3 supported");
        require(!WorkshopProjectFormatPolicy.supported(4), "future version rejected");
        require("pong".equals(WorkshopProjectFormatPolicy.templateId(1, "sample", "")),
                "v1 sample migrates to Pong");
        require("pong".equals(WorkshopProjectFormatPolicy.templateId(2, "sample", "")),
                "v2 sample migrates to Pong");
        require("exploration".equals(WorkshopProjectFormatPolicy.templateId(3, "sample", "exploration")),
                "v3 preserves explicit template");
        require("".equals(WorkshopProjectFormatPolicy.templateId(3, "import", "pong")),
                "imports are template-free");
        require(".stasis-workshop.json.v1.bak".equals(WorkshopProjectFormatPolicy.backupFileName(1)),
                "v1 backup name");
        require(".stasis-workshop.json.v2.bak".equals(WorkshopProjectFormatPolicy.backupFileName(2)),
                "v2 backup name");
        System.out.println("android project format policy ok");
    }

    private static void require(boolean condition, String name) {
        if (!condition) throw new AssertionError(name);
    }
}
