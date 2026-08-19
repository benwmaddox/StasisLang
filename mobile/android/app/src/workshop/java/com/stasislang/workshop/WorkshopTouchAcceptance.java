package com.stasislang.workshop;

import android.util.Log;
import android.view.MotionEvent;

import org.json.JSONArray;
import org.json.JSONObject;

/** Acceptance-only fixed Java touch -> HostFrame -> guest -> GLES proof. */
final class WorkshopTouchAcceptance {
    private static final String LOG_TAG = "StasisWorkshop";
    private static final String SCHEMA = "stasis.workshop_touch_roundtrip.v1";

    private WorkshopTouchAcceptance() {}

    static String run(MainActivity activity, String projectRoot) {
        JSONArray cases = new JSONArray();
        int[][] phases = {{160, 90, MotionEvent.ACTION_DOWN, 1},
                {320, 180, MotionEvent.ACTION_MOVE, 1},
                {400, 225, MotionEvent.ACTION_UP, 0}};
        String[] names = {"down", "move", "up"};
        try {
            for (int index = 0; index < phases.length; index += 1) {
                int[] phase = phases[index];
                JSONObject frame = new JSONObject(activity.runIt027Frame(projectRoot,
                        phase[0], phase[1], phase[2], phase[3], index + 1));
                if (!"passed".equals(frame.optString("status"))) {
                    return failed(frame.optString("error", "phase failed"));
                }
                JSONObject render = frame.optJSONObject("render");
                if (render == null) return failed("phase omitted render evidence");
                if (!names[index].equals(frame.optString("phase"))
                        || frame.optInt("sequence", -1) != index + 1
                        || !frame.optBoolean("gles_presented", false)
                        || frame.optInt("gles_frame_token", -1)
                        != render.optInt("frame_token", -2)
                        || frame.optBoolean("java_only", true)) {
                    return failed("phase evidence did not prove ordered GLES presentation");
                }
                JSONObject guest = frame.getJSONObject("guest");
                if (guest.optInt("x", Integer.MIN_VALUE) != phase[0]
                        || guest.optInt("y", Integer.MIN_VALUE) != phase[1]
                        || guest.optInt("active", Integer.MIN_VALUE) != phase[3]) {
                    return failed("guest HostFrame coordinates or active state mismatch");
                }
                if (index == 0 && (guest.optInt("down_edge") != 1
                        || guest.optInt("up_edge") != 0
                        || guest.optInt("dx") != 0 || guest.optInt("dy") != 0)) {
                    return failed("down edge/delta mismatch");
                }
                if (index == 1 && (guest.optInt("down_edge") != 0
                        || guest.optInt("up_edge") != 0
                        || guest.optInt("dx") != 160 || guest.optInt("dy") != 90)) {
                    return failed("move edge/delta mismatch");
                }
                if (index == 2 && (guest.optInt("down_edge") != 0
                        || guest.optInt("up_edge") != 1
                        || guest.optInt("dx") != 80 || guest.optInt("dy") != 45)) {
                    return failed("up edge/delta mismatch");
                }
                JSONObject marker = render.optJSONObject("marker");
                if (marker == null) return failed("phase omitted marker evidence");
                if (!marker.optBoolean("active", false)
                        || Math.round((float)marker.optDouble("x", -1.0)) != phase[0] - 8
                        || Math.round((float)marker.optDouble("y", -1.0)) != phase[1] - 8
                        || Math.round((float)marker.optDouble("w", -1.0)) != 16
                        || Math.round((float)marker.optDouble("h", -1.0)) != 16) {
                    return failed("guest marker geometry mismatch");
                }
                cases.put(frame);
                Log.i(LOG_TAG, "Stasis Workshop IT-027 case: " + frame);
            }
            JSONObject summary = new JSONObject().put("schema", SCHEMA)
                    .put("test_id", "IT-027").put("event", "touch_roundtrip")
                    .put("status", "passed").put("phases", 3)
                    .put("ordered", true).put("unique", true)
                    .put("java_motion_events", 3).put("jni_jit_frames", 3)
                    .put("gles_presented_frames", 3).put("java_only", false)
                    .put("cases", cases);
            Log.i(LOG_TAG, "Stasis Workshop IT-027: " + summary);
            return summary.toString();
        } catch (Exception error) {
            return failed(error.getMessage() == null ? error.getClass().getSimpleName()
                    : error.getMessage());
        }
    }

    private static String failed(String reason) {
        String output = "{\"schema\":\"" + SCHEMA
                + "\",\"test_id\":\"IT-027\",\"event\":\"touch_roundtrip\","
                + "\"status\":\"failed\",\"error\":" + JSONObject.quote(reason) + "}";
        Log.e(LOG_TAG, "Stasis Workshop IT-027: " + output);
        return output;
    }
}
