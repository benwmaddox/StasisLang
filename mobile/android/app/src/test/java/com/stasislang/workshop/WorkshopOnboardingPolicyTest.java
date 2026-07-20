package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopOnboardingPolicyTest {
    @Test
    public void workflowRequiresOrderedActionsFromOneProjectAndChange() {
        WorkshopOnboardingPolicy.Progress progress = WorkshopOnboardingPolicy.fresh();
        assertSame(progress, WorkshopOnboardingPolicy.recordProjectStep(
                progress, WorkshopOnboardingPolicy.Step.PROJECT_RAN, "project-a"));

        progress = WorkshopOnboardingPolicy.recordWelcome(progress);
        progress = WorkshopOnboardingPolicy.recordProjectOpened(progress, "project-a");
        assertSame(progress, WorkshopOnboardingPolicy.recordProjectStep(
                progress, WorkshopOnboardingPolicy.Step.PROJECT_RAN, "project-b"));
        progress = WorkshopOnboardingPolicy.recordProjectStep(
                progress, WorkshopOnboardingPolicy.Step.PROJECT_RAN, "project-a");
        assertSame(progress, WorkshopOnboardingPolicy.recordChangeApplied(
                progress, "project-b", "function|main", "hash-a"));
        progress = WorkshopOnboardingPolicy.recordChangeApplied(
                progress, "project-a", "function|main", "hash-a");
        progress = WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.TESTS_PASSED,
                "project-a", "function|main", "hash-a");
        assertSame(progress, WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED,
                "project-a", "function|main", "wrong-hash"));
        progress = WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED,
                "project-a", "function|main", "hash-a");
        progress = WorkshopOnboardingPolicy.recordChangeStep(
                progress, WorkshopOnboardingPolicy.Step.CHANGE_REVERTED,
                "project-a", "function|main", "hash-a");

        assertTrue(progress.isComplete());
        assertEquals(WorkshopOnboardingPolicy.Step.COMPLETE, progress.nextStep());
    }

    @Test
    public void currentVersionRestoresExactDeferredStepAndContext() {
        WorkshopOnboardingPolicy.Progress restored = WorkshopOnboardingPolicy.restore(
                WorkshopOnboardingPolicy.CURRENT_VERSION, 5, true, false,
                "project-a", "function|main", "hash-a");

        assertEquals(WorkshopOnboardingPolicy.Step.CHANGES_REVIEWED, restored.nextStep());
        assertTrue(restored.deferred);
        WorkshopOnboardingPolicy.Progress resumed = WorkshopOnboardingPolicy.resume(restored);
        assertEquals("project-a", resumed.projectId);
        assertEquals("function|main", resumed.changeId);
        assertEquals("hash-a", resumed.changeHash);
        assertFalse(resumed.deferred);
    }

    @Test
    public void incompleteContextRollsBackToTheFirstSafeStep() {
        WorkshopOnboardingPolicy.Progress missingProject = WorkshopOnboardingPolicy.restore(
                WorkshopOnboardingPolicy.CURRENT_VERSION, 4, false, false,
                "", "function|main", "hash-a");
        assertEquals(WorkshopOnboardingPolicy.Step.PROJECT_OPENED, missingProject.nextStep());

        WorkshopOnboardingPolicy.Progress missingChange = WorkshopOnboardingPolicy.restore(
                WorkshopOnboardingPolicy.CURRENT_VERSION, 6, false, false,
                "project-a", "", "");
        assertEquals(WorkshopOnboardingPolicy.Step.CHANGE_APPLIED, missingChange.nextStep());
    }

    @Test
    public void versionChangeStartsTheNewWorkflowWithoutOverclaimingCompletion() {
        WorkshopOnboardingPolicy.Progress restored = WorkshopOnboardingPolicy.restore(
                WorkshopOnboardingPolicy.CURRENT_VERSION - 1, 7, false, false,
                "project-a", "function|main", "hash-a");

        assertEquals(WorkshopOnboardingPolicy.CURRENT_VERSION, restored.version);
        assertEquals(0, restored.completedSteps);
        assertEquals(WorkshopOnboardingPolicy.Step.WELCOME, restored.nextStep());
    }

    @Test
    public void legacyGuideDismissalDefersButDoesNotCompleteTheInteractiveWorkflow() {
        WorkshopOnboardingPolicy.Progress restored = WorkshopOnboardingPolicy.restore(
                0, 0, false, true, "", "", "");

        assertTrue(restored.deferred);
        assertFalse(restored.isComplete());
        assertEquals(WorkshopOnboardingPolicy.Step.WELCOME, restored.nextStep());
    }

    @Test
    public void checklistExplainsOptionalAccountsAndTheCurrentUiAction() {
        WorkshopOnboardingPolicy.Progress progress = WorkshopOnboardingPolicy.restore(
                WorkshopOnboardingPolicy.CURRENT_VERSION, 5, false, false,
                "project-a", "function|main", "hash-a");
        String checklist = WorkshopOnboardingPolicy.checklist(progress);

        assertTrue(checklist.contains("[x] Test the change"));
        assertTrue(checklist.contains("[ ] Review the saved change"));
        assertTrue(checklist.contains("Choose Changes or Raw Diffs"));
        assertTrue(checklist.contains("Neither is required"));
        assertTrue(checklist.contains("permissions are requested only when you start"));
    }
}
