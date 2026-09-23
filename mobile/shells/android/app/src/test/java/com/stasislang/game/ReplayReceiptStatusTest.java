package @STASIS_PACKAGE_ID@;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class ReplayReceiptStatusTest {
    @Test
    public void onlyVerifiedCompletionUsesSuccessStatus() {
        assertTrue(MainActivity.isVerifiedReplayCompletionReceipt("Replay complete at tick 4"));
        assertFalse(MainActivity.isVerifiedReplayCompletionReceipt(null));
        assertFalse(MainActivity.isVerifiedReplayCompletionReceipt(""));
        assertFalse(MainActivity.isVerifiedReplayCompletionReceipt(
                "Replay diverged at tick 4: state hash mismatch"));
        assertFalse(MainActivity.isVerifiedReplayCompletionReceipt(
                "Replay identity_mismatch at tick 0: compatibility differs"));
    }

    @Test
    public void replayFailureCodeIsReadableInTheHostStatus() {
        assertEquals("Replay diverged at tick 120: final state hash differs",
                MainActivity.formatReplayReceiptForDisplay(
                        "Replay replay_diverged at tick 120: final state hash differs"));
        assertEquals("Replay complete at tick 120",
                MainActivity.formatReplayReceiptForDisplay("Replay complete at tick 120"));
    }
}
