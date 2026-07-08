import init, { SolcoreLsp } from "../pkg/solcore_lsp.js";

const ctx = self as unknown as DedicatedWorkerGlobalScope;

let server: SolcoreLsp | null = null;
const pending: unknown[] = [];

ctx.onmessage = (event: MessageEvent<unknown>) => {
  if (server === null) {
    pending.push(event.data);
    return;
  }

  handleMessage(event.data);
};

async function start(): Promise<void> {
  await init();
  server = new SolcoreLsp();

  while (pending.length > 0) {
    handleMessage(pending.shift());
  }
}

function handleMessage(data: unknown): void {
  if (server === null) {
    pending.push(data);
    return;
  }

  const outgoing = server.handle(JSON.stringify(data));
  for (const message of outgoing) {
    ctx.postMessage(JSON.parse(message));
  }
}

void start();
