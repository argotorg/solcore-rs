"use strict";

const assert = require("node:assert/strict");
const Module = require("node:module");
const path = require("node:path");
const test = require("node:test");

function loadExtension({
  startResults = [Promise.resolve()],
  trusted = true,
  documents = [],
  languages = ["solcore", "plaintext"],
} = {}) {
  let compilerPath = "/installed/solcore";
  let trustListener;
  let configuredPath = "";
  let configurationListener;
  let openListener;
  let saveListener;
  const errors = [];
  const executedCommands = [];
  const output = [];
  const clients = [];
  const languageChanges = [];

  class FakeLanguageClient {
    constructor(_id, _name, serverOptions) {
      this.command = serverOptions.command;
      this.args = serverOptions.args;
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
    languages: {
      getLanguages() {
        return Promise.resolve(languages);
      },
      setTextDocumentLanguage(document, languageId) {
        languageChanges.push([document.uri.path, languageId]);
        document.languageId = languageId;
        return Promise.resolve(document);
      },
    },
    workspace: {
      isTrusted: trusted,
      textDocuments: documents,
      onDidOpenTextDocument(listener) {
        openListener = listener;
        return { dispose() {} };
      },
      onDidSaveTextDocument(listener) {
        saveListener = listener;
        return { dispose() {} };
      },
      onDidGrantWorkspaceTrust(listener) {
        trustListener = listener;
        return { dispose() {} };
      },
      createFileSystemWatcher() {
        return { dispose() {} };
      },
      getConfiguration(section) {
        return {
          get() {
            return section === "solcore" ? compilerPath : configuredPath;
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
    languageChanges,
    output,
    open(document) {
      openListener(document);
    },
    save(document) {
      saveListener(document);
    },
    grantTrust() {
      vscode.workspace.isTrusted = true;
      trustListener();
    },
    setConfiguredPath(value) {
      compilerPath = "";
      configuredPath = value;
    },
    setCompilerPath(value) {
      compilerPath = value;
    },
    signalConfigurationChange(changed = "solcore.lsp.serverPath") {
      configurationListener({
        affectsConfiguration(name) {
          return name === changed;
        },
      });
    },
  };
}

function context() {
  return { subscriptions: [] };
}

function document(path, text, languageId) {
  return {
    uri: { scheme: "file", path },
    languageId,
    text,
    getText() {
      return this.text;
    },
  };
}

function settled() {
  return new Promise((resolve) => setImmediate(resolve));
}

test("sol files are Solcore exactly when they declare pragma solcore", async () => {
  const harness = loadExtension({ languages: ["solcore", "solidity", "plaintext"] });
  harness.extension.activate(context());
  harness.open(document("/w/a.sol", "// header\npragma solcore;\n", "solidity"));
  harness.open(document("/w/b.sol", "pragma solidity ^0.8.0;\n", "solcore"));
  harness.open(document("/w/c.sol", "pragma solcore;\n", "solcore"));
  harness.open(document("/w/d.sol", "pragma solidity ^0.8.0;\n", "solidity"));
  harness.open(document("/w/notes.txt", "pragma solcore;\n", "plaintext"));
  await settled();
  await harness.extension.deactivate();

  assert.deepEqual(harness.languageChanges, [
    ["/w/a.sol", "solcore"],
    ["/w/b.sol", "solidity"],
  ]);
});

test("without a Solidity extension other sol files become plain text", async () => {
  const harness = loadExtension({
    documents: [document("/w/token.sol", "contract Token {}\n", "solcore")],
  });
  harness.extension.activate(context());
  await settled();
  await harness.extension.deactivate();

  assert.deepEqual(harness.languageChanges, [["/w/token.sol", "plaintext"]]);
});

test("saving a file that now declares the pragma makes it Solcore", async () => {
  const harness = loadExtension();
  harness.extension.activate(context());
  const created = document("/w/new.sol", "", "plaintext");
  harness.open(created);
  created.text = "pragma solcore;\n";
  harness.save(created);
  await settled();
  await harness.extension.deactivate();

  assert.deepEqual(harness.languageChanges, [["/w/new.sol", "solcore"]]);
});

test("spawn failure explains how to configure the server executable", async () => {
  const harness = loadExtension({
    startResults: [Promise.reject(new Error("ENOENT"))],
  });

  harness.extension.activate(context());
  await harness.extension.deactivate();
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(harness.errors.length, 1);
  assert.match(harness.errors[0], /solcore\.path/);
  assert.match(harness.errors[0], /ENOENT/);
  assert.deepEqual(harness.executedCommands, [
    ["workbench.action.openSettings", "solcore.path"],
  ]);
  assert.match(harness.output[0], /Failed to start \/installed\/solcore lsp: ENOENT/);
});

test("serverPath changes restart the client without a window reload", async () => {
  const harness = loadExtension();
  harness.extension.activate(context());
  await new Promise((resolve) => setImmediate(resolve));
  harness.setConfiguredPath("/opt/solcore/bin/solcore-lsp");
  harness.signalConfigurationChange();
  await harness.extension.deactivate();

  assert.equal(harness.clients.length, 2);
  assert.equal(harness.clients[0].command, "/installed/solcore");
  assert.deepEqual(harness.clients[0].args, ["lsp"]);
  assert.equal(harness.clients[0].started, 1);
  assert.equal(harness.clients[0].stopped, 1);
  assert.equal(harness.clients[1].command, "/opt/solcore/bin/solcore-lsp");
  assert.deepEqual(harness.clients[1].args, []);
  assert.equal(harness.clients[1].started, 1);
  assert.equal(harness.clients[1].stopped, 1);
});


test("untrusted workspaces keep highlighting without launching a process", async () => {
  const harness = loadExtension({ trusted: false });
  harness.extension.activate(context());
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(harness.clients.length, 0);
  harness.grantTrust();
  await harness.extension.deactivate();
  assert.equal(harness.clients.length, 1);
});

test("compiler path changes replace the bundled LSP process", async () => {
  const harness = loadExtension();
  harness.extension.activate(context());
  await new Promise((resolve) => setImmediate(resolve));
  harness.setCompilerPath("/new version/solcore");
  harness.signalConfigurationChange("solcore.path");
  await harness.extension.deactivate();
  assert.equal(harness.clients.length, 2);
  assert.equal(harness.clients[0].stopped, 1);
  assert.equal(harness.clients[1].command, "/new version/solcore");
  assert.deepEqual(harness.clients[1].args, ["lsp"]);
});
