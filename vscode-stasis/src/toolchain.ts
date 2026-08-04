import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";

const MAXIMUM_PROBE_BYTES = 1024 * 1024;
const PROBE_TIMEOUT_MS = 5_000;

export interface ToolchainCapabilities {
  lsp: boolean;
  dap: boolean;
}

interface LocalToolchainConfig {
  schema: number;
  executable: string;
}

export function parseToolchainCapabilities(help: string): ToolchainCapabilities {
  return {
    lsp: /^\s*lsp\s+/mu.test(help),
    dap: /^\s*dap\s+/mu.test(help),
  };
}

export function resolveToolchainExecutable(
  configured: string | undefined,
  packaged: string | undefined,
): string {
  return configured?.trim() || packaged?.trim() || "stasis";
}

export function loadPackagedToolchain(extensionPath: string): string | undefined {
  const configPath = path.join(extensionPath, "dist", "toolchain.json");
  if (!fs.existsSync(configPath)) {
    return undefined;
  }
  let parsed: LocalToolchainConfig;
  try {
    parsed = JSON.parse(fs.readFileSync(configPath, "utf8")) as LocalToolchainConfig;
  } catch (error) {
    throw new Error(`invalid packaged Stasis toolchain config: ${String(error)}`);
  }
  if (parsed.schema !== 1 || typeof parsed.executable !== "string" || !path.isAbsolute(parsed.executable)) {
    throw new Error("invalid packaged Stasis toolchain config");
  }
  if (!fs.existsSync(parsed.executable)) {
    throw new Error(`packaged Stasis toolchain does not exist: ${parsed.executable}`);
  }
  return parsed.executable;
}

export async function requireEditorToolchain(executable: string, cwd: string): Promise<void> {
  const help = await readToolchainHelp(executable, cwd);
  const capabilities = parseToolchainCapabilities(help);
  const missing = [!capabilities.lsp && "lsp", !capabilities.dap && "dap"].filter(Boolean);
  if (missing.length > 0) {
    throw new Error(
      `Stasis toolchain '${executable}' is missing ${missing.join(" and ")}. Select a current toolchain executable.`,
    );
  }
}

function readToolchainHelp(executable: string, cwd: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ["--help"], {
      cwd,
      stdio: "pipe",
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeout);
      if (error) {
        reject(error);
      } else {
        resolve(stdout);
      }
    };
    const timeout = setTimeout(() => {
      child.kill();
      finish(new Error(`Stasis toolchain probe timed out: ${executable}`));
    }, PROBE_TIMEOUT_MS);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      if (stdout.length > MAXIMUM_PROBE_BYTES) {
        child.kill();
        finish(new Error(`Stasis toolchain probe returned too much output: ${executable}`));
      }
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      if (stderr.length > MAXIMUM_PROBE_BYTES) {
        child.kill();
        finish(new Error(`Stasis toolchain probe returned too much error output: ${executable}`));
      }
    });
    child.once("error", (error) => finish(new Error(`Unable to start Stasis toolchain '${executable}': ${error.message}`)));
    child.once("exit", (code) => {
      if (code === 0) {
        finish();
      } else {
        finish(new Error(stderr.trim() || `Stasis toolchain '${executable}' exited with code ${code ?? "unknown"}.`));
      }
    });
  });
}
