package com.stasislang.workshop;

final class WorkshopAiToolLoopPolicy {
    private final int maxReadOnlyBatches;
    private int consecutiveReadOnlyBatches;

    WorkshopAiToolLoopPolicy(int maxReadOnlyBatches) {
        if (maxReadOnlyBatches < 1) throw new IllegalArgumentException("read-only limit must be positive");
        this.maxReadOnlyBatches = maxReadOnlyBatches;
    }

    boolean shouldExecute(boolean hasWrites) {
        return hasWrites || consecutiveReadOnlyBatches < maxReadOnlyBatches;
    }

    void recordBatch(boolean hasWrites) {
        consecutiveReadOnlyBatches = hasWrites ? 0 : consecutiveReadOnlyBatches + 1;
    }

    boolean requiresWriteOrDone() {
        return consecutiveReadOnlyBatches >= maxReadOnlyBatches;
    }
}
