"use strict";

const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;
let lifecycle = Promise.resolve();

function serverCommand() {
  const configured = vscode.workspace
    .getConfiguration("solcore.lsp")
    .get("serverPath", "")
    .trim();

  return configured || process.env.SOLCORE_LSP_SERVER || "solcore-lsp";
}

function createClient(command, outputChannel, fileWatcher) {
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
      `Failed to start the Solcore language server (${command}): ${detail}. Configure solcore.lsp.serverPath to the solcore-lsp executable.`,
      "Open Settings",
    )
    .then((selection) => {
      if (selection === "Open Settings") {
        return vscode.commands.executeCommand(
          "workbench.action.openSettings",
          "solcore.lsp.serverPath",
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

  const command = serverCommand();
  const next = createClient(command, outputChannel, fileWatcher);
  client = next;
  try {
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

function activate(context) {
  const outputChannel = vscode.window.createOutputChannel("Solcore Language Server");
  const fileWatcher = vscode.workspace.createFileSystemWatcher("**/*.solc");

  context.subscriptions.push(outputChannel, fileWatcher);
  void scheduleClientReplacement(outputChannel, fileWatcher);
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("solcore.lsp.serverPath")) {
        void scheduleClientReplacement(outputChannel, fileWatcher);
      }
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
