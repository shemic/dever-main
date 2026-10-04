import { readFileSync } from 'node:fs';
import { BusinessError, Worker, main } from '../../sdk/javascript/dever_component.mjs';

const manifest = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const worker = Worker.fromManifest(manifest, {
  render: async (payload, setting) => {
    const message = payload.message;
    if (message.name === 'reject') throw new BusinessError('example.Rejected', { reason: 'rejected' });
    return { text: setting.prefix + message.name + String(message.number) };
  },
});
await main(worker);
