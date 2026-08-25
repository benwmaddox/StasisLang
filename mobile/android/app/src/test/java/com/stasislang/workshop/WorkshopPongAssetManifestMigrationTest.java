package com.stasislang.workshop;

import static org.junit.Assert.assertEquals;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;

public final class WorkshopPongAssetManifestMigrationTest {
    @Test
    public void requiredSpritesMergeWithoutDroppingUserAssetsOrMetadata() throws Exception {
        String current = "{\"schema\":\"stasis-assets\",\"version\":7,"
                + "\"custom\":\"keep\",\"assets\":["
                + "{\"id\":\"user_ship\",\"path\":\"assets/ship.svg\"},"
                + "{\"id\":\"paddle\",\"path\":\"assets/old.svg\"}]}";
        String packaged = "{\"assets\":["
                + "{\"id\":\"ball\"},"
                + "{\"id\":\"paddle\",\"path\":\"assets/paddle.svg\"},"
                + "{\"id\":\"center_line\",\"path\":\"assets/center_line.svg\"}]}";

        JSONObject merged = new JSONObject(
                WorkshopPongAssetManifestMigration.mergeRequiredSprites(current, packaged));
        JSONArray assets = merged.getJSONArray("assets");

        assertEquals(7, merged.getInt("version"));
        assertEquals("keep", merged.getString("custom"));
        assertEquals("user_ship", assets.getJSONObject(0).getString("id"));
        assertEquals("assets/paddle.svg", assets.getJSONObject(1).getString("path"));
        assertEquals("assets/center_line.svg", assets.getJSONObject(2).getString("path"));
    }
}
