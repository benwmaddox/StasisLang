package com.stasislang.workshop;

import java.nio.charset.StandardCharsets;

final class WorkshopAssetIdentity {
    private WorkshopAssetIdentity() {}

    static int stableHandle(String kind, String id) {
        int hash = 0x811c9dc5;
        byte[] identity = (kind + ":" + id).getBytes(StandardCharsets.US_ASCII);
        for (byte value : identity) {
            hash ^= value & 0xff;
            hash *= 0x01000193;
        }
        return hash == 0 ? 1 : hash;
    }
}
