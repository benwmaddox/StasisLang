package com.stasislang.workshop;

public final class AiQueuePolicyTest {
    public static void main(String[] args) {
        require(AiQueuePolicy.validProjectId("project-1"), "valid project id");
        require(!AiQueuePolicy.validProjectId("../project"), "confined project id");
        require(AiQueuePolicy.validSource("text") && AiQueuePolicy.validSource("voice"), "typed/voice parity");
        require(!AiQueuePolicy.validSource("audio"), "unknown source rejected");
        require(AiQueuePolicy.canTransition("pending", "in_progress"), "claim transition");
        require(AiQueuePolicy.canTransition("pending", "cancelled"), "pending cancellation");
        require(!AiQueuePolicy.canTransition("pending", "completed"), "pending cannot complete directly");
        require(AiQueuePolicy.canTransition("in_progress", "completed"), "completion transition");
        require(AiQueuePolicy.canTransition("in_progress", "failed"), "failure transition");
        require(!AiQueuePolicy.canTransition("completed", "pending"), "terminal state immutable");
        require(AiQueuePolicy.terminal("completed") && AiQueuePolicy.terminal("failed")
                && AiQueuePolicy.terminal("cancelled"), "terminal records are prune eligible");
        require(!AiQueuePolicy.terminal("pending") && !AiQueuePolicy.terminal("in_progress"),
                "paid or pending work is never prune eligible");
        require(AiQueuePolicy.nextPendingIndex("alpha",
                new String[] { "beta", "alpha", "alpha" },
                new String[] { "pending", "cancelled", "pending" }) == 2,
                "FIFO respects project isolation and terminal items");
        require(AiQueuePolicy.nextPendingIndex("gamma",
                new String[] { "alpha" }, new String[] { "pending" }) == -1,
                "another project is never claimed");
        require("failed".equals(AiQueuePolicy.recoveredState("in_progress", false)),
                "unsafe interrupted item fails explicitly");
        require("pending".equals(AiQueuePolicy.recoveredState("in_progress", true)),
                "safe interrupted item resumes through FIFO");
        require("pending".equals(AiQueuePolicy.recoveredState("pending", false)),
                "pending item survives recovery");
        require(AiQueuePolicy.retryNeedsNewPreview(true), "terminal preview retry requires new consent");
        require(!AiQueuePolicy.retryNeedsNewPreview(false), "non-preview terminal item can retry");
        System.out.println("android AI queue policy ok");
    }

    private static void require(boolean condition, String name) {
        if (!condition) throw new AssertionError(name);
    }
}
