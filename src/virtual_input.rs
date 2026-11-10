//! Agent-directed virtual input, offline planning only.
//!
//! Each result is a description of a hypothetical window-targeted event.
//! This module never loads a driver, sends input, moves the physical cursor,
//! changes focus, verifies observation provenance, or authorizes an action.
//! A real backend must separately prove target/coordinate binding, isolation,
//! delivery, post-action state, cancellation, and held-input cleanup.
use crate::{
    input_guard::{self, CurrentWindow, Denial, InputRequest, Observation, WindowPoint},
    Target,
};
use std::{collections::HashSet, time::Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// macOS virtual key codes are identifiers, not Unicode text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Move,
    ButtonDown(Button),
    ButtonUp(Button),
    KeyDown(u16),
    KeyUp(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    AlreadyStopped,
    Sequence,
    ClockRollback,
    ObservationRegressed,
    TargetChanged,
    Preflight(Denial),
    PositionUnknownOrChanged,
    ButtonAlreadyHeld,
    ButtonNotHeld,
    InvalidKeyCode,
    KeyAlreadyHeld,
    KeyNotHeld,
    TooManyKeys,
    InputsStillHeld,
}

/// Caller-supplied, unverified plan. It is not a delivered event or capability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedEvent {
    pub sequence: u64,
    pub target: Target,
    pub point_in_window_points: WindowPoint,
    pub intent: Intent,
    /// Virtual state before and after this event; never a report of real input.
    pub button_before: Option<Button>,
    pub button_after: Option<Button>,
    /// Bit n represents held macOS virtual keycode n (0 through 127).
    pub keys_before: [u64; 2],
    pub keys_after: [u64; 2],
}
impl PlannedEvent {
    pub const fn action_allowed(&self) -> bool {
        false
    }
}

/// Holds a hypothetical device state in this process only. Never use it as
/// proof of delivered events or as the state of the user's real devices.
pub struct VirtualSession {
    target: Target,
    initial_window: CurrentWindow,
    next_sequence: u64,
    last_time: Instant,
    last_observation: Option<Observation>,
    position: Option<WindowPoint>,
    button: Option<Button>,
    keys: HashSet<u16>,
    active: bool,
}
impl VirtualSession {
    fn key_bits(&self) -> [u64; 2] {
        let mut bits = [0_u64; 2];
        for code in &self.keys {
            bits[usize::from(*code) / 64] |= 1_u64 << (*code % 64);
        }
        bits
    }

    pub fn new(initial_window: CurrentWindow, now: Instant) -> Result<Self, StopReason> {
        let target = initial_window.window.target;
        if Target::new(target.pid, target.window_id).is_err()
            || !initial_window.process_start_unix_seconds.is_finite()
            || initial_window.process_start_unix_seconds <= 0.0
        {
            return Err(StopReason::TargetChanged);
        }
        Ok(Self {
            target,
            initial_window,
            next_sequence: 1,
            last_time: now,
            last_observation: None,
            position: None,
            button: None,
            keys: HashSet::new(),
            active: true,
        })
    }

    /// Validate a single Agent request and update virtual state. All inputs are
    /// caller assertions. Every error terminates this simulated session.
    pub fn plan(
        &mut self,
        sequence: u64,
        request: &InputRequest,
        latest: &Observation,
        current: &CurrentWindow,
        now: Instant,
        intent: Intent,
    ) -> Result<PlannedEvent, StopReason> {
        let result = self.check(sequence, request, latest, current, now, intent);
        if result.is_err() {
            self.active = false;
        }
        result
    }

    fn check(
        &mut self,
        sequence: u64,
        request: &InputRequest,
        latest: &Observation,
        current: &CurrentWindow,
        now: Instant,
        intent: Intent,
    ) -> Result<PlannedEvent, StopReason> {
        if !self.active {
            return Err(StopReason::AlreadyStopped);
        }
        if sequence != self.next_sequence || sequence == u64::MAX {
            return Err(StopReason::Sequence);
        }
        if now < self.last_time {
            return Err(StopReason::ClockRollback);
        }
        if current != &self.initial_window {
            return Err(StopReason::TargetChanged);
        }
        let checked =
            input_guard::review(request, latest, current, now).map_err(StopReason::Preflight)?;
        debug_assert!(!checked.action_allowed());
        if let Some(previous) = &self.last_observation {
            if latest.capture_sequence() < previous.capture_sequence()
                || latest.captured_at() < previous.captured_at()
                || (latest.capture_sequence() == previous.capture_sequence() && latest != previous)
            {
                return Err(StopReason::ObservationRegressed);
            }
        }
        let button_before = self.button;
        let keys_before = self.key_bits();
        match intent {
            Intent::Move => self.position = Some(request.point),
            Intent::ButtonDown(button) => {
                if self.button.is_some() {
                    return Err(StopReason::ButtonAlreadyHeld);
                }
                // A window-targeted down carries its own position. Do not require
                // an earlier move: some background apps discard move events.
                self.position = Some(request.point);
                self.button = Some(button);
            }
            Intent::ButtonUp(button) => {
                if self.button != Some(button) {
                    return Err(StopReason::ButtonNotHeld);
                }
                if self.position != Some(request.point) {
                    return Err(StopReason::PositionUnknownOrChanged);
                }
                self.button = None;
            }
            Intent::KeyDown(code) => {
                if code > 127 {
                    return Err(StopReason::InvalidKeyCode);
                }
                if self.keys.contains(&code) {
                    return Err(StopReason::KeyAlreadyHeld);
                }
                if self.keys.len() >= 8 {
                    return Err(StopReason::TooManyKeys);
                }
                self.keys.insert(code);
            }
            Intent::KeyUp(code) => {
                if code > 127 {
                    return Err(StopReason::InvalidKeyCode);
                }
                if !self.keys.remove(&code) {
                    return Err(StopReason::KeyNotHeld);
                }
            }
        }
        self.last_time = now;
        self.last_observation = Some(latest.clone());
        self.next_sequence += 1;
        Ok(PlannedEvent {
            sequence,
            target: self.target,
            point_in_window_points: request.point,
            intent,
            button_before,
            button_after: self.button,
            keys_before,
            keys_after: self.key_bits(),
        })
    }

    /// End the hypothetical session. Outstanding simulated holds are not
    /// silently cleared as if a real backend had delivered corresponding ups.
    pub fn finish(&mut self) -> Result<(), StopReason> {
        if !self.active {
            return Err(StopReason::AlreadyStopped);
        }
        self.active = false;
        if self.button.is_some() || !self.keys.is_empty() {
            return Err(StopReason::InputsStillHeld);
        }
        Ok(())
    }

    pub const fn action_allowed(&self) -> bool {
        false
    }
}
