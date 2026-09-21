import CompileWorker from "./compile.worker?worker";
import type { CompileInput, CompileRequest, WorkerResponse, CompileResult, WatchInput, WatchResult } from "./types";

interface PendingRequest {
  kind: CompileRequest["kind"];
  resolve: (result: CompileResult) => void;
  reject: (reason: Error) => void;
}

function createAbortError(message: string): Error {
  return new DOMException(message, "AbortError");
}

export class CompileClient {
  private readonly worker: Worker;
  private readonly pendingWatches = new Map<number, {
    resolve: (result: WatchResult[]) => void;
    reject: (error: Error) => void;
  }>();
  private nextId = 1;
  private latestId = 0;
  private readonly pending = new Map<number, PendingRequest>();

  constructor() {
    this.worker = new CompileWorker();
    this.worker.addEventListener("message", this.handleMessage);
    this.worker.addEventListener("error", this.handleWorkerError);
  }

  compile(input: CompileInput): Promise<CompileResult> {
    return this.request("compile", input);
  }

  run(input: CompileInput): Promise<CompileResult> {
    return this.request("run", input);
  }

  discover(input: CompileInput): Promise<CompileResult> {
    return this.request("discover", input);
  }

  watch(input: WatchInput): Promise<WatchResult[]> {
    for (const pending of this.pendingWatches.values()) pending.reject(createAbortError("Watch request superseded"));
    this.pendingWatches.clear();
    const id = this.nextId++;
    const promise = new Promise<WatchResult[]>((resolve, reject) => {
      this.pendingWatches.set(id, { resolve, reject });
    });
    this.worker.postMessage({ id, kind: "watch", input });
    return promise;
  }

  private request(kind: CompileRequest["kind"], input: CompileInput): Promise<CompileResult> {
    const id = this.nextId;
    this.nextId += 1;
    if (kind !== "discover") this.latestId = id;

    for (const [pendingId, pending] of this.pending) {
      if ((pending.kind === "discover") === (kind === "discover") && pendingId < id) {
        pending.reject(createAbortError("Compile request superseded"));
        this.pending.delete(pendingId);
      }
    }

    const request: CompileRequest = {
      id,
      kind,
      input,
    };

    const promise = new Promise<CompileResult>((resolve, reject) => {
      this.pending.set(id, { kind, resolve, reject });
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
    for (const pending of this.pendingWatches.values()) pending.reject(createAbortError("Compiler worker terminated"));
    this.pendingWatches.clear();
  }

  private readonly handleMessage = (event: MessageEvent<WorkerResponse>): void => {
    const response = event.data;
    const watch = this.pendingWatches.get(response.id);
    if (watch) {
      this.pendingWatches.delete(response.id);
      if (response.kind === "watch-result") watch.resolve(response.result);
      else watch.reject(new Error(response.kind === "error" ? response.message : "Unexpected watch response"));
      return;
    }
    if (response.kind === "watch-result") return;
    const pending = this.pending.get(response.id);

    if (!pending) {
      return;
    }

    this.pending.delete(response.id);

    if (pending.kind !== "discover" && response.id !== this.latestId) {
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
    for (const pending of this.pendingWatches.values()) pending.reject(error);
    this.pendingWatches.clear();
  };
}

export const compileClient = new CompileClient();
