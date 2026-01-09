param(
  [string]$Root = ".",
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
  if (urlPath === '/') urlPath = '/index.html';
  const fsPath = path.join(root, urlPath);
  if (!fsPath.startsWith(root)) {
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
