import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext) {
  const config = vscode.workspace.getConfiguration("flux");
  const serverPath = config.get<string>("serverPath", "flux-lsp");

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
        `Set "flux.serverPath" to an absolute path, or run \`cargo install --path crates/flux-lsp\`.`,
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
