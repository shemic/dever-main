const VERSION = 'dever-component-1';
const MAX_BYTES = 16 * 1024 * 1024;
const MAX_DEPTH = 64;
const MAX_ELEMENTS = 65536;
const CANCEL_GRACE_MS = 1000;
const decoder = new TextDecoder('utf-8', { fatal: true });

export class ProtocolError extends Error {}

export class BusinessError extends Error {
  constructor(identity, payload = null) {
    super(identity);
    this.identity = identity;
    this.payload = payload;
  }
}

// JSON.parse accepts duplicate object keys; this parser enforces the runtime's
// duplicate, depth and value budgets before returning ordinary JS values.
function parseJSON(text) {
  let offset = 0;
  let remaining = MAX_ELEMENTS;
  const fail = () => { throw new ProtocolError('invalid component JSON'); };
  const space = () => { while (offset < text.length && /[ \t\r\n]/.test(text[offset])) offset++; };
  function string() {
    const start = offset++;
    while (offset < text.length) {
      const char = text[offset++];
      if (char === '"') return JSON.parse(text.slice(start, offset));
      if (char === '\\') offset++;
      else if (char.charCodeAt(0) < 32) fail();
    }
    fail();
  }
  function value(depth) {
    if (depth > MAX_DEPTH || --remaining < 0) throw new ProtocolError('wire JSON exceeds limit');
    space();
    const char = text[offset];
    if (char === '"') return string();
    if (char === '[' || char === '{') {
      if (depth === MAX_DEPTH) throw new ProtocolError('wire JSON exceeds depth limit');
      offset++;
      const array = char === '[';
      const result = array ? [] : {};
      const seen = new Set();
      space();
      if (text[offset] === (array ? ']' : '}')) { offset++; return result; }
      for (;;) {
        if (array) result.push(value(depth + 1));
        else {
          if (text[offset] !== '"') fail();
          const key = string();
          if (seen.has(key)) throw new ProtocolError('duplicate JSON field');
          seen.add(key);
          space();
          if (text[offset++] !== ':') fail();
          Object.defineProperty(result, key, { value: value(depth + 1), enumerable: true, writable: true, configurable: true });
        }
        space();
        const end = array ? ']' : '}';
        if (text[offset] === end) { offset++; return result; }
        if (text[offset++] !== ',') fail();
        space();
      }
    }
    const match = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(text.slice(offset));
    if (!match) fail();
    offset += match[0].length;
    if (match[0] === 'true' || match[0] === 'false' || match[0] === 'null') return JSON.parse(match[0]);
    if (match[0] === '-0') return -0;
    if (/^-?(?:0|[1-9]\d*)$/.test(match[0])) {
      const integer = BigInt(match[0]);
      return integer <= BigInt(Number.MAX_SAFE_INTEGER) && integer >= BigInt(Number.MIN_SAFE_INTEGER) ? Number(integer) : integer;
    }
    const number = JSON.parse(match[0]);
    if (!Number.isFinite(number)) throw new ProtocolError('invalid JSON number');
    return number;
  }
  const result = value(0);
  space();
  if (offset !== text.length || result === null || Array.isArray(result) || typeof result !== 'object') fail();
  return result;
}

function encodeJSON(value) {
  let remaining = MAX_ELEMENTS;
  function encode(current, depth) {
    if (depth > MAX_DEPTH || --remaining < 0) throw new ProtocolError('wire JSON exceeds limit');
    if (current === null || typeof current === 'boolean') return String(current);
    if (typeof current === 'string') return JSON.stringify(current);
    if (typeof current === 'bigint') return current.toString();
    if (typeof current === 'number') {
      if (!Number.isFinite(current)) throw new ProtocolError('invalid JSON number');
      return Object.is(current, -0) ? '-0' : String(current);
    }
    if (depth === MAX_DEPTH) throw new ProtocolError('wire JSON exceeds depth limit');
    if (Array.isArray(current)) return `[${current.map(item => encode(item, depth + 1)).join(',')}]`;
    if (current && typeof current === 'object') {
      return `{${Object.keys(current).map(key => `${JSON.stringify(key)}:${encode(current[key], depth + 1)}`).join(',')}}`;
    }
    throw new ProtocolError('invalid handler response');
  }
  return encode(value, 0);
}

function exact(message, names) {
  const actual = Object.keys(message);
  if (actual.length !== names.length || names.some(name => !Object.hasOwn(message, name))) {
    throw new ProtocolError('component message has missing or extra fields');
  }
}

function requestID(value) {
  if (!Number.isSafeInteger(value) || value < 0) throw new ProtocolError('invalid request id');
  return value;
}

function names(values) {
  return Array.isArray(values) && values.every(value => typeof value === 'string' && value.length) && new Set(values).size === values.length;
}

function typed(value, schema) {
  switch (schema.type) {
    case 'nullable': return value === null ? null : typed(value, schema.value);
    case 'list':
      if (!Array.isArray(value)) throw new ProtocolError('expected list');
      return value.map(item => typed(item, schema.value));
    case 'record': {
      if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).some(name => !Object.hasOwn(schema.fields, name))) throw new ProtocolError('invalid record fields');
      const result = {};
      for (const [name, field] of Object.entries(schema.fields)) {
        if (!Object.hasOwn(value, name) && field.type !== 'nullable') throw new ProtocolError('missing required record field');
        Object.defineProperty(result, name, { value: Object.hasOwn(value, name) ? typed(value[name], field) : null, enumerable: true });
      }
      return result;
    }
    case 'int64': case 'duration': case 'model_id': {
      if ((typeof value !== 'bigint' && !Number.isSafeInteger(value)) || BigInt(value) < -(1n << 63n) || BigInt(value) >= (1n << 63n)) throw new ProtocolError('expected signed 64-bit integer');
      return value;
    }
    case 'float64':
      if (typeof value !== 'number' || !Number.isFinite(value)) throw new ProtocolError('expected finite float');
      return value;
    case 'bool':
      if (typeof value !== 'boolean') throw new ProtocolError('expected boolean');
      return value;
    case 'json': return value;
    case 'text': case 'id': case 'secret': case 'decimal': case 'uuid': case 'datetime': case 'date': case 'time':
      if (typeof value !== 'string') throw new ProtocolError('expected text');
      if (schema.type === 'decimal' && !/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(value)) throw new ProtocolError('invalid decimal');
      if (schema.type === 'uuid' && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)) throw new ProtocolError('invalid UUID');
      if (schema.type === 'date' && (!/^\d{4}-\d{2}-\d{2}$/.test(value) || Number.isNaN(Date.parse(`${value}T00:00:00Z`)))) throw new ProtocolError('invalid Date');
      if (schema.type === 'time' && !/^\d{2}:\d{2}:\d{2}(?:\.\d{3})?$/.test(value)) throw new ProtocolError('invalid Time');
      if (schema.type === 'datetime' && (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?(?:Z|[+-]\d{2}:\d{2})$/.test(value) || Number.isNaN(Date.parse(value)))) throw new ProtocolError('invalid DateTime');
      return value;
    default: throw new ProtocolError('unknown worker type');
  }
}

async function stopHandler(current) {
  current.controller.abort();
  let timeout;
  try {
    await Promise.race([
      current.settled,
      new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new ProtocolError('component handler did not stop after cancellation')), CANCEL_GRACE_MS);
      }),
    ]);
  } finally {
    clearTimeout(timeout);
  }
}

class Frames {
  constructor() {
    this.buffer = Buffer.alloc(0);
    this.queue = [];
    this.queuedBytes = 0;
    this.waiter = null;
    this.failed = null;
    process.stdin.on('data', chunk => this.receive(chunk));
    process.stdin.on('end', () => this.fail(new ProtocolError('incomplete component frame')));
    process.stdin.on('error', error => this.fail(error));
  }

  fail(error) {
    this.failed = error;
    if (this.waiter) { this.waiter.reject(error); this.waiter = null; }
  }

  receive(chunk) {
    if (this.failed) return;
    this.buffer = Buffer.concat([this.buffer, chunk]);
    try {
      while (this.buffer.length >= 4) {
        const size = this.buffer.readUInt32BE(0);
        if (!size || size > MAX_BYTES) throw new ProtocolError('component frame exceeds byte limit');
        if (this.buffer.length < size + 4) break;
        const message = parseJSON(decoder.decode(this.buffer.subarray(4, size + 4)));
        this.buffer = this.buffer.subarray(size + 4);
        if (this.waiter) { this.waiter.resolve(message); this.waiter = null; }
        else {
          if (this.queue.length === 4 || this.queuedBytes + size > MAX_BYTES) throw new ProtocolError('component input queue exceeds limit');
          this.queue.push({ message, size });
          this.queuedBytes += size;
        }
      }
    } catch (error) { this.fail(error); }
  }

  next() {
    if (this.failed) return Promise.reject(this.failed);
    if (this.queue.length) {
      const next = this.queue.shift();
      this.queuedBytes -= next.size;
      return Promise.resolve(next.message);
    }
    return new Promise((resolve, reject) => { this.waiter = { resolve, reject }; });
  }

  async send(message) {
    const body = Buffer.from(encodeJSON(message));
    if (!body.length || body.length > MAX_BYTES) throw new ProtocolError('component frame exceeds byte limit');
    parseJSON(decoder.decode(body));
    const frame = Buffer.allocUnsafe(body.length + 4);
    frame.writeUInt32BE(body.length);
    body.copy(frame, 4);
    await new Promise((resolve, reject) => process.stdout.write(frame, error => error ? reject(error) : resolve()));
  }
}

export class Worker {
  constructor(contract, decodeSetting) {
    if (!contract || ![contract.port, contract.schema, contract.adapter].every(value => typeof value === 'string' && value.length) || !names(contract.operations) || !contract.operations.length || !names(contract.capabilities ?? []) || !names(contract.errors ?? []) || typeof decodeSetting !== 'function') {
      throw new Error('incomplete component contract');
    }
    this.contract = Object.freeze({
      port: contract.port, schema: contract.schema, adapter: contract.adapter,
      operations: Object.freeze([...contract.operations]),
      capabilities: Object.freeze([...(contract.capabilities ?? [])]),
      errors: Object.freeze([...(contract.errors ?? [])]),
    });
    this.decodeSetting = decodeSetting;
    this.handlers = new Map();
    this.outputTypes = null;
    this.errorTypes = null;
  }

  static fromManifest(manifest, handlers = null) {
    const operations = manifest.operations;
    if (!names(operations) || Object.keys(manifest.inputs).sort().join() !== [...operations].sort().join() || Object.keys(manifest.outputs).sort().join() !== [...operations].sort().join()) throw new Error('incomplete typed Worker contract');
    const worker = new Worker({
      port: manifest.port, schema: manifest.schema, adapter: manifest.adapter,
      operations, capabilities: manifest.capabilities, errors: Object.keys(manifest.errors),
    }, value => {
      if (manifest.setting !== null) return typed(value, manifest.setting);
      if (value !== null) throw new ProtocolError('unexpected setting');
      return null;
    });
    worker.inputTypes = manifest.inputs;
    worker.outputTypes = manifest.outputs;
    worker.errorTypes = manifest.errors;
    if (handlers !== null) {
      if (Object.keys(handlers).sort().join() !== [...operations].sort().join()) throw new Error('component operations are not fully registered');
      for (const [operation, handler] of Object.entries(handlers)) worker.registerHandler(operation, handler);
    }
    return worker;
  }

  registerHandler(operation, handler) {
    if (!this.inputTypes || !Object.hasOwn(this.inputTypes, operation)) throw new Error('operation is not in a typed Worker contract');
    this.register(operation, value => typed(value, this.inputTypes[operation]), handler);
  }

  register(operation, decodePayload, handler) {
    if (!this.contract.operations.includes(operation) || this.handlers.has(operation) || typeof decodePayload !== 'function' || typeof handler !== 'function') throw new Error('unknown or duplicate operation');
    this.handlers.set(operation, { decodePayload, handler });
  }

  async serve() {
    const contract = this.contract;
    if (this.handlers.size !== contract.operations.length) throw new Error('component operations are not fully registered');
    const frames = new Frames();
    const hello = await frames.next();
    exact(hello, ['kind', 'version', 'port', 'schema', 'adapter', 'capabilities', 'operations', 'setting']);
    if (hello.kind !== 'hello' || hello.version !== VERSION || hello.port !== contract.port || hello.schema !== contract.schema || hello.adapter !== contract.adapter || JSON.stringify(hello.capabilities) !== JSON.stringify(contract.capabilities) || JSON.stringify(hello.operations) !== JSON.stringify(contract.operations)) {
      throw new ProtocolError('component handshake does not match registered contract');
    }
    let setting;
    try { setting = this.decodeSetting(hello.setting); }
    catch { throw new ProtocolError('invalid component setting'); }
    await frames.send({ kind: 'ready', version: VERSION, port: contract.port, schema: contract.schema, adapter: contract.adapter, capabilities: contract.capabilities, operations: contract.operations });

    let active = null;
    let nextID = 1;
    let lastCancelled = false;
    for (;;) {
      const message = await frames.next();
      switch (message.kind) {
        case 'health':
          exact(message, ['kind', 'id']);
          if (requestID(message.id) !== 0) throw new ProtocolError('invalid health id');
          await frames.send(message);
          break;
        case 'call': {
          exact(message, ['kind', 'id', 'operation', 'payload']);
          const id = requestID(message.id);
          const operation = this.handlers.get(message.operation);
          if (id !== nextID++ || active || !operation) throw new ProtocolError('invalid component call');
          let payload;
          try { payload = operation.decodePayload(message.payload); }
          catch { throw new ProtocolError('invalid component payload'); }
          const current = { id, controller: new AbortController() };
          active = current;
          current.settled = Promise.resolve().then(() => operation.handler(payload, setting, current.controller.signal)).then(
            async result => {
              if (active !== current) return;
              active = null;
              lastCancelled = false;
              if (this.outputTypes) result = typed(result, this.outputTypes[message.operation]);
              await frames.send({ kind: 'result', id, payload: result });
            },
            async error => {
              if (active !== current) return;
              active = null;
              lastCancelled = false;
              if (!(error instanceof BusinessError) || !contract.errors.includes(error.identity)) throw new ProtocolError('component handler failed');
              if (this.errorTypes) error.payload = typed(error.payload, this.errorTypes[error.identity]);
              await frames.send({ kind: 'error', id, error: error.identity, payload: error.payload });
            },
          ).catch(error => { frames.fail(error); });
          break;
        }
        case 'cancel':
          exact(message, ['kind', 'id']);
          requestID(message.id);
          if (active?.id === message.id) {
            const current = active;
            active = null;
            await stopHandler(current);
            lastCancelled = true;
            await frames.send({ kind: 'error', id: message.id, error: 'dever.cancelled', payload: null });
          } else if (message.id !== nextID - 1 || lastCancelled) throw new ProtocolError('unknown component cancel id');
          break;
        case 'shutdown':
          exact(message, ['kind']);
          if (active) {
            const current = active;
            active = null;
            await stopHandler(current);
          }
          await frames.send({ kind: 'shutdown' });
          return;
        default:
          throw new ProtocolError('unknown component message');
      }
    }
  }
}

export async function main(worker) {
  try { await worker.serve(); }
  catch (error) {
    console.error(`component worker: ${error instanceof ProtocolError ? error.message : 'internal failure'}`);
    process.exit(1);
  }
}
