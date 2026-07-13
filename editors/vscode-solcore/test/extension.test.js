"use strict";

const assert = require("node:assert/strict");
const Module = require("node:module");
const path = require("node:path");
const test = require("node:test");

function loadExtension({ startResults = [Promise.resolve()] } = {}) {
  let configuredPath = "";
  let configurationListener;
  const errors = [];
  const executedCommands = [];
  const output = [];
  const clients = [];

  class FakeLanguageClient {
    constructor(_id, _name, serverOptions) {
      this.command = serverOptions.command;
      this.startResult = startResults[clients.length] ?? Promise.resolve();
      this.started = 0;
      this.stopped = 0;
      clients.push(this);
    }

    start() {
      this.started += 1;
      return this.startResult;
    }

    stop() {
      this.stopped += 1;
      return Promise.resolve();
    }
  }

  const vscode = {
    commands: {
      executeCommand(command, argument) {
        executedCommands.push([command, argument]);
        return Promise.resolve();
      },
    },
    window: {
      createOutputChannel() {
        return {
          appendLine(line) {
            output.push(line);
          },
          dispose() {},
        };
      },
      showErrorMessage(message) {
        errors.push(message);
        return Promise.resolve("Open Settings");
      },
    },
    workspace: {
      createFileSystemWatcher() {
        return { dispose() {} };
      },
      getConfiguration() {
        return {
          get() {
            return configuredPath;
          },
        };
      },
      onDidChangeConfiguration(listener) {
        configurationListener = listener;
        return { dispose() {} };
      },
    },
  };

  const originalLoad = Module._load;
  Module._load = function mockLoad(request, parent, isMain) {
    if (request === "vscode") {
      return vscode;
    }
    if (request === "vscode-languageclient/node") {
      return { LanguageClient: FakeLanguageClient, TransportKind: { stdio: 0 } };
    }
    return originalLoad.call(this, request, parent, isMain);
  };
  const extensionPath = path.resolve(__dirname, "../extension.js");
  delete require.cache[extensionPath];
  let extension;
  try {
    extension = require(extensionPath);
  } finally {
    Module._load = originalLoad;
  }

  return {
    clients,
    errors,
    executedCommands,
    extension,
    output,
    setConfiguredPath(value) {
      configuredPath = value;
    },
    signalConfigurationChange() {
      configurationListener({
        affectsConfiguration(name) {
          return name === "solcore.lsp.serverPath";
        },
      });
    },
  };
}

function context() {
  return { subscriptions: [] };
}

test("spawn failure explains how to configure the server executable", async () => {
  const harness = loadExtension({
    startResults: [Promise.reject(new Error("ENOENT"))],
  });

  harness.extension.activate(context());
  await harness.extension.deactivate();
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(harness.errors.length, 1);
  assert.match(harness.errors[0], /solcore\.lsp\.serverPath/);
  assert.match(harness.errors[0], /ENOENT/);
  assert.deepEqual(harness.executedCommands, [
    ["workbench.action.openSettings", "solcore.lsp.serverPath"],
  ]);
  assert.match(harness.output[0], /Failed to start solcore-lsp: ENOENT/);
});

test("serverPath changes restart the client without a window reload", async () => {
  const harness = loadExtension();
  harness.extension.activate(context());
  await new Promise((resolve) => setImmediate(resolve));
  harness.setConfiguredPath("/opt/solcore/bin/solcore-lsp");
  harness.signalConfigurationChange();
  await harness.extension.deactivate();

  assert.equal(harness.clients.length, 2);
  assert.equal(harness.clients[0].command, "solcore-lsp");
  assert.equal(harness.clients[0].started, 1);
  assert.equal(harness.clients[0].stopped, 1);
  assert.equal(harness.clients[1].command, "/opt/solcore/bin/solcore-lsp");
  assert.equal(harness.clients[1].started, 1);
  assert.equal(harness.clients[1].stopped, 1);
});
