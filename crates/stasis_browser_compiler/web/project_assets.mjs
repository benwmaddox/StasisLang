const PNG_SIGNATURE = Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10);
const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const MAX_ASSET_COUNT = 16;
const MAX_ASSET_BYTES = 8 * 1024 * 1024;
const MAX_TOTAL_ASSET_BYTES = 32 * 1024 * 1024;
const MAX_IMAGE_DIMENSION = 4096;
const MAX_IMAGE_PIXELS = 16_000_000;
const MAX_SVG_BYTES = 512 * 1024;

const SVG_ELEMENTS = new Set([
  "svg", "g", "path", "rect", "circle", "ellipse", "line", "polyline", "polygon",
  "defs", "linearGradient", "radialGradient", "stop",
]);
const SVG_ATTRIBUTES = new Set([
  "xmlns", "width", "height", "viewBox", "version", "preserveAspectRatio", "id",
  "x", "y", "x1", "y1", "x2", "y2", "cx", "cy", "r", "rx", "ry", "d", "points",
  "fill", "fill-rule", "fill-opacity", "stroke", "stroke-width", "stroke-linecap",
  "stroke-linejoin", "stroke-miterlimit", "stroke-dasharray", "stroke-dashoffset",
  "opacity", "stroke-opacity", "transform", "offset", "stop-color", "stop-opacity",
  "gradientUnits", "gradientTransform",
]);

function fail(message) {
  throw new Error(message);
}

function checkedImageDimensions(width, height) {
  if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height)
      || width <= 0 || height <= 0
      || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION
      || width * height > MAX_IMAGE_PIXELS) {
    fail(`image dimensions must be at most ${MAX_IMAGE_DIMENSION} by ${MAX_IMAGE_DIMENSION} and ${MAX_IMAGE_PIXELS} pixels`);
  }
  return { width, height };
}

export function inspectPngHeader(input) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  if (bytes.byteLength < 24 || !PNG_SIGNATURE.every((value, index) => bytes[index] === value)) {
    fail("asset is not a PNG image");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(8, false) !== 13
      || String.fromCharCode(...bytes.subarray(12, 16)) !== "IHDR") {
    fail("PNG does not begin with a valid IHDR chunk");
  }
  return checkedImageDimensions(view.getUint32(16, false), view.getUint32(20, false));
}

export function normalizeAssetPath(path) {
  if (typeof path !== "string" || !/^assets\/[A-Za-z0-9][A-Za-z0-9._-]{0,119}\.(?:png)$/i.test(path)) {
    fail("asset paths must be a safe PNG path under assets/");
  }
  const name = path.slice("assets/".length);
  if (name === "." || name === "..") fail("asset path is not allowed");
  return `assets/${name}`;
}

function normalizedFilename(fileName, extension, existingPaths) {
  const rawName = String(fileName || "asset").split(/[\\/]/).at(-1) || "asset";
  const base = rawName.replace(/\.[^.]*$/, "").normalize("NFKD")
    .replace(/[^A-Za-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 80) || "asset";
  let candidate = `${base}.${extension}`;
  let suffix = 2;
  while (existingPaths.has(`assets/${candidate}`)) {
    candidate = `${base}-${suffix}.${extension}`;
    suffix += 1;
  }
  return candidate;
}

function svgDimension(value) {
  if (typeof value !== "string" || !/^\s*\d+(?:\.\d+)?(?:px)?\s*$/.test(value)) return null;
  const number = Number.parseFloat(value);
  return Number.isFinite(number) ? Math.ceil(number) : null;
}

function svgViewBox(value) {
  if (typeof value !== "string") return null;
  const values = value.trim().split(/[ ,]+/).map(Number);
  if (values.length !== 4 || values.some(number => !Number.isFinite(number)) || values[2] <= 0 || values[3] <= 0) return null;
  return { width: Math.ceil(values[2]), height: Math.ceil(values[3]) };
}

function safeSvgAttribute(name, value) {
  if (!SVG_ATTRIBUTES.has(name) || name.toLowerCase().startsWith("on") || name === "style") return false;
  if (name === "xmlns") return value === SVG_NAMESPACE;
  if (/url\s*\(|(?:javascript|https?|file|data):|\/\//i.test(value)) return false;
  if (name === "id") return /^[A-Za-z_][A-Za-z0-9_.-]{0,63}$/.test(value);
  if (["fill", "stroke", "stop-color"].includes(name)) {
    return /^(?:none|currentColor|#[\da-f]{3,8}|[a-z]{1,24})$/i.test(value);
  }
  if (["width", "height", "x", "y", "x1", "y1", "x2", "y2", "cx", "cy", "r", "rx", "ry", "stroke-width", "stroke-miterlimit", "stroke-dashoffset", "offset", "opacity", "fill-opacity", "stroke-opacity", "stop-opacity"].includes(name)) {
    return /^[-+]?(?:\d+(?:\.\d*)?|\.\d+)%?$/.test(value);
  }
  if (["viewBox", "points", "stroke-dasharray"].includes(name)) {
    return /^[-+\d.eE,%\s]+$/.test(value);
  }
  if (name === "d") return /^[MmZzLlHhVvCcSsQqTtAa0-9+\-., eE]*$/.test(value);
  if (name === "transform" || name === "gradientTransform") {
    return /^(?:(?:matrix|translate|scale|rotate|skewX|skewY)\s*\([-+\d.eE,\s]+\)\s*)+$/.test(value);
  }
  if (name === "fill-rule" || name === "stroke-linecap" || name === "stroke-linejoin" || name === "version" || name === "preserveAspectRatio" || name === "gradientUnits") {
    return /^[A-Za-z0-9_ .-]{1,64}$/.test(value);
  }
  return name === "xmlns" || name === "id";
}

function sanitizeSvgDocument(source, DOMParserType) {
  const text = typeof source === "string" ? source : new TextDecoder("utf-8", { fatal: true }).decode(source);
  if (new TextEncoder().encode(text).byteLength > MAX_SVG_BYTES) fail("SVG files must be 512 KiB or smaller");
  if (/<!\s*(?:DOCTYPE|ENTITY)|<\s*(?:script|foreignObject|style|image|use|iframe|audio|video|animate|set)\b|\bon[a-z]+\s*=|\b(?:href|xlink:href)\s*=|url\s*\(/i.test(text)) {
    fail("SVG contains an unsupported active or external feature");
  }
  if (typeof DOMParserType !== "function") fail("safe SVG parsing is unavailable in this browser");
  const document = new DOMParserType().parseFromString(text, "image/svg+xml");
  if (document.querySelector("parsererror")) fail("SVG markup is invalid");
  const root = document.documentElement;
  if (!root || root.localName !== "svg" || root.namespaceURI !== SVG_NAMESPACE) fail("SVG root must use the SVG namespace");
  const visit = element => {
    if (element.nodeType !== 1 || element.namespaceURI !== SVG_NAMESPACE || !SVG_ELEMENTS.has(element.localName)) {
      fail(`SVG element <${element.localName || "?"}> is not allowed`);
    }
    for (const attribute of Array.from(element.attributes)) {
      if (attribute.namespaceURI && attribute.name !== "xmlns") fail(`SVG attribute ${attribute.name} is not allowed`);
      if (!safeSvgAttribute(attribute.name, attribute.value)) fail(`SVG attribute ${attribute.name} is not allowed`);
    }
    for (const child of Array.from(element.childNodes)) {
      if (child.nodeType === 1) visit(child);
      else if (child.nodeType === 3 && !child.textContent.trim()) continue;
      else fail("SVG text and non-element content are not supported");
    }
  };
  visit(root);
  const size = svgViewBox(root.getAttribute("viewBox")) || {
    width: svgDimension(root.getAttribute("width")),
    height: svgDimension(root.getAttribute("height")),
  };
  const width = svgDimension(root.getAttribute("width")) || size.width;
  const height = svgDimension(root.getAttribute("height")) || size.height;
  if (!width || !height) fail("SVG needs pixel width and height or a positive viewBox");
  checkedImageDimensions(width, height);
  return { markup: new XMLSerializer().serializeToString(root), width, height };
}

async function blobAsPng(svgBlob, width, height) {
  let bitmap;
  let sourceUrl = null;
  try {
    if (typeof createImageBitmap === "function") {
      try { bitmap = await createImageBitmap(svgBlob, { resizeWidth: width, resizeHeight: height }); }
      catch { bitmap = null; }
    }
    if (!bitmap) {
      sourceUrl = URL.createObjectURL(svgBlob);
      const image = new Image();
      image.decoding = "async";
      if (typeof image.decode === "function") {
        image.src = sourceUrl;
        await image.decode();
      } else {
        const loaded = new Promise((resolve, reject) => {
          image.onload = resolve;
          image.onerror = () => reject(new Error("sanitized SVG could not be decoded"));
        });
        image.src = sourceUrl;
        await loaded;
      }
      bitmap = image;
    }
    if (!bitmap.width || !bitmap.height || bitmap.width > MAX_IMAGE_DIMENSION || bitmap.height > MAX_IMAGE_DIMENSION) {
      fail("SVG decoded dimensions exceed the safe raster limits");
    }
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d", { alpha: true, willReadFrequently: false });
    if (!context) fail("browser canvas is unavailable for SVG rasterization");
    context.drawImage(bitmap, 0, 0, width, height);
    const png = await new Promise((resolve, reject) => canvas.toBlob(blob => {
      if (blob) resolve(blob);
      else reject(new Error("sanitized SVG could not be rasterized"));
    }, "image/png"));
    if (png.size > MAX_ASSET_BYTES) fail("rasterized SVG exceeds the 8 MiB asset limit");
    return png;
  } finally {
    bitmap?.close?.();
    if (sourceUrl) URL.revokeObjectURL(sourceUrl);
  }
}

async function decodePng(blob, expected) {
  let bitmap;
  try {
    if (typeof createImageBitmap === "function") {
      bitmap = await createImageBitmap(blob);
    } else {
      const url = URL.createObjectURL(blob);
      try {
        const image = new Image();
        image.src = url;
        if (typeof image.decode === "function") await image.decode();
        else await new Promise((resolve, reject) => {
          image.onload = resolve;
          image.onerror = () => reject(new Error("PNG could not be decoded"));
        });
        bitmap = image;
      } finally {
        URL.revokeObjectURL(url);
      }
    }
    if (bitmap.width !== expected.width || bitmap.height !== expected.height) fail("PNG decoded dimensions do not match its IHDR header");
  } catch (error) {
    throw new Error(`PNG image decoding failed: ${error?.message || error}`);
  } finally {
    bitmap?.close?.();
  }
}

async function sha256(bytes) {
  if (!globalThis.crypto?.subtle) fail("SHA-256 is unavailable; serve the playground from localhost or HTTPS");
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, byte => byte.toString(16).padStart(2, "0")).join("");
}

export class ProjectAssetStore {
  #assets = new Map();
  #totalBytes = 0;

  get size() { return this.#assets.size; }
  get totalBytes() { return this.#totalBytes; }

  list() {
    return Array.from(this.#assets.values(), asset => ({
      path: asset.path, name: asset.name, mimeType: asset.mimeType,
      width: asset.width, height: asset.height, byteLength: asset.byteLength, sha256: asset.sha256,
      previewUrl: asset.objectUrl,
    }));
  }

  has(path) { return this.#assets.has(normalizeAssetPath(path)); }

  configOverrides() {
    const assets = Object.create(null);
    const metadata = Object.create(null);
    for (const asset of this.#assets.values()) {
      assets[asset.path] = asset.objectUrl;
      metadata[asset.path] = {
        encoding: "png", width: asset.width, height: asset.height,
        byte_length: asset.byteLength, sha256: asset.sha256,
      };
    }
    return { assets, asset_metadata: metadata };
  }

  async importFile(file) {
    if (!file || typeof file.arrayBuffer !== "function") fail("choose a PNG or SVG image file");
    if (this.#assets.size >= MAX_ASSET_COUNT) fail(`a project may contain at most ${MAX_ASSET_COUNT} assets`);
    if (file.size > MAX_ASSET_BYTES) fail("image files must be 8 MiB or smaller");
    let input = new Uint8Array(await file.arrayBuffer());
    if (input.byteLength > MAX_ASSET_BYTES) fail("image files must be 8 MiB or smaller");
    let blob;
    let dimensions;
    let extension;
    if (file.type === "image/png" || /\.png$/i.test(file.name)) {
      dimensions = inspectPngHeader(input);
      blob = new Blob([input], { type: "image/png" });
      await decodePng(blob, dimensions);
      extension = "png";
    } else if (file.type === "image/svg+xml" || /\.svg$/i.test(file.name)) {
      const sanitized = sanitizeSvgDocument(input, globalThis.DOMParser);
      dimensions = { width: sanitized.width, height: sanitized.height };
      blob = await blobAsPng(new Blob([sanitized.markup], { type: "image/svg+xml" }), dimensions.width, dimensions.height);
      input = new Uint8Array(await blob.arrayBuffer());
      dimensions = inspectPngHeader(input);
      await decodePng(blob, dimensions);
      extension = "png";
    } else {
      fail("only PNG and SVG image files are supported");
    }
    if (input.byteLength > MAX_ASSET_BYTES || this.#totalBytes + input.byteLength > MAX_TOTAL_ASSET_BYTES) {
      fail("project assets exceed the 32 MiB total limit");
    }
    const name = normalizedFilename(file.name, extension, new Set(this.#assets.keys()));
    const path = normalizeAssetPath(`assets/${name}`);
    const outputBlob = blob.type === "image/png" ? blob : new Blob([input], { type: "image/png" });
    const digest = await sha256(input);
    const objectUrl = URL.createObjectURL(outputBlob);
    const asset = {
      path, name, mimeType: "image/png", width: dimensions.width, height: dimensions.height,
      byteLength: input.byteLength, bytes: input, blob: outputBlob, objectUrl, sha256: digest,
    };
    this.#assets.set(path, asset);
    this.#totalBytes += input.byteLength;
    return { path, name, mimeType: asset.mimeType, width: asset.width, height: asset.height, byteLength: asset.byteLength, sha256: asset.sha256 };
  }

  remove(path) {
    const normalized = normalizeAssetPath(path);
    const asset = this.#assets.get(normalized);
    if (!asset) return false;
    URL.revokeObjectURL(asset.objectUrl);
    this.#totalBytes -= asset.byteLength;
    this.#assets.delete(normalized);
    return true;
  }

  exportEntries() {
    return Array.from(this.#assets.values(), asset => ({ path: asset.path, bytes: asset.bytes.slice(), mimeType: asset.mimeType }));
  }

  clear() {
    for (const asset of this.#assets.values()) URL.revokeObjectURL(asset.objectUrl);
    this.#assets.clear();
    this.#totalBytes = 0;
  }
}

export const projectAssetLimits = Object.freeze({
  maxAssetCount: MAX_ASSET_COUNT,
  maxAssetBytes: MAX_ASSET_BYTES,
  maxTotalAssetBytes: MAX_TOTAL_ASSET_BYTES,
  maxImageDimension: MAX_IMAGE_DIMENSION,
  maxImagePixels: MAX_IMAGE_PIXELS,
});
