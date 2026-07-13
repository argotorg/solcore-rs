import LspWorker from "./lsp.worker?worker";
import type {
  JsonRpcError,
  JsonRpcId,
  JsonRpcMessage,
  JsonRpcNotification,
  JsonRpcRequest,
  JsonRpcResponse,
} from "./protocol";

interface PendingRequest<T = unknown> {
  resolve: (result: T) => void;
  reject: (reason: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
}

type NotificationHandler = (params: any) => void;

const SERVER_REQUEST_NOT_SUPPORTED = -32601;
const PLAYGROUND_WORKSPACE_URI = "file:///main";
const LSP_REQUEST_TIMEOUT_MS = 30_000;
const WORKER_BOOT_ERROR = "solcore-lsp/boot-error";

function createAbortError(message: string): Error {
  return new DOMException(message, "AbortError");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isJsonRpcMessage(value: unknown): value is JsonRpcMessage {
  return isRecord(value) && value.jsonrpc === "2.0";
}

function isWorkerBootError(
  value: unknown,
): value is { type: typeof WORKER_BOOT_ERROR; message: string } {
  return (
    isRecord(value) &&
    value.type === WORKER_BOOT_ERROR &&
    typeof value.message === "string"
  );
}

function isResponse(value: JsonRpcMessage): value is JsonRpcResponse {
  return "id" in value && !("method" in value);
}

function isNotification(value: JsonRpcMessage): value is JsonRpcNotification {
  return "method" in value && !("id" in value);
}

function isRequest(value: JsonRpcMessage): value is JsonRpcRequest {
  return "method" in value && "id" in value;
}

function isPendingId(id: JsonRpcId): id is number {
  return typeof id === "number";
}

function errorFromJsonRpc(error: JsonRpcError): Error {
  const message = error.message || `LSP request failed (${error.code})`;
  return new Error(message);
}

export class LspClient {
  private readonly worker: Worker;
  private nextId = 1;
  private initializePromise: Promise<void> | null = null;
  private disposed = false;
  private workerFailure: Error | null = null;
  private readonly pending = new Map<number, PendingRequest>();
  private readonly notificationHandlers = new Map<string, Set<NotificationHandler>>();

  constructor(private readonly requestTimeoutMs = LSP_REQUEST_TIMEOUT_MS) {
    this.worker = new LspWorker();
    this.worker.addEventListener("message", this.handleMessage);
    this.worker.addEventListener("error", this.handleWorkerError);
  }

  initialize(): Promise<void> {
    this.initializePromise ??= this.request("initialize", {
      processId: null,
      clientInfo: {
        name: "solcore-playground",
      },
      rootUri: PLAYGROUND_WORKSPACE_URI,
      capabilities: {
        textDocument: {
          publishDiagnostics: {
            relatedInformation: true,
          },
          codeAction: {
            dynamicRegistration: false,
            codeActionLiteralSupport: {
              codeActionKind: {
                valueSet: ["quickfix"],
              },
            },
            isPreferredSupport: true,
            disabledSupport: true,
          },
          formatting: {
            dynamicRegistration: false,
          },
          foldingRange: {
            dynamicRegistration: false,
            lineFoldingOnly: true,
          },
          selectionRange: {
            dynamicRegistration: false,
          },
          synchronization: {
            dynamicRegistration: false,
            didSave: false,
          },
        },
        workspace: {
          workspaceFolders: true,
        },
        general: {
          positionEncodings: ["utf-16"],
        },
      },
      workspaceFolders: [
        {
          uri: PLAYGROUND_WORKSPACE_URI,
          name: "main",
        },
      ],
    }).then(() => {
      this.notify("initialized", {});
    });

    return this.initializePromise;
  }

  request<T>(method: string, params: unknown): Promise<T> {
    if (this.disposed) {
      return Promise.reject(createAbortError("LSP client disposed"));
    }
    if (this.workerFailure) {
      return Promise.reject(this.workerFailure);
    }

    const id = this.nextId;
    this.nextId += 1;

    const request: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    const promise = new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (!this.pending.delete(id)) {
          return;
        }
        reject(
          new Error(
            `LSP request '${method}' timed out after ${this.requestTimeoutMs}ms`,
          ),
        );
      }, this.requestTimeoutMs);
      this.pending.set(id, {
        resolve: resolve as (result: unknown) => void,
        reject,
        timeout,
      });
    });

    this.worker.postMessage(request);
    return promise;
  }

  notify(method: string, params: unknown): void {
    if (this.disposed || this.workerFailure) {
      return;
    }

    const notification: JsonRpcNotification = {
      jsonrpc: "2.0",
      method,
      params,
    };
    this.worker.postMessage(notification);
  }

  onNotification(method: string, handler: NotificationHandler): () => void {
    let handlers = this.notificationHandlers.get(method);
    if (!handlers) {
      handlers = new Set();
      this.notificationHandlers.set(method, handlers);
    }

    handlers.add(handler);

    return () => {
      handlers.delete(handler);
      if (handlers.size === 0) {
        this.notificationHandlers.delete(method);
      }
    };
  }

  didOpen(uri: string, version: number, text: string): void {
    this.notify("textDocument/didOpen", {
      textDocument: {
        uri,
        languageId: "solcore",
        version,
        text,
      },
    });
  }

  didChange(uri: string, version: number, text: string): void {
    this.notify("textDocument/didChange", {
      textDocument: {
        uri,
        version,
      },
      contentChanges: [
        {
          text,
        },
      ],
    });
  }

  didClose(uri: string): void {
    this.notify("textDocument/didClose", {
      textDocument: {
        uri,
      },
    });
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }

    this.disposed = true;
    this.worker.removeEventListener("message", this.handleMessage);
    this.worker.removeEventListener("error", this.handleWorkerError);
    this.worker.terminate();

    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(createAbortError("LSP worker terminated"));
    }
    this.pending.clear();
    this.notificationHandlers.clear();
  }

  private readonly handleMessage = (event: MessageEvent<unknown>): void => {
    const message = event.data;
    if (isWorkerBootError(message)) {
      this.failWorker(new Error(`Solcore LSP worker failed to start: ${message.message}`));
      return;
    }
    if (!isJsonRpcMessage(message)) {
      return;
    }

    if (isResponse(message)) {
      this.handleResponse(message);
      return;
    }

    if (isNotification(message)) {
      this.handleNotification(message);
      return;
    }

    if (isRequest(message)) {
      this.replyUnsupportedRequest(message);
    }
  };

  private handleResponse(response: JsonRpcResponse): void {
    if (!isPendingId(response.id)) {
      return;
    }

    const pending = this.pending.get(response.id);
    if (!pending) {
      return;
    }

    this.pending.delete(response.id);
    clearTimeout(pending.timeout);

    if (response.error) {
      pending.reject(errorFromJsonRpc(response.error));
      return;
    }

    pending.resolve(response.result);
  }

  private handleNotification(notification: JsonRpcNotification): void {
    const handlers = this.notificationHandlers.get(notification.method);
    if (!handlers) {
      return;
    }

    for (const handler of handlers) {
      handler(notification.params);
    }
  }

  private replyUnsupportedRequest(request: JsonRpcRequest): void {
    if (this.disposed) {
      return;
    }

    const response: JsonRpcResponse = {
      jsonrpc: "2.0",
      id: request.id,
      error: {
        code: SERVER_REQUEST_NOT_SUPPORTED,
        message: `Client does not support server request '${request.method}'`,
      },
    };
    this.worker.postMessage(response);
  }

  private readonly handleWorkerError = (event: ErrorEvent): void => {
    this.failWorker(new Error(event.message || "LSP worker failed"));
  };

  private failWorker(error: Error): void {
    this.workerFailure = error;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(error);
    }
    this.pending.clear();
  }
}
