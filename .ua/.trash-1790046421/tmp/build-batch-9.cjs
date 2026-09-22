#!/usr/bin/env node
const fs = require('fs');
const path = require('path');

const UA_DIR = '/Users/dean/Documents/git/virtuoso-cli/.ua';
const PROJECT = '/Users/dean/Documents/git/virtuoso-cli';
const extract = JSON.parse(fs.readFileSync(path.join(UA_DIR, 'tmp/ua-file-extract-results-9.json'), 'utf8'));
const dispatch = JSON.parse(fs.readFileSync(path.join(UA_DIR, 'intermediate/dispatch-input.json'), 'utf8'));
const batch = dispatch.find(b => b.batchIndex === 9);
const importData = batch.batchImportData;

const files = extract.results;

// Significance threshold (body line count)
const MIN_FN_LINES = 10;       // standalone free functions
const MIN_CLS_LINES = 12;      // structs/enums (need to carry weight)

const nodes = [];
const edges = [];

function pushNode(n) { nodes.push(n); }

// ─── File-level nodes ──────────────────────────────────────────────────────
const fileComplexity = (f) => {
  const lines = f.totalLines || 0;
  const fnCount = (f.functions || []).length + (f.classes || []).length;
  if (lines > 1000 || fnCount > 15) return 'complex';
  if (lines > 300 || fnCount > 5) return 'moderate';
  return 'simple';
};

const fileSummaries = {
  'src/transport/ipc/daemon.rs': 'Synchronous `NativeTransportClient` (Unix-only) that talks the versioned IPC protocol to the transport daemon. Implements `RemoteTransport` over a length-prefixed JSON frame channel on a Unix domain socket: performs the `Hello` handshake (profile, auth token, daemon nonce), then offers test_connection / run_command / upload_file|upload_text / download_file|download_dir / challenge / request_shutdown. Also owns the end-to-end integration `mod tests` that round-trips through the real server.',
  'src/transport/ipc/framing.rs': 'Transport-agnostic byte framing for the IPC protocol: four-byte big-endian length prefix + UTF-8 payload, with an 8 MiB hard ceiling (`MAX_FRAME_SIZE`). Exposes `encode_frame`, `FrameReader<R: Read>` (streaming, deadline-aware `read_frame_until`/`read_frame_until_with`), and `FrameWriter<W: Write>` (`write_frame_with_deadline`). `FrameError` covers TooLarge, Truncated, and Io variants. The layer never sees SSH, Tokio, or daemon-internal types.',
  'src/transport/ipc/messages.rs': 'Plain `serde` request/response vocabulary for the IPC protocol: `ProtocolVersion` (major/minor, current constants pulled from `framing`, major must match exactly), `Operation` enum (Hello, RunCommand, UploadFile, DownloadDir, Health, Shutdown, etc., with custom serde via `as_str`), `Hello`/`HelloAck`, `RequestEnvelope`/`ResponseEnvelope`/`ResponseResult`, and the `IpcError` taxonomy (codes, connection-level predicate, and From impls for serde_json / std::io errors).',
  'src/transport/ipc/mod.rs': 'Module root for `transport::ipc`. Declares `daemon`, `framing`, `messages`, and `server` submodules, with `daemon`/`server` gated on `(unix, any(test, feature = "native-ssh"))` because the real daemon is feature-gated but the test suite exercises the same code. The module owns only the wire format; no russh, Tokio, SSH, or daemon-internal types leak.',
  'src/transport/ipc/server.rs': 'Server half of the versioned IPC protocol (Unix-only). `serve_one` / `serve_one_with_shutdown` handle a single connection: `Hello` handshake, then dispatch every subsequent `RequestEnvelope` onto the supplied `RemoteTransport` (or an `EndpointPool` via `serve_one_pooled`/`run_with_pool`) until the peer closes. `dispatch` is the central request router (~130 lines, handles RunCommand, uploads, downloads, Health, Shutdown, Challenge, Cancel, local-forward). `run` binds the Unix socket, sets mode `0600`, and accepts forever. Implements the Tier-1 liveness `Challenge` probe by answering with the recorded daemon nonce.',
};

const fileTags = {
  'src/transport/ipc/daemon.rs': ['rust', 'ipc', 'client', 'unix-domain-socket', 'remote-transport', 'feature-gated', 'integration-test'],
  'src/transport/ipc/framing.rs': ['rust', 'ipc', 'framing', 'length-prefix', 'streaming', 'no-i/o-deps'],
  'src/transport/ipc/messages.rs': ['rust', 'ipc', 'serde', 'protocol-version', 'error-taxonomy', 'wire-types'],
  'src/transport/ipc/mod.rs': ['rust', 'ipc', 'module-root', 'cfg-gate'],
  'src/transport/ipc/server.rs': ['rust', 'ipc', 'server', 'dispatch', 'connection-pool', 'shutdown-handling', 'feature-gated', 'unix-only'],
};

for (const f of files) {
  pushNode({
    id: `file:${f.path}`,
    type: 'file',
    name: path.basename(f.path),
    filePath: f.path,
    summary: fileSummaries[f.path] || '',
    tags: fileTags[f.path] || ['rust'],
    complexity: fileComplexity(f),
    languageNotes: 'Rust, edition per workspace Cargo.toml. Module gated on `(unix, any(test, feature = "native-ssh"))` where applicable; serde for JSON wire types; std-only framing layer (no tokio, no russh at the IPC layer).',
  });
}

// ─── Function/class nodes ──────────────────────────────────────────────────
function fnId(filePath, name) { return `function:${filePath}::${name}`; }
function clsId(filePath, name) { return `class:${filePath}::${name}`; }

for (const f of files) {
  if (!f.functions && !f.classes) continue;

  for (const fn of f.functions || []) {
    const lines = (fn.endLine || 0) - (fn.startLine || 0) + 1;
    if (lines < MIN_FN_LINES) continue; // skip trivial helpers
    pushNode({
      id: fnId(f.path, fn.name),
      type: 'function',
      name: fn.name,
      filePath: f.path,
      summary: `${fn.name}(${(fn.params || []).join(', ')}) — ${lines} lines.`,
      tags: ['rust', 'function'],
      complexity: lines > 50 ? 'complex' : lines > 20 ? 'moderate' : 'simple',
      languageNotes: 'Free function or inherent impl method. Body size measured by extractor.',
    });
    edges.push({ source: `file:${f.path}`, target: fnId(f.path, fn.name), type: 'contains', weight: 1.0 });
  }

  for (const c of f.classes || []) {
    const lines = (c.endLine || 0) - (c.startLine || 0) + 1;
    if (lines < MIN_CLS_LINES) continue; // skip tiny data structs
    pushNode({
      id: clsId(f.path, c.name),
      type: 'class',
      name: c.name,
      filePath: f.path,
      summary: `${c.name} — ${lines}-line definition with ${(c.methods || []).length} method(s): ${(c.methods || []).join(', ') || 'none'}.`,
      tags: ['rust', c.methods && c.methods.length ? 'impl-block' : 'data-struct'],
      complexity: lines > 50 ? 'complex' : lines > 20 ? 'moderate' : 'simple',
      languageNotes: 'Rust struct/enum definition (impl block methods tracked separately).',
    });
    edges.push({ source: `file:${f.path}`, target: clsId(f.path, c.name), type: 'contains', weight: 1.0 });
  }
}

// ─── imports edges ────────────────────────────────────────────────────────
const inBatch = new Set(files.map(f => f.path));
for (const [filePath, deps] of Object.entries(importData)) {
  for (const dep of deps) {
    if (inBatch.has(dep)) {
      edges.push({ source: `file:${filePath}`, target: `file:${dep}`, type: 'imports', weight: 0.9 });
    }
  }
}

// ─── calls edges (function→function, in-batch only) ───────────────────────
// Build map of local function→id for both files in this batch
const localFn = new Map(); // "filePath::name" -> id
for (const f of files) {
  for (const fn of f.functions || []) localFn.set(`${f.path}::${fn.name}`, fnId(f.path, fn.name));
}

// Inspect callGraph entries: keep ones where caller and callee both resolve to in-batch functions
// (we only treat callees that are simple names matching localFn, to avoid std/russh noise).
const seenCall = new Set();
for (const f of files) {
  for (const call of f.callGraph || []) {
    const caller = call.caller;
    const rawCallee = call.callee || '';
    // strip leading `self.` / `Type::` / `module::` prefixes to get a short name
    const calleeShort = rawCallee.split(/[.:]/).pop();
    if (!calleeShort) continue;
    const callerId = localFn.get(`${f.path}::${caller}`);
    const calleeId = localFn.get(`${f.path}::${calleeShort}`);
    if (callerId && calleeId && callerId !== calleeId) {
      const key = callerId + '->' + calleeId;
      if (!seenCall.has(key)) {
        seenCall.add(key);
        edges.push({ source: callerId, target: calleeId, type: 'calls', weight: 0.6 });
      }
    }
  }
}

// ─── tested_by edges (inline #[cfg(test)] modules) ────────────────────────
// All four non-trivial files have an inline `mod tests { ... }` block (verified via grep).
for (const f of files) {
  if (f.path === 'src/transport/ipc/mod.rs') continue;
  // Inline tests are co-located in the same file
  edges.push({
    source: `file:${f.path}`,
    target: `file:${f.path}#tests`,
    type: 'tested_by',
    weight: 0.5,
  });
}

// ─── Write output ─────────────────────────────────────────────────────────
const out = {
  batchIndex: 9,
  projectRoot: PROJECT,
  nodes,
  edges,
};

fs.writeFileSync(path.join(UA_DIR, 'intermediate/batch-9.json'), JSON.stringify(out, null, 2));
console.log('wrote', path.join(UA_DIR, 'intermediate/batch-9.json'));
console.log('nodes:', nodes.length, 'edges:', edges.length);
console.log('file nodes:', nodes.filter(n => n.type === 'file').length);
console.log('function nodes:', nodes.filter(n => n.type === 'function').length);
console.log('class nodes:', nodes.filter(n => n.type === 'class').length);
console.log('edge types:');
const et = {};
edges.forEach(e => { et[e.type] = (et[e.type] || 0) + 1; });
console.log(et);