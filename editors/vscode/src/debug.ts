// The Flux debugger (proposal 0188 S4, S6): VS Code starts `flux-dap`, which
// speaks the Debug Adapter Protocol and runs the program under
// `flux debug --engine`, or, with `"backend": "native"`, builds it with `-g`
// and runs it under `lldb-dap`.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

const DAP_BINARY = process.platform === "win32" ? "flux-dap.exe" : "flux-dap";

export function registerDebugger(context: vscode.ExtensionContext) {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory("flux", {
      createDebugAdapterDescriptor() {
        return new vscode.DebugAdapterExecutable(resolveDapPath(context));
      },
    }),
    vscode.debug.registerDebugConfigurationProvider("flux", new FluxConfigurations()),
    vscode.debug.registerDebugConfigurationProvider(
      "flux",
      {
        provideDebugConfigurations() {
          return [
            {
              type: "flux",
              request: "launch",
              name: "Debug current Flux file",
              program: "${file}",
            },
          ];
        },
      },
      vscode.DebugConfigurationProviderTriggerKind.Dynamic,
    ),
  );
}

/** Fills in what a launch configuration leaves out. */
class FluxConfigurations implements vscode.DebugConfigurationProvider {
  resolveDebugConfiguration(
    _folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration,
  ): vscode.DebugConfiguration | undefined {
    // F5 with no launch.json: debug the open Flux file.
    if (!config.type && !config.request && !config.name) {
      const editor = vscode.window.activeTextEditor;
      if (editor?.document.languageId !== "flux") {
        vscode.window.showInformationMessage("Open a .flx file to debug it.");
        return undefined;
      }
      config.type = "flux";
      config.request = "launch";
      config.name = "Debug current Flux file";
      config.program = "${file}";
    }
    if (!config.program) {
      vscode.window.showErrorMessage(
        "The Flux launch configuration needs a `program`.",
      );
      return undefined;
    }
    if (config.backend === "native" && !config.lldbDap) {
      const lldbDap = vscode.workspace
        .getConfiguration("flux")
        .get<string>("lldbDapPath", "")
        .trim();
      if (lldbDap.length > 0) {
        config.lldbDap = lldbDap;
      }
    }
    if (!config.fluxCommand) {
      const runCommand = vscode.workspace
        .getConfiguration("flux")
        .get<string>("runCommand", "cargo run --")
        .trim();
      config.fluxCommand = runCommand.split(/\s+/).filter((part) => part);
    }
    return config;
  }
}

/**
 * Resolution order, as for flux-lsp:
 *   1. `flux.dapPath` setting if the user set one.
 *   2. The binary bundled inside the .vsix at `<ext>/server/flux-dap{.exe}`.
 *   3. `flux-dap` on PATH.
 */
function resolveDapPath(context: vscode.ExtensionContext): string {
  const override = vscode.workspace
    .getConfiguration("flux")
    .get<string>("dapPath", "")
    .trim();
  if (override.length > 0) {
    return override;
  }
  const bundled = path.join(context.extensionPath, "server", DAP_BINARY);
  if (fs.existsSync(bundled)) {
    return bundled;
  }
  return "flux-dap";
}
