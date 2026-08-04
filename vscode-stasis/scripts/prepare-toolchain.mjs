import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const extensionRoot = path.resolve(import.meta.dirname, "..");
const sourceRoot = process.env.STASIS_TOOLCHAIN_DIR?.trim();
if (!sourceRoot || !path.isAbsolute(sourceRoot)) {
  throw new Error("STASIS_TOOLCHAIN_DIR must name the absolute root of the release toolchain bundle.");
}
const executableRelative = process.env.STASIS_TOOLCHAIN_EXECUTABLE?.trim()
  || (process.platform === "win32" ? "stasis.exe" : "bin/stasis");
const resolvedSourceRoot = path.resolve(sourceRoot);
const resolvedSourceExecutable = path.resolve(sourceRoot, executableRelative);
if (
  path.isAbsolute(executableRelative)
  || !resolvedSourceExecutable.startsWith(`${resolvedSourceRoot}${path.sep}`)
) {
  throw new Error("STASIS_TOOLCHAIN_EXECUTABLE must stay inside STASIS_TOOLCHAIN_DIR.");
}
const sourceExecutable = path.resolve(sourceRoot, executableRelative);
if (!fs.existsSync(sourceExecutable)) {
  throw new Error(`Stasis toolchain executable does not exist: ${sourceExecutable}`);
}

const bundleRoot = path.join(extensionRoot, "dist", "toolchain");
fs.rmSync(bundleRoot, { recursive: true, force: true });
fs.mkdirSync(path.dirname(bundleRoot), { recursive: true });
fs.cpSync(sourceRoot, bundleRoot, { recursive: true, dereference: true });

const executable = path.resolve(bundleRoot, executableRelative);
const envelope = JSON.parse(execFileSync(executable, ["--json", "editor-info"], {
  cwd: path.dirname(executable),
  encoding: "utf8",
  timeout: 10_000,
  maxBuffer: 1024 * 1024,
}));
if (!envelope.ok || envelope.command !== "editor-info" || envelope.result?.schema !== 1) {
  throw new Error("The bundled executable did not return a valid Stasis editor identity.");
}
const identity = envelope.result;
if (identity.graphics_runtime?.release_id !== identity.release_id) {
  throw new Error("The Stasis executable and graphics runtime have different release identities.");
}

const nativePath = (value) => {
  if (process.platform !== "win32" || !value.startsWith("\\\\?\\")) return value;
  return value.startsWith("\\\\?\\UNC\\") ? `\\\\${value.slice(8)}` : value.slice(4);
};
const relativeInsideBundle = (absolute) => {
  absolute = nativePath(absolute);
  const relative = path.relative(bundleRoot, absolute);
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) {
    throw new Error(`Editor identity path is outside the toolchain bundle: ${absolute}`);
  }
  return relative.split(path.sep).join("/");
};
const sha256 = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const files = [
  ["executable", executable, identity.executable.sha256],
  ["graphics_runtime", path.resolve(nativePath(identity.graphics_runtime.path)), identity.graphics_runtime.sha256],
].map(([role, file, reportedHash]) => {
  const actualHash = sha256(file);
  if (actualHash !== reportedHash) {
    throw new Error(`Stasis reported the wrong hash for ${file}.`);
  }
  return { path: relativeInsideBundle(file), sha256: actualHash, role };
});
const manifest = {
  schema: 1,
  executable: executableRelative.split(path.sep).join("/"),
  identity,
  files,
};
fs.writeFileSync(
  path.join(extensionRoot, "dist", "toolchain-manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
  "ascii",
);
