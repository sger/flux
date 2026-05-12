use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

use crate::runtime::value::Value;

use super::task::VmSendValue;

type ChannelResult = Result<VmSendValue, String>;

pub struct ChannelCell {
    pub capacity: usize,
    pub buf: VecDeque<VmSendValue>,
    pub closed: bool,
    pub waiting_recv: VecDeque<u64>,
    pub waiting_send: VecDeque<(u64, VmSendValue)>,
    pub event_recv_watchers: VecDeque<u64>,
    pub event_send_watchers: VecDeque<u64>,
}

struct ChannelEntry {
    cell: Mutex<ChannelCell>,
    ready: Condvar,
}

struct CompletionQueue {
    tx: mpsc::Sender<(u64, ChannelResult)>,
    rx: Mutex<mpsc::Receiver<(u64, ChannelResult)>>,
}

static NEXT_CHANNEL_ID: AtomicI64 = AtomicI64::new(1);
static CHANNELS: OnceLock<Mutex<HashMap<i64, Arc<ChannelEntry>>>> = OnceLock::new();
static COMPLETIONS: OnceLock<CompletionQueue> = OnceLock::new();

fn channels() -> &'static Mutex<HashMap<i64, Arc<ChannelEntry>>> {
    CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn completion_queue() -> &'static CompletionQueue {
    COMPLETIONS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        CompletionQueue {
            tx,
            rx: Mutex::new(rx),
        }
    })
}

fn lookup(id: i64) -> Result<Arc<ChannelEntry>, String> {
    channels()
        .lock()
        .expect("VM channel registry poisoned")
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("channel {id} not found"))
}

fn publish(request_id: u64, result: ChannelResult) {
    let _ = completion_queue().tx.send((request_id, result));
}

fn unit() -> VmSendValue {
    // The VM accepts Value::None as Unit; existing Unit-returning async
    // primops (sleep, cancel, TCP write) resume with the same value.
    VmSendValue::None
}

fn some(value: VmSendValue) -> VmSendValue {
    VmSendValue::Some(Box::new(value))
}

fn recv_ready(cell: &ChannelCell) -> bool {
    cell.closed || !cell.buf.is_empty() || !cell.waiting_send.is_empty()
}

fn send_ready(cell: &ChannelCell) -> bool {
    cell.closed
        || !cell.waiting_recv.is_empty()
        || (cell.capacity > 0 && cell.buf.len() < cell.capacity)
}

fn notify_event_watchers(cell: &mut ChannelCell) {
    let mut ready = Vec::new();
    if recv_ready(cell) {
        compact_event_watchers(&mut cell.event_recv_watchers);
        ready.extend(cell.event_recv_watchers.drain(..));
    }
    if send_ready(cell) {
        compact_event_watchers(&mut cell.event_send_watchers);
        ready.extend(cell.event_send_watchers.drain(..));
    }
    for request_id in ready {
        super::event::complete_wait(request_id);
    }
}

fn compact_event_watchers(watchers: &mut VecDeque<u64>) {
    // Called while holding a channel cell lock; `is_wait_active` takes the
    // event wait lock. Keep that channel -> event lock order if refactored.
    watchers.retain(|request_id| super::event::is_wait_active(*request_id));
}

fn push_event_watcher(watchers: &mut VecDeque<u64>, request_id: u64) {
    compact_event_watchers(watchers);
    if !watchers.contains(&request_id) {
        watchers.push_back(request_id);
    }
}

fn flush_waiters(cell: &mut ChannelCell) {
    while let Some(req) = cell.waiting_recv.pop_front() {
        if let Some(value) = cell.buf.pop_front() {
            publish(req, Ok(some(value)));
            continue;
        }
        if let Some((send_req, value)) = cell.waiting_send.pop_front() {
            publish(send_req, Ok(unit()));
            publish(req, Ok(some(value)));
            continue;
        }
        if cell.closed {
            publish(req, Ok(VmSendValue::None));
            continue;
        }
        cell.waiting_recv.push_front(req);
        break;
    }

    while cell.capacity > 0 && cell.buf.len() < cell.capacity {
        let Some((send_req, value)) = cell.waiting_send.pop_front() else {
            break;
        };
        if cell.closed {
            publish(send_req, Ok(unit()));
        } else if let Some(recv_req) = cell.waiting_recv.pop_front() {
            publish(send_req, Ok(unit()));
            publish(recv_req, Ok(some(value)));
        } else {
            cell.buf.push_back(value);
            publish(send_req, Ok(unit()));
        }
    }

    if cell.closed {
        while let Some((send_req, _)) = cell.waiting_send.pop_front() {
            publish(send_req, Ok(unit()));
        }
    }
    notify_event_watchers(cell);
}

pub(super) fn make(capacity: i64) -> Result<i64, String> {
    if capacity < 0 {
        return Err("chan_make: capacity must be non-negative".to_string());
    }
    let id = NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed);
    let entry = Arc::new(ChannelEntry {
        cell: Mutex::new(ChannelCell {
            capacity: capacity as usize,
            buf: VecDeque::new(),
            closed: false,
            waiting_recv: VecDeque::new(),
            waiting_send: VecDeque::new(),
            event_recv_watchers: VecDeque::new(),
            event_send_watchers: VecDeque::new(),
        }),
        ready: Condvar::new(),
    });
    channels()
        .lock()
        .expect("VM channel registry poisoned")
        .insert(id, entry);
    Ok(id)
}

pub(super) fn send(id: i64, value: &Value) -> Result<(), String> {
    let value = VmSendValue::try_from_value(value)?;
    send_value(id, value)
}

pub(super) fn send_move(id: i64, value: Value) -> Result<(), String> {
    let value = VmSendValue::try_transfer_from_value(value)?;
    send_value(id, value)
}

fn send_value(id: i64, value: VmSendValue) -> Result<(), String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if cell.closed {
        return Ok(());
    }
    if let Some(req) = cell.waiting_recv.pop_front() {
        publish(req, Ok(some(value)));
        notify_event_watchers(&mut cell);
        entry.ready.notify_all();
        return Ok(());
    }
    if cell.capacity > 0 && cell.buf.len() < cell.capacity {
        cell.buf.push_back(value);
        notify_event_watchers(&mut cell);
        entry.ready.notify_all();
        return Ok(());
    }
    while !cell.closed {
        cell = entry.ready.wait(cell).expect("VM channel cell poisoned");
        if let Some(req) = cell.waiting_recv.pop_front() {
            publish(req, Ok(some(value)));
            notify_event_watchers(&mut cell);
            entry.ready.notify_all();
            return Ok(());
        }
        if cell.capacity > 0 && cell.buf.len() < cell.capacity {
            cell.buf.push_back(value);
            notify_event_watchers(&mut cell);
            entry.ready.notify_all();
            return Ok(());
        }
    }
    Ok(())
}

pub(super) fn start_send(id: i64, value: &Value, request_id: u64) -> Result<(), String> {
    let value = VmSendValue::try_from_value(value)?;
    start_send_value(id, value, request_id)
}

pub(super) fn start_send_move(id: i64, value: Value, request_id: u64) -> Result<(), String> {
    let value = VmSendValue::try_transfer_from_value(value)?;
    start_send_value(id, value, request_id)
}

fn start_send_value(id: i64, value: VmSendValue, request_id: u64) -> Result<(), String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if cell.closed {
        publish(request_id, Ok(unit()));
    } else if let Some(req) = cell.waiting_recv.pop_front() {
        publish(req, Ok(some(value)));
        publish(request_id, Ok(unit()));
    } else if cell.capacity > 0 && cell.buf.len() < cell.capacity {
        cell.buf.push_back(value);
        publish(request_id, Ok(unit()));
    } else {
        cell.waiting_send.push_back((request_id, value));
    }
    flush_waiters(&mut cell);
    notify_event_watchers(&mut cell);
    entry.ready.notify_all();
    Ok(())
}

pub(super) fn recv(id: i64) -> Result<Value, String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    loop {
        if let Some(value) = cell.buf.pop_front() {
            flush_waiters(&mut cell);
            notify_event_watchers(&mut cell);
            entry.ready.notify_all();
            return some(value).to_value();
        }
        if let Some((send_req, value)) = cell.waiting_send.pop_front() {
            publish(send_req, Ok(unit()));
            notify_event_watchers(&mut cell);
            entry.ready.notify_all();
            return some(value).to_value();
        }
        if cell.closed {
            return Ok(Value::None);
        }
        cell = entry.ready.wait(cell).expect("VM channel cell poisoned");
    }
}

pub(super) fn start_recv(id: i64, request_id: u64) -> Result<(), String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if let Some(value) = cell.buf.pop_front() {
        publish(request_id, Ok(some(value)));
    } else if let Some((send_req, value)) = cell.waiting_send.pop_front() {
        publish(send_req, Ok(unit()));
        publish(request_id, Ok(some(value)));
    } else if cell.closed {
        publish(request_id, Ok(VmSendValue::None));
    } else {
        cell.waiting_recv.push_back(request_id);
    }
    flush_waiters(&mut cell);
    notify_event_watchers(&mut cell);
    entry.ready.notify_all();
    Ok(())
}

pub(super) fn try_send(id: i64, value: &Value) -> Result<bool, String> {
    let value = VmSendValue::try_from_value(value)?;
    try_send_value(id, value)
}

pub(super) fn try_send_move(id: i64, value: Value) -> Result<bool, String> {
    let value = VmSendValue::try_transfer_from_value(value)?;
    try_send_value(id, value)
}

pub(super) fn try_send_value(id: i64, value: VmSendValue) -> Result<bool, String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if cell.closed {
        return Ok(false);
    }
    if let Some(req) = cell.waiting_recv.pop_front() {
        publish(req, Ok(some(value)));
        notify_event_watchers(&mut cell);
        entry.ready.notify_all();
        return Ok(true);
    }
    if cell.capacity > 0 && cell.buf.len() < cell.capacity {
        cell.buf.push_back(value);
        notify_event_watchers(&mut cell);
        entry.ready.notify_all();
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn try_recv(id: i64) -> Result<Value, String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if let Some(value) = cell.buf.pop_front() {
        flush_waiters(&mut cell);
        notify_event_watchers(&mut cell);
        entry.ready.notify_all();
        return some(value).to_value();
    }
    if let Some((send_req, value)) = cell.waiting_send.pop_front() {
        publish(send_req, Ok(unit()));
        notify_event_watchers(&mut cell);
        entry.ready.notify_all();
        return some(value).to_value();
    }
    Ok(Value::None)
}

pub(super) fn close(id: i64) -> Result<(), String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if cell.closed {
        return Ok(());
    }
    cell.closed = true;
    while let Some(req) = cell.waiting_recv.pop_front() {
        if let Some(value) = cell.buf.pop_front() {
            publish(req, Ok(some(value)));
        } else {
            publish(req, Ok(VmSendValue::None));
        }
    }
    while let Some((req, _)) = cell.waiting_send.pop_front() {
        publish(req, Ok(unit()));
    }
    notify_event_watchers(&mut cell);
    entry.ready.notify_all();
    Ok(())
}

pub(super) fn len(id: i64) -> Result<i64, String> {
    let entry = lookup(id)?;
    Ok(entry
        .cell
        .lock()
        .expect("VM channel cell poisoned")
        .buf
        .len() as i64)
}

pub(super) fn cap(id: i64) -> Result<i64, String> {
    let entry = lookup(id)?;
    Ok(entry
        .cell
        .lock()
        .expect("VM channel cell poisoned")
        .capacity as i64)
}

pub(super) fn is_closed(id: i64) -> Result<bool, String> {
    let entry = lookup(id)?;
    Ok(entry.cell.lock().expect("VM channel cell poisoned").closed)
}

pub(super) fn can_recv(id: i64) -> Result<bool, String> {
    let entry = lookup(id)?;
    let cell = entry.cell.lock().expect("VM channel cell poisoned");
    Ok(recv_ready(&cell))
}

pub(super) fn can_send(id: i64) -> Result<bool, String> {
    let entry = lookup(id)?;
    let cell = entry.cell.lock().expect("VM channel cell poisoned");
    Ok(send_ready(&cell))
}

pub(super) fn watch_recv(id: i64, request_id: u64) -> Result<(), String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if recv_ready(&cell) {
        super::event::complete_wait(request_id);
    } else {
        push_event_watcher(&mut cell.event_recv_watchers, request_id);
    }
    Ok(())
}

pub(super) fn watch_send(id: i64, request_id: u64) -> Result<(), String> {
    let entry = lookup(id)?;
    let mut cell = entry.cell.lock().expect("VM channel cell poisoned");
    if send_ready(&cell) {
        super::event::complete_wait(request_id);
    } else {
        push_event_watcher(&mut cell.event_send_watchers, request_id);
    }
    Ok(())
}

pub(super) fn try_recv_completion() -> Option<(u64, Result<Value, String>)> {
    let queue = completion_queue();
    let rx = queue
        .rx
        .lock()
        .expect("VM channel completion queue poisoned");
    match rx.try_recv() {
        Ok((request_id, result)) => Some((request_id, result.and_then(|v| v.to_value()))),
        Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_buffer_start_send_parks_until_recv_drains_space() {
        let id = make(1).expect("make channel");
        send(id, &Value::Integer(3)).expect("initial send");

        start_send(id, &Value::Integer(4), 42).expect("start blocked send");
        assert!(
            try_recv_completion().is_none(),
            "sender should remain parked while buffer is full"
        );

        let first = recv(id).expect("recv first buffered value");
        assert_eq!(first, Value::Some(std::rc::Rc::new(Value::Integer(3))));

        let (req, sent) = try_recv_completion().expect("sender wake completion");
        assert_eq!(req, 42);
        assert_eq!(sent.expect("sender completion value"), Value::None);

        let second = recv(id).expect("recv value from parked sender");
        assert_eq!(second, Value::Some(std::rc::Rc::new(Value::Integer(4))));
    }
}
