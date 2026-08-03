import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { PNG } from "pngjs";
import * as vscode from "vscode";
import type { LiveResponse, LiveValue } from "../protocol";
import type { LiveSessionState } from "../liveSession";

interface StasisExtensionApi {
  state(): LiveSessionState;
  values(): readonly LiveValue[];
  start(): Promise<void>;
  stop(): Promise<void>;
  request(type: string, fields?: Record<string, unknown>): Promise<LiveResponse>;
  testFiles(): readonly string[];
  runTestFile(uri: string): Promise<{ stdout: string; stderr: string }>;
}

type Rgba = readonly [number, number, number, number];

function pixelAt(png: PNG, x: number, y: number): Rgba {
  assert.ok(x >= 0 && x < png.width, `pixel x ${x} is inside the ${png.width}-pixel framebuffer`);
  assert.ok(y >= 0 && y < png.height, `pixel y ${y} is inside the ${png.height}-pixel framebuffer`);
  const offset = (y * png.width + x) * 4;
  return [png.data[offset]!, png.data[offset + 1]!, png.data[offset + 2]!, png.data[offset + 3]!];
}

function isNear(actual: Rgba, expected: Rgba, tolerance: number): boolean {
  return actual.every((channel, index) => Math.abs(channel - expected[index]!) <= tolerance);
}

function assertRenderedFrame(framePath: string): void {
  const png = PNG.sync.read(fs.readFileSync(framePath));
  const logicalWidth = 800;
  const logicalHeight = 600;
  const scaleX = png.width / logicalWidth;
  const scaleY = png.height / logicalHeight;

  assert.ok(scaleX >= 1 && scaleY >= 1, `framebuffer is at least ${logicalWidth}x${logicalHeight}`);
  assert.ok(Math.abs(scaleX - scaleY) < 0.001, "framebuffer preserves the 4:3 logical render size");

  const background: Rgba = [10, 20, 40, 255];
  for (const [logicalX, logicalY] of [
    [40, 40],
    [400, 100],
    [40, 560],
    [760, 560],
  ] as const) {
    const actual = pixelAt(png, Math.floor(logicalX * scaleX), Math.floor(logicalY * scaleY));
    assert.ok(
      isNear(actual, background, 2),
      `rendered background at (${logicalX}, ${logicalY}) is ${background.join(",")}, received ${actual.join(",")}`,
    );
  }

  const line: Rgba = [229, 51, 25, 255];
  const minX = Math.floor(120 * scaleX);
  const maxX = Math.ceil(680 * scaleX);
  const centerY = 300 * scaleY;
  const minY = Math.max(0, Math.floor(centerY - 2 * scaleY));
  const maxY = Math.min(png.height - 1, Math.ceil(centerY + 2 * scaleY));
  let linePixels = 0;
  for (let y = minY; y <= maxY; y += 1) {
    for (let x = minX; x <= maxX; x += 1) {
      if (isNear(pixelAt(png, x, y), line, 3)) {
        linePixels += 1;
      }
    }
  }
  assert.ok(
    linePixels >= 400 * scaleX,
    `rendered command-buffer line contains enough expected pixels, found ${linePixels}`,
  );
}

async function waitFor(description: string, predicate: () => boolean, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Timed out waiting for ${description}.`);
}

function inspectedI32(response: LiveResponse): number {
  const data = response.data as Record<string, unknown> | undefined;
  const value = data?.value as Record<string, unknown> | undefined;
  if (!value) {
    throw new Error(`Inspection response did not contain a typed value: ${JSON.stringify(response)}`);
  }
  assert.equal(value?.type, "i32");
  assert.equal(typeof value.value, "number");
  return value.value as number;
}

function applyTextEdits(document: vscode.TextDocument, edits: readonly vscode.TextEdit[]): string {
  let text = document.getText();
  const replacements = edits
    .map((edit) => ({
      start: document.offsetAt(edit.range.start),
      end: document.offsetAt(edit.range.end),
      newText: edit.newText,
    }))
    .sort((left, right) => right.start - left.start || right.end - left.end);
  for (const replacement of replacements) {
    text = `${text.slice(0, replacement.start)}${replacement.newText}${text.slice(replacement.end)}`;
  }
  return text;
}

export async function run(): Promise<void> {
  const executable = process.env.STASIS_E2E_EXECUTABLE;
  const screenshot = process.env.STASIS_E2E_SCREENSHOT;
  if (!executable || !fs.existsSync(executable)) {
    throw new Error("The built Stasis executable is not available.");
  }
  if (!screenshot) {
    throw new Error("The screenshot output path is not configured.");
  }

  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    throw new Error("The fixture workspace is not open.");
  }
  await vscode.workspace
    .getConfiguration("stasis", folder.uri)
    .update("executablePath", executable, vscode.ConfigurationTarget.Global);

  const extension = vscode.extensions.getExtension<StasisExtensionApi>("stasislang.stasis");
  if (!extension) {
    throw new Error("The packaged Stasis VSIX is not installed.");
  }
  const api = await extension.activate();

  const grammarPath = path.join(extension.extensionPath, "syntaxes", "stasis.tmLanguage.json");
  assert.equal(fs.existsSync(grammarPath), true, "the installed VSIX contains its Stasis color grammar");
  const grammar = fs.readFileSync(grammarPath, "utf8");
  assert.match(grammar, /entity\.name\.function\.stasis/, "the color grammar scopes function names");
  assert.match(grammar, /storage\.type\.builtin\.stasis/, "the color grammar scopes built-in types");

  const formatUri = vscode.Uri.file(
    path.join(folder.uri.fsPath, `format-input-${process.pid}.stasis`),
  );
  fs.writeFileSync(
    formatUri.fsPath,
    "global value:i32;\nfunction sample():i32 {\nvalue += 1;\nreturn value;\n}\n",
  );
  const formatDocument = await vscode.workspace.openTextDocument(formatUri);

  const formatEdits = await vscode.commands.executeCommand<vscode.TextEdit[]>(
    "vscode.executeFormatDocumentProvider",
    formatUri,
    { tabSize: 4, insertSpaces: true },
  );
  assert.ok(formatEdits && formatEdits.length > 0, "the packaged formatter returns canonical edits");
  const formatted = applyTextEdits(formatDocument, formatEdits);
  assert.match(
    formatted,
    /global value: i32;/,
    "formatter output applies canonical type spacing",
  );
  assert.match(
    formatted,
    /function sample\(\): i32 \{\r?\n    value \+= 1;/,
    "formatter output applies canonical block newlines and indentation",
  );

  const sourceUri = vscode.Uri.file(path.join(folder.uri.fsPath, "src", "main.stasis"));
  const document = await vscode.workspace.openTextDocument(sourceUri);
  await vscode.window.showTextDocument(document);
  assert.equal(document.languageId, "stasis", "the fixture opens as a Stasis document");

  const validLength = document.getText().length;
  const invalidSuffix = "\nfunction lsp_diagnostic_probe(): i32 { while (true) { return 1; } }\n";
  const introduceDiagnostic = new vscode.WorkspaceEdit();
  introduceDiagnostic.insert(sourceUri, document.positionAt(validLength), invalidSuffix);
  assert.equal(
    await vscode.workspace.applyEdit(introduceDiagnostic),
    true,
    "VS Code applies an unsaved diagnostic probe",
  );
  try {
    await waitFor("LSP compiler diagnostic", () =>
      vscode.languages
        .getDiagnostics(sourceUri)
        .some((diagnostic) => diagnostic.source === "stasis" && diagnostic.message.includes("while")),
    );
  } catch (error) {
    const observed = vscode.languages.getDiagnostics(sourceUri).map((diagnostic) => ({
      message: diagnostic.message,
      source: diagnostic.source,
      severity: diagnostic.severity,
      range: diagnostic.range,
    }));
    throw new Error(`${String(error)} Observed diagnostics: ${JSON.stringify(observed)}`);
  }
  const compilerDiagnostic = vscode.languages
    .getDiagnostics(sourceUri)
    .find((diagnostic) => diagnostic.source === "stasis" && diagnostic.message.includes("while"));
  assert.equal(compilerDiagnostic?.severity, vscode.DiagnosticSeverity.Error);
  assert.ok(
    compilerDiagnostic && !compilerDiagnostic.range.isEmpty,
    "the compiler diagnostic has a source range",
  );

  const repairDiagnostic = new vscode.WorkspaceEdit();
  repairDiagnostic.delete(
    sourceUri,
    new vscode.Range(document.positionAt(validLength), document.positionAt(document.getText().length)),
  );
  assert.equal(
    await vscode.workspace.applyEdit(repairDiagnostic),
    true,
    "VS Code repairs the unsaved diagnostic probe",
  );
  await waitFor(
    "cleared LSP compiler diagnostic",
    () => vscode.languages.getDiagnostics(sourceUri).length === 0,
  );

  const tickLineNumber = document
    .getText()
    .split(/\r?\n/)
    .findIndex((line) => line.includes("function tick"));
  assert.notEqual(tickLineNumber, -1, "the fixture contains the tick function");
  const tickLine = document.lineAt(tickLineNumber);
  const completionPosition = new vscode.Position(
    tickLineNumber,
    tickLine.text.indexOf("tick") + "tick".length,
  );
  const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
    "vscode.executeCompletionItemProvider",
    sourceUri,
    completionPosition,
  );
  assert.ok(
    completions?.items.some((item) => item.label === "tick"),
    "standard LSP completion returns the fixture function",
  );

  const scoreUseOffset = document.getText().indexOf("score += 1");
  assert.notEqual(scoreUseOffset, -1, "the fixture contains a typed score use");
  const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
    "vscode.executeHoverProvider",
    sourceUri,
    document.positionAt(scoreUseOffset + 2),
  );
  const hoverText = hovers
    ?.flatMap((hover) => hover.contents)
    .map((content) => (typeof content === "string" ? content : content.value))
    .join("\n");
  assert.match(hoverText ?? "", /score: i32/, "standard LSP hover reports the global type");

  const signatureCall = "add_score(1, 2)";
  const signatureOffset = document.getText().indexOf(signatureCall);
  assert.notEqual(signatureOffset, -1, "the fixture contains a signature-help call");
  const signatureHelp = await vscode.commands.executeCommand<vscode.SignatureHelp>(
    "vscode.executeSignatureHelpProvider",
    sourceUri,
    document.positionAt(signatureOffset + "add_score(1, ".length),
    ",",
  );
  assert.equal(signatureHelp?.activeParameter, 1, "signature help selects the second parameter");
  assert.equal(
    signatureHelp?.signatures[0]?.label,
    "add_score(amount: i32, bonus: i32): i32",
    "signature help returns compiler-owned parameter names and types",
  );
  const signatureDocumentation = signatureHelp?.signatures[0]?.documentation;
  assert.match(
    typeof signatureDocumentation === "string"
      ? signatureDocumentation
      : signatureDocumentation?.value ?? "",
    /Adds two score components/,
    "signature help includes source documentation",
  );

  const mainLineNumber = document
    .getText()
    .split(/\r?\n/)
    .findIndex((line) => line.includes("tick();"));
  assert.notEqual(mainLineNumber, -1, "the fixture calls tick from main");
  const mainLine = document.lineAt(mainLineNumber);
  const callPosition = new vscode.Position(mainLineNumber, mainLine.text.indexOf("tick") + 1);
  const definitions = await vscode.commands.executeCommand<vscode.Location[]>(
    "vscode.executeDefinitionProvider",
    sourceUri,
    callPosition,
  );
  assert.equal(definitions?.length, 1, "Go to Definition resolves through compiler-owned spans");
  assert.equal(definitions?.[0]?.uri.fsPath, sourceUri.fsPath);
  assert.equal(definitions?.[0]?.range.start.line, tickLineNumber);

  const references = await vscode.commands.executeCommand<vscode.Location[]>(
    "vscode.executeReferenceProvider",
    sourceUri,
    completionPosition,
  );
  assert.ok(
    references && references.length >= 2,
    "Find All References includes the function declaration and call",
  );

  const speedLineNumber = document
    .getText()
    .split(/\r?\n/)
    .findIndex((line) => line.includes("state.enemies[0].speed"));
  assert.notEqual(speedLineNumber, -1, "the fixture uses an indexed struct field");
  const speedLine = document.lineAt(speedLineNumber);
  const speedPosition = new vscode.Position(speedLineNumber, speedLine.text.indexOf("speed") + 1);
  const fieldDefinitions = await vscode.commands.executeCommand<vscode.Location[]>(
    "vscode.executeDefinitionProvider",
    sourceUri,
    speedPosition,
  );
  assert.equal(fieldDefinitions?.length, 1, "Go to Definition resolves an indexed struct field");
  assert.equal(fieldDefinitions?.[0]?.range.start.line, 10);
  const fieldReferences = await vscode.commands.executeCommand<vscode.Location[]>(
    "vscode.executeReferenceProvider",
    sourceUri,
    speedPosition,
  );
  assert.ok(
    fieldReferences && fieldReferences.length >= 2,
    "Find All References includes the indexed field declaration and write",
  );

  await waitFor("Test Explorer discovery", () => api.testFiles().some((uri) => uri.endsWith("editor.test.stasis")));
  const testUri = api.testFiles().find((uri) => uri.endsWith("editor.test.stasis"));
  assert.ok(testUri, "Test Explorer discovers the packaged fixture test");
  const testResult = await api.runTestFile(testUri);
  const testEnvelope = JSON.parse(testResult.stdout) as { ok?: boolean; result?: { tests_passed?: number } };
  assert.equal(testEnvelope.ok, true, "Test Explorer executes the test through the Stasis CLI");
  assert.equal(testEnvelope.result?.tests_passed, 1, "the discovered fixture test passes");

  try {
    await api.start();
    await waitFor("running live session", () => api.state() === "running");
    await api.request("pause");
    await waitFor("paused live session", () => api.state() === "paused");

    const memberSource = document.getText();
    const memberPrefix = "state.enemies[0].";
    const memberOffset = memberSource.indexOf(memberPrefix);
    assert.notEqual(memberOffset, -1, "the fixture contains an indexed state receiver");
    const memberCompletions = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider",
      sourceUri,
      document.positionAt(memberOffset + memberPrefix.length),
    );
    const memberLabels = memberCompletions?.items.map((item) =>
      typeof item.label === "string" ? item.label : item.label.label,
    );
    assert.ok(
      memberLabels?.includes("state.enemies[0].hp"),
      "live compiler completion resolves a field through an indexed state path",
    );
    assert.ok(
      memberLabels?.includes("state.enemies[0].speed"),
      "live compiler completion returns sibling fields for the indexed receiver",
    );

    const before = inspectedI32(await api.request("inspect", { path: "score" }));
    await api.request("watch", { path: "score" });
    await api.request("step", { ticks: 1 });
    const after = inspectedI32(await api.request("inspect", { path: "score" }));
    assert.equal(after, before + 1, "single-step executes exactly one game tick");
    assert.ok(api.values().some((value) => value.path === "score"), "the Live Values model receives runtime state");

    const source = document.getText();
    const oldTick = "score += 1";
    const oldTickOffset = source.indexOf(oldTick);
    assert.notEqual(oldTickOffset, -1, "the fixture contains its original tick operation");
    const hotEdit = new vscode.WorkspaceEdit();
    hotEdit.replace(
      sourceUri,
      new vscode.Range(
        document.positionAt(oldTickOffset),
        document.positionAt(oldTickOffset + oldTick.length),
      ),
      "score += 3",
    );
    assert.equal(await vscode.workspace.applyEdit(hotEdit), true, "VS Code applies the live source edit");
    assert.equal(await document.save(), true, "VS Code saves the live source edit");

    let current = after;
    let hotSwapObserved = false;
    const hotSwapDeadline = Date.now() + 30_000;
    while (Date.now() < hotSwapDeadline) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      await api.request("step", { ticks: 1 });
      const next = inspectedI32(await api.request("inspect", { path: "score" }));
      const delta = next - current;
      assert.ok(delta === 1 || delta === 3, `tick delta remains old or newly swapped logic, received ${delta}`);
      current = next;
      if (delta === 3) {
        hotSwapObserved = true;
        break;
      }
    }
    assert.equal(hotSwapObserved, true, "saving in VS Code hot-swaps the running tick function");

    await api.request("resume");
    await waitFor("resumed live session", () => api.state() === "running");
    await waitFor(
      "runtime framebuffer capture",
      () => fs.existsSync(screenshot) && fs.statSync(screenshot).size > 100,
    );
    assertRenderedFrame(screenshot);
  } finally {
    await api.stop();
    await waitFor("stopped live session", () => api.state() === "stopped");
  }
}
