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
    /// The call stack, newest first. Answered with [`Reply::Stack`].
    Stack,
    /// The variables of a frame's scope, or the parts of one of them, named
    /// by a path: a variable, then indexes into its parts. Answered with
    /// [`Reply::Variables`].
    Variables {
        frame: usize,
        scope: Scope,
        path: Vec<String>,
    },
    /// A variable or a part of one, written `name.0.1`. Answered with
    /// [`Reply::Value`].
    Evaluate { frame: usize, expression: String },
    /// Show or hide compiler-made frames and variables.
    SetInternals { show: bool },
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
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Entry,
    Breakpoint,
    Step,
    Pause,
}

/// One frame of the call stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// The VM frame index, which [`Request::Variables`] takes.
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
    fn messages_are_one_json_object_each() {
        let request = RequestMessage {
            id: 3,
            request: Request::Resume {
                how: ResumeKind::Goto(12),
            },
        };
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(
            line,
            r#"{"id":3,"request":{"kind":"resume","how":{"goto":12}}}"#
        );
        assert_eq!(
            serde_json::from_str::<RequestMessage>(&line).unwrap(),
            request
        );

        let stopped = EngineMessage::Event {
            event: Event::Stopped {
                reason: StopKind::Breakpoint,
                position: 7,
                description: None,
            },
        };
        let line = serde_json::to_string(&stopped).unwrap();
        assert_eq!(
            line,
            r#"{"type":"event","event":{"kind":"stopped","reason":"breakpoint","position":7}}"#
        );
        assert_eq!(
            serde_json::from_str::<EngineMessage>(&line).unwrap(),
            stopped
        );

        let reply = EngineMessage::Reply {
            id: 1,
            reply: Reply::Breakpoints {
                lines: vec![Some(4), None],
            },
        };
        let line = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<EngineMessage>(&line).unwrap(), reply);
    }
}
