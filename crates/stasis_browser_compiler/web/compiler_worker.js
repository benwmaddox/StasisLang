const ABI_VERSION = 1;
const MAX_INPUT_JSON_BYTES = 32 * 1024 * 1024;
const MAX_PROJECT_SOURCE_BYTES = 4 * 1024 * 1024;
const MAX_SOURCE_FILE_BYTES = 1024 * 1024;
const MAX_PROJECT_FILES = 128;
const MAX_OUTPUT_MODULE_BYTES = 32 * 1024 * 1024;
const MAX_OUTPUT_METADATA_BYTES = 8 * 1024 * 1024;
const MAX_EDITOR_JSON_BYTES = 8 * 1024 * 1024;
const decoder = new TextDecoder();
const encoder = new TextEncoder();

let compiler;
let compilerCoreSha256;

async function sha256Hex(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, byte => byte.toString(16).padStart(2, "0")).join("");
}

function copyMemory(pointer, length) {
  const start = pointer >>> 0;
  return compiler.memory.buffer.slice(start, start + length);
}

function readCompileError() {
  const pointer = compiler.stasis_compile_error_ptr();
  const length = compiler.stasis_compile_error_len();
  return decoder.decode(copyMemory(pointer, length));
}

function readEditorError() {
  const pointer = compiler.stasis_analyze_editor_error_ptr();
  const length = compiler.stasis_analyze_editor_error_len();
  return decoder.decode(copyMemory(pointer, length));
}

function compileFixedSample() {
  const status = compiler.stasis_compile_fixed_sample();
  if (status !== 0) {
    throw new Error(readCompileError());
  }
  const pointer = compiler.stasis_compile_output_ptr();
  const length = compiler.stasis_compile_output_len();
  return copyMemory(pointer, length);
}

function validateProjectPayload(project) {
  if (!project || typeof project.entry !== "string" || !Array.isArray(project.files)) {
    throw new Error("expected compile-project entry and files");
  }
  if (project.files.length < 1 || project.files.length > MAX_PROJECT_FILES) {
    throw new Error(`compile-project must include 1..=${MAX_PROJECT_FILES} source files`);
  }
  let totalSourceBytes = 0;
  for (const file of project.files) {
    if (!file || typeof file.path !== "string" || typeof file.source !== "string") {
      throw new Error("each compile-project file must include string path and source fields");
    }
    if (file.path.length > 1024 || file.source.length > MAX_SOURCE_FILE_BYTES) {
      throw new Error("a compile-project path or source exceeds its size limit");
    }
    totalSourceBytes += encoder.encode(file.source).byteLength;
    if (totalSourceBytes > MAX_PROJECT_SOURCE_BYTES) {
      throw new Error(`project sources exceed the ${MAX_PROJECT_SOURCE_BYTES}-byte total limit`);
    }
  }
}

function compileProject(project) {
  validateProjectPayload(project);
  const serialized = JSON.stringify({ entry: project.entry, files: project.files });
  const input = encoder.encode(serialized);
  if (input.byteLength > MAX_INPUT_JSON_BYTES) {
    throw new Error(`compile-project JSON exceeds the ${MAX_INPUT_JSON_BYTES}-byte request limit`);
  }
  if (compiler.stasis_compile_project_input_resize(input.byteLength) !== 0) {
    throw new Error(readCompileError());
  }
  const inputPointer = compiler.stasis_compile_project_input_ptr() >>> 0;
  new Uint8Array(compiler.memory.buffer, inputPointer, input.byteLength).set(input);

  if (compiler.stasis_compile_project() !== 0) {
    throw new Error(readCompileError());
  }
  const gameWasmLength = compiler.stasis_compile_output_len();
  if (gameWasmLength > MAX_OUTPUT_MODULE_BYTES) {
    throw new Error(`compiled module exceeds the ${MAX_OUTPUT_MODULE_BYTES}-byte output limit`);
  }
  const metadataLength = compiler.stasis_compile_metadata_len();
  if (metadataLength > MAX_OUTPUT_METADATA_BYTES) {
    throw new Error(`compile metadata exceeds the ${MAX_OUTPUT_METADATA_BYTES}-byte output limit`);
  }
  const gameWasm = copyMemory(compiler.stasis_compile_output_ptr(), gameWasmLength);
  const metadataBytes = copyMemory(compiler.stasis_compile_metadata_ptr(), metadataLength);
  const metadata = JSON.parse(decoder.decode(metadataBytes));
  metadata.provenance.compilerCoreSha256 = compilerCoreSha256;
  return { gameWasm, metadata };
}

function validateEditorPayload(request) {
  if (!request || typeof request.path !== "string" || typeof request.source !== "string") {
    throw new Error("expected analyze-editor path and source strings");
  }
  const files = request.files === undefined ? [] : request.files;
  if (!Array.isArray(files) || files.length > MAX_PROJECT_FILES) {
    throw new Error(`analyze-editor must include at most ${MAX_PROJECT_FILES} other files`);
  }
  let totalSourceBytes = encoder.encode(request.source).byteLength;
  if (totalSourceBytes > MAX_SOURCE_FILE_BYTES) {
    throw new Error(`active editor source exceeds the ${MAX_SOURCE_FILE_BYTES}-byte per-file limit`);
  }
  for (const file of files) {
    if (!file || typeof file.path !== "string" || typeof file.source !== "string") {
      throw new Error("each analyze-editor file must include string path and source fields");
    }
    if (file.path === request.path) {
      continue;
    }
    const sourceBytes = encoder.encode(file.source).byteLength;
    if (sourceBytes > MAX_SOURCE_FILE_BYTES) {
      throw new Error(`editor source '${file.path}' exceeds the ${MAX_SOURCE_FILE_BYTES}-byte per-file limit`);
    }
    totalSourceBytes += sourceBytes;
    if (totalSourceBytes > MAX_PROJECT_SOURCE_BYTES) {
      throw new Error(`editor project sources exceed the ${MAX_PROJECT_SOURCE_BYTES}-byte total limit`);
    }
  }
  if (request.cursor !== undefined && (!Number.isInteger(request.cursor) || request.cursor < 0 || request.cursor > 0xffffffff)) {
    throw new Error("analyze-editor cursor must be a non-negative UTF-16 offset");
  }
  return {
    path: request.path,
    source: request.source,
    files: files.filter(file => file.path !== request.path),
    ...(request.cursor === undefined ? {} : { cursor: request.cursor }),
  };
}

function analyzeEditor(request) {
  const payload = validateEditorPayload(request);
  const input = encoder.encode(JSON.stringify(payload));
  if (input.byteLength > MAX_EDITOR_JSON_BYTES) {
    throw new Error(`analyze-editor JSON exceeds the ${MAX_EDITOR_JSON_BYTES}-byte request limit`);
  }
  if (compiler.stasis_analyze_editor_input_resize(input.byteLength) !== 0) {
    throw new Error(readEditorError());
  }
  const inputPointer = compiler.stasis_analyze_editor_input_ptr() >>> 0;
  new Uint8Array(compiler.memory.buffer, inputPointer, input.byteLength).set(input);

  if (compiler.stasis_analyze_editor() !== 0) {
    throw new Error(readEditorError());
  }
  const outputLength = compiler.stasis_analyze_editor_output_len();
  if (outputLength > MAX_EDITOR_JSON_BYTES) {
    throw new Error(`editor analysis exceeds the ${MAX_EDITOR_JSON_BYTES}-byte output limit`);
  }
  return JSON.parse(decoder.decode(copyMemory(compiler.stasis_analyze_editor_output_ptr(), outputLength)));
}

self.addEventListener("message", event => {
  const request = event.data;
  if (!request || request.requestId === undefined) {
    self.postMessage({
      type: "compile-error",
      requestId: request?.requestId ?? null,
      message: "expected a compilation request with requestId",
    });
    return;
  }

  try {
    if (request.type === "compile-fixed") {
      const gameWasm = compileFixedSample();
      self.postMessage({ type: "compiled", requestId: request.requestId, gameWasm }, [gameWasm]);
      return;
    }
    if (request.type === "compile-project") {
      const project = request.project ?? request;
      const { gameWasm, metadata } = compileProject(project);
      self.postMessage(
        { type: "compiled", requestId: request.requestId, gameWasm, metadata },
        [gameWasm],
      );
      return;
    }
    if (request.type === "analyze-editor") {
      const analysis = analyzeEditor(request);
      self.postMessage({ type: "editor-analysis", requestId: request.requestId, ...analysis });
      return;
    }
    throw new Error("expected compile-fixed, compile-project, or analyze-editor request");
  } catch (error) {
    if (request.type === "analyze-editor") {
      self.postMessage({
        type: "editor-analysis-error",
        requestId: request.requestId,
        path: typeof request.path === "string" ? request.path : "",
        message: error instanceof Error ? error.message : String(error),
      });
      return;
    }
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
  compilerCoreSha256 = await sha256Hex(await response.clone().arrayBuffer());
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
