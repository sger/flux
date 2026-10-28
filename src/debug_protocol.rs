//! What a debug adapter and `flux debug --engine` say to each other
//! (proposal 0188 S4).
//!
//! `flux-dap` speaks the Debug Adapter Protocol to the editor and starts the
//! program as `flux debug <file> --engine <address>`. The child connects to
//! that address and the two exchange one JSON object per line:
//!
//! - the adapter sends [`RequestMessage`]s, each with an `id`;
//! - the engine answers each with a [`EngineMessage::Reply`] or
//!   [`EngineMessage::Error`] carrying the same `id`, and sends
//!   [`EngineMessage::Event`]s of its own: ready to take breakpoints, and
//!   stopped.
//!
//! While the program runs, the engine reads requests at the next stop site:
//! breakpoint changes are applied there, and a pause stops there.

use serde::{Deserialize, Serialize};

/// A request from the adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMessage {
    pub id: u64,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// The breakpoints of one file, replacing the ones it had. Answered with
    /// [`Reply::Breakpoints`].
    SetBreakpoints { file: String, lines: Vec<usize> },
    /// Run the program, stopping at its first line when asked.
    Start { stop_on_entry: bool },
    /// Go on from a stop.
    Resume { how: ResumeKind },
    /// Stop at the next stop site.
    Pause,
    /// The call stack of a thread, newest first: the one stopped unless
    /// `thread` names another. Answered with [`Reply::Stack`].
    Stack {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<u64>,
    },
    /// The variables of a frame's scope, or the parts of one of them, named
    /// by a path: a variable, then indexes into its parts. Answered with
    /// [`Reply::Variables`]. The frame is in `thread`'s stack.
    Variables {
        frame: usize,
        scope: Scope,
        path: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<u64>,
    },
    /// A variable or a part of one, written `name.0.1`. Answered with
    /// [`Reply::Value`].
    Evaluate {
        frame: usize,
        expression: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<u64>,
    },
    /// How many references the value at `expression` (`xs`, `pair.0`) has,
    /// the debugger's own left out (0188 S10b). Answered with
    /// [`Reply::Refs`].
    Refs {
        frame: usize,
        expression: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<u64>,
    },
    /// The program's threads: its fibers and tasks (0188 S8). Answered
    /// with [`Reply::Threads`].
    Threads,
    /// Show or hide compiler-made frames and variables.
    SetInternals { show: bool },
    /// The runtime errors to stop at, by [`ExceptionInfo::kind`] id:
    /// `panic`, `division_by_zero`, `unhandled_effect`, `fiber_failure`,
    /// `runtime_error`.
    SetExceptionFilters { filters: Vec<String> },
    /// The effects the program has performed. Answered with
    /// [`Reply::Events`].
    Events,
    /// Where the program is in its run. Answered with [`Reply::Position`].
    Where,
    /// End the program.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeKind {
    Continue,
    StepIn,
    StepOver,
    StepOut,
    ReverseStep,
    ReverseNext,
    ReverseFinish,
    ReverseContinue,
    /// To a position, earlier or later.
    Goto(u64),
    /// To the last stop before effect `n`.
    Event(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Locals,
    Captured,
    /// A local, else a captured value: how an expression names a variable.
    Any,
    /// The active effect handlers, innermost first, each named by its
    /// effect, with a parameterized handler's state as its value.
    Handlers,
}

/// What the engine sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineMessage {
    Reply { id: u64, reply: Reply },
    Error { id: u64, message: String },
    Event { event: Event },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    /// For each line asked for, the line the breakpoint was set at (the
    /// next line with code), or none.
    Breakpoints {
        lines: Vec<Option<usize>>,
    },
    Stack {
        frames: Vec<Frame>,
    },
    Variables {
        variables: Vec<Variable>,
    },
    Value {
        variable: Variable,
    },
    Events {
        events: Vec<TapeEvent>,
    },
    Position {
        position: u64,
    },
    Threads {
        threads: Vec<Thread>,
    },
    /// A value's references: none for a value that is not reference
    /// counted, such as an Int.
    Refs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<u64>,
        type_name: String,
    },
}

/// How `refs <path>` reads, on every surface (0188 S10b): the count, and
/// whether the value is unique, so the compiled code can reuse it in place.
/// `across_threads` is native's count shared between threads.
pub fn describe_refs(
    path: &str,
    count: Option<u64>,
    across_threads: bool,
    type_name: &str,
) -> String {
    match count {
        None => format!("{path}: not reference counted ({type_name})"),
        Some(count) => {
            let noun = if count == 1 { "reference" } else { "references" };
            let how = if across_threads {
                "shared across threads"
            } else if count == 1 {
                "unique: can be reused in place"
            } else {
                "shared"
            };
            format!("{path}: {count} {noun} ({how})")
        }
    }
}

/// A thread of the paused program: a fiber, or a task (0188 S8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: u64,
    pub name: String,
    /// `stopped`, `ready`, `not started`, `waiting channel #1 recv`, ...
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The program is compiled and has not started: breakpoints can be set.
    Ready,
    Stopped {
        reason: StopKind,
        position: u64,
        /// More to say, such as "start of the recording".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// The runtime error an `exception` stop is at.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exception: Option<ExceptionInfo>,
        /// The thread that stopped: 1, the program, or a fiber (S8).
        #[serde(default = "main_thread", skip_serializing_if = "is_main_thread")]
        thread: u64,
    },
}

fn main_thread() -> u64 {
    1
}

fn is_main_thread(thread: &u64) -> bool {
    *thread == 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Entry,
    Breakpoint,
    Step,
    Pause,
    Exception,
}

/// A runtime error the program stopped at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionInfo {
    /// `panic`, `division_by_zero`, `unhandled_effect`, `fiber_failure` or
    /// `runtime_error`.
    pub kind: String,
    pub message: String,
}

/// What a line of the stack is (0188 S7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// A frame running, or waiting for a call to return.
    #[default]
    Live,
    /// A frame a perform took off the stack, waiting for `resume`.
    Suspended,
    /// A frame a perform took off the stack for a handler that never
    /// resumes.
    Dropped,
    /// Not a frame: `handling Effect.op — performed at file:line:col`, as
    /// its name.
    Label,
}

impl FrameKind {
    fn is_live(&self) -> bool {
        *self == FrameKind::Live
    }
}

/// One frame of the call stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// The frame's place on the stack, counted from the bottom, which
    /// [`Request::Variables`] takes.
    pub index: usize,
    pub name: String,
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
    /// Compiler-made: shown only with internals on.
    pub artificial: bool,
    /// Calls this frame no longer shows because tail calls replaced them,
    /// and the latest of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elided: Option<(u64, String)>,
    #[serde(default, skip_serializing_if = "FrameKind::is_live")]
    pub kind: FrameKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    /// It has parts, listed by asking for its path.
    pub has_children: bool,
    pub artificial: bool,
}

/// An effect on the session's recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeEvent {
    pub index: u64,
    pub text: String,
    /// The position it happened at, once the run has reached it.
    pub position: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refs_request_and_its_reply_cross_as_json() {
        let request = Request::Refs { frame: 0, expression: "pair.0".to_string(), thread: None };
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(line, r#"{"kind":"refs","frame":0,"expression":"pair.0"}"#);
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
        for reply in [
            Reply::Refs { count: Some(2), type_name: "List".to_string() },
            Reply::Refs { count: None, type_name: "Int".to_string() },
        ] {
            let line = serde_json::to_string(&reply).unwrap();
            assert_eq!(serde_json::from_str::<Reply>(&line).unwrap(), reply);
        }
    }

    #[test]
    fn refs_read_the_same_everywhere() {
        assert_eq!(
            describe_refs("xs", Some(1), false, "List"),
            "xs: 1 reference (unique: can be reused in place)"
        );
        assert_eq!(describe_refs("xs", Some(2), false, "List"), "xs: 2 references (shared)");
        assert_eq!(
            describe_refs("xs", Some(3), true, "List"),
            "xs: 3 references (shared across threads)"
        );
        assert_eq!(describe_refs("n", None, false, "Int"), "n: not reference counted (Int)");
    }

    #[test]
    fn messages_are_one_json_object_each() {
        let request =
            RequestMessage { id: 3, request: Request::Resume { how: ResumeKind::Goto(12) } };
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(line, r#"{"id":3,"request":{"kind":"resume","how":{"goto":12}}}"#);
        assert_eq!(serde_json::from_str::<RequestMessage>(&line).unwrap(), request);

        let stopped = EngineMessage::Event {
            event: Event::Stopped {
                reason: StopKind::Breakpoint,
                position: 7,
                description: None,
                exception: None,
                thread: 1,
            },
        };
        let line = serde_json::to_string(&stopped).unwrap();
        assert_eq!(
            line,
            r#"{"type":"event","event":{"kind":"stopped","reason":"breakpoint","position":7}}"#
        );
        assert_eq!(serde_json::from_str::<EngineMessage>(&line).unwrap(), stopped);

        let exception = EngineMessage::Event {
            event: Event::Stopped {
                reason: StopKind::Exception,
                position: 9,
                description: None,
                exception: Some(ExceptionInfo {
                    kind: "panic".to_string(),
                    message: "boom".to_string(),
                }),
                thread: 1,
            },
        };
        let line = serde_json::to_string(&exception).unwrap();
        assert_eq!(
            line,
            r#"{"type":"event","event":{"kind":"stopped","reason":"exception","position":9,"exception":{"kind":"panic","message":"boom"}}}"#
        );
        assert_eq!(serde_json::from_str::<EngineMessage>(&line).unwrap(), exception);

        let frames = Reply::Stack {
            frames: vec![
                Frame {
                    index: 2,
                    name: "handling Flip.flip — performed at app.flx:6:13".to_string(),
                    file: None,
                    line: 0,
                    column: 0,
                    artificial: false,
                    elided: None,
                    kind: FrameKind::Label,
                },
                Frame {
                    index: 1,
                    name: "decide".to_string(),
                    file: Some("app.flx".to_string()),
                    line: 6,
                    column: 13,
                    artificial: false,
                    elided: None,
                    kind: FrameKind::Live,
                },
            ],
        };
        let line = serde_json::to_string(&frames).unwrap();
        assert!(line.contains(r#""kind":"label""#), "{line}");
        assert_eq!(line.matches(r#""kind":"#).count(), 2, "live is the default: {line}");
        assert_eq!(serde_json::from_str::<Reply>(&line).unwrap(), frames);

        let fiber = EngineMessage::Event {
            event: Event::Stopped {
                reason: StopKind::Breakpoint,
                position: 3,
                description: None,
                exception: None,
                thread: 2,
            },
        };
        let line = serde_json::to_string(&fiber).unwrap();
        assert!(line.ends_with(r#""thread":2}}"#), "{line}");
        assert_eq!(serde_json::from_str::<EngineMessage>(&line).unwrap(), fiber);

        let stack = RequestMessage { id: 4, request: Request::Stack { thread: None } };
        let line = serde_json::to_string(&stack).unwrap();
        assert_eq!(line, r#"{"id":4,"request":{"kind":"stack"}}"#);
        assert_eq!(serde_json::from_str::<RequestMessage>(&line).unwrap(), stack);

        let threads = Reply::Threads {
            threads: vec![Thread {
                id: 2,
                name: "worker".to_string(),
                state: "waiting sleep 30 ms".to_string(),
            }],
        };
        let line = serde_json::to_string(&threads).unwrap();
        assert_eq!(serde_json::from_str::<Reply>(&line).unwrap(), threads);

        let reply = EngineMessage::Reply {
            id: 1,
            reply: Reply::Breakpoints { lines: vec![Some(4), None] },
        };
        let line = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<EngineMessage>(&line).unwrap(), reply);
    }
}
