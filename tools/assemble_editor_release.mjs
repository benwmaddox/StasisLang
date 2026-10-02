import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const MAX_RELEASE_PAYLOAD_BYTES = 40 * 1024 * 1024;
const MANIFEST_NAME = "stasis-editor-release.json";
const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  args.set(process.argv[index], process.argv[index + 1]);
}
const required = (name) => {
  const value = args.get(name);
  if (!value) throw new Error(`Missing ${name}.`);
  return path.resolve(value);
};
const vsix = required("--vsix");
const output = required("--out");
const releaseId = args.get("--release-id");
const platform = args.get("--platform");
if (!releaseId || !platform) throw new Error("Missing --release-id or --platform.");
if (!fs.existsSync(vsix) || !fs.statSync(vsix).isFile()) {
  throw new Error(`Release input is not a file: ${vsix}`);
}
const vsixSize = fs.statSync(vsix).size;
if (vsixSize > MAX_RELEASE_PAYLOAD_BYTES) {
  throw new Error(
    `editor release VSIX is ${vsixSize} bytes, above the ${MAX_RELEASE_PAYLOAD_BYTES}-byte budget`,
  );
}
const vsixBytes = fs.readFileSync(vsix);
const stagedVsixName = path.basename(vsix);
const vsixEntry = {
  role: "vscode_extension",
  name: stagedVsixName,
  bytes: vsixBytes.byteLength,
  sha256: createHash("sha256").update(vsixBytes).digest("hex"),
};
const manifest = {
  schema: 2,
  release_id: releaseId,
  platform,
  files: [vsixEntry],
};
const manifestBytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`, "ascii");
const releasePayloadBytes = vsixBytes.byteLength + manifestBytes.byteLength;
if (releasePayloadBytes > MAX_RELEASE_PAYLOAD_BYTES) {
  throw new Error(
    `editor release payload is ${releasePayloadBytes} bytes, above the ${MAX_RELEASE_PAYLOAD_BYTES}-byte budget`,
  );
}

fs.mkdirSync(output, { recursive: true });
const expectedOutputNames = new Set([stagedVsixName, MANIFEST_NAME]);
const staleOutputNames = fs.readdirSync(output).filter((name) => !expectedOutputNames.has(name));
if (staleOutputNames.length > 0) {
  throw new Error(
    `Release output directory contains stale files: ${staleOutputNames.join(", ")}`,
  );
}
fs.writeFileSync(path.join(output, stagedVsixName), vsixBytes);
fs.writeFileSync(path.join(output, MANIFEST_NAME), manifestBytes);
