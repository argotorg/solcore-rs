import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";

class FakeWorker {
  listeners = new Map();
  messages = [];

  addEventListener(type, listener) {
    this.listeners.set(type, listener);
  }

  removeEventListener(type) {
    this.listeners.delete(type);
  }

  postMessage(message) {
    this.messages.push(message);
  }

  terminate() {}

  emitMessage(data) {
    this.listeners.get("message")?.({ data });
  }
}

async function loadClient(worker) {
  globalThis.__createSolcoreLspWorker = () => worker;
  const result = await build({
    bundle: true,
    entryPoints: [fileURLToPath(new URL("./lspClient.ts", import.meta.url))],
    format: "esm",
    platform: "browser",
    write: false,
    plugins: [
      {
        name: "fake-lsp-worker",
        setup(buildApi) {
          buildApi.onResolve({ filter: /lsp\.worker\?worker$/ }, () => ({
            path: "fake-lsp-worker",
            namespace: "test",
          }));
          buildApi.onLoad(
            { filter: /.*/, namespace: "test" },
            () => ({
              contents:
                "export default class FakeLspWorker { constructor() { return globalThis.__createSolcoreLspWorker(); } }",
              loader: "js",
            }),
          );
        },
      },
    ],
  });
  const source = result.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
}

test("wasm boot failure rejects initialization and future requests", async () => {
  const worker = new FakeWorker();
  const { LspClient } = await loadClient(worker);
  const client = new LspClient(1_000);
  const initialized = client.initialize();

  worker.emitMessage({
    type: "solcore-lsp/boot-error",
    message: "failed to fetch wasm",
  });

  await assert.rejects(initialized, /failed to fetch wasm/);
  await assert.rejects(
    client.request("textDocument/hover", {}),
    /failed to fetch wasm/,
  );
  client.dispose();
});

test("requests reject when the worker never responds", async () => {
  const worker = new FakeWorker();
  const { LspClient } = await loadClient(worker);
  const client = new LspClient(5);

  await assert.rejects(
    client.request("textDocument/hover", {}),
    /timed out after 5ms/,
  );
  client.dispose();
});
