"use strict";

const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function serverCommand() {
  const configured = vscode.workspace
    .getConfiguration("solcore.lsp")
    .get("serverPath", "")
    .trim();

  return configured || process.env.SOLCORE_LSP_SERVER || "solcore-lsp";
}

function activate(context) {
  const command = serverCommand();
  const outputChannel = vscode.window.createOutputChannel("Solcore Language Server");
  const serverOptions = {
    command,
    args: [],
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
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.solc"),
    },
  };

  client = new LanguageClient(
    "solcore-lsp",
    "Solcore Language Server",
    serverOptions,
    clientOptions,
  );

  context.subscriptions.push(outputChannel);
  void client.start();
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("solcore.lsp.serverPath")) {
        vscode.window.showInformationMessage(
          "Reload the window to restart solcore-lsp with the new server path.",
        );
      }
    }),
  );
}

function deactivate() {
  return client?.stop();
}

module.exports = {
  activate,
  deactivate,
};
