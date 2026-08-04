import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const dist = path.join(root, "dist");
const vsix = path.join(root, ".vsix");
const config = path.join(dist, "toolchain.json");
fs.mkdirSync(dist, { recursive: true });
fs.mkdirSync(vsix, { recursive: true });

const executable = process.env.STASIS_LOCAL_TOOLCHAIN?.trim();
if (!executable) {
  fs.rmSync(config, { force: true });
  process.exit(0);
}
if (!path.isAbsolute(executable)) {
  throw new Error("STASIS_LOCAL_TOOLCHAIN must be an absolute path.");
}
if (!fs.existsSync(executable)) {
  throw new Error(`STASIS_LOCAL_TOOLCHAIN does not exist: ${executable}`);
}
fs.writeFileSync(config, `${JSON.stringify({ schema: 1, executable }, null, 2)}\n`, "ascii");
