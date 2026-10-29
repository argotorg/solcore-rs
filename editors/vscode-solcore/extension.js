"use strict";

const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");
const { declaresPragmaSolcore } = require("./pragma");
const { resolveServer } = require("./server-command");

let client;
let lifecycle = Promise.resolve();

function serverCommand() {
  return resolveServer({
    compilerPath: vscode.workspace.getConfiguration("solcore").get("path", ""),
    serverPath: vscode.workspace.getConfiguration("solcore.lsp").get("serverPath", ""),
  });
}

function createClient({ command, args }, outputChannel, fileWatcher) {
  const serverOptions = {
    command,
    args,
    transport: TransportKind.stdio,
    options: {
      env: { ...process.env },
    },
  };
  const clientOptions = {
    documentSelector: [
      { scheme: "file", language: "solcore" },
      { scheme: "untitled", language: "solcore" },
    ],
    outputChannel,
    synchronize: {
      fileEvents: fileWatcher,
    },
  };

  return new LanguageClient(
    "solcore-lsp",
    "Solcore Language Server",
    serverOptions,
    clientOptions,
  );
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function reportStartFailure(command, error, outputChannel) {
  const detail = errorMessage(error);
  outputChannel.appendLine(`Failed to start ${command}: ${detail}`);
  void vscode.window
    .showErrorMessage(
      `Failed to start the Solcore language server (${command}): ${detail}. Configure solcore.path to the solcore executable.`,
      "Open Settings",
    )
    .then((selection) => {
      if (selection === "Open Settings") {
        return vscode.commands.executeCommand(
          "workbench.action.openSettings",
          "solcore.path",
        );
      }
      return undefined;
    })
    .catch((notificationError) => {
      outputChannel.appendLine(
        `Failed to show language server configuration action: ${errorMessage(notificationError)}`,
      );
    });
}

async function replaceClient(outputChannel, fileWatcher) {
  const previous = client;
  client = undefined;
  if (previous) {
    try {
      await previous.stop();
    } catch (error) {
      outputChannel.appendLine(
        `Failed to stop the previous Solcore language server: ${errorMessage(error)}`,
      );
    }
  }

  if (vscode.workspace.isTrusted === false) return;

  let command = "solcore lsp";
  let next;
  try {
    const server = serverCommand();
    command = [server.command, ...server.args].join(" ");
    next = createClient(server, outputChannel, fileWatcher);
    client = next;
    await next.start();
  } catch (error) {
    if (client === next) {
      client = undefined;
    }
    reportStartFailure(command, error, outputChannel);
  }
}

function scheduleClientReplacement(outputChannel, fileWatcher) {
  lifecycle = lifecycle.then(() => replaceClient(outputChannel, fileWatcher));
  return lifecycle;
}

function isSolFile(document) {
  return document.uri.scheme === "file" && document.uri.path.endsWith(".sol");
}

// A `.sol` file is Solcore exactly when it declares `pragma solcore;`. Other
// `.sol` files are Solidity: hand them to a Solidity extension when one is
// installed, and never start the Solcore server for them.
async function syncLanguage(document) {
  if (!isSolFile(document)) return;
  const solcore = declaresPragmaSolcore(document.getText());
  if (solcore === (document.languageId === "solcore")) return;
  let language = "solcore";
  if (!solcore) {
    const languages = await vscode.languages.getLanguages();
    language = languages.includes("solidity") ? "solidity" : "plaintext";
  }
  await vscode.languages.setTextDocumentLanguage(document, language);
}

function activate(context) {
  const outputChannel = vscode.window.createOutputChannel("Solcore Language Server");
  const fileWatcher = vscode.workspace.createFileSystemWatcher("**/*.sol");
  const syncDocumentLanguage = (document) => {
    syncLanguage(document).catch((error) => {
      outputChannel.appendLine(
        `Failed to update the language of ${document.uri.path}: ${errorMessage(error)}`,
      );
    });
  };

  context.subscriptions.push(outputChannel, fileWatcher);
  vscode.workspace.textDocuments.forEach(syncDocumentLanguage);
  void scheduleClientReplacement(outputChannel, fileWatcher);
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(syncDocumentLanguage),
    vscode.workspace.onDidSaveTextDocument(syncDocumentLanguage),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("solcore.path") || event.affectsConfiguration("solcore.lsp.serverPath")) {
        void scheduleClientReplacement(outputChannel, fileWatcher);
      }
    }),
    vscode.workspace.onDidGrantWorkspaceTrust(() => {
      void scheduleClientReplacement(outputChannel, fileWatcher);
    }),
  );
}

function deactivate() {
  const stopClient = async () => {
    const active = client;
    client = undefined;
    await active?.stop();
  };
  lifecycle = lifecycle.then(stopClient, stopClient);
  return lifecycle;
}

module.exports = {
  activate,
  deactivate,
};
