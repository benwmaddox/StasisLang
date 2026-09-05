package com.stasislang.workshop;

import com.stasislang.workshop.WorkshopProjectBaselinePolicy.Action;

public final class WorkshopProjectBaselinePolicyCheck {
    public static void main(String[] args) {
        require(Action.KEEP,
                WorkshopProjectBaselinePolicy.requiredAction(true, true, true),
                "an exact marker must keep the imported baseline");
        require(Action.UPDATE_MARKER,
                WorkshopProjectBaselinePolicy.requiredAction(true, true, false),
                "a stale imported marker must preserve baseline contents");
        require(Action.REBUILD,
                WorkshopProjectBaselinePolicy.requiredAction(true, false, false),
                "a fresh import must initialize its baseline from current source");
        require(Action.REBUILD,
                WorkshopProjectBaselinePolicy.requiredAction(false, true, false),
                "a stale sample marker must refresh from bundled assets");
        System.out.println("android project baseline policy check ok");
    }

    private static void require(Action expected, Action actual, String message) {
        if (expected != actual) {
            throw new AssertionError(message + ": expected " + expected + ", got " + actual);
        }
    }
}
