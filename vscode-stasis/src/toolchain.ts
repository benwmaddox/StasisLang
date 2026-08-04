import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";

const MAXIMUM_INFO_BYTES = 1024 * 1024;
const INFO_TIMEOUT_MS = 10_000;

interface ToolchainFile {
  path: string;
  sha256: string;
  role: "executable" | "graphics_runtime" | "support";
}

interface EditorInfo {
  schema: number;
  release_id: string;
  target: string;
  protocols: {
    lsp: number;
    dap: number;
    live: number;
    graphics_abi: number;
  };
  executable: { path: string; sha256: string };
  graphics_runtime: { path: string; release_id: string; sha256: string };
}

interface EditorInfoEnvelope {
  ok: boolean;
  command: string;
  result: EditorInfo;
}

interface PackagedToolchainManifest {
  schema: number;
  executable: string;
  identity: EditorInfo;
  files: ToolchainFile[];
}

export async function resolveEditorToolchain(
  extensionPath: string,
  developerExecutable: string | undefined,
): Promise<string> {
  const override = developerExecutable?.trim();
  if (override) {
    if (!path.isAbsolute(override)) {
      throw new Error("stasis.developer.executablePath must be an absolute path");
    }
    await readEditorInfo(override);
    return override;
  }
  return verifyPackagedToolchain(extensionPath);
}

export async function verifyPackagedToolchain(extensionPath: string): Promise<string> {
  const bundleRoot = path.join(extensionPath, "dist", "toolchain");
  const manifestPath = path.join(extensionPath, "dist", "toolchain-manifest.json");
  if (!fs.existsSync(manifestPath)) {
    throw new Error("this Stasis extension does not contain its pinned toolchain; reinstall the platform VSIX");
  }
  const manifest = parseManifest(fs.readFileSync(manifestPath, "utf8"));
  const executable = resolveBundlePath(bundleRoot, manifest.executable);
  const executableFiles = manifest.files.filter((file) => file.role === "executable");
  const runtimeFiles = manifest.files.filter((file) => file.role === "graphics_runtime");
  if (executableFiles.length !== 1 || runtimeFiles.length !== 1) {
    throw new Error("the packaged Stasis toolchain manifest is missing required files");
  }
  const executableFile = executableFiles[0]!;
  const runtimeFile = runtimeFiles[0]!;
  if (resolveBundlePath(bundleRoot, executableFile.path) !== executable) {
    throw new Error("the packaged Stasis executable is not bound to its manifest hash");
  }
  for (const file of manifest.files) {
    const filePath = resolveBundlePath(bundleRoot, file.path);
    const actual = await sha256File(filePath);
    if (actual !== file.sha256) {
      throw new Error(`the packaged Stasis toolchain is corrupt: ${file.path}`);
    }
  }
  const actual = await readEditorInfo(executable);
  if (
    actual.release_id !== manifest.identity.release_id ||
    actual.target !== manifest.identity.target ||
    JSON.stringify(actual.protocols) !== JSON.stringify(manifest.identity.protocols)
  ) {
    throw new Error("the packaged Stasis executable does not match the extension toolchain manifest");
  }
  if (
    actual.executable.sha256 !== executableFile.sha256 ||
    actual.graphics_runtime.sha256 !== runtimeFile.sha256
  ) {
    throw new Error("the running Stasis toolchain does not match the packaged binary hashes");
  }
  if (actual.graphics_runtime.release_id !== actual.release_id) {
    throw new Error("the packaged Stasis executable and graphics runtime have different release identities");
  }
  return executable;
}

function parseManifest(source: string): PackagedToolchainManifest {
  let value: PackagedToolchainManifest;
  try {
    value = JSON.parse(source) as PackagedToolchainManifest;
  } catch (error) {
    throw new Error(`invalid packaged Stasis toolchain manifest: ${String(error)}`);
  }
  if (
    value.schema !== 1 ||
    typeof value.executable !== "string" ||
    !value.identity ||
    value.identity.schema !== 1 ||
    !Array.isArray(value.files) ||
    value.files.some((file) =>
      !file ||
      typeof file.path !== "string" ||
      !/^[0-9a-f]{64}$/u.test(file.sha256) ||
      !["executable", "graphics_runtime", "support"].includes(file.role)
    )
  ) {
    throw new Error("invalid packaged Stasis toolchain manifest");
  }
  return value;
}

function resolveBundlePath(root: string, relative: string): string {
  if (path.isAbsolute(relative)) {
    throw new Error("packaged Stasis toolchain paths must be relative");
  }
  const resolvedRoot = path.resolve(root);
  const resolved = path.resolve(root, relative);
  if (resolved !== resolvedRoot && !resolved.startsWith(`${resolvedRoot}${path.sep}`)) {
    throw new Error(`packaged Stasis toolchain path escapes its bundle: ${relative}`);
  }
  return resolved;
}

function sha256File(filePath: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const digest = createHash("sha256");
    const input = fs.createReadStream(filePath);
    input.on("error", reject);
    input.on("data", (chunk) => digest.update(chunk));
    input.on("end", () => resolve(digest.digest("hex")));
  });
}

async function readEditorInfo(executable: string): Promise<EditorInfo> {
  const output = await runEditorInfo(executable);
  let envelope: EditorInfoEnvelope;
  try {
    envelope = JSON.parse(output) as EditorInfoEnvelope;
  } catch (error) {
    throw new Error(`Stasis editor identity is not valid JSON: ${String(error)}`);
  }
  if (!envelope.ok || envelope.command !== "editor-info" || envelope.result?.schema !== 1) {
    throw new Error("the selected Stasis executable does not provide the editor identity contract");
  }
  if (
    envelope.result.protocols?.lsp !== 1 ||
    envelope.result.protocols?.dap !== 1 ||
    envelope.result.protocols?.live !== 1 ||
    envelope.result.protocols?.graphics_abi !== 1
  ) {
    throw new Error("the selected Stasis toolchain uses incompatible editor or graphics protocols");
  }
  if (envelope.result.graphics_runtime?.release_id !== envelope.result.release_id) {
    throw new Error("the selected Stasis executable and graphics runtime do not belong to the same release");
  }
  return envelope.result;
}

function runEditorInfo(executable: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ["--json", "editor-info"], {
      cwd: path.dirname(executable),
      stdio: "pipe",
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      error ? reject(error) : resolve(stdout);
    };
    const timeout = setTimeout(() => {
      child.kill();
      finish(new Error(`Stasis editor identity probe timed out: ${executable}`));
    }, INFO_TIMEOUT_MS);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      if (stdout.length > MAXIMUM_INFO_BYTES) {
        child.kill();
        finish(new Error("Stasis editor identity exceeded the output limit"));
      }
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      if (stderr.length > MAXIMUM_INFO_BYTES) {
        child.kill();
        finish(new Error("Stasis editor identity error exceeded the output limit"));
      }
    });
    child.once("error", (error) => finish(new Error(`Unable to start '${executable}': ${error.message}`)));
    child.once("exit", (code) => {
      code === 0
        ? finish()
        : finish(new Error(stderr.trim() || `Stasis editor identity exited with code ${code ?? "unknown"}`));
    });
  });
}
