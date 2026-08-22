use crate::{
    cfg::{IrBinaryOp, IrCallTarget, IrConst, IrExpr, IrInstr, IrMetadata, IrStringPart, IrVar},
    core::{CoreExpr, CoreLit, CorePrimOp},
    diagnostics::position::Span,
};

/// Map a promoted primop back to its original function name for CFG lowering.
///
/// The CFG/bytecode path doesn't benefit from the promotion — the bytecode
/// compiler already dispatches these names via `resolve_primop_call` and
/// `OpCallBase`.  This function reverses the promotion so the CFG IR emits a
/// normal named call.
fn promoted_primop_name(op: &CorePrimOp) -> &'static str {
    match op {
        CorePrimOp::Print => "print",
        CorePrimOp::Println => "println",
        CorePrimOp::DebugTrace => "__primop_debug_trace",
        CorePrimOp::ReadFile => "read_file",
        CorePrimOp::TryReadFile => "try_read_file",
        CorePrimOp::FsExists => "fs_exists",
        CorePrimOp::FsIsDir => "fs_is_dir",
        CorePrimOp::FsIsFile => "fs_is_file",
        CorePrimOp::FsCreateDirAll => "fs_create_dir_all",
        CorePrimOp::FsRemoveFile => "fs_remove_file",
        CorePrimOp::FsRemoveDirAll => "fs_remove_dir_all",
        CorePrimOp::FsWriteFile => "fs_write_file",
        CorePrimOp::FsRename => "fs_rename",
        CorePrimOp::FsListDir => "fs_list_dir",
        CorePrimOp::FsMetadata => "fs_metadata",
        CorePrimOp::Sha256 => "sha256",
        CorePrimOp::Sha256File => "sha256_file",
        CorePrimOp::EnvVar => "env_var",
        CorePrimOp::EnvArgs => "env_args",
        CorePrimOp::EnvCwd => "env_cwd",
        CorePrimOp::EnvHomeDir => "env_home_dir",
        CorePrimOp::ProcRun => "proc_run",
        CorePrimOp::WriteFile => "write_file",
        CorePrimOp::ReadStdin => "read_stdin",
        CorePrimOp::ReadLines => "read_lines",
        CorePrimOp::StringLength => "string_length",
        CorePrimOp::StringConcat => "string_concat",
        CorePrimOp::StringSlice => "string_slice",
        CorePrimOp::ToString => "to_string",
        CorePrimOp::Split => "split",
        CorePrimOp::Trim => "trim",
        CorePrimOp::Upper => "upper",
        CorePrimOp::Lower => "lower",
        CorePrimOp::Replace => "replace",
        CorePrimOp::Substring => "substring",
        CorePrimOp::ArrayLen => "array_len",
        CorePrimOp::ArrayGet => "array_get",
        CorePrimOp::ArraySet => "array_set",
        CorePrimOp::ArrayPush => "array_push",
        CorePrimOp::ArrayConcat => "array_concat",
        CorePrimOp::ArraySlice => "array_slice",
        CorePrimOp::HamtGet => "get",
        CorePrimOp::HamtSet => "put",
        CorePrimOp::HamtDelete => "delete",
        CorePrimOp::HamtKeys => "keys",
        CorePrimOp::HamtValues => "values",
        CorePrimOp::HamtMerge => "merge",
        CorePrimOp::HamtSize => "size",
        CorePrimOp::HamtContains => "has_key",
        CorePrimOp::TypeOf => "type_of",
        CorePrimOp::IsInt => "is_int",
        CorePrimOp::IsFloat => "is_float",
        CorePrimOp::IsString => "is_string",
        CorePrimOp::IsBool => "is_bool",
        CorePrimOp::IsArray => "is_array",
        CorePrimOp::IsNone => "is_none",
        CorePrimOp::IsSome => "is_some",
        CorePrimOp::IsList => "is_list",
        CorePrimOp::IsMap => "is_map",
        CorePrimOp::CmpEq => "cmp_eq",
        CorePrimOp::CmpNe => "cmp_ne",
        CorePrimOp::Panic => "panic",
        CorePrimOp::Unwrap => "unwrap",
        CorePrimOp::SafeDiv => "safe_div",
        CorePrimOp::SafeMod => "safe_mod",
        CorePrimOp::ClockNow => "now_ms",
        CorePrimOp::Time => "time",
        CorePrimOp::Try => "try",
        CorePrimOp::AssertThrows => "assert_throws",
        CorePrimOp::ParseInt => "parse_int",
        CorePrimOp::Abs => "abs",
        CorePrimOp::FSqrt => "sqrt",
        CorePrimOp::FSin => "sin",
        CorePrimOp::FCos => "cos",
        CorePrimOp::FExp => "exp",
        CorePrimOp::FLog => "log",
        CorePrimOp::FFloor => "floor",
        CorePrimOp::FCeil => "ceil",
        CorePrimOp::FRound => "round",
        CorePrimOp::FTan => "tan",
        CorePrimOp::FAsin => "asin",
        CorePrimOp::FAcos => "acos",
        CorePrimOp::FAtan => "atan",
        CorePrimOp::FSinh => "sinh",
        CorePrimOp::FCosh => "cosh",
        CorePrimOp::FTanh => "tanh",
        CorePrimOp::FTruncate => "truncate",
        CorePrimOp::BitAnd => "bit_and",
        CorePrimOp::BitOr => "bit_or",
        CorePrimOp::BitXor => "bit_xor",
        CorePrimOp::BitShl => "bit_shl",
        CorePrimOp::BitShr => "bit_shr",
        CorePrimOp::Min => "min",
        CorePrimOp::Max => "max",
        CorePrimOp::Len => "len",
        CorePrimOp::TaskSpawn => "task_spawn",
        CorePrimOp::TaskSpawnMove => "task_spawn_move",
        CorePrimOp::TaskSpawnScoped => "task_spawn_scoped",
        CorePrimOp::TaskSpawnScopedMove => "task_spawn_scoped_move",
        CorePrimOp::TaskBlockingJoin => "task_blocking_join",
        CorePrimOp::TaskCancel => "task_cancel",
        // Fiber primops (proposal 0174 Phase 1b)
        CorePrimOp::FiberSuspend => "fiber_suspend",
        CorePrimOp::FiberFork => "fiber_fork",
        CorePrimOp::FiberGetContext => "fiber_get_context",
        CorePrimOp::FiberFail => "fiber_fail",
        CorePrimOp::TaskAwait => "task_await",
        CorePrimOp::FiberRunAsync => "fiber_run_async",
        CorePrimOp::FiberYieldNow => "fiber_yield_now",
        CorePrimOp::FiberSleep => "fiber_sleep",
        CorePrimOp::TcpConnect => "tcp_connect",
        CorePrimOp::TcpRead => "tcp_read",
        CorePrimOp::TcpWriteAll => "tcp_write_all",
        CorePrimOp::TcpClose => "tcp_close",
        CorePrimOp::TcpListen => "tcp_listen",
        CorePrimOp::TcpAccept => "tcp_accept",
        CorePrimOp::FiberBoth => "fiber_both",
        CorePrimOp::FiberRace => "fiber_race",
        CorePrimOp::FiberTimeout => "fiber_timeout",
        CorePrimOp::FiberNewScope => "fiber_new_scope",
        CorePrimOp::FiberForkScoped => "fiber_fork_scoped",
        CorePrimOp::FiberCancelScope => "fiber_cancel_scope",
        CorePrimOp::FiberCheckCancelled => "fiber_check_cancelled",
        CorePrimOp::FiberRunAsyncWith => "fiber_run_async_with",
        CorePrimOp::FiberFirstOf => "fiber_first_of",
        CorePrimOp::FiberTry => "fiber_try",
        CorePrimOp::FiberCurrentWorkerCount => "fiber_current_worker_count",
        CorePrimOp::HttpServeConfig => "http_serve_config",
        CorePrimOp::HttpShutdown => "http_shutdown",
        CorePrimOp::HttpShutdownNow => "http_shutdown_now",
        CorePrimOp::HttpParseRequest => "http_parse_request",
        CorePrimOp::HttpWriteResponse => "http_write_response",
        CorePrimOp::HttpRegisterConnection => "http_register_connection",
        CorePrimOp::HttpUnregisterConnection => "http_unregister_connection",
        CorePrimOp::HttpActiveConnectionCount => "http_active_connection_count",
        CorePrimOp::HttpIsShuttingDown => "http_is_shutting_down",
        CorePrimOp::HttpServerStopped => "http_server_stopped",
        CorePrimOp::HttpIsServerStopped => "http_is_server_stopped",
        CorePrimOp::HttpParseUrl => "http_parse_url",
        CorePrimOp::HttpWriteRequest => "http_write_request",
        CorePrimOp::HttpParseResponse => "http_parse_response",
        CorePrimOp::JsonParse => "json_parse",
        CorePrimOp::JsonStringify => "json_stringify",
        CorePrimOp::HttpWriteChunkedHead => "http_write_chunked_head",
        CorePrimOp::HttpWriteChunk => "http_write_chunk",
        CorePrimOp::HttpWriteChunkedEnd => "http_write_chunked_end",
        CorePrimOp::ChanMake => "chan_make",
        CorePrimOp::ChanSend => "chan_send",
        CorePrimOp::ChanSendMove => "chan_send_move",
        CorePrimOp::ChanRecv => "chan_recv",
        CorePrimOp::ChanTrySend => "chan_try_send",
        CorePrimOp::ChanTrySendMove => "chan_try_send_move",
        CorePrimOp::ChanTryRecv => "chan_try_recv",
        CorePrimOp::ChanClose => "chan_close",
        CorePrimOp::ChanLen => "chan_len",
        CorePrimOp::ChanCap => "chan_cap",
        CorePrimOp::ChanIsClosed => "chan_is_closed",
        CorePrimOp::EventRecv => "event_recv",
        CorePrimOp::EventSend => "event_send",
        CorePrimOp::EventSendMove => "event_send_move",
        CorePrimOp::EventAfter => "event_after",
        CorePrimOp::EventAlways => "event_always",
        CorePrimOp::EventNever => "event_never",
        CorePrimOp::EventChoose => "event_choose",
        CorePrimOp::EventWrap => "event_wrap",
        CorePrimOp::EventSync => "event_sync",
        CorePrimOp::EventPoll => "event_poll",
        CorePrimOp::EventWait => "event_wait",
        _ => unreachable!("not a promoted primop"),
    }
}

impl<'a> super::fn_ctx::FnCtx<'a> {
    /// Lower a `PrimOp` node.
    pub(super) fn lower_primop(&mut self, op: &CorePrimOp, args: &[CoreExpr], span: Span) -> IrVar {
        let dest = self.ctx.alloc_var();
        let meta = IrMetadata::from_span(span);
        match op {
            CorePrimOp::Add
            | CorePrimOp::Sub
            | CorePrimOp::Mul
            | CorePrimOp::Div
            | CorePrimOp::Mod
            | CorePrimOp::IAdd
            | CorePrimOp::ISub
            | CorePrimOp::IMul
            | CorePrimOp::IDiv
            | CorePrimOp::IMod
            | CorePrimOp::FAdd
            | CorePrimOp::FSub
            | CorePrimOp::FMul
            | CorePrimOp::FDiv
            | CorePrimOp::Eq
            | CorePrimOp::NEq
            | CorePrimOp::Lt
            | CorePrimOp::Le
            | CorePrimOp::Gt
            | CorePrimOp::Ge
            | CorePrimOp::ICmpEq
            | CorePrimOp::ICmpNe
            | CorePrimOp::ICmpLt
            | CorePrimOp::ICmpLe
            | CorePrimOp::ICmpGt
            | CorePrimOp::ICmpGe
            | CorePrimOp::FCmpEq
            | CorePrimOp::FCmpNe
            | CorePrimOp::FCmpLt
            | CorePrimOp::FCmpLe
            | CorePrimOp::FCmpGt
            | CorePrimOp::FCmpGe
            | CorePrimOp::And
            | CorePrimOp::Or
            | CorePrimOp::Concat => {
                let lv = self.lower_expr(&args[0]);
                let rv = self.lower_expr(&args[1]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Binary(primop_to_binop(op), lv, rv),
                    metadata: meta,
                });
            }
            CorePrimOp::Neg => {
                let v = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: "-".to_string(),
                        right: v,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::Not => {
                let v = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: "!".to_string(),
                        right: v,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::Interpolate => {
                let parts: Vec<IrStringPart> = args
                    .iter()
                    .map(|a| IrStringPart::Interpolation(self.lower_expr(a)))
                    .collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::InterpolatedString(parts),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeList => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeList(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeArray => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeArray(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeTuple => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeTuple(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeHash => {
                let pairs: Vec<(IrVar, IrVar)> = args
                    .chunks(2)
                    .map(|chunk| (self.lower_expr(&chunk[0]), self.lower_expr(&chunk[1])))
                    .collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeHash(pairs),
                    metadata: meta,
                });
            }
            CorePrimOp::Index => {
                let left = self.lower_expr(&args[0]);
                let index = self.lower_expr(&args[1]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Index { left, index },
                    metadata: meta,
                });
            }
            // Promoted primops — lower back to named function calls.
            // The bytecode compiler already handles these via
            // resolve_primop_call / OpCallBase dispatch.
            CorePrimOp::Print
            | CorePrimOp::Println
            | CorePrimOp::DebugTrace
            | CorePrimOp::ReadFile
            | CorePrimOp::TryReadFile
            | CorePrimOp::FsExists
            | CorePrimOp::FsIsDir
            | CorePrimOp::FsIsFile
            | CorePrimOp::FsCreateDirAll
            | CorePrimOp::FsRemoveFile
            | CorePrimOp::FsRemoveDirAll
            | CorePrimOp::FsWriteFile
            | CorePrimOp::FsRename
            | CorePrimOp::FsListDir
            | CorePrimOp::FsMetadata
            | CorePrimOp::Sha256
            | CorePrimOp::Sha256File
            | CorePrimOp::EnvVar
            | CorePrimOp::EnvArgs
            | CorePrimOp::EnvCwd
            | CorePrimOp::EnvHomeDir
            | CorePrimOp::ProcRun
            | CorePrimOp::WriteFile
            | CorePrimOp::ReadStdin
            | CorePrimOp::ReadLines
            | CorePrimOp::StringLength
            | CorePrimOp::StringConcat
            | CorePrimOp::StringSlice
            | CorePrimOp::ToString
            | CorePrimOp::Split
            | CorePrimOp::Trim
            | CorePrimOp::Upper
            | CorePrimOp::Lower
            | CorePrimOp::Replace
            | CorePrimOp::Substring
            | CorePrimOp::ArrayLen
            | CorePrimOp::ArrayGet
            | CorePrimOp::ArraySet
            | CorePrimOp::ArrayPush
            | CorePrimOp::ArrayConcat
            | CorePrimOp::ArraySlice
            | CorePrimOp::HamtGet
            | CorePrimOp::HamtSet
            | CorePrimOp::HamtDelete
            | CorePrimOp::HamtKeys
            | CorePrimOp::HamtValues
            | CorePrimOp::HamtMerge
            | CorePrimOp::HamtSize
            | CorePrimOp::HamtContains
            | CorePrimOp::TypeOf
            | CorePrimOp::IsInt
            | CorePrimOp::IsFloat
            | CorePrimOp::IsString
            | CorePrimOp::IsBool
            | CorePrimOp::IsArray
            | CorePrimOp::IsNone
            | CorePrimOp::IsSome
            | CorePrimOp::IsList
            | CorePrimOp::IsMap
            | CorePrimOp::Panic
            | CorePrimOp::Unwrap
            | CorePrimOp::SafeDiv
            | CorePrimOp::SafeMod
            | CorePrimOp::ClockNow
            | CorePrimOp::Time
            | CorePrimOp::ParseInt
            | CorePrimOp::Abs
            | CorePrimOp::FSqrt
            | CorePrimOp::FSin
            | CorePrimOp::FCos
            | CorePrimOp::FExp
            | CorePrimOp::FLog
            | CorePrimOp::FFloor
            | CorePrimOp::FCeil
            | CorePrimOp::FRound
            | CorePrimOp::FTan
            | CorePrimOp::FAsin
            | CorePrimOp::FAcos
            | CorePrimOp::FAtan
            | CorePrimOp::FSinh
            | CorePrimOp::FCosh
            | CorePrimOp::FTanh
            | CorePrimOp::FTruncate
            | CorePrimOp::BitAnd
            | CorePrimOp::BitOr
            | CorePrimOp::BitXor
            | CorePrimOp::BitShl
            | CorePrimOp::BitShr
            | CorePrimOp::Min
            | CorePrimOp::Max
            | CorePrimOp::Len
            | CorePrimOp::CmpEq
            | CorePrimOp::CmpNe
            | CorePrimOp::Try
            | CorePrimOp::AssertThrows
            | CorePrimOp::TaskSpawn
            | CorePrimOp::TaskSpawnMove
            | CorePrimOp::TaskSpawnScoped
            | CorePrimOp::TaskSpawnScopedMove
            | CorePrimOp::TaskBlockingJoin
            | CorePrimOp::TaskCancel
            // Fiber primops (proposal 0174 Phase 1b)
            | CorePrimOp::FiberSuspend
            | CorePrimOp::FiberFork
            | CorePrimOp::FiberGetContext
            | CorePrimOp::FiberFail
            | CorePrimOp::TaskAwait
            | CorePrimOp::FiberRunAsync
            | CorePrimOp::FiberYieldNow
            | CorePrimOp::FiberSleep
            // TCP primops (proposal 0174 Phase 1b-vii)
            | CorePrimOp::TcpConnect
            | CorePrimOp::TcpRead
            | CorePrimOp::TcpWriteAll
            | CorePrimOp::TcpClose
            | CorePrimOp::TcpListen
            | CorePrimOp::TcpAccept
            | CorePrimOp::FiberBoth
            | CorePrimOp::FiberRace
            | CorePrimOp::FiberTimeout
            | CorePrimOp::FiberNewScope
            | CorePrimOp::FiberForkScoped
            | CorePrimOp::FiberCancelScope
            | CorePrimOp::FiberCheckCancelled
            | CorePrimOp::FiberRunAsyncWith
            | CorePrimOp::FiberFirstOf
            | CorePrimOp::FiberTry
            | CorePrimOp::FiberCurrentWorkerCount
            | CorePrimOp::ChanMake
            | CorePrimOp::ChanSend
            | CorePrimOp::ChanSendMove
            | CorePrimOp::ChanRecv
            | CorePrimOp::ChanTrySend
            | CorePrimOp::ChanTrySendMove
            | CorePrimOp::ChanTryRecv
            | CorePrimOp::ChanClose
            | CorePrimOp::ChanLen
            | CorePrimOp::ChanCap
            | CorePrimOp::ChanIsClosed
            | CorePrimOp::EventRecv
            | CorePrimOp::EventSend
            | CorePrimOp::EventSendMove
            | CorePrimOp::EventAfter
            | CorePrimOp::EventAlways
            | CorePrimOp::EventNever
            | CorePrimOp::EventChoose
            | CorePrimOp::EventWrap
            | CorePrimOp::EventSync
            | CorePrimOp::EventPoll
            | CorePrimOp::EventWait
            | CorePrimOp::HttpServeConfig
            | CorePrimOp::HttpShutdown
            | CorePrimOp::HttpShutdownNow
            | CorePrimOp::HttpParseRequest
            | CorePrimOp::HttpWriteResponse
            | CorePrimOp::HttpRegisterConnection
            | CorePrimOp::HttpUnregisterConnection
            | CorePrimOp::HttpActiveConnectionCount
            | CorePrimOp::HttpIsShuttingDown
            | CorePrimOp::HttpServerStopped
            | CorePrimOp::HttpIsServerStopped
            | CorePrimOp::HttpParseUrl
            | CorePrimOp::HttpWriteRequest
            | CorePrimOp::HttpParseResponse
            | CorePrimOp::JsonParse
            | CorePrimOp::JsonStringify
            | CorePrimOp::HttpWriteChunkedHead
            | CorePrimOp::HttpWriteChunk
            | CorePrimOp::HttpWriteChunkedEnd => {
                let name_str = promoted_primop_name(op);
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                // Emit as a named builtin call using the BuiltinCall target
                // which carries the string name directly without interning.
                self.emit(IrInstr::Call {
                    dest,
                    target: IrCallTarget::Builtin(name_str),
                    args: arg_vars,
                    metadata: meta,
                });
            }
            // Effect handler ops — native-only, should never appear in CFG pipeline
            CorePrimOp::EvvGet
            | CorePrimOp::EvvSet
            | CorePrimOp::FreshMarker
            | CorePrimOp::EvvInsert
            | CorePrimOp::YieldTo
            | CorePrimOp::YieldExtend
            | CorePrimOp::YieldPrompt
            | CorePrimOp::IsYielding
            | CorePrimOp::PerformDirect => {
                // These are emitted only by the LIR lowerer for the native backend.
                // Emit a no-op constant for the VM path.
                self.emit(IrInstr::Assign {
                    dest,
                    expr: crate::cfg::IrExpr::None,
                    metadata: meta,
                });
            }
        }
        dest
    }
}

pub(super) fn primop_to_binop(op: &CorePrimOp) -> IrBinaryOp {
    match op {
        // Generic arithmetic
        CorePrimOp::Add | CorePrimOp::Concat => IrBinaryOp::Add,
        CorePrimOp::Sub => IrBinaryOp::Sub,
        CorePrimOp::Mul => IrBinaryOp::Mul,
        CorePrimOp::Div => IrBinaryOp::Div,
        CorePrimOp::Mod => IrBinaryOp::Mod,
        // Typed integer arithmetic — skip the runtime type-dispatch path
        CorePrimOp::IAdd => IrBinaryOp::IAdd,
        CorePrimOp::ISub => IrBinaryOp::ISub,
        CorePrimOp::IMul => IrBinaryOp::IMul,
        CorePrimOp::IDiv => IrBinaryOp::IDiv,
        CorePrimOp::IMod => IrBinaryOp::IMod,
        // Typed float arithmetic
        CorePrimOp::FAdd => IrBinaryOp::FAdd,
        CorePrimOp::FSub => IrBinaryOp::FSub,
        CorePrimOp::FMul => IrBinaryOp::FMul,
        CorePrimOp::FDiv => IrBinaryOp::FDiv,
        // Comparisons and logical
        CorePrimOp::Eq => IrBinaryOp::Eq,
        CorePrimOp::NEq => IrBinaryOp::NotEq,
        CorePrimOp::Lt => IrBinaryOp::Lt,
        CorePrimOp::Le => IrBinaryOp::Le,
        CorePrimOp::Gt => IrBinaryOp::Gt,
        CorePrimOp::Ge => IrBinaryOp::Ge,
        // Typed integer comparisons — map to generic IR comparison ops
        CorePrimOp::ICmpEq => IrBinaryOp::Eq,
        CorePrimOp::ICmpNe => IrBinaryOp::NotEq,
        CorePrimOp::ICmpLt => IrBinaryOp::Lt,
        CorePrimOp::ICmpLe => IrBinaryOp::Le,
        CorePrimOp::ICmpGt => IrBinaryOp::Gt,
        CorePrimOp::ICmpGe => IrBinaryOp::Ge,
        // Typed float comparisons
        CorePrimOp::FCmpEq => IrBinaryOp::Eq,
        CorePrimOp::FCmpNe => IrBinaryOp::NotEq,
        CorePrimOp::FCmpLt => IrBinaryOp::Lt,
        CorePrimOp::FCmpLe => IrBinaryOp::Le,
        CorePrimOp::FCmpGt => IrBinaryOp::Gt,
        CorePrimOp::FCmpGe => IrBinaryOp::Ge,
        CorePrimOp::And => IrBinaryOp::And,
        CorePrimOp::Or => IrBinaryOp::Or,
        _ => unreachable!("not a binary op: {:?}", op),
    }
}

pub(super) fn lower_lit(lit: &CoreLit) -> IrConst {
    match lit {
        CoreLit::Int(n) => IrConst::Int(*n),
        CoreLit::Float(f) => IrConst::Float(*f),
        CoreLit::Bool(b) => IrConst::Bool(*b),
        CoreLit::String(s) => IrConst::String(s.clone()),
        CoreLit::Unit => IrConst::Unit,
    }
}
