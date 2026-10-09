const encoder = new TextEncoder();
const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < table.length; index += 1) {
    let crc = index;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc & 1) ? (0xedb88320 ^ (crc >>> 1)) : (crc >>> 1);
    table[index] = crc >>> 0;
  }
  return table;
})();

function bytesOf(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  return encoder.encode(String(value ?? ""));
}

function u16(view, offset, value) { view.setUint16(offset, value, true); }
function u32(view, offset, value) { view.setUint32(offset, value >>> 0, true); }

export function normalizeArchivePath(path) {
  if (typeof path !== "string" || !path || path.startsWith("/") || path.includes("\\") || path.includes(":") || /[\u0000-\u001f]/.test(path)) {
    throw new Error("ZIP entry path is unsafe");
  }
  const segments = path.split("/");
  if (segments.some(segment => !segment || segment === "." || segment === "..")) throw new Error("ZIP entry path is unsafe");
  if (encoder.encode(path).byteLength > 1024) throw new Error("ZIP entry path is too long");
  return path;
}

export function crc32(input) {
  const bytes = bytesOf(input);
  let crc = 0xffffffff;
  for (const byte of bytes) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

export function buildStoredZip(inputEntries) {
  if (!Array.isArray(inputEntries) || inputEntries.length === 0 || inputEntries.length > 0xffff) {
    throw new Error("ZIP package must contain between 1 and 65535 entries");
  }
  const seen = new Set();
  const entries = inputEntries.map(entry => {
    const path = normalizeArchivePath(entry.path);
    if (seen.has(path)) throw new Error(`ZIP package contains duplicate path ${path}`);
    seen.add(path);
    const name = encoder.encode(path);
    const data = bytesOf(entry.bytes).slice();
    if (name.byteLength > 0xffff || data.byteLength > 0xffffffff) throw new Error(`ZIP entry ${path} is too large`);
    return { path, name, data, crc: crc32(data), offset: 0 };
  });

  let localSize = 0;
  for (const entry of entries) {
    entry.offset = localSize;
    localSize += 30 + entry.name.byteLength + entry.data.byteLength;
    if (localSize > 0xffffffff) throw new Error("ZIP package exceeds the 4 GiB classic ZIP limit");
  }
  let centralSize = 0;
  for (const entry of entries) centralSize += 46 + entry.name.byteLength;
  if (localSize + centralSize + 22 > 0xffffffff) throw new Error("ZIP package exceeds the 4 GiB classic ZIP limit");

  const output = new Uint8Array(localSize + centralSize + 22);
  const view = new DataView(output.buffer);
  let cursor = 0;
  for (const entry of entries) {
    u32(view, cursor, 0x04034b50);
    u16(view, cursor + 4, 20);
    u16(view, cursor + 6, 0x0800);
    u16(view, cursor + 8, 0);
    u16(view, cursor + 10, 0);
    u16(view, cursor + 12, 0x0021); // Fixed DOS date: 1980-01-01.
    u32(view, cursor + 14, entry.crc);
    u32(view, cursor + 18, entry.data.byteLength);
    u32(view, cursor + 22, entry.data.byteLength);
    u16(view, cursor + 26, entry.name.byteLength);
    u16(view, cursor + 28, 0);
    output.set(entry.name, cursor + 30);
    output.set(entry.data, cursor + 30 + entry.name.byteLength);
    cursor += 30 + entry.name.byteLength + entry.data.byteLength;
  }
  const centralOffset = cursor;
  for (const entry of entries) {
    u32(view, cursor, 0x02014b50);
    u16(view, cursor + 4, 20);
    u16(view, cursor + 6, 20);
    u16(view, cursor + 8, 0x0800);
    u16(view, cursor + 10, 0);
    u16(view, cursor + 12, 0);
    u16(view, cursor + 14, 0x0021);
    u32(view, cursor + 16, entry.crc);
    u32(view, cursor + 20, entry.data.byteLength);
    u32(view, cursor + 24, entry.data.byteLength);
    u16(view, cursor + 28, entry.name.byteLength);
    u16(view, cursor + 30, 0);
    u16(view, cursor + 32, 0);
    u16(view, cursor + 34, 0);
    u16(view, cursor + 36, 0);
    u32(view, cursor + 38, 0);
    u32(view, cursor + 42, entry.offset);
    output.set(entry.name, cursor + 46);
    cursor += 46 + entry.name.byteLength;
  }
  u32(view, cursor, 0x06054b50);
  u16(view, cursor + 4, 0);
  u16(view, cursor + 6, 0);
  u16(view, cursor + 8, entries.length);
  u16(view, cursor + 10, entries.length);
  u32(view, cursor + 12, centralSize);
  u32(view, cursor + 16, centralOffset);
  u16(view, cursor + 20, 0);
  return output;
}

function safeJson(value) {
  return JSON.stringify(value).replace(/[<>&\u2028\u2029]/g, character => ({
    "<": "\\u003c", ">": "\\u003e", "&": "\\u0026", "\u2028": "\\u2028", "\u2029": "\\u2029",
  })[character]);
}

async function sha256Hex(input) {
  if (!globalThis.crypto?.subtle) throw new Error("SHA-256 is unavailable; serve the playground from localhost or HTTPS");
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytesOf(input)));
  return Array.from(digest, byte => byte.toString(16).padStart(2, "0")).join("");
}

export async function createExportEntries({ entry, files, assets, metadata, gameWasm, runtimeScript }) {
  if (!metadata || !metadata.config || !metadata.provenance) throw new Error("compile metadata is incomplete; compile the project before exporting");
  const wasm = bytesOf(gameWasm).slice();
  if (wasm.byteLength < 8 || wasm[0] !== 0 || wasm[1] !== 97 || wasm[2] !== 115 || wasm[3] !== 109) throw new Error("compiled WebAssembly is invalid");
  if (typeof runtimeScript !== "string" || !runtimeScript.includes("__STASIS_WASM_URL__")) throw new Error("production runtime script is missing its wasm URL placeholder");
  const config = structuredClone(metadata.config);
  const assetList = Array.from(assets || []);
  const assetOverrides = Object.create(null);
  const assetMetadata = Object.create(null);
  const assetRecords = [];
  for (const asset of assetList) {
    const path = normalizeArchivePath(asset.path);
    if (!path.startsWith("assets/")) throw new Error(`unsupported exported asset path ${path}`);
    const bytes = bytesOf(asset.bytes).slice();
    const byteLength = bytes.byteLength;
    const sha256 = asset.sha256 || await sha256Hex(bytes);
    const extension = path.split(".").at(-1).toLowerCase();
    if (extension === "png") {
      if (!Number.isInteger(asset.width) || asset.width <= 0 || !Number.isInteger(asset.height) || asset.height <= 0) {
        throw new Error(`PNG asset ${path} has invalid dimensions`);
      }
      assetOverrides[path] = path;
      assetMetadata[path] = { encoding: "png", width: asset.width, height: asset.height, byte_length: byteLength, sha256 };
      assetRecords.push({ path, width: asset.width, height: asset.height, byteLength, sha256 });
    } else if (extension === "ttf") {
      assetOverrides[path] = path;
      assetMetadata[path] = { encoding: "ttf", byte_length: byteLength, sha256 };
      assetRecords.push({ path, byteLength, sha256, encoding: "ttf" });
    } else if (path === "assets/OFL.txt") {
      assetRecords.push({ path, byteLength, sha256, role: "license" });
    } else {
      throw new Error(`unsupported exported asset path ${path}`);
    }
  }
  config.assets = assetOverrides;
  config.asset_urls = {};
  config.asset_metadata = assetMetadata;

  const runtime = runtimeScript.replaceAll("__STASIS_WASM_URL__", "./game.wasm");
  const manifestFiles = [];
  const output = [];
  for (const file of files) {
    const path = normalizeArchivePath(file.path);
    if (!path.endsWith(".stasis")) throw new Error(`source file ${path} must end in .stasis`);
    const bytes = encoder.encode(file.source);
    const packagePath = `project/${path}`;
    output.push({ path: packagePath, bytes });
    manifestFiles.push({ path: packagePath, size: bytes.byteLength, sha256: await sha256Hex(bytes) });
  }
  for (const asset of assetList) {
    const path = normalizeArchivePath(asset.path);
    const bytes = bytesOf(asset.bytes).slice();
    output.push({ path, bytes });
  }
  output.push({ path: "game.wasm", bytes: wasm });
  output.push({ path: "game.js", bytes: encoder.encode(runtime) });

  const html = `<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n<meta name="viewport" content="width=device-width,initial-scale=1">\n<meta http-equiv="Content-Security-Policy" content="default-src 'self' blob: data:; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; connect-src 'self' blob:; img-src 'self' blob: data:; media-src 'self' blob:; font-src 'self' blob: data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'">\n<title>Stasis Playground Export</title>\n<style>html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#081613}main{width:100%;height:100%;display:grid;place-items:center}canvas{display:block;width:min(100vw,177.78vh);height:min(56.25vw,100vh);background:#091b2d;touch-action:none}#stasis-loading{position:fixed;inset:0;display:grid;place-items:center;background:#081613;color:#f3eee1;font:16px system-ui}#stasis-loading[data-hidden="true"]{display:none}#stasis-error{position:fixed;inset:0;margin:0;padding:1rem;color:#ff8f8f;white-space:pre-wrap;pointer-events:none}#stasis-error:empty{display:none}</style>\n</head>\n<body>\n<main><canvas id="stasis-canvas" width="640" height="360" data-logical-width="640" data-logical-height="360" aria-label="Stasis game canvas" tabindex="0"></canvas></main>\n<div id="stasis-loading"><span id="stasis-loading-status">Preparing...</span></div><pre id="stasis-error"></pre><div id="stasis-hud" hidden></div>\n<script>window.STASIS_GAME=${safeJson(config)};</script>\n<script src="./game.js" defer></script>\n</body>\n</html>\n`;
  output.push({ path: "index.html", bytes: encoder.encode(html) });

  const manifest = {
    schemaVersion: 1,
    entry,
    compiler: metadata.provenance,
    layoutDigest: metadata.layoutDigest,
    imports: metadata.imports || [],
    sources: manifestFiles,
    assets: assetRecords,
    files: ["index.html", "game.js", "game.wasm", "manifest.json", "provenance.json", ...assetList.map(asset => asset.path), ...manifestFiles.map(file => file.path)],
  };
  const provenance = {
    ...metadata.provenance,
    schemaVersion: metadata.schemaVersion,
    layoutDigest: metadata.layoutDigest,
    imports: metadata.imports || [],
    wasmSha256: await sha256Hex(wasm),
    runtimeSha256: await sha256Hex(encoder.encode(runtime)),
  };
  output.push({ path: "manifest.json", bytes: encoder.encode(`${JSON.stringify(manifest, null, 2)}\n`) });
  output.push({ path: "provenance.json", bytes: encoder.encode(`${JSON.stringify(provenance, null, 2)}\n`) });
  return output;
}

export async function exportProjectZip(options) {
  const entries = await createExportEntries(options);
  const bytes = buildStoredZip(entries);
  return new Blob([bytes], { type: "application/zip" });
}
