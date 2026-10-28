//! Bounded, opt-in evidence from the Windows backend's existing IME reads.
//! This module is available only in Windows test-support builds.

use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    rc::Rc,
};

const EVENT_LIMIT: usize = 128;
const STRING_LIMIT: usize = 4096;

thread_local! {
    static ACTIVE: RefCell<Option<ActiveTrace>> = const { RefCell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static LOST: Cell<bool> = const { Cell::new(false) };
}

struct ActiveTrace {
    generation: u64,
    hwnd: usize,
    sequence: u64,
    events: Vec<TraceEvent>,
}

/// The kind of string already read by the native IME handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImeStringKind {
    /// The current marked composition text.
    Composition,
    /// Text committed by the input method.
    Result,
}

/// A copy of one actual native message or its already-read string.
#[derive(Clone, Debug)]
pub struct TraceEvent {
    /// Monotonic sequence within this observation session.
    pub sequence: u64,
    /// The observed native window identity.
    pub hwnd: usize,
    /// The original Windows message number.
    pub message: u32,
    /// The original message's composition flags.
    pub flags: u32,
    /// Present only for a string read by the production handler.
    pub kind: Option<ImeStringKind>,
    /// Owned copy of the exact UTF-16 units read by that handler.
    pub utf16: Vec<u16>,
}

/// Events since the previous drain, including any evidence loss.
#[derive(Default)]
pub struct TraceBatch {
    /// Captured events in sequence order.
    pub events: Vec<TraceEvent>,
    /// True if bounded storage or reentrant access lost any evidence.
    pub overflow: bool,
}

/// Keeps observation confined to its creating thread and window.
pub struct TraceGuard {
    generation: u64,
    _thread_bound: PhantomData<Rc<()>>,
}

/// Starts a single observation session for a nonzero native window identity.
pub fn begin(hwnd: usize) -> Result<TraceGuard, &'static str> {
    if hwnd == 0 {
        return Err("IME observation requires a nonzero window");
    }
    ACTIVE.with(|active| {
        let mut active = active
            .try_borrow_mut()
            .map_err(|_| "IME observer is borrowed")?;
        if active.is_some() {
            return Err("IME observation is already active on this thread");
        }
        let generation = GENERATION.with(|generation| -> Result<u64, &'static str> {
            let next = generation
                .get()
                .checked_add(1)
                .ok_or("IME observation generation exhausted")?;
            generation.set(next);
            Ok(next)
        })?;
        *active = Some(ActiveTrace {
            generation,
            hwnd,
            sequence: 0,
            events: Vec::with_capacity(EVENT_LIMIT),
        });
        LOST.with(|lost| lost.set(false));
        Ok(TraceGuard {
            generation,
            _thread_bound: PhantomData,
        })
    })
}

impl TraceGuard {
    /// Takes accumulated events without ending the observation session.
    pub fn drain(&self) -> TraceBatch {
        ACTIVE.with(|active| {
            let Ok(mut active) = active.try_borrow_mut() else {
                LOST.with(|lost| lost.set(true));
                return TraceBatch {
                    overflow: true,
                    ..Default::default()
                };
            };
            let Some(active) = active
                .as_mut()
                .filter(|active| active.generation == self.generation)
            else {
                return TraceBatch {
                    overflow: true,
                    ..Default::default()
                };
            };
            TraceBatch {
                events: std::mem::replace(&mut active.events, Vec::with_capacity(EVENT_LIMIT)),
                overflow: LOST.with(Cell::get),
            }
        })
    }
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|active| {
            if let Ok(mut active) = active.try_borrow_mut()
                && active
                    .as_ref()
                    .is_some_and(|active| active.generation == self.generation)
            {
                *active = None;
            }
        });
    }
}

fn record(hwnd: usize, message: u32, flags: u32, kind: Option<ImeStringKind>, utf16: &[u16]) {
    let _ = ACTIVE.try_with(|active| {
        let Ok(mut active) = active.try_borrow_mut() else {
            let _ = LOST.try_with(|lost| lost.set(true));
            return;
        };
        let Some(active) = active.as_mut().filter(|active| active.hwnd == hwnd) else {
            return;
        };
        if active.events.len() >= EVENT_LIMIT || utf16.len() > STRING_LIMIT {
            let _ = LOST.try_with(|lost| lost.set(true));
            return;
        }
        let Some(sequence) = active.sequence.checked_add(1) else {
            let _ = LOST.try_with(|lost| lost.set(true));
            return;
        };
        active.sequence = sequence;
        active.events.push(TraceEvent {
            sequence,
            hwnd,
            message,
            flags,
            kind,
            utf16: utf16.to_vec(),
        });
    });
}

/// Copies native message metadata without calling window or application code.
pub fn record_message(hwnd: usize, message: u32, flags: u32) {
    record(hwnd, message, flags, None, &[]);
}

/// Copies a string already read by the native handler, without another IMM call.
pub fn record_string(hwnd: usize, flags: u32, kind: ImeStringKind, utf16: &[u16]) {
    record(hwnd, 0x010f, flags, Some(kind), utf16);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_is_window_and_thread_scoped_and_releases_its_session() {
        assert!(begin(0).is_err());
        let guard = begin(17).unwrap();
        assert!(begin(17).is_err());
        record_message(18, 0x10f, 8);
        std::thread::spawn(|| record_message(17, 0x10f, 8))
            .join()
            .unwrap();
        record_message(17, 0x10f, 8);
        record_string(17, 8, ImeStringKind::Composition, &[0x78]);
        let batch = guard.drain();
        assert!(!batch.overflow);
        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].sequence, 1);
        assert_eq!(batch.events[1].sequence, 2);
        assert_eq!(batch.events[1].utf16, [0x78]);
        assert!(guard.drain().events.is_empty());
        drop(guard);
        record_message(17, 0x10f, 8);
        assert!(begin(17).unwrap().drain().events.is_empty());
    }

    #[test]
    fn observer_reports_bounded_evidence_loss_instead_of_silent_truncation() {
        let guard = begin(23).unwrap();
        for _ in 0..129 {
            record_message(23, 0x10f, 8);
        }
        let batch = guard.drain();
        assert_eq!(batch.events.len(), 128);
        assert!(batch.overflow);
        assert!(
            guard.drain().overflow,
            "evidence loss remains fatal for the session"
        );
        record_string(23, 0x800, ImeStringKind::Result, &vec![1; 4097]);
        assert!(guard.drain().overflow);
        assert!(guard.drain().events.is_empty());
    }
}
