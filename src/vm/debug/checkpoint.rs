//! Checkpoints: copies of a paused VM's state a debugger can go back to
//! (proposal 0188 S3).
//!
//! Values are immutable and reference-counted, so a checkpoint copies the
//! roots (stack, frames, globals, handlers) and bumps counts instead of
//! copying values. While a checkpoint holds a value, the VM's in-place
//! reuse of it is off: that changes speed, never results.
//!
//! Checkpoints are taken only at stop sites of the top-level dispatch loop,
//! where the VM's state is all in its fields: a site inside a callback run
//! by `List.map` sits on Rust frames a checkpoint could not bring back.

use std::time::{Duration, Instant};

use crate::runtime::r#async::context::EffectContext;
use crate::runtime::frame::Frame;
use crate::runtime::handler_frame::HandlerFrame;
use crate::runtime::nanbox::NanTag;
use crate::runtime::replay::{self, TapeMark};

use super::super::{VM, slot, slot::Slot};
use super::effects::Suspension;
use super::{DebugControl, TailCalls};

/// The VM and debugger state at one stop site, before it ran.
pub(super) struct Checkpoint {
    /// Sites run before this one.
    pub(super) position: u64,
    stack: Vec<Slot>,
    last_popped: Slot,
    globals: Vec<Slot>,
    frames: Vec<Frame>,
    call_ips: Vec<usize>,
    handler_stack: Vec<HandlerFrame>,
    context: EffectContext,
    last_line: Vec<Option<(usize, usize)>>,
    tail_calls: Vec<TailCalls>,
    serials: Vec<u64>,
    next_serial: u64,
    suspensions: Vec<Suspension>,
    tape: Option<TapeMark>,
    /// The next channel, task and event ids: a re-run gives what it makes
    /// the same ids (0188 S9a).
    next_ids: (i64, i64, i64),
}

impl Checkpoint {
    pub(super) fn take(vm: &VM, control: &DebugControl) -> Self {
        let globals_used = vm
            .globals
            .iter()
            .rposition(|global| global.is_float() || global.tag() != NanTag::None)
            .map_or(0, |last| last + 1);
        Self {
            position: control.position,
            stack: vm.stack[..vm.sp].to_vec(),
            last_popped: vm.last_popped.clone(),
            globals: vm.globals[..globals_used].to_vec(),
            frames: vm.frames[..=vm.frame_index].to_vec(),
            call_ips: vm.call_ips.clone(),
            handler_stack: vm.handler_stack.clone(),
            context: vm.context.clone(),
            last_line: control.last_line.clone(),
            tail_calls: control.tail_calls.clone(),
            serials: control.serials.clone(),
            next_serial: control.next_serial,
            suspensions: control.suspensions.clone(),
            tape: replay::mark(),
            next_ids: (
                crate::vm::channel::next_id(),
                crate::vm::task::next_id(),
                crate::vm::event::next_id(),
            ),
        }
    }

    /// Puts the VM and debugger back as they were. The VM then runs from
    /// the checkpoint's site again.
    pub(super) fn restore(&self, vm: &mut VM, control: &mut DebugControl) {
        let old_sp = vm.sp;
        if vm.stack.len() < self.stack.len() {
            vm.stack.resize(self.stack.len(), slot::uninit());
        }
        vm.stack[..self.stack.len()].clone_from_slice(&self.stack);
        for dead in self.stack.len()..old_sp.min(vm.stack.len()) {
            vm.stack[dead] = slot::uninit();
        }
        vm.sp = self.stack.len();
        vm.last_popped = self.last_popped.clone();
        vm.globals[..self.globals.len()].clone_from_slice(&self.globals);
        let none = slot::to_slot(crate::runtime::value::Value::None);
        for global in &mut vm.globals[self.globals.len()..] {
            if global.is_float() || global.tag() != NanTag::None {
                *global = none.clone();
            }
        }
        for (index, frame) in self.frames.iter().enumerate() {
            match vm.frames.get_mut(index) {
                Some(slot) => *slot = frame.clone(),
                None => vm.frames.push(frame.clone()),
            }
        }
        vm.frame_index = self.frames.len() - 1;
        vm.call_ips.clone_from(&self.call_ips);
        vm.handler_stack.clone_from(&self.handler_stack);
        vm.context = self.context.clone();
        vm.cont_pieces.clear();
        vm.tail_arg_scratch.clear();
        vm.nested_runs = 0;

        control.position = self.position;
        control.last_line.clone_from(&self.last_line);
        control.tail_calls.clone_from(&self.tail_calls);
        control.serials.clone_from(&self.serials);
        control.next_serial = self.next_serial;
        // Checkpoints are taken in the top-level loop, where no resume runs.
        control.suspensions.clone_from(&self.suspensions);
        control.pending_suspension = None;
        control.resumes.clear();
        control.top_ip = None;
        if let Some(mark) = self.tape.clone() {
            replay::rewind_to(mark);
        }
        let (channels, tasks, events) = self.next_ids;
        crate::vm::channel::rewind_ids(channels);
        crate::vm::task::rewind_ids(tasks);
        crate::vm::event::rewind_ids(events);
    }

    /// How many of this checkpoint's slots are copies of `slot`'s box: a
    /// debugger's own references to the value in it (0188 S10b).
    fn copies_of(&self, slot: &Slot) -> usize {
        self.stack
            .iter()
            .chain(&self.globals)
            .chain(std::iter::once(&self.last_popped))
            .filter(|copy| slot.same_box(copy))
            .count()
    }

    /// About how much memory the checkpoint holds itself: its copies of the
    /// roots, not the values they share with the running program.
    fn bytes(&self) -> usize {
        use std::mem::size_of;
        (self.stack.len() + self.globals.len()) * size_of::<Slot>()
            + self.frames.len() * size_of::<Frame>()
            + self.call_ips.len() * size_of::<usize>()
            + self.handler_stack.len() * size_of::<HandlerFrame>()
            + self.last_line.len() * size_of::<Option<(usize, usize)>>()
            + self.serials.len() * size_of::<u64>()
            + size_of::<Self>()
    }
}

/// Most checkpoints a session keeps.
const MAX_CHECKPOINTS: usize = 256;
/// Most memory the checkpoints' own copies may take.
const MAX_BYTES: usize = 256 << 20;
/// Sites between checkpoints at the start; the interval then adapts.
const INITIAL_EVERY: u64 = 1000;
/// The time between checkpoints the interval aims for.
const TARGET_GAP: Duration = Duration::from_millis(50);

/// The checkpoints of a session, oldest first.
///
/// When there are too many, or they take too much memory, every other one
/// is dropped (never the first) and the interval doubles, so coverage thins
/// out towards the start and memory stays bounded. The first checkpoint,
/// taken before the program ran, is always kept, so every position can be
/// reached again, at worst by running from the start.
pub(super) struct Checkpoints {
    list: Vec<Checkpoint>,
    bytes: usize,
    /// Sites between checkpoints.
    every: u64,
    /// Set by `FLUX_DEBUG_CHECKPOINT_EVERY`: tests fix the interval.
    fixed: bool,
    last_taken: Option<Instant>,
    max_count: usize,
    max_bytes: usize,
}

impl Checkpoints {
    /// How many slots of all the checkpoints are copies of `slot`'s box.
    pub(super) fn copies_of(&self, slot: &Slot) -> usize {
        self.list
            .iter()
            .map(|checkpoint| checkpoint.copies_of(slot))
            .sum()
    }

    pub(super) fn new() -> Self {
        let fixed = std::env::var("FLUX_DEBUG_CHECKPOINT_EVERY")
            .ok()
            .and_then(|every| every.parse::<u64>().ok())
            .filter(|every| *every > 0);
        Self::with_limits(
            fixed.unwrap_or(INITIAL_EVERY),
            fixed.is_some(),
            MAX_CHECKPOINTS,
            MAX_BYTES,
        )
    }

    /// No checkpoints, standing in while the real ones are read.
    pub(super) fn empty() -> Self {
        Self::with_limits(INITIAL_EVERY, true, MAX_CHECKPOINTS, MAX_BYTES)
    }

    fn with_limits(every: u64, fixed: bool, max_count: usize, max_bytes: usize) -> Self {
        Self {
            list: Vec::new(),
            bytes: 0,
            every,
            fixed,
            last_taken: None,
            max_count,
            max_bytes,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.list.len()
    }

    /// Whether a checkpoint is due at `position`: past the newest one by
    /// the interval. Positions already behind the newest, which a re-run
    /// passes again, never are.
    pub(super) fn due(&self, position: u64) -> bool {
        match self.list.last() {
            None => true,
            Some(newest) => position >= newest.position + self.every,
        }
    }

    pub(super) fn push(&mut self, checkpoint: Checkpoint) {
        if !self.fixed {
            let now = Instant::now();
            if let Some(last) = self.last_taken {
                let gap = now.duration_since(last);
                if gap < TARGET_GAP / 2 {
                    self.every = self.every.saturating_mul(2);
                } else if gap > TARGET_GAP * 2 && self.every > INITIAL_EVERY {
                    self.every /= 2;
                }
            }
            self.last_taken = Some(now);
        }
        self.bytes += checkpoint.bytes();
        self.list.push(checkpoint);
        while self.list.len() > self.max_count
            || (self.bytes > self.max_bytes && self.list.len() > 2)
        {
            self.thin();
        }
    }

    /// Drops every other checkpoint after the first, and doubles the
    /// interval.
    fn thin(&mut self) {
        let mut index = 0;
        self.list.retain(|_| {
            let keep = index % 2 == 0;
            index += 1;
            keep
        });
        self.bytes = self.list.iter().map(Checkpoint::bytes).sum();
        self.every = self.every.saturating_mul(2);
    }

    /// The newest checkpoint taken before the site at `position` ran.
    pub(super) fn before(&self, position: u64) -> Option<&Checkpoint> {
        self.list.iter().rev().find(|cp| cp.position < position)
    }

    /// The checkpoint at exactly `position`.
    pub(super) fn at(&self, position: u64) -> Option<&Checkpoint> {
        self.list.iter().find(|cp| cp.position == position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(position: u64) -> Checkpoint {
        Checkpoint {
            position,
            stack: vec![slot::uninit(); 8],
            last_popped: slot::uninit(),
            globals: Vec::new(),
            frames: Vec::new(),
            call_ips: Vec::new(),
            handler_stack: Vec::new(),
            context: EffectContext::default(),
            last_line: Vec::new(),
            tail_calls: Vec::new(),
            serials: Vec::new(),
            next_serial: 0,
            suspensions: Vec::new(),
            tape: None,
            next_ids: (0, 0, 0),
        }
    }

    #[test]
    fn thinning_keeps_the_first_and_bounds_the_count() {
        let mut ring = Checkpoints::with_limits(1, true, 8, usize::MAX);
        for position in 0..1000 {
            if ring.due(position) {
                ring.push(fake(position));
            }
        }
        assert!(ring.len() <= 8);
        assert_eq!(ring.list[0].position, 0);
        assert!(ring.every > 1, "the interval grows as checkpoints thin");
        let positions: Vec<u64> = ring.list.iter().map(|cp| cp.position).collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn thinning_bounds_the_memory() {
        let one = fake(0).bytes();
        let mut ring = Checkpoints::with_limits(1, true, usize::MAX, one * 5);
        for position in 0..500 {
            ring.push(fake(position));
        }
        assert!(ring.bytes <= one * 5);
        assert_eq!(ring.list[0].position, 0);
    }

    #[test]
    fn finds_the_checkpoint_before_a_position() {
        let mut ring = Checkpoints::with_limits(10, true, usize::MAX, usize::MAX);
        for position in [0, 10, 20] {
            ring.push(fake(position));
        }
        assert_eq!(ring.before(15).map(|cp| cp.position), Some(10));
        assert_eq!(ring.before(10).map(|cp| cp.position), Some(0));
        assert_eq!(ring.before(0).map(|cp| cp.position), None);
        assert!(!ring.due(25));
        assert!(ring.due(30));
    }
}
