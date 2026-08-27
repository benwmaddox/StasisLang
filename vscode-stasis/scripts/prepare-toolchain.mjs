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
  timeout: 60_000,
  maxBuffer: 1024 * 1024,
}));
if (!envelope.ok || envelope.command !== "editor-info" || envelope.result?.schema !== 1) {
  throw new Error("The bundled executable did not return a valid Stasis editor identity.");
}
const identity = envelope.result;
const fingerprintPattern = /^[0-9a-f]{64}$/u;
if (!fingerprintPattern.test(identity.build_fingerprint ?? "") ||
    !fingerprintPattern.test(identity.graphics_runtime?.build_fingerprint ?? "") ||
    identity.graphics_runtime.build_fingerprint !== identity.build_fingerprint) {
  throw new Error("The bundled executable did not return a matching verified build fingerprint.");
}
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
const requiredFiles = [
  ["executable", executable, identity.executable.sha256],
  ["graphics_runtime", path.resolve(nativePath(identity.graphics_runtime.path)), identity.graphics_runtime.sha256],
];
const roles = new Map(requiredFiles.map(([role, file]) => [relativeInsideBundle(file), role]));
for (const [, file, reportedHash] of requiredFiles) {
  const actualHash = sha256(file);
  if (actualHash !== reportedHash) {
    throw new Error(`Stasis reported the wrong hash for ${file}.`);
  }
}
const bundleFiles = (directory) => fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
  const file = path.join(directory, entry.name);
  if (entry.isDirectory()) return bundleFiles(file);
  if (!entry.isFile()) throw new Error(`Unsupported entry in Stasis toolchain bundle: ${file}`);
  return [file];
});
const files = bundleFiles(bundleRoot)
  .map((file) => {
    const relative = relativeInsideBundle(file);
    return {
      path: relative,
      sha256: sha256(file),
      role: roles.get(relative) ?? "support",
    };
  })
  .sort((left, right) => left.path.localeCompare(right.path));
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
