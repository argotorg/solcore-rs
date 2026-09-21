import CompileWorker from "./compile.worker?worker";
import type { CompileInput, CompileRequest, CompileResponse, CompileResult } from "./types";

interface PendingRequest {
  resolve: (result: CompileResult) => void;
  reject: (reason: Error) => void;
}

function createAbortError(message: string): Error {
  return new DOMException(message, "AbortError");
}

export class CompileClient {
  private readonly worker: Worker;
  private nextId = 1;
  private latestId = 0;
  private readonly pending = new Map<number, PendingRequest>();

  constructor() {
    this.worker = new CompileWorker();
    this.worker.addEventListener("message", this.handleMessage);
    this.worker.addEventListener("error", this.handleWorkerError);
  }

  compile(input: CompileInput): Promise<CompileResult> {
    const id = this.nextId;
    this.nextId += 1;
    this.latestId = id;

    for (const [pendingId, pending] of this.pending) {
      if (pendingId < id) {
        pending.reject(createAbortError("Compile request superseded"));
        this.pending.delete(pendingId);
      }
    }

    const request: CompileRequest = {
      id,
      kind: "compile",
      input,
    };

    const promise = new Promise<CompileResult>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });

    this.worker.postMessage(request);
    return promise;
  }

  terminate(): void {
    this.worker.removeEventListener("message", this.handleMessage);
    this.worker.removeEventListener("error", this.handleWorkerError);
    this.worker.terminate();

    for (const pending of this.pending.values()) {
      pending.reject(createAbortError("Compiler worker terminated"));
    }
    this.pending.clear();
  }

  private readonly handleMessage = (event: MessageEvent<CompileResponse>): void => {
    const response = event.data;
    const pending = this.pending.get(response.id);

    if (!pending) {
      return;
    }

    this.pending.delete(response.id);

    if (response.id !== this.latestId) {
      pending.reject(createAbortError("Compile response superseded"));
      return;
    }

    if (response.kind === "result") {
      pending.resolve(response.result);
      return;
    }

    pending.reject(new Error(response.message));
  };

  private readonly handleWorkerError = (event: ErrorEvent): void => {
    const error = new Error(event.message || "Compiler worker failed");
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
  };
}

export const compileClient = new CompileClient();
