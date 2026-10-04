import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync, gunzipSync } from 'node:zlib';

const require = createRequire(import.meta.url);
const isNumber = require('is-number');

export async function probe() {
  assert.equal(process.arch, 'arm64');
  const bundle = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  assert.equal(process.execPath, resolve(bundle, 'runtime/bin/node'));
  assert.equal(require('is-number/package.json').version, '7.0.0');
  assert.equal(isNumber('42'), true);
  assert.equal(isNumber('invalid'), false);
  const message = Buffer.from('ARM 独立依赖');
  assert.deepEqual(gunzipSync(gzipSync(message)), message);
  assert.equal(createHash('sha256').update(message).digest().length, 32);
  await Promise.resolve();
  return { okay: true };
}

export async function reject() {
  throw new Error('expected ARM Worker failure');
}
