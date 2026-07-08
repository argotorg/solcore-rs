import { compile, initializeCompiler } from "./runtime";
import type { CompileRequest, CompileResponse } from "./types";

const workerScope = self as unknown as {
  addEventListener: (
    type: "message",
    listener: (event: MessageEvent<CompileRequest>) => void,
  ) => void;
  postMessage: (response: CompileResponse) => void;
};

workerScope.addEventListener("message", (event: MessageEvent<CompileRequest>) => {
  const request = event.data;

  if (request.kind !== "compile") {
    return;
  }

  void handleCompile(request);
});

async function handleCompile(request: CompileRequest): Promise<void> {
  try {
    await initializeCompiler();
    const result = compile(request.input);
    const response: CompileResponse = {
      id: request.id,
      kind: "result",
      result,
    };
    workerScope.postMessage(response);
  } catch (error: unknown) {
    const response: CompileResponse = {
      id: request.id,
      kind: "error",
      message: error instanceof Error ? error.message : "Unknown compiler error",
    };
    workerScope.postMessage(response);
  }
}

export {};
