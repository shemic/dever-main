// Fixed registry-package lifecycle frontend. Runs only in the build sandbox.
const fs = require('node:fs');
const path = require('node:path');
const runScript = require('/worker/npm/frontend/node_modules/npm/node_modules/@npmcli/run-script');
const binLinks = require('/worker/npm/frontend/node_modules/npm/node_modules/bin-links');

async function main() {
  const request = JSON.parse(fs.readFileSync('/worker/request.json', 'utf8'));
  const root = '/data/build/output/install';
  fs.mkdirSync('/data/build/output');
  fs.cpSync('/worker/install', root, { recursive: true, dereference: false, errorOnExist: true });
  fs.mkdirSync('/data/build/tmp');
  fs.mkdirSync('/data/build/home');
  Object.assign(process.env, {
    PATH: '/worker/runtime/bin:/usr/bin',
    CC: '/usr/bin/cc', CXX: '/usr/bin/c++', AR: '/usr/bin/ar',
    PYTHON: request.python,
    npm_config_python: request.python,
    npm_config_nodedir: '/worker/npm',
    npm_config_node_gyp: '/worker/npm/frontend/node_modules/npm/node_modules/node-gyp/bin/node-gyp.js',
    npm_config_build_from_source: 'true',
    npm_config_cache: '/data/build/npm-cache',
    TMPDIR: '/data/build/tmp', SOURCE_DATE_EPOCH: '0',
    HOME: '/data/build/home',
  });
  const nodes = new Map(request.instances.map(node => [node.path, node]));
  for (const node of request.instances) {
    const directory = path.join(root, node.path);
    const pkg = JSON.parse(fs.readFileSync(path.join(directory, 'package.json'), 'utf8'));
    await binLinks({ path: directory, pkg, global: false, top: false, force: false });
  }
  const visited = new Set();
  const required = new Set(request.required);
  const failed = new Set();
  const nativeEntries = new Set();
  async function install(name) {
    if (visited.has(name)) return;
    visited.add(name);
    const node = nodes.get(name);
    try {
      for (const edge of node.edges) {
        if (edge.target && nodes.has(edge.target)) {
          await install(edge.target);
          if (failed.has(edge.target) && ['dependency', 'peer'].includes(edge.kind)) {
            throw new Error('required lifecycle dependency failed: ' + edge.target);
          }
        }
      }
      const directory = path.join(root, name);
      for (const event of ['preinstall', 'install', 'postinstall']) {
        await runScript({ event, path: directory, stdio: 'inherit', scriptShell: '/bin/sh' });
      }
      const native = path.join(directory, 'build', 'Release');
      if (fs.existsSync(native)) {
        for (const file of fs.readdirSync(native)) {
          if (file.endsWith('.node')) {
            const addon = path.join(native, file);
            require(addon);
            nativeEntries.add(path.relative(root, addon));
          }
        }
      }
    } catch (error) {
      if (required.has(name)) throw error;
      process.stderr.write('optional npm lifecycle failed: ' + name + ': ' + String(error.message || error) + '\n');
      failed.add(name);
    }
  }
  for (const node of request.instances) await install(node.path);
  let changed = true;
  while (changed) {
    changed = false;
    for (const node of request.instances) {
      if (!failed.has(node.path) && node.edges.some(edge => ['dependency', 'peer'].includes(edge.kind) && failed.has(edge.target))) {
        if (required.has(node.path)) throw new Error('required lifecycle dependency failed: ' + node.path);
        failed.add(node.path);
        changed = true;
      }
    }
  }
  const retained = new Set();
  function retain(name) {
    if (retained.has(name) || failed.has(name)) return;
    retained.add(name);
    for (const edge of nodes.get(name).edges) {
      if (edge.target) retain(edge.target);
    }
  }
  for (const node of request.roots) retain(node);
  for (const node of request.instances) {
    if (!retained.has(node.path)) fs.rmSync(path.join(root, node.path), { recursive: true, force: true });
  }
  // Bin links are a build-time PATH input. The immutable Worker tree stores
  // package files only; it never persists symlinks created by installer hooks.
  for (const node of request.instances) {
    fs.rmSync(path.join(root, node.path, 'node_modules', '.bin'), { recursive: true, force: true });
    fs.rmSync(path.join(root, node.path, 'build', 'node_gyp_bins'), { recursive: true, force: true });
  }
  fs.rmSync(path.join(root, 'node_modules', '.bin'), { recursive: true, force: true });
  fs.writeFileSync('/data/build/output/result.json', JSON.stringify({ failed: [...failed].sort(), native: [...nativeEntries].filter(file => retained.has(file.split('/build/Release/')[0])).sort() }));
}

main().catch(error => { process.stderr.write(String(error.stack || error) + '\n'); process.exitCode = 1; });
