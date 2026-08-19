package com.stasislang.workshop;

import android.util.Log;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

import org.json.JSONArray;
import org.json.JSONObject;

/** Acceptance-only proof that JNI frame lanes reject unsafe buffer shapes. */
final class WorkshopJniFrameAbiAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    private static final int GUARD_BYTES = 16;
    private static final byte GUARD = 0x5a;
    private static final int INNER = 0xa5;

    private WorkshopJniFrameAbiAcceptance() {}

    static String[] invalidScenarioNames() {
        return new String[] {"short_i32", "short_f32", "short_u8", "oversized_i32", "oversized_f32",
                "oversized_u8", "swapped_i32_f32", "wrong_order_i32", "wrong_order_f32", "wrong_order_u8",
                "heap_i32", "heap_f32", "heap_u8", "null_i32", "null_f32", "null_u8",
                "misaligned_i32", "misaligned_f32"};
    }

    static boolean isStructuredError(String value) {
        try {
            JSONObject error = new JSONObject(value);
            return "stasis.workshop_jni_frame_abi.v1".equals(error.optString("schema"))
                    && "IT-026".equals(error.optString("test_id"))
                    && "error".equals(error.optString("event"))
                    && error.has("lane") && error.has("reason")
                    && error.has("expected") && error.has("actual");
        } catch (Exception ignored) {
            return false;
        }
    }

    static boolean isDescriptorEnvelope(String value) {
        try {
            JSONObject descriptor = new JSONObject(value);
            return "stasis.workshop_jni_frame_abi.v1".equals(descriptor.optString("schema"))
                    && "IT-026".equals(descriptor.optString("test_id"))
                    && "descriptor".equals(descriptor.optString("event"));
        } catch (Exception ignored) {
            return false;
        }
    }

    static boolean guardsIntact(ByteBuffer parent, int offset, int length, byte guard) {
        if (parent == null) return true;
        for (int index = 0; index < offset; index++) if (parent.get(index) != guard) return false;
        for (int index = offset + length; index < parent.capacity(); index++) {
            if (parent.get(index) != guard) return false;
        }
        return true;
    }

    static String run(String projectRoot) {
        JSONArray invalid = new JSONArray();
        try {
            FrameDescriptor descriptor = readDescriptor();
            Guarded exact = Guarded.exact(descriptor);
            int status = MainActivity.nativeRunFrameInto(projectRoot, 0, 0, 0, 320, 240,
                    exact.i32, exact.f32, exact.u8);
            if (status != 0 || !exact.guardsIntact() || !exact.innerChanged()) {
                return fail("exact frame did not write only inside its guards: status=" + status);
            }

            checkInvalid(projectRoot, "short_i32", invalid,
                    Guarded.withCapacities(descriptor.i32Bytes - 1, descriptor.f32Bytes,
                            descriptor.u8Bytes));
            checkInvalid(projectRoot, "short_f32", invalid,
                    Guarded.withCapacities(descriptor.i32Bytes, descriptor.f32Bytes - 1,
                            descriptor.u8Bytes));
            checkInvalid(projectRoot, "short_u8", invalid,
                    Guarded.withCapacities(descriptor.i32Bytes, descriptor.f32Bytes,
                            descriptor.u8Bytes - 1));
            checkInvalid(projectRoot, "oversized_i32", invalid,
                    Guarded.withCapacities(descriptor.i32Bytes + 1, descriptor.f32Bytes,
                            descriptor.u8Bytes));
            checkInvalid(projectRoot, "oversized_f32", invalid,
                    Guarded.withCapacities(descriptor.i32Bytes, descriptor.f32Bytes + 1,
                            descriptor.u8Bytes));
            checkInvalid(projectRoot, "oversized_u8", invalid,
                    Guarded.withCapacities(descriptor.i32Bytes, descriptor.f32Bytes,
                            descriptor.u8Bytes + 1));
            Guarded swapped = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "swapped_i32_f32", invalid, swapped,
                    swapped.f32, swapped.i32, swapped.u8);
            checkInvalid(projectRoot, "wrong_order_i32", invalid, Guarded.wrongOrder(descriptor, "i32"));
            checkInvalid(projectRoot, "wrong_order_f32", invalid, Guarded.wrongOrder(descriptor, "f32"));
            checkInvalid(projectRoot, "wrong_order_u8", invalid, Guarded.wrongOrder(descriptor, "u8"));
            Guarded heapI32 = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "heap_i32", invalid, heapI32,
                    ByteBuffer.allocate(descriptor.i32Bytes).order(ByteOrder.nativeOrder()),
                    heapI32.f32, heapI32.u8);
            Guarded heapF32 = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "heap_f32", invalid, heapF32,
                    heapF32.i32, ByteBuffer.allocate(descriptor.f32Bytes).order(ByteOrder.nativeOrder()), heapF32.u8);
            Guarded heapU8 = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "heap_u8", invalid, heapU8,
                    heapU8.i32, heapU8.f32, ByteBuffer.allocate(descriptor.u8Bytes).order(ByteOrder.nativeOrder()));
            Guarded nullI32 = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "null_i32", invalid, nullI32,
                    null, nullI32.f32, nullI32.u8);
            Guarded nullF32 = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "null_f32", invalid, nullF32,
                    nullF32.i32, null, nullF32.u8);
            Guarded nullU8 = Guarded.exact(descriptor);
            checkInvalid(projectRoot, "null_u8", invalid, nullU8,
                    nullU8.i32, nullU8.f32, null);
            Guarded misalignedI32 = Guarded.misaligned(descriptor, "i32");
            checkInvalid(projectRoot, "misaligned_i32", invalid, misalignedI32,
                    misalignedI32.i32, misalignedI32.f32, misalignedI32.u8);
            Guarded misalignedF32 = Guarded.misaligned(descriptor, "f32");
            checkInvalid(projectRoot, "misaligned_f32", invalid, misalignedF32,
                    misalignedF32.i32, misalignedF32.f32, misalignedF32.u8);

            JSONObject marker = new JSONObject()
                    .put("schema", "stasis.workshop_jni_frame_abi.v1")
                    .put("test_id", "IT-026")
                    .put("event", "buffer_abi")
                    .put("status", "passed")
                    .put("descriptor", descriptor.json)
                    .put("valid_guards_intact", exact.guardsIntact())
                    .put("all_invalid_unchanged", allUnchanged(invalid))
                    .put("valid_calls", 1)
                    .put("invalid_calls", invalid.length());
            String output = marker.toString();
            Log.i(LOG_TAG, "Stasis Workshop IT-026: " + output);
            for (int index = 0; index < invalid.length(); index++) {
                Log.i(LOG_TAG, "Stasis Workshop IT-026 case: "
                        + invalid.getJSONObject(index).toString());
            }
            return output;
        } catch (Exception error) {
            return fail(error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage());
        }
    }

    private static FrameDescriptor readDescriptor() throws Exception {
        String descriptorJson = MainActivity.nativeFrameAbiDescriptor();
        if (!isDescriptorEnvelope(descriptorJson)) {
            throw new IllegalStateException("invalid canonical JNI frame descriptor");
        }
        JSONObject response = new JSONObject(descriptorJson);
        JSONArray lanes = response.getJSONArray("lanes");
        if (lanes.length() != 3) throw new IllegalStateException("incomplete JNI frame descriptor");
        FrameDescriptor descriptor = new FrameDescriptor(lanes.getJSONObject(0), lanes.getJSONObject(1),
                lanes.getJSONObject(2));
        if (descriptor.i32Bytes != StasisPreviewRenderer.FRAME_I32_CAPACITY * 4
                || descriptor.f32Bytes != StasisPreviewRenderer.FRAME_F32_CAPACITY * 4
                || descriptor.u8Bytes != StasisPreviewRenderer.TEXT_U8_CAPACITY
                || descriptor.i32Alignment != Integer.BYTES || descriptor.f32Alignment != Float.BYTES
                || descriptor.u8Alignment != 1) {
            throw new IllegalStateException("renderer/JNI frame descriptor mismatch");
        }
        return descriptor;
    }

    private static void checkInvalid(String projectRoot, String name, JSONArray results,
            Guarded guarded) throws Exception {
        checkInvalid(projectRoot, name, results, guarded, guarded.i32, guarded.f32, guarded.u8);
    }

    private static void checkInvalid(String projectRoot, String name, JSONArray results,
            Guarded guarded, ByteBuffer i32, ByteBuffer f32, ByteBuffer u8) throws Exception {
        byte[] beforeI32 = snapshot(i32);
        byte[] beforeF32 = snapshot(f32);
        byte[] beforeU8 = snapshot(u8);
        int status = MainActivity.nativeRunFrameInto(projectRoot, 0, 0, 0, 320, 240,
                i32, f32, u8);
        String nativeError = MainActivity.nativeLastFrameError();
        JSONObject error = new JSONObject(nativeError);
        boolean unchanged = java.util.Arrays.equals(beforeI32, snapshot(i32))
                && java.util.Arrays.equals(beforeF32, snapshot(f32))
                && java.util.Arrays.equals(beforeU8, snapshot(u8));
        if (status == 0 || !guarded.guardsIntact() || !unchanged || !isStructuredError(nativeError)) {
            throw new IllegalStateException(name + " was not rejected without writes: status=" + status
                    + " unchanged=" + unchanged + " error=" + nativeError);
        }
        results.put(new JSONObject().put("schema", "stasis.workshop_jni_frame_abi.v1")
                .put("test_id", "IT-026").put("event", "case")
                .put("name", name).put("error", error).put("unchanged", unchanged));
    }

    private static byte[] snapshot(ByteBuffer buffer) {
        if (buffer == null) return null;
        byte[] bytes = new byte[buffer.capacity()];
        for (int index = 0; index < bytes.length; index++) bytes[index] = buffer.get(index);
        return bytes;
    }

    private static boolean allUnchanged(JSONArray results) {
        for (int index = 0; index < results.length(); index++) {
            if (!results.optJSONObject(index).optBoolean("unchanged", false)) return false;
        }
        return true;
    }

    private static String fail(String reason) {
        String output = "{\"schema\":\"stasis.workshop_jni_frame_abi.v1\","
                + "\"test_id\":\"IT-026\",\"event\":\"buffer_abi\","
                + "\"status\":\"failed\",\"error\":" + JSONObject.quote(reason) + "}";
        Log.e(LOG_TAG, "Stasis Workshop IT-026: " + output);
        return output;
    }

    private static final class FrameDescriptor {
        final int i32Bytes;
        final int f32Bytes;
        final int u8Bytes;
        final int i32Alignment;
        final int f32Alignment;
        final int u8Alignment;
        final JSONObject json;

        FrameDescriptor(JSONObject i32, JSONObject f32, JSONObject u8) throws Exception {
            if (!"i32".equals(i32.optString("lane")) || !"f32".equals(f32.optString("lane"))
                    || !"u8".equals(u8.optString("lane"))) {
                throw new IllegalStateException("JNI frame descriptor lanes are not canonical");
            }
            i32Bytes = i32.getInt("bytes");
            f32Bytes = f32.getInt("bytes");
            u8Bytes = u8.getInt("bytes");
            i32Alignment = i32.getInt("alignment");
            f32Alignment = f32.getInt("alignment");
            u8Alignment = u8.getInt("alignment");
            json = new JSONObject().put("lanes", new JSONArray()
                    .put(i32).put(f32).put(u8));
        }
    }

    private static final class Guarded {
        final ByteBuffer parentI32;
        final ByteBuffer parentF32;
        final ByteBuffer parentU8;
        final ByteBuffer i32;
        final ByteBuffer f32;
        final ByteBuffer u8;
        final int i32Offset;
        final int f32Offset;
        final int u8Offset;
        final int i32Capacity;
        final int f32Capacity;
        final int u8Capacity;

        private Guarded(ByteBuffer parentI32, ByteBuffer parentF32, ByteBuffer parentU8,
                ByteBuffer i32, ByteBuffer f32, ByteBuffer u8,
                int i32Offset, int f32Offset, int u8Offset) {
            this.parentI32 = parentI32;
            this.parentF32 = parentF32;
            this.parentU8 = parentU8;
            this.i32 = i32;
            this.f32 = f32;
            this.u8 = u8;
            this.i32Offset = i32Offset;
            this.f32Offset = f32Offset;
            this.u8Offset = u8Offset;
            i32Capacity = i32 == null ? 0 : i32.capacity();
            f32Capacity = f32 == null ? 0 : f32.capacity();
            u8Capacity = u8 == null ? 0 : u8.capacity();
        }

        static Guarded exact(FrameDescriptor descriptor) {
            return withCapacities(descriptor.i32Bytes, descriptor.f32Bytes, descriptor.u8Bytes);
        }

        static Guarded withCapacities(int i32Capacity, int f32Capacity, int u8Capacity) {
            ByteBuffer i32 = parent(i32Capacity, GUARD_BYTES);
            ByteBuffer f32 = parent(f32Capacity, GUARD_BYTES);
            ByteBuffer u8 = parent(u8Capacity, GUARD_BYTES);
            return new Guarded(i32, f32, u8, slice(i32, GUARD_BYTES, i32Capacity),
                    slice(f32, GUARD_BYTES, f32Capacity), slice(u8, GUARD_BYTES, u8Capacity),
                    GUARD_BYTES, GUARD_BYTES, GUARD_BYTES);
        }

        static Guarded misaligned(FrameDescriptor descriptor, String lane) {
            int i32Offset = "i32".equals(lane) ? 1 : GUARD_BYTES;
            int f32Offset = "f32".equals(lane) ? 1 : GUARD_BYTES;
            ByteBuffer parentI32 = parent(descriptor.i32Bytes, i32Offset);
            ByteBuffer parentF32 = parent(descriptor.f32Bytes, f32Offset);
            ByteBuffer parentU8 = parent(descriptor.u8Bytes, GUARD_BYTES);
            return new Guarded(parentI32, parentF32, parentU8,
                    slice(parentI32, i32Offset, descriptor.i32Bytes),
                    slice(parentF32, f32Offset, descriptor.f32Bytes),
                    slice(parentU8, GUARD_BYTES, descriptor.u8Bytes),
                    i32Offset, f32Offset, GUARD_BYTES);
        }

        static Guarded wrongOrder(FrameDescriptor descriptor, String lane) {
            Guarded guarded = exact(descriptor);
            ByteOrder wrong = ByteOrder.nativeOrder() == ByteOrder.BIG_ENDIAN
                    ? ByteOrder.LITTLE_ENDIAN : ByteOrder.BIG_ENDIAN;
            if ("i32".equals(lane)) guarded.i32.order(wrong);
            if ("f32".equals(lane)) guarded.f32.order(wrong);
            if ("u8".equals(lane)) guarded.u8.order(wrong);
            return guarded;
        }

        boolean guardsIntact() {
            return guardsIntact(parentI32, i32Offset, i32Capacity, GUARD)
                    && guardsIntact(parentF32, f32Offset, f32Capacity, GUARD)
                    && guardsIntact(parentU8, u8Offset, u8Capacity, GUARD);
        }
        boolean innerChanged() {
            return containsNonInner(i32) || containsNonInner(f32) || containsNonInner(u8);
        }


        private static boolean containsNonInner(ByteBuffer buffer) {
            for (int index = 0; index < buffer.capacity(); index++) if (buffer.get(index) != INNER) return true;
            return false;
        }

        private static ByteBuffer parent(int capacity, int offset) {
            ByteBuffer buffer = ByteBuffer.allocateDirect(capacity + offset + GUARD_BYTES)
                    .order(ByteOrder.nativeOrder());
            for (int index = 0; index < buffer.capacity(); index++) buffer.put(index, GUARD);
            return buffer;
        }

        private static ByteBuffer slice(ByteBuffer parent, int offset, int capacity) {
            ByteBuffer view = parent.duplicate();
            view.position(offset).limit(offset + capacity);
            ByteBuffer slice = view.slice().order(ByteOrder.nativeOrder());
            for (int index = 0; index < slice.capacity(); index++) slice.put(index, (byte)INNER);
            return slice;
        }
    }
}
