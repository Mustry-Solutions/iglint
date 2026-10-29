import * as fs from "fs";
import * as os from "os";
import * as vscode from "vscode";
import {
  DocumentSelector,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

import { Settings, needsRestart, readSettings, resolveServerPath, serverSettings } from "./config";

const SECTION = "iglint";

/** Files the server lints: Perspective and gateway resources, scripts. */
const DOCUMENTS: DocumentSelector = [
  ...[
    "view.json",
    "resource.json",
    "unary-resource.json",
    "project.json",
    "style.json",
    "config.json",
    "props.json",
    "tags.json",
    "udts.json",
  ].map((name) => ({ scheme: "file", pattern: `**/${name}` })),
  { scheme: "file", language: "python", pattern: "**/ignition/**/*.py" },
  { scheme: "file", language: "python", pattern: "**/com.inductiveautomation.*/**/*.py" },
];

let client: LanguageClient | undefined;
let output: vscode.OutputChannel;
let traceOutput: vscode.OutputChannel;
let settings: Settings;

function currentSettings(): Settings {
  const config = vscode.workspace.getConfiguration(SECTION);
  return readSettings((key) => config.get(key));
}

function isFile(file: string): boolean {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

async function start(context: vscode.ExtensionContext): Promise<void> {
  settings = currentSettings();
  const command = resolveServerPath(settings.path, {
    platform: process.platform,
    arch: process.arch,
    pathVariable: process.env.PATH ?? "",
    home: os.homedir(),
    workspaceFolder: vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
    extensionPath: context.extensionPath,
    isFile,
  });
  if (command === undefined) {
    const choice = await vscode.window.showErrorMessage(
      "iglint: no `iglint` binary found. Install it on PATH or set `iglint.path`.",
      "Open Settings",
    );
    if (choice === "Open Settings") {
      await vscode.commands.executeCommand("workbench.action.openSettings", "iglint.path");
    }
    return;
  }
  const serverOptions: ServerOptions = {
    command,
    args: ["server", ...settings.args],
    // Executables use stdio by default. An explicit transport adds a
    // `--stdio` argument, which `iglint server` does not accept.
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: DOCUMENTS,
    outputChannel: output,
    traceOutputChannel: traceOutput,
    initializationOptions: serverSettings(settings),
    synchronize: {
      // The server also registers watchers itself; these cover clients
      // without dynamic registration.
      fileEvents: vscode.workspace.createFileSystemWatcher("**/{iglint.toml,.iglint.toml,pyproject.toml,rule_config.json}"),
    },
  };
  client = new LanguageClient("iglint", "iglint", serverOptions, clientOptions);
  output.appendLine(`Starting ${command} server`);
  try {
    await client.start();
  } catch (error) {
    output.appendLine(`iglint server failed to start: ${error}`);
    void vscode.window.showErrorMessage(`iglint: the server failed to start (${command}). See the output for details.`);
  }
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  if (running !== undefined) {
    await running.stop();
  }
}

async function restart(context: vscode.ExtensionContext): Promise<void> {
  await stop();
  await start(context);
}

async function fixAll(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (client === undefined || editor === undefined) {
    return;
  }
  const uri = editor.document.uri.toString();
  const applied = await client.sendRequest("workspace/executeCommand", {
    command: "iglint.applyFixAll",
    arguments: [uri],
  });
  if (applied !== true) {
    void vscode.window.showInformationMessage("iglint: nothing to fix.");
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  output = vscode.window.createOutputChannel("iglint");
  traceOutput = vscode.window.createOutputChannel("iglint Trace");
  context.subscriptions.push(
    output,
    traceOutput,
    vscode.commands.registerCommand("iglint.restartServer", () => restart(context)),
    vscode.commands.registerCommand("iglint.fixAll", fixAll),
    vscode.commands.registerCommand("iglint.showOutput", () => output.show(true)),
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (!event.affectsConfiguration(SECTION)) {
        return;
      }
      const next = currentSettings();
      if (needsRestart(settings, next)) {
        await restart(context);
      } else if (client !== undefined) {
        settings = next;
        await client.sendNotification("workspace/didChangeConfiguration", {
          settings: { iglint: serverSettings(next) },
        });
      }
    }),
  );
  await start(context);
}

export async function deactivate(): Promise<void> {
  await stop();
}
