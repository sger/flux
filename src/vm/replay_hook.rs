//! Running a primop through the effect tape (proposal 0188 S1).
//!
//! `execute_primop_opcode` sends every primop here while a tape is active.
//! Deterministic primops run as usual; effects are recorded or answered from
//! the tape; async primops run as usual, the async backend recording what
//! they wait on; tasks, which this stage cannot record, stop the run.

use std::io::Write;

use crate::core::CorePrimOp;
use crate::diagnostics::position::{Position, Span};
use crate::runtime::replay::{self, EffectRequest, ReplayMode, TapeBytes, TapeClass, TapeError};
use crate::runtime::value::Value;

use super::VM;
use super::core_dispatch::{execute_core_primop, render_console};
use super::replay_codec;

impl VM {
    // Kept out of line: the primop dispatch it hangs off is hot, and the tape
    // is off in almost every run.
    #[cold]
    #[inline(never)]
    pub(super) fn execute_taped_primop(
        &mut self,
        op: CorePrimOp,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        match replay::tape_class(op) {
            TapeClass::Untaped | TapeClass::Scheduled => execute_core_primop(self, op, args),
            TapeClass::Refused if replay::is_implicit() => {
                // A debug session's own recording: drop it rather than the run.
                replay::stop_implicit();
                super::debug::turn_off_reverse(format!(
                    "the program used `{}`, which cannot be recorded yet (0188 S8)",
                    replay_codec::op_name(op)
                ));
                execute_core_primop(self, op, args)
            }
            TapeClass::Refused => self.tape_fail(replay::unsupported(replay_codec::op_name(op))),
            TapeClass::Console => {
                let text = render_console(op, &args);
                let to_stderr = op == CorePrimOp::DebugTrace;
                let request = if to_stderr {
                    EffectRequest::Stderr {
                        text: TapeBytes::from(text.as_str()),
                    }
                } else {
                    EffectRequest::Stdout {
                        text: TapeBytes::from(text.as_str()),
                    }
                };
                let shown = match replay::console_shown(request) {
                    Ok(shown) => shown,
                    Err(err) => self.tape_fail(err),
                };
                // Printed in every mode, so a replay shows the recorded
                // output, but only once when a debugger runs it again.
                if shown && to_stderr {
                    eprint!("{text}");
                } else if shown {
                    print!("{text}");
                }
                Ok(Value::None)
            }
            TapeClass::Input | TapeClass::Output => {
                let request = replay_codec::request(op, &args)?;
                let outcome = replay::effect(request, || {
                    replay_codec::outcome(op, execute_core_primop(self, op, args))
                });
                match outcome {
                    Ok(outcome) => replay_codec::to_value(outcome),
                    Err(err) => self.tape_fail(err),
                }
            }
            TapeClass::Time => {
                // The callback runs in both modes: its own effects are on the
                // tape, ahead of the elapsed time this event records.
                let outcome = if replay::mode() == ReplayMode::Replay {
                    if let Some(callback) = args.into_iter().next() {
                        self.invoke_value(callback, vec![])
                            .map_err(|e| format!("time: callback error: {e}"))?;
                    }
                    replay::effect(EffectRequest::Time, || {
                        unreachable!("replay answers `time` from the tape")
                    })
                } else {
                    replay::effect(EffectRequest::Time, || {
                        replay_codec::outcome(op, execute_core_primop(self, op, args))
                    })
                };
                match outcome {
                    Ok(outcome) => replay_codec::to_value(outcome),
                    Err(err) => self.tape_fail(err),
                }
            }
        }
    }

    /// Where the program performed the current effect: the innermost named
    /// function with a source position. Effects run inside generated code
    /// (the `Flow.Primops` wrapper, `main`'s default handlers) that has none.
    fn effect_site(&self) -> Option<(String, Span)> {
        self.frames[..=self.frame_index]
            .iter()
            .rev()
            .find_map(|frame| {
                let info = frame.closure.function.debug_info.as_ref()?;
                info.name.as_ref()?;
                let location = info.location_at(frame.ip)?;
                if location.span.start.line == 0 {
                    return None;
                }
                let file = info.file_for(location.file_id)?;
                Some((file.to_string(), location.span))
            })
    }

    /// Stops the run on a tape error. A diverged replay no longer reproduces
    /// the recorded run, so this is not an error the program may catch: it is
    /// reported where the effect happened and the process exits.
    fn tape_fail(&self, err: TapeError) -> ! {
        let (file, span) = self
            .effect_site()
            .or_else(|| self.current_location())
            .unwrap_or_else(|| {
                (
                    String::from("<unknown>"),
                    Span::new(Position::default(), Position::default()),
                )
            });
        let rendered = self.render_runtime_diagnostic(&err.to_diagnostic(&file, Some(span)), &file);
        // A recording that stops here still ends with how it ended.
        let _ = replay::finish(true, Some(err.message()));
        let _ = std::io::stdout().flush();
        eprintln!("{rendered}");
        std::process::exit(1);
    }
}
