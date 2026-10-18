// The Flux Timeline (proposal 0188 S9b): the effects a debugged run has
// performed or will replay, from its tape, in the Run and Debug view.
// Clicking one goes there: on the VM, to the last stop before it, earlier or
// later; natively, forward to it.
//
// The list comes from `flux-dap`'s `fluxEvents` request and the move from
// `fluxGoToEvent`, the same on both backends.

import * as vscode from "vscode";

interface TapeEvent {
  index: number;
  text: string;
  position?: number | null;
  reached: boolean;
  next?: boolean;
}

interface Events {
  events: TapeEvent[];
  message?: string;
}

class EventItem extends vscode.TreeItem {
  constructor(event: TapeEvent) {
    super(`#${event.index} ${event.text}`, vscode.TreeItemCollapsibleState.None);
    if (event.next) {
      this.description = "next";
      this.iconPath = new vscode.ThemeIcon("debug-stackframe");
    } else if (event.reached) {
      this.description = event.position != null ? `position ${event.position}` : "done";
      this.iconPath = new vscode.ThemeIcon("pass-filled");
    } else {
      this.description = "not reached";
      this.iconPath = new vscode.ThemeIcon("circle-outline");
    }
    this.tooltip = `Go to effect #${event.index}: ${event.text}`;
    this.command = {
      command: "flux.timeline.goTo",
      title: "Go to this effect",
      arguments: [event.index],
    };
  }
}

class Timeline implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  private items: vscode.TreeItem[] = [];

  /** Asks the stopped session for its tape, and shows it. */
  async refresh(session: vscode.DebugSession | undefined): Promise<void> {
    if (!session || session.type !== "flux") {
      this.items = [];
    } else {
      try {
        const answer: Events = await session.customRequest("fluxEvents");
        this.items = answer.events.map((event) => new EventItem(event));
        if (this.items.length === 0) {
          this.items = [note(answer.message || "No effects yet.")];
        }
      } catch (error) {
        this.items = [note(error instanceof Error ? error.message : String(error))];
      }
    }
    this.changed.fire();
  }

  clear(): void {
    this.items = [];
    this.changed.fire();
  }

  getTreeItem(item: vscode.TreeItem): vscode.TreeItem {
    return item;
  }

  getChildren(): vscode.TreeItem[] {
    return this.items;
  }
}

function note(text: string): vscode.TreeItem {
  const item = new vscode.TreeItem(text, vscode.TreeItemCollapsibleState.None);
  item.iconPath = new vscode.ThemeIcon("info");
  return item;
}

export function registerTimeline(context: vscode.ExtensionContext) {
  const timeline = new Timeline();
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("flux.timeline", timeline),
    // Refreshed at every stop, when the session can answer.
    vscode.debug.registerDebugAdapterTrackerFactory("flux", {
      createDebugAdapterTracker(session) {
        return {
          onDidSendMessage(message) {
            if (message.type === "event" && message.event === "stopped") {
              void timeline.refresh(session);
            }
          },
        };
      },
    }),
    vscode.debug.onDidTerminateDebugSession((session) => {
      if (session.type === "flux") {
        timeline.clear();
      }
    }),
    vscode.commands.registerCommand("flux.timeline.refresh", () =>
      timeline.refresh(vscode.debug.activeDebugSession),
    ),
    vscode.commands.registerCommand("flux.timeline.goTo", async (index: number) => {
      const session = vscode.debug.activeDebugSession;
      if (!session || session.type !== "flux") {
        return;
      }
      try {
        await session.customRequest("fluxGoToEvent", { index });
      } catch (error) {
        void vscode.window.showInformationMessage(
          error instanceof Error ? error.message : String(error),
        );
      }
    }),
  );
}
