import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  args.set(process.argv[index], process.argv[index + 1]);
}
const required = (name) => {
  const value = args.get(name);
  if (!value) throw new Error(`Missing ${name}.`);
  return path.resolve(value);
};
const toolchainArchive = required("--toolchain-archive");
const vsix = required("--vsix");
const output = required("--out");
const releaseId = args.get("--release-id");
const platform = args.get("--platform");
if (!releaseId || !platform) throw new Error("Missing --release-id or --platform.");
for (const file of [toolchainArchive, vsix]) {
  if (!fs.statSync(file).isFile()) throw new Error(`Release input is not a file: ${file}`);
}

fs.mkdirSync(output, { recursive: true });
const copy = (source) => {
  const destination = path.join(output, path.basename(source));
  fs.copyFileSync(source, destination);
  return destination;
};
const stagedToolchain = copy(toolchainArchive);
const stagedVsix = copy(vsix);
const fileEntry = (file, role) => ({
  role,
  name: path.basename(file),
  bytes: fs.statSync(file).size,
  sha256: createHash("sha256").update(fs.readFileSync(file)).digest("hex"),
});
const manifest = {
  schema: 1,
  release_id: releaseId,
  platform,
  files: [
    fileEntry(stagedToolchain, "toolchain_archive"),
    fileEntry(stagedVsix, "vscode_extension"),
  ],
};
fs.writeFileSync(
  path.join(output, "stasis-editor-release.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
  "ascii",
);
