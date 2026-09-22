import fs from 'fs';

const extractPath = '/Users/dean/Documents/git/virtuoso-cli/.ua/tmp/ua-file-extract-results-4.json';
const dispatchPath = '/Users/dean/Documents/git/virtuoso-cli/.ua/intermediate/dispatch-input.json';
const outPath = '/Users/dean/Documents/git/virtuoso-cli/.ua/intermediate/batch-4.json';

const ext = JSON.parse(fs.readFileSync(extractPath, 'utf8'));
const dispatch = JSON.parse(fs.readFileSync(dispatchPath, 'utf8'));
const batch = dispatch.find(b => b.batchIndex === 4);
const imports = batch.batchImportData || {};
const projectRoot = '/Users/dean/Documents/git/virtuoso-cli';

// === File summaries (English) ===
const summaries = {
  'src/client/skill_loading.rs': {
    summary: 'Helper that stages a local SKILL file on the remote host over the SSH transport before invocation. Copies the local file into a remote temp directory via tempfile + run_command, then executes it through CommandRequest::untimed and validates the response.',
    tags: ['client', 'skill', 'ssh', 'file-staging'],
    complexity: 'moderate',
    languageNotes: 'Single async function using a transport trait abstraction and tempfile; the remote call path is exercised by tests.',
  },
  'src/commands/config.rs': {
    summary: 'CLI subcommand for inspecting the resolved configuration. Implements `check`, an entries JSON dump, source-label resolution, per-entry status reporting, and a tabular renderer; consumed by the `config` subcommand of the vcli binary.',
    tags: ['cli', 'config', 'command', 'diagnostics'],
    complexity: 'moderate',
    languageNotes: 'Pure functions over ResolvedConfig and ConfigReport; output formatting lives in src/output.rs.',
  },
  'src/commands/design.rs': {
    summary: 'Device-sizing command providing MOSFET `size`, `explore`, and gm/Id lookup-table helpers. Builds lookup tables from raw arrays, finds the closest L for a target gm/Id, and interpolates current density; used during analog sizing flows.',
    tags: ['cli', 'analog', 'design', 'sizing', 'gmid'],
    complexity: 'moderate',
    languageNotes: 'Pure numeric helpers plus a CLI entrypoint; no transport calls.',
  },
  'src/commands/diag.rs': {
    summary: 'Diagnostics subcommands including `cdslck` (library lock inspector) and `batch_read_locks` (batched read of cdslck records). Includes shell-quoting, owner-record parsing, age formatting, and a parallel SSH fan-out path over the client bridge.',
    tags: ['cli', 'diagnostics', 'cdslck', 'locks', 'ssh'],
    complexity: 'complex',
    languageNotes: 'Combines batched remote command dispatch via the transport bridge with structured stdout parsing.',
  },
  'src/commands/session.rs': {
    summary: 'Largest CLI module: orchestrates session discovery, listing, current-selection, history, cleanup, and detailed show output. Handles local-direct vs remote-tunnel endpoint resolution, hostname/IP matching, version-skew detection, cross-user validation, and TCP reachability checks across the client bridge and transport tunnel.',
    tags: ['cli', 'session', 'discovery', 'transport', 'endpoint'],
    complexity: 'complex',
    languageNotes: 'Heavily uses async, transport bridge traits, and Model types; endpoint resolution has multiple fallback paths.',
  },
  'src/commands/sim.rs': {
    summary: 'Simulation command surface: setup, run, validate_measure_expr, measure, sweep, corner, results, create_netlist, async/parallel job dispatch, job_status/list/cancel, and license check. Calls the spectre jobs/runner and ocean/corner modules for netlist composition, measure-expression validation, and worker pool scheduling.',
    tags: ['cli', 'simulation', 'spectre', 'ocean', 'jobs'],
    complexity: 'complex',
    languageNotes: 'Module is a thin CLI wrapper around the spectre::* and ocean::corner APIs with extensive clap-derived argument plumbing.',
  },
  'src/commands/skill.rs': {
    summary: 'SKILL command surface for the vcli binary: exec, broadcast, load, eval, find, info, plus skill_finder directory discovery, remote skill_finder sync, and cache show. Wires SkillFinder paths from the local filesystem or remote skill_runtime through the client bridge.',
    tags: ['cli', 'skill', 'skill-finder', 'exec', 'cache'],
    complexity: 'complex',
    languageNotes: 'Combines local filesystem lookups with remote sync; relies on skill_finder::discover and skill_runtime transports.',
  },
  'src/commands/transport_daemon.rs': {
    summary: 'Subcommand entrypoints for the optional `virtuoso-daemon` binary (feature-gated). Provides `run`, `run_with`, and a transport-to-VirtuosoError converter. Backed by transport pool/scheduler/contract modules for Unix-socket daemon management.',
    tags: ['cli', 'transport', 'daemon', 'feature-gated'],
    complexity: 'moderate',
    languageNotes: 'Likely gated behind a Cargo feature; thin glue over transport::* internals.',
  },
  'src/commands/tunnel.rs': {
    summary: 'SSH tunnel lifecycle management: start, scoped session attach, attach candidate discovery, live session discovery, attach/stop/detach/restart, diagnose, status, backend diagnostics, run, JSON serialization, and PID cleanup helpers. Coordinates transport tunnel/session discovery modules with the client bridge.',
    tags: ['cli', 'tunnel', 'ssh', 'lifecycle', 'diagnostics'],
    complexity: 'complex',
    languageNotes: 'Largest non-session CLI module; mixes JSON serialization, PID file management, and SSH process control.',
  },
  'src/commands/window.rs': {
    summary: 'GUI window/dialog command surface: list, dismiss_dialog, x11 enumeration and dismissal, screenshot, action_x11/action_x11_batch execution, mode classification, Skill octal escape decoding, and JSON parsing helpers. Provides mode-annotation utilities consumed by the TUI.',
    tags: ['cli', 'window', 'x11', 'screenshot', 'dialog'],
    complexity: 'complex',
    languageNotes: 'Includes X11 integration (likely via xdotool/x11rb) and SKILL octal string decoding; window_mode enum classifies windows for downstream display.',
  },
  'src/config.rs': {
    summary: 'Core configuration module: defines ResolvedConfig, ConfigReport, ValueSource, and ConfigValue helpers. Implements env-with-profile resolution, target-based resolution, SSH target derivation, profile selection, digest hashing, build_report generation, and a large set of typed accessor/report methods. Central to the CLI’s config model.',
    tags: ['config', 'core', 'env-resolution', 'profile', 'ssh-target'],
    complexity: 'complex',
    languageNotes: 'Very large module with many tiny accessor functions; structure is data-oriented with explicit ValueSource tracking and a digest for change detection.',
  },
  'src/context.rs': {
    summary: 'Runtime context that bundles a ResolvedConfig with a target id and exposes ownership validators. Owns config_digest, target_id, and validates session/tunnel/endpoint ownership so commands can refuse actions targeting other users’ resources.',
    tags: ['context', 'config', 'ownership', 'runtime'],
    complexity: 'moderate',
    languageNotes: 'Pure struct with validation methods; the cross-user guard is critical for daemon security.',
  },
  'src/error.rs': {
    summary: 'Defines VirtuosoError, the project-wide error enum, plus CliError helpers. Provides exit_code, error_type, retryable, suggestion, diagnostic_context, and to_cli_error conversion. Implemented via thiserror for ergonomic Display/Error derives.',
    tags: ['error', 'core', 'thiserror', 'cli-error'],
    complexity: 'moderate',
    languageNotes: 'Central error type shared across crates; includes a retryable hint and human-readable suggestions for CLI display.',
  },
  'src/models.rs': {
    summary: 'Domain models and small helper modules: SkillEvalResult, RpcResponse<T>, VersionInfo, AppPaths (with cache_dir, sessions_dir, load/save), SessionRecord (list/list_remote/sync_from_remote/is_alive), SessionFile helpers, SshBackend, and per-profile config persistence.',
    tags: ['models', 'domain', 'session', 'rpc', 'persistence'],
    complexity: 'complex',
    languageNotes: 'Many small data types and accessors; SessionRecord::list_remote and sync_from_remote are the most consequential async methods.',
  },
  'src/output.rs': {
    summary: 'CLI output formatting helpers: resolve format choice, print JSON or human-readable text, print_json, and print_value. Acts as the sink layer between commands and stdout.',
    tags: ['output', 'formatting', 'cli', 'json'],
    complexity: 'simple',
    languageNotes: 'Tiny module with no external deps; relies on serde_json for pretty printing.',
  },
  'src/rpc/dispatcher.rs': {
    summary: 'RPC dispatcher routing JSON-RPC method names to specialized handlers: dispatch_schematic, dispatch_maestro, dispatch_window, dispatch_cell, dispatch_tx, dispatch_file, dispatch_util, dispatch_skill, and dispatch_sim. Includes Skill result parsing, octal-escape normalization, and JSON-field coercion helpers.',
    tags: ['rpc', 'dispatcher', 'json-rpc', 'skill', 'core'],
    complexity: 'complex',
    languageNotes: 'Largest function file by handler count; each dispatch_* fn is a match arm on a typed payload struct.',
  },
  'src/session/heartbeat.rs': {
    summary: 'Heartbeat watchdog: track active sessions, periodically mark stale ones, and clear stale entries. Used by the daemon/bridge to detect dead SSH sessions and trigger cleanup.',
    tags: ['session', 'heartbeat', 'watchdog', 'cleanup'],
    complexity: 'moderate',
    languageNotes: 'Background task driven by tokio timers; pure state transitions plus async I/O for stale checks.',
  },
  'src/session/mod.rs': {
    summary: 'Module declaration file for the `session` submodule. Re-exports heartbeat and any sibling modules (currently only heartbeat) and provides no behavior of its own.',
    tags: ['module-root', 'session'],
    complexity: 'simple',
    languageNotes: 'Trivial mod file with only `pub mod heartbeat;` and re-exports.',
  },
  'src/spectre/parsers.rs': {
    summary: 'Spectre PSF output parsers: parse_psf_ascii, parse_psf_dir, parse_psf_signal_file, parse_sweep_psf_directory, strict variants, sweep-flat parser, structured op-block parser, scalar/vector/frequency accessors, and exact-result-file discovery. Implements both lenient and strict PSF formats used by the sim command’s results phase.',
    tags: ['spectre', 'psf', 'parser', 'simulation', 'sweep'],
    complexity: 'complex',
    languageNotes: 'Large parser module with many small functions; strict vs lenient parsers coexist, and op-block parsing is recursive (find_closing_paren).',
  },
};

// Significance filter: only emit function:/class: nodes for items with substantial body.
// For Rust, we treat functions spanning ≥ ~20 lines (or notable handlers) as substantial.
function isSubstantialFunction(name, startLine, endLine) {
  const len = endLine - startLine + 1;
  if (len >= 20) return true;
  return false;
}

// class: nodes are detected via classCount metric; we have no per-class locations, so skip detailed class nodes here
// (the analyzer only reported class counts). We'll note classes in the file-level summary.

const nodes = [];
const edges = [];

// === File nodes ===
for (const fileResult of ext.results) {
  const p = fileResult.path;
  const meta = summaries[p];
  if (!meta) {
    // fallback minimal entry (should not occur)
    nodes.push({
      id: 'file:' + p,
      type: 'file',
      name: p.split('/').pop(),
      filePath: p,
      summary: 'Rust source file.',
      tags: ['rust'],
      complexity: 'simple',
      languageNotes: '',
    });
    continue;
  }
  nodes.push({
    id: 'file:' + p,
    type: 'file',
    name: p.split('/').pop(),
    filePath: p,
    summary: meta.summary,
    tags: meta.tags,
    complexity: meta.complexity,
    languageNotes: meta.languageNotes,
  });
}

// === Function nodes (significant only) ===
const fileIdFor = (p) => 'file:' + p;

for (const fileResult of ext.results) {
  const p = fileResult.path;
  for (const fn of fileResult.functions || []) {
    if (!isSubstantialFunction(fn.name, fn.startLine, fn.endLine)) continue;
    const fnId = 'function:' + p + ':' + fn.name + '@' + fn.startLine;
    const params = (fn.params || []).filter(x => x && !x.includes('\n') && !x.includes('.'));
    nodes.push({
      id: fnId,
      type: 'function',
      name: fn.name,
      filePath: p,
      summary: `Function ${fn.name}(${params.join(', ')}) in ${p} (lines ${fn.startLine}-${fn.endLine}).`,
      tags: ['rust-function'],
      complexity: (fn.endLine - fn.startLine + 1) > 80 ? 'complex' : 'moderate',
      languageNotes: '',
    });
    // contains edge: file -> function
    edges.push({
      source: fileIdFor(p),
      target: fnId,
      type: 'contains',
      weight: 0.5,
    });
  }
}

// === imports edges (from batchImportData) ===
// Weight ~0.7 for module-level imports.
for (const src of Object.keys(imports)) {
  const srcId = 'file:' + src;
  for (const tgt of imports[src]) {
    const tgtId = 'file:' + tgt;
    edges.push({
      source: srcId,
      target: tgtId,
      type: 'imports',
      weight: 0.7,
    });
  }
}

// === tested_by edges (heuristic): scan tests/ directory for any file referencing our paths ===
// Find test files that mention the source paths (case-insensitive substring match).
let testedByEdges = 0;
try {
  const { execSync } = await import('node:child_process');
  // list test files in the repo (Rust convention: tests/ dir and #[cfg(test)] inline)
  const testList = execSync("find /Users/dean/Documents/git/virtuoso-cli/tests -type f -name '*.rs' 2>/dev/null", { encoding: 'utf8' })
    .split('\n').filter(Boolean);
  for (const tfile of testList) {
    let body = '';
    try { body = fs.readFileSync(tfile, 'utf8'); } catch (_) { continue; }
    for (const src of Object.keys(imports)) {
      const basename = src.split('/').pop(); // e.g. config.rs
      if (basename && body.includes(basename)) {
        edges.push({
          source: 'file:' + src,
          target: 'file:' + tfile.replace('/Users/dean/Documents/git/virtuoso-cli/', ''),
          type: 'tested_by',
          weight: 0.4,
        });
        testedByEdges++;
      }
    }
  }
} catch (e) {
  // tests/ dir may not exist; skip silently
}

// === Calls edges (within same file, based on extractor callGraph) ===
// Heuristic: caller is a known function name in the file; callee is a function-like token.
function edgesForCalls(fileResult) {
  const p = fileResult.path;
  const localFns = new Map();
  for (const fn of fileResult.functions || []) {
    localFns.set(fn.name, 'function:' + p + ':' + fn.name + '@' + fn.startLine);
  }
  const seen = new Set();
  for (const call of fileResult.callGraph || []) {
    const callerId = localFns.get(call.caller);
    if (!callerId) continue;
    // callee is often noisy; only keep clean identifiers (no \n, no quotes, single token)
    let callee = (call.callee || '').trim();
    if (callee.includes('\n') || callee.length > 60) continue;
    if (callee.startsWith('"') || callee.includes('::')) continue; // skip string literals and crate paths
    // take first identifier
    const m = callee.match(/^([A-Za-z_][A-Za-z0-9_]*)/);
    if (!m) continue;
    const calleeName = m[1];
    const calleeId = localFns.get(calleeName);
    if (!calleeId || calleeId === callerId) continue;
    const key = callerId + '->' + calleeId;
    if (seen.has(key)) continue;
    seen.add(key);
    edges.push({
      source: callerId,
      target: calleeId,
      type: 'calls',
      weight: 0.5,
    });
  }
}
for (const fr of ext.results) edgesForCalls(fr);

const out = {
  batchIndex: 4,
  projectRoot,
  nodes,
  edges,
};

fs.writeFileSync(outPath, JSON.stringify(out, null, 2));
console.log('nodes:', nodes.length);
console.log('edges:', edges.length);
console.log('tested_by edges added:', testedByEdges);
console.log('written to:', outPath);
