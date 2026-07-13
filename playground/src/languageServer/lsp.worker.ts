import init, { SolcoreLsp } from "solcore-lsp";
import wasmUrl from "solcore-lsp/solcore_lsp_bg.wasm?url";

const workerScope = self as unknown as {
  addEventListener: (
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ) => void;
  postMessage: (message: unknown) => void;
};

let server: SolcoreLsp | null = null;
const pending: unknown[] = [];
const WORKER_BOOT_ERROR = "solcore-lsp/boot-error";

workerScope.addEventListener("message", (event: MessageEvent<unknown>) => {
  if (!server) {
    pending.push(event.data);
    return;
  }

  handleMessage(event.data);
});

async function start(): Promise<void> {
  await init(wasmUrl);
  server = new SolcoreLsp();

  const queued = pending.splice(0);
  for (const data of queued) {
    handleMessage(data);
  }
}

function handleMessage(data: unknown): void {
  if (!server) {
    pending.push(data);
    return;
  }

  const outgoing = server.handle(JSON.stringify(data));
  for (const message of outgoing) {
    workerScope.postMessage(JSON.parse(message) as unknown);
  }
}

void start().catch((error: unknown) => {
  pending.length = 0;
  workerScope.postMessage({
    type: WORKER_BOOT_ERROR,
    message: error instanceof Error ? error.message : String(error),
  });
});

export {};
