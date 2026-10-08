const TOKEN_KINDS = new Set([
  "keyword", "type", "function", "string", "number", "comment", "operator", "identifier", "punctuation",
]);

export function buildHighlightedFragment(source, tokens, document) {
  const text = String(source ?? "");
  const fragment = document.createDocumentFragment();
  let cursor = 0;
  const ordered = Array.isArray(tokens)
    ? tokens.filter(token => Number.isInteger(token?.start) && Number.isInteger(token?.end))
      .slice().sort((left, right) => left.start - right.start || right.end - left.end)
    : [];

  for (const token of ordered) {
    const start = Math.max(cursor, Math.min(text.length, token.start));
    const end = Math.max(start, Math.min(text.length, token.end));
    if (end <= start) continue;
    if (start > cursor) fragment.append(document.createTextNode(text.slice(cursor, start)));
    const span = document.createElement("span");
    const kind = TOKEN_KINDS.has(token.kind) ? token.kind : "identifier";
    span.className = `syntax-${kind}`;
    span.textContent = text.slice(start, end);
    fragment.append(span);
    cursor = end;
  }
  if (cursor < text.length) fragment.append(document.createTextNode(text.slice(cursor)));
  return fragment;
}

export function isCompletionContext(source, cursor, force = false) {
  if (force) return true;
  const text = String(source ?? "");
  if (!Number.isInteger(cursor) || cursor < 0 || cursor > text.length) return false;
  return /[A-Za-z0-9_.]$/.test(text.slice(0, cursor));
}

export function applyEditorCompletion(source, range, insertText) {
  const text = String(source ?? "");
  if (typeof insertText !== "string") throw new Error("completion insert text must be a string");
  const rawStart = Number.isInteger(range?.start) ? range.start : text.length;
  const rawEnd = Number.isInteger(range?.end) ? range.end : rawStart;
  const start = Math.max(0, Math.min(text.length, rawStart));
  const end = Math.max(start, Math.min(text.length, rawEnd));
  const result = `${text.slice(0, start)}${insertText}${text.slice(end)}`;
  const cursor = start + insertText.length;
  return { source: result, cursor, selectionStart: cursor, selectionEnd: cursor };
}

export function isCurrentEditorAnalysis(analysis, current) {
  return Boolean(analysis && current
    && analysis.revision === current.revision
    && analysis.path === current.path
    && analysis.source === current.source
    && analysis.cursor === current.cursor);
}

export function editorKeyAction(event, completionOpen) {
  const command = Boolean(event.ctrlKey || event.metaKey);
  if (command && event.key === "Enter") return "run";
  if (command && (event.key === " " || event.key === "Space" || event.code === "Space")) return "open";
  if (!completionOpen) return event.key === "Tab" ? "indent" : "none";
  if (event.key === "Escape") return "close";
  if (event.key === "ArrowDown") return "next";
  if (event.key === "ArrowUp") return "previous";
  if (event.key === "Enter" || event.key === "Tab") return "accept";
  return "none";
}
