import { BusinessError, Worker, main } from './dever_component.mjs';

function setting(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).join() !== 'prefix' || typeof value.prefix !== 'string') throw new Error('invalid setting');
  return value.prefix;
}

function payload(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).sort().join() !== 'delay_ms,text' || typeof value.text !== 'string' || !Number.isInteger(value.delay_ms) || value.delay_ms < 0 || value.delay_ms > 10000) throw new Error('invalid payload');
  return value;
}

function number(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).join() !== 'number' || (typeof value.number !== 'bigint' && !Number.isSafeInteger(value.number))) throw new Error('invalid number');
  return value.number;
}

const worker = new Worker({ port: 'example.Text', schema: 'fixture-schema-v1', adapter: 'example.TextAdapter', operations: ['text.render', 'text.fail', 'number.echo', 'text.stubborn'], errors: ['example.rejected'] }, setting);
worker.register('text.render', payload, async (value, prefix, signal) => {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, value.delay_ms);
    signal.addEventListener('abort', () => { clearTimeout(timer); reject(new Error('cancelled')); }, { once: true });
  });
  return { value: prefix + value.text };
});
worker.register('text.fail', payload, async () => { throw new BusinessError('example.rejected', { reason: 'rejected' }); });
worker.register('number.echo', number, async value => ({ number: value }));
worker.register('text.stubborn', payload, async () => new Promise(() => {}));
await main(worker);
