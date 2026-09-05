package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.nio.ByteBuffer;

import org.junit.Test;

public final class WorkshopJniFrameAbiAcceptanceTest {
    @Test
    public void invalidScenarioSetIsDeterministic() {
        assertTrue(java.util.Arrays.equals(
                new String[] {"short_i32", "short_f32", "short_u8", "oversized_i32", "oversized_f32",
                        "oversized_u8", "swapped_i32_f32", "wrong_order_i32", "wrong_order_f32", "wrong_order_u8",
                        "heap_i32", "heap_f32", "heap_u8", "null_i32", "null_f32", "null_u8",
                        "misaligned_i32", "misaligned_f32"},
                WorkshopJniFrameAbiAcceptance.invalidScenarioNames()));
    }

    @Test
    public void canaryAndStructuredErrorHelpersRejectMutationOrShape() {
        ByteBuffer parent = ByteBuffer.allocate(8);
        for (int index = 0; index < parent.capacity(); index++) parent.put(index, (byte)0x5a);
        assertTrue(WorkshopJniFrameAbiAcceptance.guardsIntact(parent, 2, 4, (byte)0x5a));
        parent.put(1, (byte)0);
        assertFalse(WorkshopJniFrameAbiAcceptance.guardsIntact(parent, 2, 4, (byte)0x5a));
        assertTrue(WorkshopJniFrameAbiAcceptance.isStructuredError(
                "{\"schema\":\"stasis.workshop_jni_frame_abi.v1\",\"test_id\":\"IT-026\","
                        + "\"event\":\"error\",\"lane\":\"i32\",\"reason\":\"capacity\","
                        + "\"expected\":4,\"actual\":3}"));
        assertFalse(WorkshopJniFrameAbiAcceptance.isStructuredError("{\"reason\":\"capacity\"}"));
    }

    @Test
    public void descriptorEnvelopeRequiresCanonicalSchema() {
        String descriptor = "{\"schema\":\"stasis.workshop_jni_frame_abi.v1\","
                + "\"test_id\":\"IT-026\",\"event\":\"descriptor\",\"lanes\":[]}";
        assertTrue(WorkshopJniFrameAbiAcceptance.isDescriptorEnvelope(descriptor));
        assertFalse(WorkshopJniFrameAbiAcceptance.isDescriptorEnvelope(
                descriptor.replace("stasis.workshop_jni_frame_abi.v1", "wrong.schema")));
    }
}
