import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import type { LiveResponse, LiveValue } from "../protocol";
import type { LiveSessionState } from "../liveSession";

interface StasisExtensionApi {
  state(): LiveSessionState;
  values(): readonly LiveValue[];
  start(): Promise<void>;
  stop(): Promise<void>;
  request(type: string, fields?: Record<string, unknown>): Promise<LiveResponse>;
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

  const formatUri = vscode.Uri.file(path.join(folder.uri.fsPath, "format-input.stasis"));
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
    "compiler-backed completion returns the fixture function",
  );

  try {
    await api.start();
    await waitFor("running live session", () => api.state() === "running");
    await api.request("pause");
    await waitFor("paused live session", () => api.state() === "paused");

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
    await waitFor("runtime framebuffer capture", () => fs.existsSync(screenshot));
    assert.ok(fs.statSync(screenshot).size > 100, "the graphical play process produced a PNG framebuffer");
  } finally {
    await api.stop();
    await waitFor("stopped live session", () => api.state() === "stopped");
  }
}
