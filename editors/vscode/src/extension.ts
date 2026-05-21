import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

const BINARY_NAME = process.platform === "win32" ? "flux-lsp.exe" : "flux-lsp";

export function activate(context: vscode.ExtensionContext) {
  const serverPath = resolveServerPath(context);

  const serverOptions: ServerOptions = {
    run: { command: serverPath, transport: TransportKind.stdio },
    debug: { command: serverPath, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "flux" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.flx"),
    },
    // Evaluated on every (re)start, so a config change picked up after a
    // restart sends the new value.
    initializationOptions: () => ({
      workspaceDiagnostics: {
        scanAllFiles: vscode.workspace
          .getConfiguration("flux")
          .get<boolean>("workspaceDiagnostics.scanAllFiles", false),
      },
    }),
  };

  client = new LanguageClient(
    "flux",
    "Flux Language Server",
    serverOptions,
    clientOptions,
  );

  client.start().catch((err) => {
    vscode.window.showErrorMessage(
      `Failed to start flux-lsp at "${serverPath}": ${err}. ` +
        `Set "flux.serverPath" to an absolute path, or reinstall the .vsix.`,
    );
  });

  // Commands invoked by the server's "▶ Run" / "▶ Run Test" / "▶ Run all
  // tests" code lenses (handlers::code_lens). Each launches the Flux CLI on the
  // file in a terminal; the runner command is configurable via `flux.runCommand`.
  context.subscriptions.push(
    vscode.commands.registerCommand("flux.run", (uriArg: string) => {
      runFlux(uriArg, []);
    }),
    vscode.commands.registerCommand(
      "flux.runTest",
      (uriArg: string, testName: string) => {
        runFlux(uriArg, ["--test", "--test-filter", testName]);
      },
    ),
    vscode.commands.registerCommand("flux.runTests", (uriArg: string) => {
      runFlux(uriArg, ["--test"]);
    }),
  );

  // `initializationOptions` is only read at startup, so restart the server when
  // a setting that feeds it changes (the flag controls the diagnostic scope).
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("flux.workspaceDiagnostics.scanAllFiles")) {
        client?.restart().catch((err) => {
          vscode.window.showErrorMessage(`Failed to restart flux-lsp: ${err}`);
        });
      }
    }),
  );

  context.subscriptions.push({
    dispose: () => {
      client?.stop();
    },
  });
}

/** Shared terminal so repeated runs reuse one panel instead of stacking up. */
let runTerminal: vscode.Terminal | undefined;

/**
 * Launch the Flux CLI on the file at `uriArg` with `extraArgs` appended, in an
 * integrated terminal rooted at the file's workspace folder. The base command
 * is `flux.runCommand` (default `cargo run --`); the file path is passed
 * relative to the workspace folder when possible.
 */
function runFlux(uriArg: string, extraArgs: string[]) {
  const uri = vscode.Uri.parse(uriArg);
  const folder = vscode.workspace.getWorkspaceFolder(uri);
  const cwd = folder?.uri.fsPath;
  const filePath = folder
    ? vscode.workspace.asRelativePath(uri, false)
    : uri.fsPath;
  const runner = vscode.workspace
    .getConfiguration("flux")
    .get<string>("runCommand", "cargo run --")
    .trim();

  const parts = [runner, quoteArg(filePath), ...extraArgs.map(quoteArg)];
  const command = parts.join(" ");

  if (!runTerminal || runTerminal.exitStatus !== undefined) {
    runTerminal = vscode.window.createTerminal({ name: "Flux Run", cwd });
  }
  runTerminal.show();
  runTerminal.sendText(command);
}

/** Quote an argument for the shell only when it contains whitespace. */
function quoteArg(arg: string): string {
  return /\s/.test(arg) ? `"${arg}"` : arg;
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

/**
 * Resolution order:
 *   1. `flux.serverPath` setting if the user set one.
 *   2. The binary bundled inside the .vsix at `<ext>/server/flux-lsp{.exe}`.
 *   3. `flux-lsp` on PATH (for dev builds where you `cargo install`ed manually).
 */
function resolveServerPath(context: vscode.ExtensionContext): string {
  const override = vscode.workspace
    .getConfiguration("flux")
    .get<string>("serverPath", "")
    .trim();
  if (override.length > 0) {
    return override;
  }

  const bundled = path.join(context.extensionPath, "server", BINARY_NAME);
  if (fs.existsSync(bundled)) {
    return bundled;
  }

  return "flux-lsp";
}
