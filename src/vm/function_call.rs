use std::{rc::Rc, sync::mpsc, time::Duration};

use crate::bytecode::bytecode::Bytecode;
use crate::diagnostics::NOT_A_FUNCTION;
use crate::runtime::RuntimeContext;
use crate::runtime::value::format_value;
use crate::runtime::{
    r#async::{
        backend::{
            AsyncError, AsyncErrorKind, Completion, CompletionPayload, IoHandle, RequestId,
            RuntimeTarget,
        },
        scheduler::SuspendedContinuation,
        send_value::{SendClosure, SendValue},
        task::{TaskCancelToken, TaskError, TaskHandle, TaskPriority},
    },
    closure::Closure,
    continuation::Continuation,
    frame::Frame,
    value::Value,
};

use super::VM;

// OpPerform instruction size: opcode (1) + const_idx (1) + arity (1) = 3 bytes.
// This constant is used during continuation resume to advance the captured frame's IP past OpPerform.
// We don't need it here since the IP is already advanced during capture, but kept for documentation.
const _OP_PERFORM_SIZE: usize = 3;

#[derive(Debug, Clone, Copy)]
enum TcpAddrRequest {
    Local,
    Remote,
}

impl TcpAddrRequest {
    fn op_name(self) -> &'static str {
        match self {
            TcpAddrRequest::Local => "tcp_local_addr",
            TcpAddrRequest::Remote => "tcp_remote_addr",
        }
    }
}

fn run_send_closure_on_worker(
    send_closure: SendClosure,
    cancel_token: TaskCancelToken,
    #[cfg(feature = "async-mio")]
    parent_backend: crate::runtime::r#async::backends::mio::MioDriverBackend,
    #[cfg(feature = "async-mio")]
    parent_request_ids: crate::runtime::r#async::scheduler::RequestIdAllocator,
) -> Result<SendValue, String> {
    let constants = send_closure.constants_into_values();
    let globals = send_closure.globals_into_values();
    let action = send_closure.into_closure_value();
    let bytecode = Bytecode {
        instructions: vec![crate::bytecode::op_code::OpCode::OpReturn as u8],
        constants,
        debug_info: None,
    };
    #[cfg(feature = "async-mio")]
    let mut worker_vm = VM::new_with_backend_and_ids(
        bytecode,
        parent_backend.child(),
        None,
        Some(parent_request_ids),
    );
    #[cfg(not(feature = "async-mio"))]
    let mut worker_vm = VM::new(bytecode);
    worker_vm.async_cancel_token = Some(cancel_token);
    for (index, global) in globals.into_iter().enumerate() {
        if let Some(global) = global {
            worker_vm.globals[index] = super::slot::to_slot(global);
        }
    }
    let result = worker_vm.invoke_value(action, vec![])?;
    SendValue::try_from_value(&result).map_err(VM::send_value_error)
}

fn join_send_task(
    label: &str,
    handle: TaskHandle<Result<SendValue, String>>,
) -> Result<SendValue, String> {
    match handle.blocking_join() {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error),
        Err(TaskError::Canceled) => Err(format!("{label} was canceled")),
        Err(TaskError::AlreadyJoined) => Err(format!("{label} was already joined")),
        Err(TaskError::Shutdown) => Err(format!("{label} was shut down")),
    }
}

impl VM {
    pub(super) fn perform_builtin_suspend(
        &mut self,
        effect_name: &str,
        op_name: &str,
        perform_args: &[Value],
    ) -> Option<Result<(), String>> {
        match (effect_name, op_name) {
            ("Suspend", "sleep") => Some(self.perform_suspend_sleep(perform_args)),
            ("Suspend", "await_task") => Some(self.perform_suspend_await_task(perform_args)),
            ("Suspend", "tcp_listen") => Some(self.perform_suspend_tcp_listen(perform_args)),
            ("Suspend", "tcp_accept") => Some(self.perform_suspend_tcp_accept(perform_args)),
            ("Suspend", "tcp_connect") => Some(self.perform_suspend_tcp_connect(perform_args)),
            ("Suspend", "tcp_read") => Some(self.perform_suspend_tcp_read(perform_args)),
            ("Suspend", "tcp_write") => Some(self.perform_suspend_tcp_write(perform_args)),
            ("Suspend", "tcp_close") => Some(self.perform_suspend_tcp_close(perform_args)),
            ("Suspend", "tcp_local_addr") => {
                Some(self.perform_suspend_tcp_local_addr(perform_args))
            }
            ("Suspend", "tcp_remote_addr") => {
                Some(self.perform_suspend_tcp_remote_addr(perform_args))
            }
            ("Suspend", "tcp_close_listener") => {
                Some(self.perform_suspend_tcp_close_listener(perform_args))
            }
            ("Suspend", "tcp_listener_local_addr") => {
                Some(self.perform_suspend_tcp_listener_local_addr(perform_args))
            }
            ("Suspend", "dns_resolve") => Some(self.perform_suspend_dns_resolve(perform_args)),
            _ => None,
        }
    }

    fn perform_suspend_sleep(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [Value::Integer(ms)] = perform_args else {
            let got = perform_args
                .first()
                .map(Value::type_name)
                .unwrap_or("missing");
            return Err(format!("Suspend.sleep expects Int milliseconds, got {got}"));
        };
        let duration = Duration::from_millis((*ms).max(0) as u64);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_timer(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                duration,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;

        let resume_value = match resumed
            .cancel_handle
            .filter(|handle| handle.request_id() == request_id)
        {
            _ => Value::None,
        };
        self.push(resumed.continuation)?;
        self.push(resume_value)?;
        self.execute_resume(1, None)?;
        Ok(())
    }

    fn perform_suspend_tcp_connect(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [Value::String(host), Value::Integer(port)] = perform_args else {
            return Err("Suspend.tcp_connect expects (String, Int)".to_string());
        };
        let port = u16::try_from(*port)
            .map_err(|_| format!("Suspend.tcp_connect port out of range: {port}"))?;
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_connect(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                host.as_ref().clone(),
                port,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Handle(handle))) => VM::make_tcp_value(handle),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.tcp_connect completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error("tcp_connect", &error));
            }
            None => return Err("Suspend.tcp_connect completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_tcp_listen(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [Value::String(host), Value::Integer(port)] = perform_args else {
            return Err("Suspend.tcp_listen expects (String, Int)".to_string());
        };
        let port = u16::try_from(*port)
            .map_err(|_| format!("Suspend.tcp_listen port out of range: {port}"))?;
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_listen(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                host.as_ref().clone(),
                port,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Handle(handle))) => VM::make_tcp_listener_value(handle),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.tcp_listen completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error("tcp_listen", &error));
            }
            None => return Err("Suspend.tcp_listen completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_tcp_accept(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [listener] = perform_args else {
            return Err("Suspend.tcp_accept expects TcpListener".to_string());
        };
        let handle = IoHandle(VM::tcp_listener_handle_from_value(listener)?);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_accept(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Handle(handle))) => VM::make_tcp_value(handle),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.tcp_accept completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error("tcp_accept", &error));
            }
            None => return Err("Suspend.tcp_accept completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_tcp_read(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [conn, Value::Integer(max)] = perform_args else {
            return Err("Suspend.tcp_read expects (Tcp, Int)".to_string());
        };
        let handle = IoHandle(VM::tcp_handle_from_value(conn)?);
        let max = usize::try_from((*max).max(0))
            .map_err(|_| format!("Suspend.tcp_read size out of range: {max}"))?;
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_read(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
                max,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Bytes(bytes))) => Value::Bytes(bytes.into()),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.tcp_read completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => return Err(VM::format_suspend_error("tcp_read", &error)),
            None => return Err("Suspend.tcp_read completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_tcp_write(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [conn, Value::Bytes(bytes)] = perform_args else {
            return Err("Suspend.tcp_write expects (Tcp, Bytes)".to_string());
        };
        let handle = IoHandle(VM::tcp_handle_from_value(conn)?);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_write(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
                bytes.as_ref().clone(),
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Count(count))) => Value::Integer(count as i64),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.tcp_write completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => return Err(VM::format_suspend_error("tcp_write", &error)),
            None => return Err("Suspend.tcp_write completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_tcp_close(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [conn] = perform_args else {
            return Err("Suspend.tcp_close expects Tcp".to_string());
        };
        let handle = IoHandle(VM::tcp_handle_from_value(conn)?);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_close(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Unit)) => {
                self.resume_suspended_continuation(resumed, Value::None)
            }
            Some(Ok(other)) => Err(format!(
                "Suspend.tcp_close completed with unexpected payload {other:?}"
            )),
            Some(Err(error)) => Err(VM::format_suspend_error("tcp_close", &error)),
            None => Err("Suspend.tcp_close completed without payload".to_string()),
        }
    }

    fn perform_suspend_tcp_local_addr(&mut self, perform_args: &[Value]) -> Result<(), String> {
        self.perform_suspend_tcp_addr(perform_args, TcpAddrRequest::Local)
    }

    fn perform_suspend_tcp_remote_addr(&mut self, perform_args: &[Value]) -> Result<(), String> {
        self.perform_suspend_tcp_addr(perform_args, TcpAddrRequest::Remote)
    }

    fn perform_suspend_tcp_close_listener(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [listener] = perform_args else {
            return Err("Suspend.tcp_close_listener expects TcpListener".to_string());
        };
        let handle = IoHandle(VM::tcp_listener_handle_from_value(listener)?);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_close_listener(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Unit)) => {
                self.resume_suspended_continuation(resumed, Value::None)
            }
            Some(Ok(other)) => Err(format!(
                "Suspend.tcp_close_listener completed with unexpected payload {other:?}"
            )),
            Some(Err(error)) => Err(VM::format_suspend_error("tcp_close_listener", &error)),
            None => Err("Suspend.tcp_close_listener completed without payload".to_string()),
        }
    }

    fn perform_suspend_tcp_listener_local_addr(
        &mut self,
        perform_args: &[Value],
    ) -> Result<(), String> {
        let [listener] = perform_args else {
            return Err("Suspend.tcp_listener_local_addr expects TcpListener".to_string());
        };
        let handle = IoHandle(VM::tcp_listener_handle_from_value(listener)?);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_tcp_listener_local_addr(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Text(text))) => Value::String(Rc::new(text)),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.tcp_listener_local_addr completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error("tcp_listener_local_addr", &error));
            }
            None => {
                return Err("Suspend.tcp_listener_local_addr completed without payload".to_string());
            }
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_dns_resolve(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [Value::String(host), Value::Integer(port)] = perform_args else {
            return Err("Suspend.dns_resolve expects (String, Int)".to_string());
        };
        let port = u16::try_from(*port)
            .map_err(|_| format!("Suspend.dns_resolve port out of range: {port}"))?;
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .start_dns_resolve(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                host.as_ref().clone(),
                port,
            )
            .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::AddressList(addrs))) => {
                if let Some(first) = addrs.first() {
                    Value::String(Rc::new(first.to_string()))
                } else {
                    return Err(VM::format_suspend_error(
                        "dns_resolve",
                        &crate::runtime::r#async::backend::AsyncError::new(
                            crate::runtime::r#async::backend::AsyncErrorKind::Other,
                            "no addresses resolved",
                        ),
                    ));
                }
            }
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.dns_resolve completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error("dns_resolve", &error));
            }
            None => return Err("Suspend.dns_resolve completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn perform_suspend_tcp_addr(
        &mut self,
        perform_args: &[Value],
        kind: TcpAddrRequest,
    ) -> Result<(), String> {
        let [conn] = perform_args else {
            return Err(format!("Suspend.{} expects Tcp", kind.op_name()));
        };
        let handle = IoHandle(VM::tcp_handle_from_value(conn)?);
        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = match kind {
            TcpAddrRequest::Local => self.async_runtime.start_tcp_local_addr(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
            ),
            TcpAddrRequest::Remote => self.async_runtime.start_tcp_remote_addr(
                RuntimeTarget::Task(self.async_task_id),
                continuation,
                handle,
            ),
        }
        .map_err(VM::async_runtime_error)?;
        let resumed = self.wait_for_async_completion(request_id)?;
        let value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Text(text))) => Value::String(Rc::new(text)),
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.{} completed with unexpected payload {other:?}",
                    kind.op_name()
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error(kind.op_name(), &error));
            }
            None => {
                return Err(format!(
                    "Suspend.{} completed without payload",
                    kind.op_name()
                ));
            }
        };
        self.resume_suspended_continuation(resumed, value)
    }

    fn wait_for_async_completion(
        &mut self,
        request_id: RequestId,
    ) -> Result<SuspendedContinuation, String> {
        loop {
            if self
                .async_cancel_token
                .as_ref()
                .is_some_and(TaskCancelToken::is_canceled)
            {
                self.async_runtime.request_cancel(request_id);
                self.async_runtime.poll().map_err(VM::async_runtime_error)?;
                return Err("async task was canceled".to_string());
            }
            self.drain_task_await_completions()?;
            self.async_runtime.poll().map_err(VM::async_runtime_error)?;
            if let Some(resumed) = self.async_runtime.pop_resumed_continuation() {
                if resumed.request_id == request_id {
                    return Ok(resumed);
                }
                return Err(format!(
                    "unexpected async completion: expected {:?}, got {:?}",
                    request_id, resumed.request_id
                ));
            }
            std::thread::yield_now();
        }
    }

    fn resume_suspended_continuation(
        &mut self,
        resumed: SuspendedContinuation,
        value: Value,
    ) -> Result<(), String> {
        self.push(resumed.continuation)?;
        self.push(value)?;
        self.execute_resume(1, None)?;
        Ok(())
    }

    fn drain_task_await_completions(&mut self) -> Result<(), String> {
        loop {
            match self.task_await_rx.try_recv() {
                Ok(completion) => {
                    let target = RuntimeTarget::Task(self.async_task_id);
                    let completion = match completion.result {
                        Ok(value) => Completion::ok(
                            completion.request_id,
                            target,
                            CompletionPayload::Value(value.into_value()),
                        ),
                        Err(error) => Completion::err(
                            completion.request_id,
                            target,
                            AsyncError::new(AsyncErrorKind::Other, error),
                        ),
                    };
                    self.async_runtime
                        .deliver_local_completion(completion)
                        .map_err(VM::async_runtime_error)?;
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("Task.await completion channel closed".to_string());
                }
            }
        }
    }

    fn perform_suspend_await_task(&mut self, perform_args: &[Value]) -> Result<(), String> {
        let [task] = perform_args else {
            return Err("Suspend.await_task expects one Task argument".to_string());
        };
        let task_id = VM::task_id_from_value(task)?;
        let record = self
            .task_registry
            .tasks
            .remove(&task_id)
            .ok_or_else(|| format!("unknown Task handle {}", task_id))?;
        if record.canceled {
            return Err(format!("Task {} was canceled", task_id));
        }
        let handle = record
            .handle
            .ok_or_else(|| format!("Task {} was already joined", task_id))?;

        let continuation = self.capture_continuation_piece(self.sp, 4);
        let request_id = self
            .async_runtime
            .park_task(self.async_task_id, continuation, None)
            .map_err(VM::async_runtime_error)?;
        let tx = self.task_await_tx.clone();
        std::thread::spawn(move || {
            let result = match handle.blocking_join() {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(error),
                Err(TaskError::Canceled) => Err(format!("Task {} was canceled", task_id)),
                Err(TaskError::AlreadyJoined) => {
                    Err(format!("Task {} was already joined", task_id))
                }
                Err(TaskError::Shutdown) => Err(format!("Task {} was shut down", task_id)),
            };
            let _ = tx.send(super::VmTaskAwaitCompletion { request_id, result });
        });

        let resumed = self.wait_for_async_completion(request_id)?;
        let resume_value = match resumed.completion.clone() {
            Some(Ok(CompletionPayload::Value(value))) => value,
            Some(Ok(other)) => {
                return Err(format!(
                    "Suspend.await_task completed with unexpected payload {other:?}"
                ));
            }
            Some(Err(error)) => {
                return Err(VM::format_suspend_error("await_task", &error));
            }
            None => return Err("Suspend.await_task completed without payload".to_string()),
        };
        self.resume_suspended_continuation(resumed, resume_value)
    }

    pub(super) fn capture_continuation_piece(
        &self,
        resume_slot: usize,
        advance_ip: usize,
    ) -> Value {
        let mut frame = self.frames[self.frame_index].clone();
        frame.ip += advance_ip;
        let entry_sp = frame.base_pointer;
        let stack: Vec<Value> = self.stack[entry_sp..resume_slot]
            .iter()
            .map(super::slot::from_slot_ref)
            .collect();
        Value::Continuation(Rc::new(std::cell::RefCell::new(Continuation {
            frames: vec![frame],
            stack,
            sp: resume_slot,
            entry_sp,
            entry_frame_index: self.frame_index.saturating_sub(1),
            inner_handlers: vec![],
            state_marker: None,
        })))
    }

    #[inline]
    fn check_closure_contract_stack_args(
        &self,
        closure: &Closure,
        num_args: usize,
    ) -> Result<(), String> {
        let Some(contract) = closure.function.contract.as_ref() else {
            return Ok(());
        };
        let args_start = self.sp - num_args;
        for (index, maybe_expected) in contract.params.iter().enumerate() {
            let Some(expected) = maybe_expected.as_ref() else {
                continue;
            };
            if index >= num_args {
                break;
            }
            let actual = self.stack_get(args_start + index);
            if !expected.matches_value(&actual, self) {
                let expected_name = expected.type_name();
                let actual_type = actual.type_name();
                let actual_value = format_value(&actual);
                return Err(self.runtime_type_error_enhanced(
                    &expected_name,
                    actual_type,
                    Some(&actual_value),
                ));
            }
        }
        Ok(())
    }

    #[inline]
    fn check_closure_contract_value_args(
        &self,
        closure: &Closure,
        args: &[Value],
    ) -> Result<(), String> {
        let Some(contract) = closure.function.contract.as_ref() else {
            return Ok(());
        };
        for (index, maybe_expected) in contract.params.iter().enumerate() {
            let Some(expected) = maybe_expected.as_ref() else {
                continue;
            };
            let Some(actual) = args.get(index) else {
                break;
            };
            if !expected.matches_value(actual, self) {
                let expected_name = expected.type_name();
                let actual_type = actual.type_name();
                let actual_value = format_value(actual);
                return Err(self.runtime_type_error_enhanced(
                    &expected_name,
                    actual_type,
                    Some(&actual_value),
                ));
            }
        }
        Ok(())
    }

    fn unwind_invoke_error(&mut self, start_sp: usize, start_frame_index: usize) {
        while self.frame_index > start_frame_index {
            let return_slot = self.pop_frame_return_slot();
            let _ = self.reset_sp(return_slot);
        }
        let _ = self.reset_sp(start_sp);
    }

    pub(super) fn execute_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee_idx = self.sp - 1 - num_args;

        match self.stack_get(callee_idx) {
            Value::Closure(closure) => self.call_closure(closure, num_args),
            other => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[other.type_name()])),
        }
    }

    pub(super) fn execute_call_self(&mut self, num_args: usize) -> Result<(), String> {
        let closure = self.current_frame().closure.clone();
        let args_start = self.sp - num_args;
        self.call_closure_with_return_slot(closure, num_args, args_start, args_start)
    }

    fn call_closure(&mut self, closure: Rc<Closure>, num_args: usize) -> Result<(), String> {
        let args_start = self.sp - num_args;
        self.call_closure_at_args_start(closure, num_args, args_start)
    }

    fn call_closure_at_args_start(
        &mut self,
        closure: Rc<Closure>,
        num_args: usize,
        args_start: usize,
    ) -> Result<(), String> {
        self.call_closure_with_return_slot(closure, num_args, args_start, args_start - 1)
    }

    fn call_closure_with_return_slot(
        &mut self,
        closure: Rc<Closure>,
        num_args: usize,
        args_start: usize,
        return_slot: usize,
    ) -> Result<(), String> {
        let mut num_args = num_args;
        if num_args > closure.function.num_parameters
            && closure
                .function
                .debug_info
                .as_ref()
                .and_then(|info| info.name.as_deref())
                .is_some_and(|name| name.starts_with("__tc_"))
        {
            let expected = closure.function.num_parameters;
            let extras = num_args - expected;
            if expected > 0 {
                for i in 1..expected {
                    let value = self.stack_get(args_start + extras + i);
                    self.stack_set(args_start + i, value);
                }
                self.reset_sp(self.sp - extras)?;
                num_args = expected;
            }
        }
        if num_args != closure.function.num_parameters {
            return Err(format!(
                "wrong number of arguments: want={}, got={}",
                closure.function.num_parameters, num_args
            ));
        }
        self.check_closure_contract_stack_args(&closure, num_args)?;
        let frame = Frame::new_with_return_slot(closure, args_start, return_slot);
        let num_locals = frame.closure.function.num_locals;
        let max_stack = frame.closure.function.max_stack;
        self.push_frame(frame);
        self.ensure_stack_capacity_with_headroom(
            self.sp + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.sp += num_locals;
        Ok(())
    }

    pub(super) fn execute_tail_call(&mut self, num_args: usize) -> Result<(), String> {
        if self.profiling {
            self.exit_cost_centre();
        }
        let callee_idx = self.sp - 1 - num_args;
        let callee_val = self.stack_get(callee_idx);
        match &callee_val {
            Value::Closure(closure) => self.tail_call_closure(closure.clone(), num_args),
            other => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[other.type_name()])),
        }
    }

    fn tail_call_closure(&mut self, closure: Rc<Closure>, num_args: usize) -> Result<(), String> {
        if num_args != closure.function.num_parameters {
            return Err(format!(
                "wrong number of arguments: want={}, got={}",
                closure.function.num_parameters, num_args
            ));
        }
        self.check_closure_contract_stack_args(&closure, num_args)?;

        let base_pointer = self.current_frame().base_pointer;

        // CRITICAL: Pre-copy arguments to handle cases like f(x, x) where
        // multiple arguments reference the same local
        self.tail_arg_scratch.clear();
        self.tail_arg_scratch.reserve(num_args);
        for i in 0..num_args {
            self.tail_arg_scratch
                .push(self.stack[self.sp - num_args + i].clone());
        }

        // Overwrite old locals with new arguments
        for (i, arg) in self.tail_arg_scratch.drain(..).enumerate() {
            self.stack[base_pointer + i] = arg;
        }

        // Reset stack pointer and instruction pointer
        let max_stack = closure.function.max_stack;
        self.ensure_stack_capacity_with_headroom(
            base_pointer + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.reset_sp(base_pointer + closure.function.num_locals)?;
        self.current_frame_mut().ip = 0;
        self.current_frame_mut().closure = closure;

        Ok(())
    }

    pub(super) fn push_closure(
        &mut self,
        const_index: usize,
        num_free: usize,
    ) -> Result<(), String> {
        let const_val = self.const_get(const_index);
        match &const_val {
            Value::Function(func) => {
                let func = func.clone();
                let mut free = Vec::with_capacity(num_free);
                for i in 0..num_free {
                    free.push(self.stack_get(self.sp - num_free + i));
                }
                self.reset_sp(self.sp - num_free)?;
                let closure = Closure::new(func, free);
                self.push(Value::Closure(Rc::new(closure)))
            }
            _ => Err("not a function".to_string()),
        }
    }

    /// Resume a captured continuation.
    ///
    /// Called from the OpCall dispatch when the callee is `Value::Continuation`.
    /// `num_args` must be 1 (the resume value) or 2 for parameterized
    /// handlers (`resume_value`, `next_state`).
    ///
    /// Tail-position resume transfers control directly to the captured
    /// continuation. Non-tail resume runs the captured continuation to the
    /// handler boundary, restores the caller frame, and pushes the continuation
    /// result in the ordinary call result slot.
    pub(super) fn execute_resume(
        &mut self,
        num_args: usize,
        caller_ip_advance: Option<usize>,
    ) -> Result<(), String> {
        if num_args != 1 && num_args != 2 {
            return Err(format!("resume expects 1 or 2 arguments, got {}", num_args));
        }
        let return_slot = self
            .sp
            .checked_sub(1 + num_args)
            .ok_or_else(|| "stack underflow".to_string())?;
        let next_state = if num_args == 2 {
            Some(self.pop_untracked()?)
        } else {
            None
        };
        let resume_val = self.pop_untracked()?;
        let cont_val = self.pop_untracked()?; // the callee (Continuation)
        let next_state_for_caller = next_state.clone();

        let caller = caller_ip_advance.map(|advance| {
            let mut frame = self.current_frame().clone();
            frame.ip += advance;
            let stack = self.stack[..self.sp]
                .iter()
                .map(super::slot::from_slot_ref)
                .collect::<Vec<_>>();
            (self.frame_index, frame, stack, self.handler_stack.clone())
        });

        let cont_rc = match cont_val {
            Value::Continuation(rc) => rc,
            _ => unreachable!("execute_resume called with non-Continuation callee"),
        };

        let (entry_frame_index, entry_sp, frames, stack, captured_sp, inner_handlers, state_marker) = {
            let cont = cont_rc.borrow();
            (
                cont.entry_frame_index,
                cont.entry_sp,
                cont.frames.clone(),
                cont.stack.clone(),
                cont.sp,
                cont.inner_handlers.clone(),
                cont.state_marker,
            )
        };

        // Unwind all frames above the handler boundary.
        self.frame_index = entry_frame_index;

        // Reset stack to handler boundary.
        self.reset_sp(entry_sp)?;

        // Restore inner handlers that were nested inside the captured region.
        for h in inner_handlers {
            self.handler_stack.push(h);
        }

        if let Some(state_marker) = state_marker {
            let next_state = next_state.ok_or_else(|| {
                "parameterized handler resume expects next state argument".to_string()
            })?;
            let handler = self
                .handler_stack
                .iter_mut()
                .rfind(|h| h.marker == state_marker)
                .ok_or_else(|| {
                    "parameterized handler resume could not find handler frame".to_string()
                })?;
            handler.state = Some(next_state);
        } else if next_state.is_some() {
            return Err("non-parameterized resume received next state argument".to_string());
        }

        // Restore the captured stack slice.
        let stack_len = stack.len();
        self.ensure_stack_capacity(entry_sp + stack_len + 1)?;
        for (i, v) in stack.into_iter().enumerate() {
            self.stack_set(entry_sp + i, v);
        }

        // Place the resume value at the position corresponding to the result
        // of the perform expression (= captured_sp, right after the saved stack).
        self.stack_set(captured_sp, resume_val);
        self.sp = captured_sp + 1;

        // Restore captured frames above the handler boundary.
        for frame in frames {
            self.push_frame(frame);
        }

        if let Some((caller_frame_index, caller_frame, caller_stack, mut caller_handlers)) = caller
        {
            while self.frame_index > entry_frame_index {
                if self.current_frame().ip >= self.current_frame().instructions().len() {
                    return Err("resumed continuation exited without return".to_string());
                }
                self.execute_current_instruction(Some(entry_frame_index + 1))?;
            }

            let result = self.pop()?;
            if let (Some(marker), Some(next_state)) = (state_marker, next_state_for_caller)
                && let Some(handler) = caller_handlers.iter_mut().rfind(|h| h.marker == marker)
            {
                handler.state = Some(next_state);
            }

            self.ensure_stack_capacity(caller_stack.len() + 1)?;
            for (i, v) in caller_stack.into_iter().enumerate() {
                self.stack_set(i, v);
            }
            self.sp = return_slot;
            self.handler_stack = caller_handlers;
            if caller_frame_index >= self.frames.len() {
                self.frames.push(caller_frame);
            } else {
                self.frames[caller_frame_index] = caller_frame;
            }
            self.frame_index = caller_frame_index;
            self.push(result)?;
        }

        Ok(())
    }

    /// Invokes a callable Value (closure or Flow function) with the given arguments
    /// and returns the result synchronously.
    ///
    /// Used by higher-order Flow functions (map, filter, fold) to call user-provided
    /// functions from within the Flow function implementation.
    pub fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        match callee {
            Value::Closure(closure) => {
                let start_sp = self.sp;
                let start_frame_index = self.frame_index;
                let num_args = args.len();
                if num_args != closure.function.num_parameters {
                    return Err(format!(
                        "wrong number of arguments: want={}, got={}",
                        closure.function.num_parameters, num_args
                    ));
                }
                self.check_closure_contract_value_args(&closure, &args)?;

                // Push the closure onto the stack (callee slot)
                self.push(Value::Closure(closure.clone()))?;

                // Push arguments onto the stack
                for arg in args {
                    self.push(arg)?;
                }

                // Push a new frame
                let frame = Frame::new(closure, self.sp - num_args);
                let num_locals = frame.closure.function.num_locals;
                let max_stack = frame.closure.function.max_stack;
                self.push_frame(frame);
                self.ensure_stack_capacity_with_headroom(
                    self.sp + max_stack,
                    super::STACK_PREGROW_HEADROOM,
                )?;
                self.sp += num_locals;

                // Track frame index so we know when the closure returns
                let target_frame_index = self.frame_index;

                // Run the dispatch loop until this frame returns
                while self.frame_index >= target_frame_index {
                    if self.frame_index == target_frame_index
                        && self.current_frame().ip >= self.current_frame().instructions().len()
                    {
                        self.unwind_invoke_error(start_sp, start_frame_index);
                        return Err("callable exited without return".to_string());
                    }
                    if let Err(err) = self.execute_current_instruction(Some(target_frame_index)) {
                        self.unwind_invoke_error(start_sp, start_frame_index);
                        return Err(err);
                    }
                }

                // The return value is on the stack (pushed by OpReturnValue/OpReturn)
                self.pop()
            }
            _ => Err(format!("not callable: {}", callee.type_name())),
        }
    }

    #[inline]
    fn invoke_closure_arity1(&mut self, closure: Rc<Closure>, arg: Value) -> Result<Value, String> {
        if closure.function.num_parameters != 1 {
            return Err(format!(
                "wrong number of arguments: want={}, got=1",
                closure.function.num_parameters
            ));
        }
        self.check_closure_contract_value_args(&closure, std::slice::from_ref(&arg))?;

        self.push(Value::Closure(closure.clone()))?;
        self.push(arg)?;

        let frame = Frame::new(closure, self.sp - 1);
        let num_locals = frame.closure.function.num_locals;
        let max_stack = frame.closure.function.max_stack;
        self.push_frame(frame);
        self.ensure_stack_capacity_with_headroom(
            self.sp + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.sp += num_locals;

        let target_frame_index = self.frame_index;
        while self.frame_index >= target_frame_index {
            if self.frame_index == target_frame_index
                && self.current_frame().ip >= self.current_frame().instructions().len()
            {
                return Err("callable exited without return".to_string());
            }
            self.execute_current_instruction(Some(target_frame_index))?;
        }

        self.pop()
    }

    #[inline]
    fn invoke_closure_arity2(
        &mut self,
        closure: Rc<Closure>,
        left: Value,
        right: Value,
    ) -> Result<Value, String> {
        if closure.function.num_parameters != 2 {
            return Err(format!(
                "wrong number of arguments: want={}, got=2",
                closure.function.num_parameters
            ));
        }
        let args = [left.clone(), right.clone()];
        self.check_closure_contract_value_args(&closure, &args)?;

        self.push(Value::Closure(closure.clone()))?;
        self.push(left)?;
        self.push(right)?;

        let frame = Frame::new(closure, self.sp - 2);
        let num_locals = frame.closure.function.num_locals;
        let max_stack = frame.closure.function.max_stack;
        self.push_frame(frame);
        self.ensure_stack_capacity_with_headroom(
            self.sp + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.sp += num_locals;

        let target_frame_index = self.frame_index;
        while self.frame_index >= target_frame_index {
            if self.frame_index == target_frame_index
                && self.current_frame().ip >= self.current_frame().instructions().len()
            {
                return Err("callable exited without return".to_string());
            }
            self.execute_current_instruction(Some(target_frame_index))?;
        }

        self.pop()
    }
}

impl RuntimeContext for VM {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        VM::invoke_value(self, callee, args)
    }

    fn task_spawn(&mut self, action: Value) -> Result<Value, String> {
        let task_id = self.task_registry.next_id.max(1);
        self.task_registry.next_id = task_id + 1;
        let send_closure = SendClosure::try_from_value_with_context(
            &action,
            self.constant_values(),
            self.global_values(),
        )
        .map_err(VM::send_value_error)?;
        #[cfg(feature = "async-mio")]
        let parent_backend = self.async_runtime.backend().clone();
        #[cfg(feature = "async-mio")]
        let parent_request_ids = self.async_runtime.request_id_allocator();
        let handle = self
            .task_manager
            .spawn(TaskPriority::NORMAL, move |cancel| {
                #[cfg(feature = "async-mio")]
                {
                    run_send_closure_on_worker(
                        send_closure,
                        cancel,
                        parent_backend,
                        parent_request_ids,
                    )
                }
                #[cfg(not(feature = "async-mio"))]
                {
                    run_send_closure_on_worker(send_closure, cancel)
                }
            })
            .map_err(|err| format!("Task.spawn failed: {err:?}"))?;
        self.task_registry.tasks.insert(
            task_id,
            super::VmTaskRecord {
                handle: Some(handle),
                canceled: false,
            },
        );
        Ok(VM::make_task_value(task_id))
    }

    fn task_blocking_join(&mut self, task: Value) -> Result<Value, String> {
        let task_id = VM::task_id_from_value(&task)?;
        let record = self
            .task_registry
            .tasks
            .remove(&task_id)
            .ok_or_else(|| format!("unknown Task handle {}", task_id))?;
        if record.canceled {
            return Err(format!("Task {} was canceled", task_id));
        }
        let handle = record
            .handle
            .ok_or_else(|| format!("Task {} was already joined", task_id))?;
        match handle.blocking_join() {
            Ok(Ok(result)) => Ok(result.into_value()),
            Ok(Err(error)) => Err(error),
            Err(TaskError::Canceled) => Err(format!("Task {} was canceled", task_id)),
            Err(TaskError::AlreadyJoined) => Err(format!("Task {} was already joined", task_id)),
            Err(TaskError::Shutdown) => Err(format!("Task {} was shut down", task_id)),
        }
    }

    fn task_cancel(&mut self, task: Value) -> Result<Value, String> {
        let task_id = VM::task_id_from_value(&task)?;
        let record = self
            .task_registry
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("unknown Task handle {}", task_id))?;
        if let Some(handle) = &record.handle {
            record.canceled = handle.cancel();
        }
        Ok(Value::None)
    }

    fn async_sleep(&mut self, ms: Value) -> Result<Value, String> {
        let Value::Integer(ms) = ms else {
            return Err(format!(
                "Async.sleep expects Int milliseconds, got {}",
                ms.type_name()
            ));
        };
        if ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms as u64));
        }
        Ok(Value::None)
    }

    fn async_yield_now(&mut self) -> Result<Value, String> {
        std::thread::yield_now();
        Ok(Value::None)
    }

    fn async_both(&mut self, left: Value, right: Value) -> Result<Value, String> {
        let left = self.spawn_async_action(left)?;
        let right = self.spawn_async_action(right)?;
        let left = join_send_task("Async.both left branch", left)?.into_value();
        let right = join_send_task("Async.both right branch", right)?.into_value();
        Ok(Value::Tuple(Rc::new(vec![left, right])))
    }

    fn async_race(&mut self, left: Value, right: Value) -> Result<Value, String> {
        enum RaceSide {
            Left,
            Right,
        }

        let left = self.spawn_async_action(left)?;
        let right = self.spawn_async_action(right)?;
        let left_cancel = left.clone();
        let right_cancel = right.clone();
        let (tx, rx) = mpsc::channel();
        let left_tx = tx.clone();
        std::thread::spawn(move || {
            let _ = left_tx.send((
                RaceSide::Left,
                join_send_task("Async.race left branch", left),
            ));
        });
        std::thread::spawn(move || {
            let _ = tx.send((
                RaceSide::Right,
                join_send_task("Async.race right branch", right),
            ));
        });

        let (winner, result) = rx
            .recv()
            .map_err(|_| "Async.race workers did not report a result".to_string())?;
        match winner {
            RaceSide::Left => {
                let _ = right_cancel.cancel();
            }
            RaceSide::Right => {
                let _ = left_cancel.cancel();
            }
        };
        Ok(result?.into_value())
    }

    fn async_timeout(&mut self, ms: Value, action: Value) -> Result<Value, String> {
        let Value::Integer(ms) = ms else {
            return Err(format!(
                "Async.timeout expects Int milliseconds, got {}",
                ms.type_name()
            ));
        };
        self.run_async_timeout(ms, action)
    }

    fn async_timeout_result(&mut self, ms: Value, action: Value) -> Result<Value, String> {
        let Value::Integer(ms) = ms else {
            return Err(format!(
                "Async.timeout_result expects Int milliseconds, got {}",
                ms.type_name()
            ));
        };
        match self.run_async_timeout(ms, action)? {
            Value::Some(value) => Ok(Value::Right(value)),
            Value::None => Ok(Value::Left(Rc::new(Value::AdtUnit(Rc::new(
                "AsyncTimedOut".to_string(),
            ))))),
            other => Err(format!(
                "Async.timeout_result expected timeout to return Option, got {}",
                other.type_name()
            )),
        }
    }

    fn async_scope(&mut self, body: Value) -> Result<Value, String> {
        let scope_id = self.async_next_scope_id;
        self.async_next_scope_id += 1;
        self.async_scopes.insert(scope_id, Vec::new());

        let body_result = self.invoke_unary_value(&body, VM::scope_value(scope_id));
        let mut children = self.async_scopes.remove(&scope_id).unwrap_or_default();

        let body_value = match body_result {
            Ok(value) => value,
            Err(error) => {
                for child in &children {
                    let _ = child.cancel();
                }
                for child in children {
                    let _ = child.blocking_join();
                }
                return Err(error);
            }
        };

        for child in children.drain(..) {
            join_send_task("Async.scope child", child)?;
        }
        Ok(body_value)
    }

    fn async_fork(&mut self, scope: Value, action: Value) -> Result<Value, String> {
        let scope_id = VM::scope_id_from_value(&scope)?;
        let handle = self.spawn_async_action(action)?;
        let children = self
            .async_scopes
            .get_mut(&scope_id)
            .ok_or_else(|| format!("unknown or closed Async scope {}", scope_id))?;
        children.push(handle);
        Ok(Value::None)
    }

    fn async_try(&mut self, body: Value) -> Result<Value, String> {
        match self.invoke_value(body, Vec::new()) {
            Ok(value) => Ok(Value::Right(Rc::new(value))),
            Err(error) => Ok(Value::Left(Rc::new(VM::async_failed_value(error)))),
        }
    }

    fn async_finally(&mut self, body: Value, cleanup: Value) -> Result<Value, String> {
        let body_result = self.invoke_value(body, Vec::new());
        let cleanup_result = self.invoke_value(cleanup, Vec::new());

        match (body_result, cleanup_result) {
            (Ok(value), Ok(_)) => Ok(value),
            (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
            (Err(body_error), Ok(_)) => Err(body_error),
            (Err(body_error), Err(cleanup_error)) => {
                Err(format!("{body_error}; cleanup failed: {cleanup_error}"))
            }
        }
    }

    fn async_bracket(
        &mut self,
        acquire: Value,
        release: Value,
        body: Value,
    ) -> Result<Value, String> {
        let resource = self.invoke_value(acquire, Vec::new())?;
        let body_result = self.invoke_unary_value(&body, resource.clone());
        let release_result = self.invoke_unary_value(&release, resource);

        match (body_result, release_result) {
            (Ok(value), Ok(_)) => Ok(value),
            (Ok(_), Err(release_error)) => Err(release_error),
            (Err(body_error), Ok(_)) => Err(body_error),
            (Err(body_error), Err(release_error)) => {
                Err(format!("{body_error}; release failed: {release_error}"))
            }
        }
    }

    fn invoke_base_function_borrowed(
        &mut self,
        _base_fn_index: usize,
        _args: &[&Value],
    ) -> Result<Value, String> {
        Err("invoke_base_function_borrowed is deprecated; base functions are now compiled from lib/Flow/".to_string())
    }

    #[inline]
    fn invoke_unary_value(&mut self, callee: &Value, arg: Value) -> Result<Value, String> {
        match callee {
            Value::Closure(closure) => self.invoke_closure_arity1(closure.clone(), arg),
            other => Err(format!("not callable: {}", other.type_name())),
        }
    }

    #[inline]
    fn invoke_binary_value(
        &mut self,
        callee: &Value,
        left: Value,
        right: Value,
    ) -> Result<Value, String> {
        match callee {
            Value::Closure(closure) => self.invoke_closure_arity2(closure.clone(), left, right),
            other => Err(format!("not callable: {}", other.type_name())),
        }
    }

    fn callable_contract<'a>(
        &'a self,
        callee: &'a Value,
    ) -> Option<&'a crate::runtime::function_contract::FunctionContract> {
        match callee {
            Value::Closure(closure) => closure.function.contract.as_ref(),
            _ => None,
        }
    }
}

impl VM {
    fn scope_value(scope_id: i64) -> Value {
        Value::Adt(Rc::new(crate::runtime::value::AdtValue {
            constructor: Rc::new("Scope".to_string()),
            fields: crate::runtime::value::AdtFields::One(Value::Integer(scope_id)),
        }))
    }

    fn async_failed_value(error: String) -> Value {
        // Errors crossing the suspend boundary may carry a structured kind tag of
        // the form "async failure[Kind]: <message>" injected by `format_suspend_error`.
        // Decode that tag here so user code receives the matching AsyncError variant.
        let raw = error
            .strip_prefix("async failure: ")
            .unwrap_or(error.as_str());

        if let Some(rest) = raw.strip_prefix("async failure[") {
            if let Some(close) = rest.find("]: ") {
                let kind = &rest[..close];
                let message = rest[close + 3..].to_string();
                return VM::async_error_from_kind(kind, message);
            }
        }

        // Unit-constructor pass-through: `format_value` renders nullary AsyncError
        // constructors as their bare name. Preserve the variant rather than
        // collapsing to AsyncFailed.
        match raw {
            "AsyncCancelled" => return Value::AdtUnit(Rc::new("AsyncCancelled".to_string())),
            "AsyncTimedOut" => return Value::AdtUnit(Rc::new("AsyncTimedOut".to_string())),
            "ConnectionClosed" => {
                return Value::AdtUnit(Rc::new("ConnectionClosed".to_string()));
            }
            _ => {}
        }

        let message = raw
            .strip_prefix("AsyncFailed(\"")
            .and_then(|message| message.strip_suffix("\")"))
            .unwrap_or(raw)
            .to_string();
        Value::Adt(Rc::new(crate::runtime::value::AdtValue {
            constructor: Rc::new("AsyncFailed".to_string()),
            fields: crate::runtime::value::AdtFields::One(Value::String(Rc::new(message))),
        }))
    }

    fn async_error_from_kind(kind: &str, message: String) -> Value {
        let unit = |name: &str| {
            Value::AdtUnit(Rc::new(name.to_string()))
        };
        let one_string = |name: &str, s: String| {
            Value::Adt(Rc::new(crate::runtime::value::AdtValue {
                constructor: Rc::new(name.to_string()),
                fields: crate::runtime::value::AdtFields::One(Value::String(Rc::new(s))),
            }))
        };
        let io_error = |code: i64, msg: String, syscall: &str| {
            Value::Adt(Rc::new(crate::runtime::value::AdtValue {
                constructor: Rc::new("IoError".to_string()),
                fields: crate::runtime::value::AdtFields::Many(vec![
                    Value::Integer(code),
                    Value::String(Rc::new(msg)),
                    Value::String(Rc::new(syscall.to_string())),
                ]),
            }))
        };
        match kind {
            "Cancelled" => unit("AsyncCancelled"),
            "TimedOut" => unit("AsyncTimedOut"),
            "Closed" => unit("ConnectionClosed"),
            "ConnectionRefused" => io_error(0, message, "connect"),
            "InvalidInput" => one_string("InvalidAddress", message),
            "WouldBlock" => io_error(0, message, "wouldblock"),
            "Other" | _ => one_string("AsyncFailed", message),
        }
    }

    pub(super) fn format_suspend_error(op: &str, error: &AsyncError) -> String {
        let kind = match error.kind {
            AsyncErrorKind::Cancelled => "Cancelled",
            AsyncErrorKind::Closed => "Closed",
            AsyncErrorKind::ConnectionRefused => "ConnectionRefused",
            AsyncErrorKind::InvalidInput => "InvalidInput",
            AsyncErrorKind::TimedOut => "TimedOut",
            AsyncErrorKind::WouldBlock => "WouldBlock",
            AsyncErrorKind::Other => "Other",
        };
        format!("async failure[{kind}]: Suspend.{op} failed: {}", error.message)
    }

    fn scope_id_from_value(scope: &Value) -> Result<i64, String> {
        match scope {
            Value::Adt(adt) if adt.constructor.as_ref() == "Scope" => match adt.fields.get(0) {
                Some(Value::Integer(id)) => Ok(*id),
                Some(other) => Err(format!("Scope id must be Int, got {}", other.type_name())),
                None => Err("Scope value is missing id field".to_string()),
            },
            other => Err(format!("expected Scope handle, got {}", other.type_name())),
        }
    }

    fn run_async_timeout(&mut self, ms: i64, action: Value) -> Result<Value, String> {
        let timeout = Duration::from_millis(ms.max(0) as u64);
        let handle = self.spawn_async_action(action)?;
        let cancel = handle.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(join_send_task("Async.timeout action", handle));
        });
        match rx.recv_timeout(timeout) {
            Ok(result) => Ok(Value::Some(Rc::new(result?.into_value()))),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = cancel.cancel();
                Ok(Value::None)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Async.timeout worker did not report a result".to_string())
            }
        }
    }

    fn spawn_async_action(
        &mut self,
        action: Value,
    ) -> Result<TaskHandle<Result<SendValue, String>>, String> {
        let send_closure = SendClosure::try_from_value_with_context(
            &action,
            self.constant_values(),
            self.global_values(),
        )
        .map_err(VM::send_value_error)?;
        #[cfg(feature = "async-mio")]
        let parent_backend = self.async_runtime.backend().clone();
        #[cfg(feature = "async-mio")]
        let parent_request_ids = self.async_runtime.request_id_allocator();
        self.task_manager
            .spawn(TaskPriority::NORMAL, move |cancel| {
                #[cfg(feature = "async-mio")]
                {
                    run_send_closure_on_worker(
                        send_closure,
                        cancel,
                        parent_backend,
                        parent_request_ids,
                    )
                }
                #[cfg(not(feature = "async-mio"))]
                {
                    run_send_closure_on_worker(send_closure, cancel)
                }
            })
            .map_err(|err| format!("Async branch spawn failed: {err:?}"))
    }
}
