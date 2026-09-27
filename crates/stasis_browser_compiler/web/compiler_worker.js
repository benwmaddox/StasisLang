const ABI_VERSION = 1;
const decoder = new TextDecoder();

let compiler;

function copyMemory(pointer, length) {
  return compiler.memory.buffer.slice(pointer, pointer + length);
}

function compileFixedSample() {
  const status = compiler.stasis_compile_fixed_sample();
  if (status !== 0) {
    const pointer = compiler.stasis_compile_error_ptr();
    const length = compiler.stasis_compile_error_len();
    throw new Error(decoder.decode(copyMemory(pointer, length)));
  }
  const pointer = compiler.stasis_compile_output_ptr();
  const length = compiler.stasis_compile_output_len();
  return copyMemory(pointer, length);
}

self.addEventListener("message", event => {
  const request = event.data;
  if (!request || request.type !== "compile-fixed" || request.requestId === undefined) {
    self.postMessage({
      type: "compile-error",
      requestId: request?.requestId ?? null,
      message: "expected a compile-fixed request with requestId",
    });
    return;
  }

  try {
    const gameWasm = compileFixedSample();
    self.postMessage({ type: "compiled", requestId: request.requestId, gameWasm }, [gameWasm]);
  } catch (error) {
    self.postMessage({
      type: "compile-error",
      requestId: request.requestId,
      message: error instanceof Error ? error.message : String(error),
    });
  }
});

try {
  const response = await fetch(new URL("./compiler_core.wasm", import.meta.url));
  if (!response.ok) {
    throw new Error(`compiler core request failed with HTTP ${response.status}`);
  }
  const result = await WebAssembly.instantiateStreaming(response, {});
  compiler = result.instance.exports;
  const actualAbi = compiler.stasis_browser_compiler_abi_version();
  if (actualAbi !== ABI_VERSION) {
    throw new Error(`compiler ABI ${actualAbi} does not match Worker ABI ${ABI_VERSION}`);
  }
  self.postMessage({ type: "ready", abiVersion: actualAbi });
} catch (error) {
  self.postMessage({
    type: "fatal",
    message: error instanceof Error ? error.message : String(error),
  });
}
