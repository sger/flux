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

  context.subscriptions.push({
    dispose: () => {
      client?.stop();
    },
  });
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
