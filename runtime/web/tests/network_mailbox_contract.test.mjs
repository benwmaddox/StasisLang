import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";

const source = fs.readFileSync(new URL("../game.js", import.meta.url), "utf8");

test("browser mailbox checkpoints only bounded semantic metadata", () => {
  assert.match(source, /stasis_web_network_checkpoint: networkCheckpoint/);
  assert.match(source, /networkClient\.desiredSeat = seat/);
  assert.match(source, /networkClient\.lastSequence = lastSequence/);
  assert.match(source, /lastSequence > 0x7fffffff/);
  assert.match(source, /seat < -1 \|\| seat >= 8/);
  assert.match(source, /JSON\.stringify\(\{ seat, lastSequence \}\)/);
  assert.match(source, /networkCheckpointKey\(networkResumeCredential\(\)\)/);
  // The credential may select an opaque storage namespace, but is not stored
  // as checkpoint JSON or returned through the Stasis import.
  assert.doesNotMatch(source, /JSON\.stringify\(\{[^}]*credential/);
});

test("browser network runtime keeps credential and pairing secret adapter-only", () => {
  assert.match(source, /new WebSocket\(socketUrl, \["stasis-v1", secret, protocol\]\)/);
  assert.match(source, /const socketUrl = `\$\{currentLocation\.protocol/);
  assert.match(source, /const networkPairingSecret = \(\) =>/);
  assert.match(source, /const networkResumeCredential = \(\) =>/);
  assert.match(source, /networkLoadCheckpoint\(\);/);
  assert.doesNotMatch(source, /networkClient\.queue\.push\([^)]*secret/);
});
