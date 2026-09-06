import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { encodeVideo } from "../network_browser_video.mjs";

const exec = promisify(execFile);
const ffmpeg = process.env.STASIS_FFMPEG_EXECUTABLE || "ffmpeg";
const ffprobe = process.env.STASIS_FFPROBE_EXECUTABLE
  || ffmpeg.replace(/ffmpeg(?=\.exe$|$)/i, "ffprobe");
const root = path.resolve("target/network-browser-video-tests");

for (const [width, height] of [[764, 485], [765, 484], [764, 484]]) {
  test(`encodes ${width}x${height} captures without dropping pixels or frames`, { timeout: 30_000 }, async () => {
    await mkdir(root, { recursive: true });
    const directory = await mkdtemp(path.join(root, "case-"));
    try {
      await exec(ffmpeg, [
        "-y", "-f", "lavfi", "-i", `testsrc=size=${width}x${height}:rate=1`,
        "-frames:v", "2", path.join(directory, "frame-%02d.png"),
      ], { timeout: 10_000 });
      await encodeVideo(directory);
      const { stdout } = await exec(ffprobe, [
        "-v", "error", "-select_streams", "v:0", "-count_frames",
        "-show_entries", "stream=width,height,pix_fmt,nb_read_frames", "-of", "json",
        path.join(directory, "browser.mp4"),
      ], { timeout: 10_000 });
      const [stream] = JSON.parse(stdout).streams;
      assert.equal(stream.width, width + width % 2);
      assert.equal(stream.height, height + height % 2);
      assert.equal(stream.pix_fmt, "yuv420p");
      assert.equal(Number(stream.nb_read_frames), 2);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
}
