import { compile, initializeCompiler, run, watch } from "./runtime";
import type { WorkerRequest, WorkerResponse } from "./types";

const workerScope = self as unknown as {
  addEventListener: (
    type: "message",
    listener: (event: MessageEvent<WorkerRequest>) => void,
  ) => void;
  postMessage: (response: WorkerResponse) => void;
};

workerScope.addEventListener("message", (event: MessageEvent<WorkerRequest>) => {
  const request = event.data;

  if (request.kind !== "compile" && request.kind !== "run" && request.kind !== "discover" && request.kind !== "watch") {
    return;
  }

  void handleCompile(request);
});

async function handleCompile(request: WorkerRequest): Promise<void> {
  try {
    await initializeCompiler();
    if (request.kind === "watch") {
      workerScope.postMessage({ id: request.id, kind: "watch-result", result: watch(request.input) });
      return;
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
