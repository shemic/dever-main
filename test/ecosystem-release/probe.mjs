import { createHash } from 'node:crypto';
import { gzipSync, gunzipSync } from 'node:zlib';
import assert from 'node:assert/strict';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const addon = createRequire(import.meta.url)('bufferutil/build/Release/bufferutil.node');

export async function probe() {
  const bundle = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  assert.equal(process.execPath, resolve(bundle, 'runtime/bin/node'));
  const message = Buffer.from('独立标准库');
  const mask = Buffer.from([1, 2, 3, 4]);
  const masked = Buffer.alloc(message.length);
  addon.mask(message, mask, masked, 0, message.length);
  assert.notDeepEqual(masked, message);
  addon.unmask(masked, mask);
  assert.deepEqual(masked, message);
  assert.deepEqual(gunzipSync(gzipSync(message)), message);
  assert.equal(createHash('sha256').update(message).digest().length, 32);
  assert.equal(9007199254740993n + 1n, 9007199254740994n);
  await Promise.resolve();
  return { okay: true };
}
