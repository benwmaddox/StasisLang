package com.stasislang.workshop;

final class CanonicalSymbolIdentity {
    private CanonicalSymbolIdentity() {}

    static boolean matchesRustItem(
            String kind, String file, String name, String signature, int start, int end,
            String rustKind, String rustFile, String rustName, String rustSignature,
            int[] rustSpanStarts, int[] rustSpanEnds) {
        if (!kind.equals(rustKind)
                || !file.equals(rustFile)
                || !name.equals(rustName)
                || !signature.equals(rustSignature)
                || rustSpanStarts.length != rustSpanEnds.length) {
            return false;
        }
        for (int index = 0; index < rustSpanStarts.length; index += 1) {
            if (rustSpanStarts[index] <= start && rustSpanEnds[index] >= end) {
                return true;
            }
        }
        return false;
    }

    static String identityKey(
            String canonicalSymbolId, String kind, String file, String owner, String name) {
        if (!canonicalSymbolId.isEmpty()) return canonicalSymbolId;
        return legacySchemaV1IdentityKey(kind, file, owner, name);
    }

    static boolean sameIdentity(
            String leftCanonical, String leftKind, String leftFile, String leftOwner, String leftName,
            String rightCanonical, String rightKind, String rightFile, String rightOwner, String rightName) {
        if (!leftCanonical.isEmpty() || !rightCanonical.isEmpty()) {
            return !leftCanonical.isEmpty() && leftCanonical.equals(rightCanonical);
        }
        return legacySchemaV1IdentityKey(leftKind, leftFile, leftOwner, leftName)
                .equals(legacySchemaV1IdentityKey(
                        rightKind, rightFile, rightOwner, rightName));
    }

    private static String legacySchemaV1IdentityKey(
            String kind, String file, String owner, String name) {
        return kind + "|" + file + "|" + owner + "|" + name;
    }
}
