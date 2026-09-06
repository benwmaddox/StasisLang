import assert from "node:assert/strict";
import path from "node:path";
import { spawn } from "node:child_process";

export async function encodeVideo(evidenceRoot) {
  const ffmpeg = process.env.STASIS_FFMPEG_EXECUTABLE || "ffmpeg";
  const args = [
    "-y", "-framerate", "1", "-i", path.join(evidenceRoot, "frame-%02d.png"),
    // H.264 yuv420p requires even dimensions; pad without cropping capture pixels.
    "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2",
    "-c:v", "libx264", "-pix_fmt", "yuv420p", path.join(evidenceRoot, "browser.mp4"),
  ];
  const result = await new Promise((resolve, reject) => {
    const child = spawn(ffmpeg, args, { stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr.on("data", chunk => { stderr += chunk; });
    child.once("error", reject);
    child.once("exit", code => resolve({ code, stderr }));
  });
  assert.equal(result.code, 0, `ffmpeg failed to encode acceptance evidence: ${result.stderr}`);
}
