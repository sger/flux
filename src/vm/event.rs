use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::runtime::{
    RuntimeContext,
    value::{AdtFields, AdtValue, Value},
};

#[derive(Clone)]
enum VmEvent {
    Recv(i64),
    Send(i64, Value),
    After(Instant),
    Always(Value),
    Never,
    Choose(Vec<i64>),
    Wrap(i64, Value),
}

thread_local! {
    static NEXT_EVENT_ID: RefCell<i64> = const { RefCell::new(1) };
    static EVENTS: RefCell<HashMap<i64, VmEvent>> = RefCell::new(HashMap::new());
}

fn insert(event: VmEvent) -> i64 {
    let id = NEXT_EVENT_ID.with(|next| {
        let mut next = next.borrow_mut();
        let id = *next;
        *next += 1;
        id
    });
    EVENTS.with(|events| {
        events.borrow_mut().insert(id, event);
    });
    id
}

fn get(id: i64) -> Result<VmEvent, String> {
    EVENTS.with(|events| {
        events
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("event {id} not found"))
    })
}

pub(super) fn recv(ch: i64) -> i64 {
    insert(VmEvent::Recv(ch))
}

pub(super) fn send(ch: i64, value: Value) -> i64 {
    insert(VmEvent::Send(ch, value))
}

pub(super) fn after(ms: i64) -> Result<i64, String> {
    if ms < 0 {
        return Err("event_after: ms must be non-negative".to_string());
    }
    Ok(insert(VmEvent::After(
        Instant::now() + Duration::from_millis(ms as u64),
    )))
}

pub(super) fn always(value: Value) -> i64 {
    insert(VmEvent::Always(value))
}

pub(super) fn never() -> i64 {
    insert(VmEvent::Never)
}

pub(super) fn choose(ids: Vec<i64>) -> Result<i64, String> {
    if ids.is_empty() {
        return Err("Event.choose called on empty list".to_string());
    }
    Ok(insert(VmEvent::Choose(ids)))
}

pub(super) fn wrap(id: i64, f: Value) -> i64 {
    insert(VmEvent::Wrap(id, f))
}

pub(super) fn poll_value(ctx: &mut dyn RuntimeContext, id: i64) -> Result<Value, String> {
    if let Some(value) = poll(ctx, id)? {
        remove_tree(id);
        Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Ready".to_string()),
            fields: AdtFields::One(value),
        })))
    } else {
        Ok(Value::AdtUnit(Rc::new("Pending".to_string())))
    }
}

fn remove_tree(id: i64) {
    let event = EVENTS.with(|events| events.borrow_mut().remove(&id));
    match event {
        Some(VmEvent::Choose(ids)) => {
            for child in ids {
                remove_tree(child);
            }
        }
        Some(VmEvent::Wrap(child, _)) => remove_tree(child),
        _ => {}
    }
}

fn poll(ctx: &mut dyn RuntimeContext, id: i64) -> Result<Option<Value>, String> {
    match get(id)? {
        VmEvent::Recv(ch) => {
            let value = super::channel::try_recv(ch)?;
            if !matches!(value, Value::None) {
                Ok(Some(value))
            } else if super::channel::is_closed(ch)? {
                Ok(Some(Value::None))
            } else {
                Ok(None)
            }
        }
        VmEvent::Send(ch, value) => {
            if super::channel::try_send(ch, &value)? || super::channel::is_closed(ch)? {
                Ok(Some(Value::None))
            } else {
                Ok(None)
            }
        }
        VmEvent::After(deadline) => {
            if Instant::now() >= deadline {
                Ok(Some(Value::None))
            } else {
                Ok(None)
            }
        }
        VmEvent::Always(value) => Ok(Some(value)),
        VmEvent::Never => Ok(None),
        VmEvent::Choose(ids) => {
            for child in ids {
                if let Some(value) = poll(ctx, child)? {
                    return Ok(Some(value));
                }
            }
            Ok(None)
        }
        VmEvent::Wrap(child, f) => {
            if let Some(value) = poll(ctx, child)? {
                ctx.invoke_value(f, vec![value]).map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

pub(super) fn collect_ids(value: &Value) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    let mut current = value.clone();
    loop {
        match current {
            Value::EmptyList | Value::None => return Ok(out),
            Value::Cons(cell) => {
                match &cell.head {
                    Value::Integer(id) => out.push(*id),
                    other => {
                        return Err(format!(
                            "event_choose expected List<Int>, got {}",
                            other.type_name()
                        ));
                    }
                }
                current = cell.tail.clone();
            }
            other => {
                return Err(format!(
                    "event_choose expected List<Int>, got {}",
                    other.type_name()
                ));
            }
        }
    }
}
