package com.stasislang.workshop;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public final class WorkshopTextureProviderTest {
    @Test
    public void projectIdentityInvalidatesTextureCache() {
        assertTrue(WorkshopTextureProvider.projectChanged(null, "projects/one"));
        assertFalse(WorkshopTextureProvider.projectChanged("projects/one", "projects/one"));
        assertTrue(WorkshopTextureProvider.projectChanged("projects/one", "projects/two"));
    }
}
