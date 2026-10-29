import { compile, configureSolcUrl, initializeCompiler, initializeSolc, needsSolc, run, watch } from "./runtime";
import type { ConfigureRuntimeRequest, WorkerRequest, WorkerResponse } from "./types";

const workerScope = self as unknown as {
  addEventListener: (
    type: "message",
    listener: (event: MessageEvent<WorkerRequest | ConfigureRuntimeRequest>) => void,
  ) => void;
  postMessage: (response: WorkerResponse) => void;
};

// Runs and watches share a persistent WASM sandbox. A backend download must
// not let a later request overtake an earlier run and then lose its deployment.
let requests = Promise.resolve();

workerScope.addEventListener("message", (event: MessageEvent<WorkerRequest | ConfigureRuntimeRequest>) => {
  const request = event.data;

  if (request.kind === "configure-runtime") {
    configureSolcUrl(request.solcUrl);
    return;
  }

  if (request.kind !== "compile" && request.kind !== "run" && request.kind !== "discover" && request.kind !== "watch") {
    return;
  }

  requests = requests.then(() => handleCompile(request));
});

async function handleCompile(request: WorkerRequest): Promise<void> {
  try {
    await initializeCompiler();
    if (request.kind === "watch") {
      workerScope.postMessage({ id: request.id, kind: "watch-result", result: watch(request.input) });
      return;
    }
    if (needsSolc(request.input, request.kind === "run")) {
      await initializeSolc();
    }
    const result = request.kind === "run" ? run(request.input) : compile(request.input);
    const response: WorkerResponse = {
      id: request.id,
      kind: "result",
      result,
    };
    workerScope.postMessage(response);
  } catch (error: unknown) {
    const response: WorkerResponse = {
      id: request.id,
      kind: "error",
      message: error instanceof Error ? error.message : "Unknown compiler error",
    };
    workerScope.postMessage(response);
  }
}

export {};
