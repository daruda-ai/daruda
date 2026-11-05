//! daruda patch: context-free ownership and lifetime of a link gesture.
use std::{
    ops::Range,
    sync::{Arc, Weak},
};

#[derive(Clone, Debug, Default)]
pub(in crate::text) struct LinkOwner(Arc<()>);

impl LinkOwner {
    pub(in crate::text) fn hit(
        &self,
        host: u64,
        origin: [f32; 2],
        range: Range<usize>,
        url: &str,
    ) -> LinkHit {
        LinkHit {
            owner: Arc::downgrade(&self.0),
            host,
            origin,
            range,
            url: url.to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::text) struct LinkHit {
    owner: Weak<()>,
    host: u64,
    origin: [f32; 2],
    range: Range<usize>,
    url: String,
}

impl LinkHit {
    fn matches(&self, other: &Self) -> bool {
        self.owner.upgrade().is_some()
            && self.owner.ptr_eq(&other.owner)
            && self.host == other.host
            && self.origin == other.origin
            && self.range == other.range
            && self.url == other.url
    }
}

#[derive(Default)]
enum Press {
    #[default]
    Idle,
    Primary(LinkHit),
    Context {
        at: [f32; 2],
        url: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    Down,
    Up,
}

#[derive(Default)]
pub(super) struct Gesture {
    generation: u64,
    dispatch: Option<(Dispatch, u64)>,
    press: Press,
}

impl Gesture {
    pub(super) fn begin_down(&mut self, sequence: u64) -> Option<u64> {
        if self.dispatch == Some((Dispatch::Down, sequence)) {
            return None;
        }
        self.generation = sequence;
        self.press = Press::Idle;
        self.dispatch = Some((Dispatch::Down, sequence));
        Some(sequence)
    }

    pub(super) fn end_down(&mut self, generation: u64) {
        if self.dispatch == Some((Dispatch::Down, generation)) {
            self.dispatch = None;
            if matches!(self.press, Press::Context { .. }) {
                self.press = Press::Idle;
            }
        }
    }

    pub(super) fn record_left(&mut self, hit: LinkHit) {
        self.press = Press::Primary(hit);
    }

    pub(super) fn record_right(&mut self, at: [f32; 2], url: &str) {
        self.press = Press::Context {
            at,
            url: url.to_owned(),
        };
    }

    pub(super) fn take_right(&mut self, at: [f32; 2]) -> Option<String> {
        match std::mem::take(&mut self.press) {
            Press::Context { at: pressed, url } => (pressed == at).then_some(url),
            other => {
                self.press = other;
                None
            }
        }
    }

    pub(super) fn begin_up(&mut self, sequence: u64) -> Option<u64> {
        if self.dispatch == Some((Dispatch::Up, sequence)) {
            return None;
        }
        // A hidden press or release must not let an old target adopt this up.
        if sequence != self.generation.wrapping_add(1) {
            self.press = Press::Idle;
        }
        self.dispatch = Some((Dispatch::Up, sequence));
        Some(sequence)
    }

    pub(super) fn take_left(&mut self, hit: &LinkHit) -> bool {
        matches!(std::mem::take(&mut self.press), Press::Primary(pressed) if pressed.matches(hit))
    }

    pub(super) fn end_up(&mut self, generation: u64) {
        if self.dispatch == Some((Dispatch::Up, generation)) {
            self.press = Press::Idle;
            self.dispatch = None;
        }
    }
}
