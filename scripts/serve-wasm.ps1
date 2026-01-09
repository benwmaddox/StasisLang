param(
  [string]$Root = (Join-Path $PSScriptRoot ".."),
  [int]$Port = 5173
)

$ErrorActionPreference = "Stop"

$rootPath = Resolve-Path $Root

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
  throw "node not found in PATH"
}

Write-Host "Serving $rootPath on http://127.0.0.1:$Port/"

node -e @"
const http = require('http');
const fs = require('fs');
const path = require('path');

const root = process.argv[1];
const port = Number(process.argv[2]);
const rootResolved = path.resolve(root);

function contentType(p) {
  if (p.endsWith('.html')) return 'text/html; charset=utf-8';
  if (p.endsWith('.js')) return 'text/javascript; charset=utf-8';
  if (p.endsWith('.wasm')) return 'application/wasm';
  if (p.endsWith('.svg')) return 'image/svg+xml';
  if (p.endsWith('.json')) return 'application/json; charset=utf-8';
  if (p.endsWith('.css')) return 'text/css; charset=utf-8';
  return 'application/octet-stream';
}

const server = http.createServer((req, res) => {
  let urlPath = req.url.split('?')[0];
  try { urlPath = decodeURIComponent(urlPath); } catch { /* ignore */ }
  if (urlPath.endsWith('/')) urlPath += 'index.html';

  const fsPath = path.resolve(path.join(rootResolved, '.' + urlPath));
  if (!fsPath.startsWith(rootResolved)) {
    res.writeHead(403); res.end('forbidden'); return;
  }
  fs.readFile(fsPath, (err, data) => {
    if (err) {
      res.writeHead(404); res.end('not found'); return;
    }
    res.writeHead(200, { 'Content-Type': contentType(fsPath) });
    res.end(data);
  });
});

server.listen(port, '127.0.0.1', () => {
  console.log('listening on http://127.0.0.1:' + port + '/');
});
"@ $rootPath $Port
