package com.stasislang.shell;

/** Selects the component whose signature permission Android enforces. */
public final class NetworkJoinPolicy {
    private NetworkJoinPolicy() {}

    public static boolean acceptsComponent(String applicationPackage,
            String componentPackage, String componentClass) {
        return applicationPackage != null
                && applicationPackage.equals(componentPackage)
                && (applicationPackage + ".NetworkJoin").equals(componentClass);
    }
}
