import { spawnSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
  runTests,
} from "@vscode/test-electron";

async function main(): Promise<void> {
  const extensionRoot = path.resolve(__dirname, "..");
  const repositoryRoot = path.resolve(extensionRoot, "..");
  const vscodeVersion = process.env.STASIS_E2E_VSCODE_VERSION ?? "1.96.0";
  const vsix = path.join(extensionRoot, ".vsix", "stasislang.stasis.vsix");
  const executable = process.env.STASIS_E2E_EXECUTABLE
    ? path.resolve(process.env.STASIS_E2E_EXECUTABLE)
    : path.join(repositoryRoot, "target", "debug", process.platform === "win32" ? "stasis.exe" : "stasis");
  if (!fs.existsSync(vsix)) {
    throw new Error(`Packaged extension not found: ${vsix}`);
  }
  if (!fs.existsSync(executable)) {
    throw new Error(`Stasis executable not found: ${executable}`);
  }

  const profileRoot = fs.mkdtempSync(path.join(os.tmpdir(), "stasis-vscode-e2e-"));
  const extensionsDir = path.join(profileRoot, "extensions");
  const userDataDir = path.join(profileRoot, "user-data");
  const fixtureWorkspace = path.join(profileRoot, "workspace");
  fs.cpSync(path.join(extensionRoot, "test", "fixture"), fixtureWorkspace, { recursive: true });
  const screenshot = process.env.STASIS_E2E_SCREENSHOT
    ? path.resolve(process.env.STASIS_E2E_SCREENSHOT)
    : path.join(profileRoot, "live-frame.png");
  fs.mkdirSync(path.dirname(screenshot), { recursive: true });
  fs.rmSync(screenshot, { force: true });

  try {
    const vscodeExecutablePath = await downloadAndUnzipVSCode(vscodeVersion);
    const [cli, ...defaultCliArgs] = resolveCliArgsFromVSCodeExecutablePath(vscodeExecutablePath);
    if (!cli) {
      throw new Error("VS Code test download did not provide a command-line launcher.");
    }
    const cliArgs = defaultCliArgs.filter(
      (arg) => !arg.startsWith("--extensions-dir=") && !arg.startsWith("--user-data-dir="),
    );
    const install = spawnSync(
      cli,
      [
        ...cliArgs,
        "--extensions-dir",
        extensionsDir,
        "--user-data-dir",
        userDataDir,
        "--install-extension",
        vsix,
        "--force",
      ],
      {
        encoding: "utf8",
        stdio: "inherit",
        shell: process.platform === "win32",
      },
    );
    if (install.error) {
      throw install.error;
    }
    if (install.status !== 0) {
      throw new Error(`VSIX installation failed with exit code ${install.status ?? "unknown"}.`);
    }
    // The Windows CLI wrapper can return just before its Electron helper releases the profile
    // mutex. Give that helper a short bounded drain window before starting the Extension Host.
    await new Promise((resolve) => setTimeout(resolve, 500));

    await runTests({
      vscodeExecutablePath,
      extensionDevelopmentPath: path.join(extensionRoot, "test", "harness"),
      extensionTestsPath: path.join(extensionRoot, "dist", "e2e-suite.cjs"),
      extensionTestsEnv: {
        ...process.env,
        STASIS_E2E_EXECUTABLE: executable,
        STASIS_E2E_SCREENSHOT: screenshot,
        STASIS_SCREENSHOT_ONCE: screenshot,
        STASIS_SCREENSHOT_FRAME: "2",
      },
      launchArgs: [
        fixtureWorkspace,
        `--extensions-dir=${extensionsDir}`,
        `--user-data-dir=${userDataDir}`,
        "--disable-workspace-trust",
        "--skip-welcome",
        "--skip-release-notes",
      ],
    });
  } finally {
    fs.rmSync(profileRoot, { recursive: true, force: true });
  }
}

void main().catch((error: unknown) => {
  console.error(error);
  process.exitCode = 1;
});
