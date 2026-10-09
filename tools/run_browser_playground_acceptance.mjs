import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import path from "node:path";

const repository = path.resolve(import.meta.dirname, "..");
const bundle = path.resolve(process.argv[2] || "target/browser-compiler-bundle");
const siteRoot = process.env.STASIS_PLAYGROUND_SITE_ROOT
  ? path.resolve(process.env.STASIS_PLAYGROUND_SITE_ROOT) : null;
const evidence = path.resolve(process.argv[3] || "target/browser-playground-acceptance");
const browserPath = process.env.STASIS_BROWSER_EXECUTABLE || (process.platform === "win32"
  ? "C:/Program Files/Google/Chrome/Application/chrome.exe" : "google-chrome");
await mkdir(evidence, { recursive: true });
const profile = await mkdtemp(path.join(evidence, "chrome-"));
const requests = [];
const exportedFiles = new Map();
const starterExportFiles = new Map();
function unpackExport(archiveBytes, files) {
  let offset = 0;
  while (archiveBytes.readUInt32LE(offset) === 0x04034b50) {
    assert.equal(archiveBytes.readUInt16LE(offset + 8), 0, "export must use stored ZIP entries");
    const length = archiveBytes.readUInt32LE(offset + 18);
    const nameLength = archiveBytes.readUInt16LE(offset + 26);
    const extraLength = archiveBytes.readUInt16LE(offset + 28);
    const name = archiveBytes.subarray(offset + 30, offset + 30 + nameLength).toString("utf8");
    assert.ok(!name.includes("..") && !name.startsWith("/") && !files.has(name));
    const begin = offset + 30 + nameLength + extraLength;
    files.set(name, archiveBytes.subarray(begin, begin + length));
    offset = begin + length;
  }
}
const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript",
  ".json": "application/json", ".css": "text/css", ".wasm": "application/wasm", ".png": "image/png", ".ttf": "font/ttf", ".txt": "text/plain" };
const server = createServer(async (request, response) => {
  const url = new URL(request.url, "http://localhost");
  requests.push({ method: request.method, path: url.pathname });
  try {
    assert.equal(request.method, "GET");
    const route = siteRoot && url.pathname.endsWith("/") ? `${url.pathname}index.html`
      : url.pathname === "/" ? "/playground.html" : url.pathname;
    const name = decodeURIComponent(route).slice(1);
    assert.ok(name && !name.split("/").some(part => !part || part === "." || part === "..") && !name.includes("\\"));
    const bytes = name.startsWith("export/") ? exportedFiles.get(name.slice(7))
      : name.startsWith("starter-export/") ? starterExportFiles.get(name.slice(15))
      : await readFile(path.join(siteRoot || bundle, name));
    assert.ok(bytes);
    response.setHeader("Content-Type", types[path.extname(name)] || "application/octet-stream");
    response.setHeader("Cache-Control", "no-store");
    response.end(bytes);
  } catch { response.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const browser = spawn(browserPath, ["--headless=new", "--no-sandbox", "--disable-gpu-sandbox",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--no-first-run",
  "--no-default-browser-check", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"],
{ stdio: "ignore" });
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(action, description) {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) { const result = await action().catch(() => null); if (result) return result; await delay(50); }
  throw new Error(`${description} timed out`);
}
let socket;
const frames = [];
const writes = [];
try {
  const port = (await until(() => readFile(path.join(profile, "DevToolsActivePort"), "utf8"), "Chrome startup")).split("\n")[0];
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(response => response.json());
  socket = new WebSocket(pages.find(page => page.type === "page").webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener("open", resolve, { once: true }));
  let nextId = 0;
  const pending = new Map();
  const contexts = new Map();
  let editorContextId;
  const failures = [];
  socket.addEventListener("message", event => {
    const message = JSON.parse(event.data);
    if (message.method === "Runtime.executionContextCreated" && message.params.context.auxData?.isDefault) {
      contexts.set(message.params.context.auxData.frameId, message.params.context.id);
    }
    if (message.method === "Runtime.exceptionThrown") failures.push(message.params.exceptionDetails.text);
    if (message.method === "Page.screencastFrame") {
      const file = `frame-${String(frames.length).padStart(5, "0")}.png`;
      frames.push({ file, timestamp: message.params.metadata.timestamp });
      writes.push(writeFile(path.join(evidence, file), Buffer.from(message.params.data, "base64")));
      void call("Page.screencastFrameAck", { sessionId: message.params.sessionId });
    }
    const callback = pending.get(message.id);
    if (callback) { pending.delete(message.id); callback(message); }
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 120_000);
    pending.set(id, message => { clearTimeout(timer); message.error ? reject(new Error(message.error.message)) : resolve(message.result); });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await call("Runtime.evaluate", { expression, contextId: editorContextId, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const screenshot = async name => {
    const result = await call("Page.captureScreenshot", { format: "png" });
    await writeFile(path.join(evidence, `${name}.png`), Buffer.from(result.data, "base64"));
  };
  await call("Page.enable");
  await call("Runtime.enable");
  const viewportHeight = siteRoot ? 1280 : 960;
  await call("Emulation.setDeviceMetricsOverride", { width: 1440, height: viewportHeight, deviceScaleFactor: 1, mobile: false });
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/${siteRoot ? "playground/" : ""}` });
  if (siteRoot) {
    editorContextId = await until(async () => {
      const tree = await call("Page.getFrameTree");
      const child = tree.frameTree.childFrames?.find(frame => frame.frame.url.endsWith("/playground/app/playground.html"));
      return child && contexts.get(child.frame.id);
    }, "site playground frame");
  }
  await until(() => evaluate("Boolean(window.STASIS_PLAYGROUND_EDITOR)"), "editor API");
  await evaluate("window.STASIS_PLAYGROUND_EDITOR.ready");
  const readyBoundary = requests.length;
  const starterCompilation = await evaluate(`(async () => {
    const result = await window.STASIS_PLAYGROUND_EDITOR.compile();
    return { byteLength: result.gameWasm.byteLength, metadata: result.metadata };
  })()`);
  assert.ok(starterCompilation.byteLength > 0, "the shipped starter must compile to Wasm");
  assert.ok(!starterCompilation.metadata.imports.some(name => name.startsWith("web_")),
    "the shipped starter must use canonical Stasis library APIs");
  assert.ok(!starterCompilation.metadata.replayCompatibility.state_snapshot.entries
    .some(entry => entry.path.startsWith("state.input_frame.")),
  "hot swaps must preserve game state without restoring host input observations");
  await call("Page.startScreencast", { format: "png", maxWidth: 1440, maxHeight: viewportHeight, everyNthFrame: 3 });
  await evaluate("window.STASIS_PLAYGROUND_EDITOR.run()");
  await delay(1000);
  const starterRender = await evaluate(`(() => {
    const child = window.STASIS_PLAYGROUND_EDITOR.getIframe().contentWindow;
    child.STASIS_PLAYGROUND.pause(true);
    return { assets: window.STASIS_PLAYGROUND_EDITOR.getAssets(),
      text: child.document.body.dataset.text, sprites: child.document.body.dataset.spriteDecodedCount,
      error: child.document.getElementById('stasis-error').textContent };
  })()`);
  assert.equal(starterRender.error, "");
  assert.ok(Number(starterRender.text) >= 2, "starter scores must render through the font pipeline");
  assert.ok(Number(starterRender.sprites) >= 3, "starter SVG sprites must decode");
  await screenshot("starter-pong");
  const starterArchive = await evaluate(`(async()=>{const blob=await window.STASIS_PLAYGROUND_EDITOR.exportProject(); const bytes=new Uint8Array(await blob.arrayBuffer()); let text=''; for(let i=0;i<bytes.length;i+=16384)text+=String.fromCharCode(...bytes.subarray(i,i+16384));return btoa(text)})()`);
  const starterArchiveBytes = Buffer.from(starterArchive, "base64");
  unpackExport(starterArchiveBytes, starterExportFiles);
  assert.ok(starterExportFiles.has("assets/ui.ttf"), "starter export includes its font");
  assert.ok(starterExportFiles.has("assets/OFL.txt"), "starter export includes the font license");
  await writeFile(path.join(evidence, "starter-export.zip"), starterArchiveBytes);
  const starterFinished = await evaluate(`(async () => {
    const editor = window.STASIS_PLAYGROUND_EDITOR;
    const runtime = editor.getIframe().contentWindow.STASIS_PLAYGROUND;
    const source = editor.getFile('main.stasis').source;
    editor.setFile('main.stasis', source + '\\nfunction @effects(state) on_code_swap(): void { state.cpu.score = 4; state.ball.x = -20.0; state.ball.vx = -4.0; }\\n');
    await editor.run();
    await runtime.step();
    const finished = runtime.snapshot();
    editor.setFile('main.stasis', source);
    return finished;
  })()`);
  await screenshot("starter-pong-game-over");
  const starterRestarted = await evaluate(`(async () => {
    const editor = window.STASIS_PLAYGROUND_EDITOR;
    const runtime = editor.getIframe().contentWindow.STASIS_PLAYGROUND;
    const child = editor.getIframe().contentWindow;
    child.dispatchEvent(new child.KeyboardEvent('keydown', { code: 'Space', key: ' ' }));
    await runtime.step();
    child.dispatchEvent(new child.KeyboardEvent('keyup', { code: 'Space', key: ' ' }));
    return runtime.snapshot();
  })()`);
  const starterMatch = { finished: starterFinished, restarted: starterRestarted };
  const starterScalar = (snapshot, name) => {
    const entry = starterCompilation.metadata.replayCompatibility.state_snapshot.entries.find(entry => entry.path === name);
    assert.ok(entry, `missing starter snapshot field ${name}`);
    return Buffer.from(Object.values(snapshot.bytes)).readInt32LE(entry.offset);
  };
  assert.equal(starterScalar(starterMatch.finished, "state.cpu.score"), 5);
  assert.equal(starterScalar(starterMatch.finished, "state.game_over"), 1);
  assert.equal(starterScalar(starterMatch.restarted, "state.cpu.score"), 0);
  assert.equal(starterScalar(starterMatch.restarted, "state.game_over"), 0);
  await screenshot("starter-pong-restarted");
  const editorDraft = 'global score: i32;\nfunction helper(value: i32): i32 { return value; }\nfunction main(): i32 { return hel';
  await evaluate(`(() => { const editor=window.STASIS_PLAYGROUND_EDITOR; editor.setFile("main.stasis",${JSON.stringify(editorDraft)}); editor.setEntry("main.stasis"); const area=document.getElementById("source-editor"); area.focus(); area.setSelectionRange(area.value.length,area.value.length); })()`);
  await call("Input.dispatchKeyEvent", {type:"keyDown",key:" ",code:"Space",modifiers:2});
  await call("Input.dispatchKeyEvent", {type:"keyUp",key:" ",code:"Space",modifiers:2});
  const editorAnalysis = await until(() => evaluate(`(() => {const view=window.STASIS_PLAYGROUND_EDITOR.getEditorView(); return !view.analysisPending && view.completionOpen && view.completions.some(item=>item.label==="helper") ? view : null;})()`), "compiler-backed incomplete-source completion");
  assert.ok(editorAnalysis.tokens.some(token=>token.kind==="keyword"));
  assert.ok(editorAnalysis.tokens.some(token=>token.kind==="type"));
  assert.ok(editorAnalysis.completions.find(item=>item.label==="helper").signature.includes("i32"));
  assert.equal(await evaluate('document.getElementById("source-highlight").textContent'), editorDraft);
  await screenshot("syntax-highlighting-autocomplete");
  const completionIndex=editorAnalysis.completions.findIndex(item=>item.label==="helper");
  for(let index=0;index<completionIndex;index++) {
    await call("Input.dispatchKeyEvent",{type:"keyDown",key:"ArrowDown",code:"ArrowDown"});
    await call("Input.dispatchKeyEvent",{type:"keyUp",key:"ArrowDown",code:"ArrowDown"});
  }
  await call("Input.dispatchKeyEvent",{type:"keyDown",key:"Enter",code:"Enter"});
  await call("Input.dispatchKeyEvent",{type:"keyUp",key:"Enter",code:"Enter"});
  assert.equal(await evaluate('document.getElementById("source-editor").value'), editorDraft.slice(0,-3)+editorAnalysis.completions[completionIndex].insertText);
  const source = `import "movement.stasis";
global counter: i32;
function @extern("web_begin_frame") web_begin_frame(r: i32, g: i32, b: i32): void;
function @extern("web_draw_rect") web_draw_rect(x: i32, y: i32, w: i32, h: i32, r: i32, g: i32, b: i32): void;
function main(): i32 { counter = 41; return 0; }
function tick(): i32 { counter += movement.amount(); return 0; }
function render(): i32 { web_begin_frame(7, 16, 32); web_draw_rect(counter % 500, 100, 64, 64, 70, 200, 240); return 0; }
function @effects(counter) on_code_swap(): void { counter += 100; }
`;
  await evaluate(`(() => { const editor = window.STASIS_PLAYGROUND_EDITOR; for (const file of editor.getProject().files) editor.deleteFile(file.path); editor.setFile("main.stasis", ${JSON.stringify(source)}); editor.setFile("movement.stasis", "function amount(): i32 { return 1; }"); editor.setEntry("main.stasis"); })()`);
  const first = await evaluate("window.STASIS_PLAYGROUND_EDITOR.restart()");
  assert.ok(Number.isInteger(first.generation) && first.generation >= 0);
  await delay(1500);
  await screenshot("initial-game");
  const snapshots = await evaluate(`(async () => {
    const editor = window.STASIS_PLAYGROUND_EDITOR;
    const runtime = editor.getIframe().contentWindow.STASIS_PLAYGROUND;
    runtime.pause(true);
    const before = runtime.snapshot();
    editor.setFile("movement.stasis", "function amount(): i32 { return 2; }");
    const swap = await editor.run();
    const after = runtime.snapshot();
    await runtime.step();
    const stepped = runtime.snapshot();
    return {before,after,stepped,swap};
  })()`);
  const scalar = snapshot => Buffer.from(Object.values(snapshot.bytes)).readInt32LE(0);
  assert.equal(snapshots.after.generation, snapshots.before.generation + 1);
  assert.equal(scalar(snapshots.after), scalar(snapshots.before) + 100, "migration/hook must preserve and update old counter");
  assert.equal(scalar(snapshots.stepped), scalar(snapshots.after) + 2, "new function must execute after publication");
  await screenshot("successful-swap");
  const rejectedSources = [
    "function main(: i32 {",
    source.replace("counter += movement.amount();", "counter += movement.amount(); incompatible += 1.0;") + "global incompatible: f64;\n",
    source.replace("@effects(counter)", "@effects(counter, code_swap)").replace("counter += 100;", "counter += 100; reject_code_swap();")
      + "\nextern function @effects(code_swap) reject_code_swap(): void;\n",
    source.replace("counter += 100;", "let divisor: i32 = counter - counter; counter = counter / divisor;"),
    source.replace("@effects(counter)", "@effects(counter, graphics)")
      .replace('function @extern("web_begin_frame")', 'function @effects(graphics) @extern("web_begin_frame")')
      .replace("counter += 100;", "web_begin_frame(255, 0, 0); counter += 100;"),
  ];
  const rollback = await evaluate(`(async () => {
    const editor = window.STASIS_PLAYGROUND_EDITOR;
    const runtime = editor.getIframe().contentWindow.STASIS_PLAYGROUND;
    const before = runtime.snapshot();
    const original = editor.getFile("main.stasis").source;
    editor.deleteFile("movement.stasis");
    let deletedModuleError = null;
    try { await editor.run(); } catch (error) { deletedModuleError = String(error.message); }
    const afterDeletion = runtime.snapshot();
    editor.setFile("movement.stasis", "function amount(): i32 { return 2; }");
    const results = [];
    for (const source of ${JSON.stringify(rejectedSources)}) {
      editor.setFile("main.stasis", source);
      let error = null;
      try { await editor.run(); } catch (failure) { error = String(failure.message); }
      results.push({error,snapshot:runtime.snapshot()});
    }
    editor.setFile("main.stasis", original);
    return {before,results,deletedModuleError,afterDeletion};
  })()`);
  assert.match(rollback.deletedModuleError, /import|movement|module/i);
  assert.deepEqual(rollback.afterDeletion, rollback.before, "deleted module must not survive in a cached compiler project");
  for (const result of rollback.results) { assert.ok(result.error, "invalid candidate must reject"); assert.deepEqual(result.snapshot, rollback.before); }
  await writeFile(path.join(evidence, "transaction-receipt.json"), JSON.stringify({first,snapshots,rollback}, null, 2));
  assert.match(rollback.results[1].error, /layout/i);
  assert.match(rollback.results[2].error, /reject/i);
  assert.match(rollback.results[3].error, /divide|zero|unreachable/i);
  assert.match(rollback.results[4].error, /blocked during hook/i);
  await screenshot("rollback-preserves-game");
  const overlappingSessions = await evaluate(`(async () => {
    const editor = window.STASIS_PLAYGROUND_EDITOR;
    const results = await Promise.allSettled([editor.restart(), editor.restart()]);
    return { results: results.map(result => ({ status: result.status,
      error: result.reason ? String(result.reason.message) : null })),
      frames: document.querySelectorAll('#preview-frame-host iframe').length,
      current: editor.getIframe().isConnected };
  })()`);
  assert.equal(overlappingSessions.results[0].status, "rejected");
  assert.match(overlappingSessions.results[0].error, /superseded/i);
  assert.equal(overlappingSessions.results[1].status, "fulfilled");
  assert.equal(overlappingSessions.frames, 1, "overlapping fresh sessions must leave one live game");
  assert.equal(overlappingSessions.current, true);
  const assetResult = await evaluate(`(async () => {
    const editor=window.STASIS_PLAYGROUND_EDITOR;
    const canvas=document.createElement('canvas'); canvas.width=16;canvas.height=16;
    const context=canvas.getContext('2d');context.fillStyle='#f050c0';context.fillRect(0,0,16,16);
    const png=await new Promise(resolve=>canvas.toBlob(resolve,'image/png'));
    const first=await editor.importAsset(new File([png],'test.png',{type:'image/png'}));
    const second=await editor.importAsset(new File(['<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><rect width="24" height="24" fill="#40e090"/></svg>'],'test.svg',{type:'image/svg+xml'}));
    const errors=[];
    for(const [name,content] of [['evil.svg','<svg xmlns="http://www.w3.org/2000/svg"><script>parent.evil=true</script></svg>'], ['remote.svg','<svg xmlns="http://www.w3.org/2000/svg"><image href="https://example.invalid/x.png"/></svg>'], ['bad.png','bad']]) {
      try{await editor.importAsset(new File([content],name));errors.push(null)}catch(error){errors.push(String(error.message))}
    }
    return {first,second,errors,executed:window.evil===true};
  })()`);
  assert.equal(assetResult.executed, false);
  assert.ok(assetResult.errors.every(Boolean));
  // Exercise caller-provided graphics libraries and imported images in the same real host.
  const spriteSource = `import "vendor/stasis/stdlib/graphics.stasis";
global uploaded: Sprite;
global uploaded_svg: Sprite;
function main(): i32 { uploaded.load_sprite_from("${assetResult.first.path}", 16, 16); uploaded_svg.load_sprite_from("${assetResult.second.path}", 24, 24); return 0; }
function tick(): i32 { return 0; }
function render(): i32 { clear(0.04, 0.07, 0.12, 1.0); draw_sprite(uploaded.sprite_ref, 100.0, 100.0, 128.0, 128.0, 0, 255); draw_sprite(uploaded_svg.sprite_ref, 280.0, 100.0, 128.0, 128.0, 0, 255); return 0; }
`;
  // A fresh session is needed when replacing the complete state schema.
  await evaluate(`(() => {const editor=window.STASIS_PLAYGROUND_EDITOR; editor.setFile('main.stasis',${JSON.stringify(spriteSource)});})()`);
  const compilation = await evaluate("window.STASIS_PLAYGROUND_EDITOR.compile().then(result=>({bytes:result.gameWasm.length,metadata:result.metadata}))");
  assert.ok(compilation.bytes > 8);
  await evaluate("window.STASIS_PLAYGROUND_EDITOR.restart()");
  await delay(1500);
  const editorAssetState = await evaluate("window.STASIS_PLAYGROUND_EDITOR.getIframe().contentDocument.body.dataset.spriteDecodedCount");
  assert.ok(Number(editorAssetState) >= 2, "uploaded PNG and rasterized SVG must decode in the playground game");
  await screenshot("uploaded-asset-game");
  const archive = await evaluate(`(async()=>{const blob=await window.STASIS_PLAYGROUND_EDITOR.exportProject(); const bytes=new Uint8Array(await blob.arrayBuffer()); let text=''; for(let i=0;i<bytes.length;i+=16384)text+=String.fromCharCode(...bytes.subarray(i,i+16384));return btoa(text)})()`);
  const archiveBytes = Buffer.from(archive, "base64");
  await writeFile(path.join(evidence, "export.zip"), archiveBytes);
  unpackExport(archiveBytes, exportedFiles);
  for (const file of ["index.html", "game.js", "game.wasm"]) assert.ok(exportedFiles.has(file), `missing export ${file}`);
  assert.ok([...exportedFiles.keys()].some(name=>name.includes(assetResult.first.path)));
  await delay(250);
  const requestsAfterReady = requests.slice(readyBoundary);
  assert.deepEqual(requestsAfterReady, [], "editor operations must make zero application-server requests");
  await call("Page.stopScreencast");
  await Promise.all(writes);
  await writeFile(path.join(evidence, "screencast.json"), JSON.stringify(frames,null,2));
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/export/index.html` });
  editorContextId = undefined;
  await until(() => evaluate("Boolean(window.STASIS_RUNTIME_PROMISE)"), "exported runtime boot");
  await evaluate("window.STASIS_RUNTIME_PROMISE");
  await delay(1500);
  const exportedState = await evaluate("({backend:document.body.dataset.backend,error:document.getElementById('stasis-error')?.textContent,sprites:document.body.dataset.spriteDecodedCount,atlasUploads:document.body.dataset.atlasUploadCount})");
  assert.equal(exportedState.error || "", "");
  assert.equal(exportedState.backend, "WebGL2");
  assert.ok(Number(exportedState.sprites) >= 2, "exported PNG and rasterized SVG must decode and render");
  await screenshot("exported-asset-game");
  await call("Page.navigate", { url: `http://127.0.0.1:${server.address().port}/starter-export/index.html` });
  await until(() => evaluate("Boolean(window.STASIS_RUNTIME_PROMISE)"), "exported starter boot");
  await evaluate("window.STASIS_RUNTIME_PROMISE");
  await delay(1000);
  const starterExportState = await evaluate("({error:document.getElementById('stasis-error')?.textContent,text:document.body.dataset.text,sprites:document.body.dataset.spriteDecodedCount})");
  assert.equal(starterExportState.error || "", "");
  assert.ok(Number(starterExportState.text) >= 2, "exported starter renders font scores");
  assert.ok(Number(starterExportState.sprites) >= 3, "exported starter renders all SVG sprites");
  await screenshot("exported-starter-pong");
  assert.deepEqual(failures, []);
  const receipt = {schema:"stasis.browser_playground_acceptance.v1",siteRoot,browser:await call("Browser.getVersion"),
    starterCompilation,starterRender,starterMatch,starterExportState,editorAnalysis,first,snapshots,rollback,overlappingSessions,assets:assetResult,export:{files:[...exportedFiles.keys()],byteLength:archiveBytes.length,state:exportedState},
    network:{readyBoundary,requestsAfterReady,requests},frames:frames.length,failures};
  await writeFile(path.join(evidence,"receipt.json"), `${JSON.stringify(receipt,null,2)}\n`);
  console.log(JSON.stringify({status:"passed",receipt:path.join(evidence,"receipt.json"),frames:frames.length,exportBytes:archiveBytes.length}));
} catch (error) {
  await writeFile(path.join(evidence,"failure.json"), JSON.stringify({status:"failed",error:String(error.stack || error),requests,frames:frames.length},null,2));
  throw error;
} finally {
  socket?.close();
  if (browser.exitCode===null) {const exited=new Promise(resolve=>browser.once("exit",resolve));browser.kill();await Promise.race([exited,delay(2000)]);}
  await new Promise(resolve=>server.close(resolve));
  if(profile.startsWith(evidence+path.sep)) await rm(profile,{recursive:true,force:true,maxRetries:2}).catch(()=>{});
}
