package com.stasislang.shell;

public final class NetworkJoinPolicyTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError("network provisioning component policy");
    }

    public static void main(String[] args) {
        String app = "com.example.game";
        check(NetworkJoinPolicy.acceptsComponent(app, app, app + ".NetworkJoin"));
        check(!NetworkJoinPolicy.acceptsComponent(app, app, app + ".MainActivity"));
        check(!NetworkJoinPolicy.acceptsComponent(app, "untrusted.app", app + ".NetworkJoin"));
        check(!NetworkJoinPolicy.acceptsComponent(app, app, "untrusted.app.NetworkJoin"));
        check(!NetworkJoinPolicy.acceptsComponent(app, app, ".NetworkJoin"));
        check(!NetworkJoinPolicy.acceptsComponent(app, null, null));
        check(!NetworkJoinPolicy.acceptsComponent(null, null, null));
        System.out.println("network join component policy: 7 scenarios passed");
    }
}
