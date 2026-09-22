const fs = require('fs');

const results = JSON.parse(fs.readFileSync('/Users/dean/Documents/git/virtuoso-cli/.ua/tmp/ua-file-extract-results-6.json', 'utf8'));
const dispatch = JSON.parse(fs.readFileSync('/Users/dean/Documents/git/virtuoso-cli/.ua/intermediate/dispatch-input.json', 'utf8'));
const batch = dispatch.find(b => b.batchIndex === 6);
const projectRoot = '/Users/dean/Documents/git/virtuoso-cli';

// ---- Helpers ----
function classify(file) {
  const loc = file.totalLines || 0;
  const fns = (file.functions || []).length;
  const cls = (file.classes || []).length;
  if (loc > 1000 || fns > 25 || cls > 8) return 'complex';
  if (loc > 200 || fns >= 7 || cls >= 3) return 'moderate';
  return 'simple';
}

function fileSummary(file) {
  const exports = (file.exports || []).map(e => e.name).filter(n => n);
  const fnCount = (file.functions || []).length;
  const clsCount = (file.classes || []).length;
  const parts = [];
  parts.push(`Rust module at ${file.path}`);
  parts.push(`${file.totalLines} LOC`);
  if (clsCount) parts.push(`${clsCount} types`);
  if (fnCount) parts.push(`${fnCount} functions`);
  if (exports.length) parts.push(`exports ${exports.slice(0,8).join(', ')}${exports.length>8?'…':''}`);
  return parts.join('; ');
}

function fnSummary(fn, filePath) {
  const span = (fn.endLine - fn.startLine + 1);
  const params = (fn.params || []).join(', ');
  return `Function \`${fn.name}(${params})\` (lines ${fn.startLine}-${fn.endLine}, ${span} LOC) in ${filePath}.`;
}

function classSummary(cls, filePath) {
  const span = (cls.endLine - cls.startLine + 1);
  const props = (cls.properties || []).length;
  const methods = (cls.methods || []).length;
  return `Type \`${cls.name}\` (lines ${cls.startLine}-${cls.endLine}, ${span} LOC) in ${filePath} — ${props} field(s), ${methods} inherent method(s).`;
}

// ---- Significance filter ----
const FN_MIN_LINES = 8;       // skip tiny helpers
const FN_KEEP_NAMES = new Set(['main', 'new', 'from_config', 'from_env', 'from_str', 'from_now',
  'parse', 'validate', 'classify', 'run_simulation', 'run_parallel', 'run_async',
  'run_command', 'upload_file', 'download_file', 'download_dir', 'dismiss',
  'action_x11', 'action_x11_batch', 'dismiss_window', 'list_dialogs', 'list_windows',
  'attach', 'detach', 'start', 'stop', 'select', 'check_license', 'challenge_via_ipc',
  'shutdown_via_ipc', 'pick_live_session', 'remote_port_alive', 'pick_port', 'spawn_runner',
  'resolve_unique_window', 'validate_action_params', 'build_xdotool_actions',
  'wait_for_window_pattern', 'from_known_hosts_token', 'as_known_hosts_token', 'matches',
  'ensure_helper_uploaded', 'resolve_effective_user', 'list_dialogs', 'action_x11_cached',
  'action_x11_batch', 'of', 'validate_directory_download', 'should_retry_cm_failure',
  'shell_quote', 'capture_stdout', 'check_server_key', 'load_known_hosts_or_empty',
  'verify_against_stores', 'from_config']);

function shouldEmitFn(fn, fileExports) {
  const span = (fn.endLine - fn.startLine + 1);
  if (span < FN_MIN_LINES && !FN_KEEP_NAMES.has(fn.name)) return false;
  if (fileExports.has(fn.name)) return true;
  if (span >= 15) return true;
  if (FN_KEEP_NAMES.has(fn.name) && span >= 4) return true;
  return false;
}

const CLASS_MIN_PROPS = 1;
const CLASS_MIN_METHODS = 1;
function shouldEmitClass(cls) {
  const props = (cls.properties || []).length;
  const methods = (cls.methods || []).length;
  // Always keep enums/structs with at least a few fields or any methods
  if (methods >= 1) return true;
  if (props >= 2) return true;
  if ((cls.endLine - cls.startLine + 1) >= 10) return true;
  return false;
}

// ---- Build nodes ----
const nodes = [];
const edges = [];
const fileIdByPath = new Map();
const fnIdByKey = new Map(); // key = filePath::fnName
const classIdByKey = new Map();

results.results.forEach(file => {
  const fileId = `file:${file.path}`;
  fileIdByPath.set(file.path, fileId);

  const exportNames = new Set((file.exports || []).map(e => e.name));
  const complexity = classify(file);

  nodes.push({
    id: fileId,
    type: 'file',
    name: file.path.split('/').pop(),
    filePath: file.path,
    summary: fileSummary(file),
    tags: ['rust', 'module', file.path.split('/')[0] === 'src' ? 'library' : 'binary'],
    complexity,
    languageNotes: 'Rust module; uses `#![allow(dead_code)]` in several transport modules because the native-ssh wiring lands in later increments.'
  });

  // Functions
  (file.functions || []).forEach(fn => {
    if (!shouldEmitFn(fn, exportNames)) return;
    const key = `${file.path}::${fn.name}::${fn.startLine}`;
    const fnId = `function:${file.path}::${fn.name}`;
    fnIdByKey.set(key, fnId);
    // disambiguate when name appears multiple times
    let unique = fnId;
    let suffix = 2;
    while (nodes.find(n => n.id === unique)) {
      unique = `${fnId}#${suffix}`;
      suffix++;
    }
    if (unique !== fnId) fnIdByKey.set(key, unique);
    nodes.push({
      id: unique,
      type: 'function',
      name: fn.name,
      filePath: file.path,
      summary: fnSummary(fn, file.path),
      tags: ['rust', 'function', exportNames.has(fn.name) ? 'exported' : 'internal'],
      complexity: (fn.endLine - fn.startLine + 1) > 40 ? 'complex' : ((fn.endLine - fn.startLine + 1) > 15 ? 'moderate' : 'simple'),
      languageNotes: 'Rust free function or `impl` block method.'
    });
    edges.push({ source: fileId, target: unique, type: 'contains', weight: 0.5 });
  });

  // Classes / enums / structs
  (file.classes || []).forEach(cls => {
    if (!shouldEmitClass(cls)) return;
    const key = `${file.path}::${cls.name}`;
    let cId = `class:${file.path}::${cls.name}`;
    if (classIdByKey.has(key)) {
      let suffix = 2;
      while (nodes.find(n => n.id === `${cId}#${suffix}`)) suffix++;
      cId = `${cId}#${suffix}`;
    }
    classIdByKey.set(key, cId);
    nodes.push({
      id: cId,
      type: 'class',
      name: cls.name,
      filePath: file.path,
      summary: classSummary(cls, file.path),
      tags: ['rust', 'type', exportNames.has(cls.name) ? 'exported' : 'internal'],
      complexity: (cls.endLine - cls.startLine + 1) > 60 ? 'complex' : ((cls.endLine - cls.startLine + 1) > 20 ? 'moderate' : 'simple'),
      languageNotes: 'Rust `struct`, `enum`, or newtype — Rust has no classes but the extractor labels user-defined types uniformly.'
    });
    edges.push({ source: fileId, target: cId, type: 'contains', weight: 0.6 });
  });
});

// ---- Imports edges ----
Object.entries(batch.batchImportData).forEach(([src, targets]) => {
  const sourceId = fileIdByPath.get(src);
  if (!sourceId) return;
  (targets || []).forEach(t => {
    const targetId = fileIdByPath.get(t);
    if (targetId) {
      edges.push({ source: sourceId, target: targetId, type: 'imports', weight: 0.7 });
    }
  });
});

// ---- Calls edges ----
results.results.forEach(file => {
  const fileId = fileIdByPath.get(file.path);
  if (!fileId || !file.callGraph) return;
  // Build map of name -> list of fn ids in this file
  const fnsByName = new Map();
  (file.functions || []).forEach(fn => {
    if (!fnsByName.has(fn.name)) fnsByName.set(fn.name, []);
    fnsByName.get(fn.name).push(fn);
  });
  file.callGraph.forEach(call => {
    const caller = call.caller;
    const callee = call.callee;
    if (!caller || !callee) return;
    // Filter: callee must be a single token (no whitespace, no dots, no parens)
    if (callee.includes('\n') || callee.includes(' ') || callee.includes('.') || callee.includes('(') || callee.includes('<')) return false;
    if (callee.length < 2 || callee.length > 40) return;
    // Find caller function in this file
    const callerFns = fnsByName.get(caller);
    if (!callerFns) return;
    const callerFn = callerFns.find(f => call.lineNumber >= f.startLine && call.lineNumber <= f.endLine) || callerFns[0];
    const callerKey = `${file.path}::${callerFn.name}::${callerFn.startLine}`;
    const callerId = fnIdByKey.get(callerKey);
    if (!callerId) return;

    // Is callee a function in the same file?
    if (fnsByName.has(callee)) {
      const targetFn = fnsByName.get(callee)[0];
      const targetKey = `${file.path}::${targetFn.name}::${targetFn.startLine}`;
      const targetId = fnIdByKey.get(targetKey);
      if (targetId && targetId !== callerId) {
        edges.push({ source: callerId, target: targetId, type: 'calls', weight: 0.5 });
      }
    }
  });
});

// ---- tested_by edges (testutil.rs is the test fixture for transport/*) ----
const testutilId = fileIdByPath.get('src/transport/testutil.rs');
if (testutilId) {
  results.results.forEach(file => {
    if (file.path.startsWith('src/transport/') && file.path !== 'src/transport/testutil.rs' && file.path !== 'src/transport/mod.rs') {
      const fid = fileIdByPath.get(file.path);
      if (fid) edges.push({ source: fid, target: testutilId, type: 'tested_by', weight: 0.4 });
    }
  });
}

// ---- Dedupe edges ----
const seen = new Set();
const finalEdges = edges.filter(e => {
  const k = `${e.source}|${e.target}|${e.type}`;
  if (seen.has(k)) return false;
  seen.add(k);
  return true;
});

const out = {
  batchIndex: 6,
  projectRoot,
  nodes,
  edges: finalEdges
};

fs.writeFileSync('/Users/dean/Documents/git/virtuoso-cli/.ua/intermediate/batch-6.json', JSON.stringify(out, null, 2));
console.log('wrote batch-6.json');
console.log('nodes:', nodes.length);
console.log('edges:', finalEdges.length);
console.log('first 3 node ids:', nodes.slice(0,3).map(n => n.id));
console.log('first 3 edge src/tgt:', finalEdges.slice(0,3).map(e => `${e.source} -[${e.type}]-> ${e.target}`));