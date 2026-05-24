import { exec } from "child_process";
import * as fs from "fs";
import * as path from "path";
import { promisify } from "util";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

const execAsync = promisify(exec);

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

  // The "▶ Eval" lens (handlers::code_lens) on a `/// >>> <expr>` line. Unlike
  // the run/test lenses (which fire into a terminal), eval needs the *captured*
  // result, so it spawns `flux eval "<expr>"` as a child process and splices the
  // output back into the buffer as a `/// => <result>` line.
  context.subscriptions.push(
    vscode.commands.registerCommand(
      "flux.evalComment",
      (uriArg: string, expr: string, line: number, indent: string) => {
        void evalComment(uriArg, expr, line, indent);
      },
    ),
  );

  // Compiler-stage inspection: ask the server to render the token stream / Core
  // IR / bytecode of the active `.flx` file, and open the result beside it (the
  // Flux analogue of rust-analyzer's "View HIR/MIR").
  context.subscriptions.push(
    vscode.commands.registerCommand("flux.viewTokens", () =>
      viewStage("flux/viewTokens", "View Tokens"),
    ),
    vscode.commands.registerCommand("flux.viewCoreIr", () =>
      viewStage("flux/viewCoreIr", "View Core IR"),
    ),
    vscode.commands.registerCommand("flux.viewBytecode", () =>
      viewStage("flux/viewBytecode", "View Bytecode"),
    ),
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

/**
 * Send a custom `flux/view*` request for the active `.flx` document and open the
 * returned dump in a scratch editor beside the source.
 */
async function viewStage(method: string, title: string) {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "flux") {
    vscode.window.showInformationMessage(`Flux: ${title} needs an open .flx file.`);
    return;
  }
  if (!client) {
    vscode.window.showErrorMessage("Flux: language server is not running.");
    return;
  }
  try {
    const content = await client.sendRequest<string>(method, {
      uri: editor.document.uri.toString(),
    });
    const doc = await vscode.workspace.openTextDocument({
      content,
      language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, {
      viewColumn: vscode.ViewColumn.Beside,
      preview: true,
      preserveFocus: true,
    });
  } catch (err) {
    vscode.window.showErrorMessage(`Flux: ${title} failed: ${err}`);
  }
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

/** How long an `flux eval` subprocess may run before it's killed (ms). */
const EVAL_TIMEOUT_MS = 10000;

/**
 * Run `<runCommand> eval "<expr>"` for the `▶ Eval` lens and write the result
 * back as a `<indent>/// => <result>` comment line just below `line`. Evaluation
 * runs in a subprocess (through the shell, so a bare `cargo`/`flux` resolves on
 * PATH) so user code is isolated from the editor and a runaway expression is
 * bounded by the timeout. Only the expression is interpolated, and it is quoted
 * for the host shell, so embedded quotes in `>>> print("hi")` are safe.
 */
async function evalComment(
  uriArg: string,
  expr: string,
  line: number,
  indent: string,
) {
  const uri = vscode.Uri.parse(uriArg);
  const folder = vscode.workspace.getWorkspaceFolder(uri);
  const cwd = folder?.uri.fsPath ?? path.dirname(uri.fsPath);
  const runner = vscode.workspace
    .getConfiguration("flux")
    .get<string>("runCommand", "cargo run --")
    .trim();
  const command = `${runner} eval ${shellQuote(expr)}`;

  let result: string;
  try {
    const { stdout } = await execAsync(command, {
      cwd,
      timeout: EVAL_TIMEOUT_MS,
      windowsHide: true,
    });
    result = formatEvalResult(stdout) || "()";
  } catch (err) {
    const e = err as { killed?: boolean; stderr?: string; message?: string };
    result = e.killed
      ? "timed out"
      : formatEvalResult(e.stderr ?? "") ||
        formatEvalResult(e.message ?? "") ||
        "error";
  }
  await applyEvalResult(uri, line, indent, result);
}

/** Quote a single argument for the host shell (cmd.exe on Windows, sh elsewhere). */
function shellQuote(arg: string): string {
  if (process.platform === "win32") {
    return `"${arg.replace(/"/g, '\\"')}"`;
  }
  return `'${arg.replace(/'/g, "'\\''")}'`;
}

/** Trim captured output and collapse it to a single comment-safe line. */
function formatEvalResult(text: string): string {
  return text.trim().replace(/\s*\r?\n\s*/g, " ");
}

/**
 * Insert or refresh the `<indent>/// => <result>` line directly below the `>>>`
 * line. If the next line is already an `=>` result line we replace it (so
 * re-running is idempotent); otherwise we insert a fresh one.
 */
async function applyEvalResult(
  uri: vscode.Uri,
  line: number,
  indent: string,
  result: string,
) {
  const doc = await vscode.workspace.openTextDocument(uri);
  const newText = `${indent}/// => ${result}`;
  const edit = new vscode.WorkspaceEdit();
  const resultLineNo = line + 1;
  const hasResultLine =
    resultLineNo < doc.lineCount &&
    /^\s*\/\/\/ =>/.test(doc.lineAt(resultLineNo).text);
  if (hasResultLine) {
    edit.replace(uri, doc.lineAt(resultLineNo).range, newText);
  } else {
    edit.insert(uri, doc.lineAt(line).range.end, `\n${newText}`);
  }
  await vscode.workspace.applyEdit(edit);
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
