package com.stasislang.workshop;

final class WorkshopBundledSourceUpgrade {
    private WorkshopBundledSourceUpgrade() {}

    static boolean shouldReplace(String current, String previousBaseline) {
        return current != null && current.equals(previousBaseline);
    }
}
