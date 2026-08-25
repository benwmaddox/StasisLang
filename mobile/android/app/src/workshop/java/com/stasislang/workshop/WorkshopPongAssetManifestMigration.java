package com.stasislang.workshop;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

final class WorkshopPongAssetManifestMigration {
    private WorkshopPongAssetManifestMigration() {}

    static String mergeRequiredSprites(String current, String packaged) throws JSONException {
        JSONObject mergedRoot = new JSONObject(current);
        JSONArray currentAssets = mergedRoot.getJSONArray("assets");
        JSONArray packagedAssets = new JSONObject(packaged).getJSONArray("assets");
        JSONArray mergedAssets = new JSONArray();
        for (int index = 0; index < currentAssets.length(); index += 1) {
            JSONObject asset = currentAssets.getJSONObject(index);
            if (!isRequiredSprite(asset.optString("id"))) mergedAssets.put(asset);
        }
        for (int index = 0; index < packagedAssets.length(); index += 1) {
            JSONObject asset = packagedAssets.getJSONObject(index);
            if (isRequiredSprite(asset.optString("id"))) mergedAssets.put(asset);
        }
        mergedRoot.put("assets", mergedAssets);
        return mergedRoot.toString(2) + "\n";
    }

    private static boolean isRequiredSprite(String id) {
        return "paddle".equals(id) || "center_line".equals(id);
    }
}
