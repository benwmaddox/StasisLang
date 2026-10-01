package com.stasislang.workshop;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

public final class WorkshopEditorScrollTargetTest {
    @Test
    public void nestedSourceCaretUsesAncestorCoordinatesAfterImeResize() {
        int viewportHeightAfterImeResize = 800;
        int contentHeight = 6400;

        // These are the production nesting boundaries: content -> manual edit body -> symbol list
        // -> selected source panel -> editor. The old getBottom() value omitted every ancestor.
        int[] ancestorOffsets = {1540, 920, 610, 430};
        int localCaretTop = 92;
        int localCaretBottom = 116;
        int contentCaretTop = localCaretTop;
        int contentCaretBottom = localCaretBottom;
        for (int offset : ancestorOffsets) {
            contentCaretTop += offset;
            contentCaretBottom += offset;
        }

        int oldScrollY = 430 + 640;
        assertTrue(contentCaretTop < oldScrollY
                || contentCaretTop >= oldScrollY + viewportHeightAfterImeResize);

        int correctedScrollY = WorkshopEditorScrollTarget.forVisibleRect(contentCaretTop,
                contentCaretBottom, 0, viewportHeightAfterImeResize, contentHeight);
        assertEquals(contentCaretBottom - viewportHeightAfterImeResize, correctedScrollY);
        assertTrue(contentCaretTop >= correctedScrollY);
        assertTrue(contentCaretBottom <= correctedScrollY + viewportHeightAfterImeResize);
    }

    @Test
    public void keepsVisibleCaretStableAndClampsAtContentEnd() {
        assertEquals(2400, WorkshopEditorScrollTarget.forVisibleRect(
                2500, 2520, 2400, 800, 5000));
        assertEquals(4200, WorkshopEditorScrollTarget.forVisibleRect(
                4970, 5000, 4000, 800, 5000));
    }

    @Test
    public void delayedFocusScrollIsCancelledAfterBlurButTutorialCanRevealSourceTop() {
        assertEquals(false, WorkshopEditorScrollTarget.shouldScroll(true, false));
        assertEquals(true, WorkshopEditorScrollTarget.shouldScroll(true, true));
        assertEquals(true, WorkshopEditorScrollTarget.shouldScroll(false, false));
    }
}
