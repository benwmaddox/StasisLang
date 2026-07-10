package com.stasislang.workshop;

public final class WorkshopAssetIdentityCheck {
    public static void main(String[] args) {
        int expected = (int)0xa55f97e3L;
        int actual = WorkshopAssetIdentity.stableHandle("sprite", "ball");
        if (actual != expected) {
            throw new AssertionError("Android stable asset handle drifted: "
                    + Integer.toUnsignedString(actual, 16));
        }
        if (WorkshopAssetIdentity.stableHandle("audio", "ball") == actual) {
            throw new AssertionError("asset kind must participate in stable identity");
        }
        System.out.println("android asset identity ok");
    }
}
