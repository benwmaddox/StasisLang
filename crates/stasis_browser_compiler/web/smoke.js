const worker = new Worker("./compiler_worker.js", { type: "module" });
const pending = new Map();
let nextRequestId = 0;

worker.addEventListener("message", event => {
  const response = event.data;
  if (response.type === "ready") {
    document.body.dataset.ready = "true";
    document.body.dataset.abiVersion = String(response.abiVersion);
    return;
  }
  if (response.type === "fatal") {
    document.body.dataset.fatal = response.message;
    return;
  }
  const request = pending.get(response.requestId);
  if (!request) return;
  pending.delete(response.requestId);
  if (response.type === "compiled") {
    request.resolve(new Uint8Array(response.gameWasm));
  } else {
    request.reject(new Error(response.message));
  }
});

function requestCompilation() {
  const requestId = ++nextRequestId;
  const result = new Promise((resolve, reject) => pending.set(requestId, { resolve, reject }));
  worker.postMessage({ type: "compile-fixed", requestId });
  return result;
}

async function sha256(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, byte => byte.toString(16).padStart(2, "0")).join("");
}

window.runBrowserCompilerSmoke = async () => {
  const first = await requestCompilation();
  const second = await requestCompilation();
  if (first.length !== second.length || first.some((byte, index) => byte !== second[index])) {
    throw new Error("repeated compilation produced different game.wasm bytes");
  }
  const module = await WebAssembly.compile(first);
  const imports = WebAssembly.Module.imports(module);
  const exports = WebAssembly.Module.exports(module);
  const instance = await WebAssembly.instantiate(module, {});
  const mainResult = instance.exports.main();
  const result = {
    byteLength: first.byteLength,
    sha256: await sha256(first),
    imports,
    exports,
    mainResult,
  };
  document.body.dataset.complete = "true";
  document.body.dataset.mainResult = String(mainResult);
  return result;
};
