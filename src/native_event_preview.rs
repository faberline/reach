//! Offline event construction only. No event handle escapes and no event is posted.
//! Coordinates are synthetic caller inputs, not a verified window mapping.
use crate::virtual_input::{Button, Intent, PlannedEvent};

#[derive(Debug, PartialEq)]
pub struct Preview {
    pub event_type: u32,
    pub modifier_flags: u64,
    pub key_code: Option<u16>,
    pub synthetic_global_point: Option<(f64, f64)>,
    pub synthetic_window_fields: Option<(u32, u32)>,
}
impl Preview {
    pub const fn action_allowed(&self) -> bool {
        false
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only used by the macOS sampler and tests
fn supported_modifier(code: u16) -> bool {
    matches!(code, 54..=56 | 58..=62)
}
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only used by the macOS sampler and tests
fn modifier_flags(bits: [u64; 2]) -> Result<u64, &'static str> {
    if [57, 63].iter().any(|code| bits[0] & (1u64 << code) != 0) {
        return Err("caps lock and fn are unsupported");
    }
    let mut flags = 0;
    for (keys, flag) in [
        ([56, 60], 1u64 << 17),
        ([59, 62], 1u64 << 18),
        ([58, 61], 1u64 << 19),
        ([54, 55], 1u64 << 20),
    ] {
        if keys.iter().any(|code| bits[0] & (1u64 << code) != 0) {
            flags |= flag;
        }
    }
    Ok(flags)
}
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only used by the macOS sampler and tests
type EventSpec = Result<(u32, u32, Option<(u16, bool)>, u64), &'static str>;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only used by the macOS sampler and tests
fn spec(plan: &PlannedEvent, global: (f64, f64)) -> EventSpec {
    if !global.0.is_finite() || !global.1.is_finite() {
        return Err("non-finite synthetic point");
    }
    let _ = modifier_flags(plan.keys_before)?;
    let flags = modifier_flags(plan.keys_after)?;
    let result = match plan.intent {
        Intent::Move => match plan.button_before {
            None => (5, 0, None),
            Some(Button::Left) => (6, 0, None),
            Some(Button::Right) => (7, 1, None),
            Some(Button::Middle) => (27, 2, None),
        },
        Intent::ButtonDown(Button::Left) => (1, 0, None),
        Intent::ButtonUp(Button::Left) => (2, 0, None),
        Intent::ButtonDown(Button::Right) => (3, 1, None),
        Intent::ButtonUp(Button::Right) => (4, 1, None),
        Intent::ButtonDown(Button::Middle) => (25, 2, None),
        Intent::ButtonUp(Button::Middle) => (26, 2, None),
        Intent::KeyDown(code) | Intent::KeyUp(code) if code > 127 || matches!(code, 57 | 63) => {
            return Err("key code unsupported")
        }
        Intent::KeyDown(code) if supported_modifier(code) => (12, 0, Some((code, true))),
        Intent::KeyUp(code) if supported_modifier(code) => (12, 0, Some((code, false))),
        Intent::KeyDown(code) => (10, 0, Some((code, true))),
        Intent::KeyUp(code) => (11, 0, Some((code, false))),
    };
    Ok((result.0, result.1, result.2, flags))
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::c_void;
    type Ref = *mut c_void;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceCreate(state: i32) -> Ref;
        fn CGEventCreateMouseEvent(source: Ref, kind: u32, point: Point, button: u32) -> Ref;
        fn CGEventCreateKeyboardEvent(source: Ref, code: u16, down: bool) -> Ref;
        fn CGEventGetType(event: Ref) -> u32;
        fn CGEventGetLocation(event: Ref) -> Point;
        fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
        fn CGEventSetIntegerValueField(event: Ref, field: u32, value: i64);
        fn CGEventSetFlags(event: Ref, flags: u64);
        fn CGEventGetFlags(event: Ref) -> u64;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const c_void);
    }
    struct Owned(Ref);
    impl Owned {
        fn new(ptr: Ref) -> Result<Self, &'static str> {
            if ptr.is_null() {
                Err("event allocation unavailable")
            } else {
                Ok(Self(ptr))
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }
    pub fn preview(
        plan: &PlannedEvent,
        global: (f64, f64),
        window_tag: Option<u32>,
    ) -> Result<Preview, &'static str> {
        let (kind, button, key, flags) = spec(plan, global)?;
        if window_tag == Some(0) || (window_tag.is_some() && key.is_some()) {
            return Err("synthetic window tag requires a pointer event and nonzero ID");
        }
        unsafe {
            let source = Owned::new(CGEventSourceCreate(-1))?;
            let event = match key {
                Some((code, down)) => Owned::new(CGEventCreateKeyboardEvent(source.0, code, down))?,
                None => Owned::new(CGEventCreateMouseEvent(
                    source.0,
                    kind,
                    Point {
                        x: global.0,
                        y: global.1,
                    },
                    button,
                ))?,
            };
            CGEventSetFlags(event.0, flags);
            if let Some(tag) = window_tag {
                CGEventSetIntegerValueField(event.0, 91, i64::from(tag));
                CGEventSetIntegerValueField(event.0, 92, i64::from(tag));
                if CGEventGetIntegerValueField(event.0, 91) != i64::from(tag)
                    || CGEventGetIntegerValueField(event.0, 92) != i64::from(tag)
                {
                    return Err("synthetic window fields differ from preview");
                }
            }
            if CGEventGetFlags(event.0) != flags {
                return Err("event flags differ from plan");
            }
            if CGEventGetType(event.0) != kind {
                return Err("event type differs from plan");
            }
            if let Some((code, _)) = key {
                if CGEventGetIntegerValueField(event.0, 9) != i64::from(code) {
                    return Err("event key differs from plan");
                }
                Ok(Preview {
                    event_type: kind,
                    modifier_flags: flags,
                    key_code: Some(code),
                    synthetic_global_point: None,
                    synthetic_window_fields: None,
                })
            } else {
                let p = CGEventGetLocation(event.0);
                if p.x != global.0 || p.y != global.1 {
                    return Err("event location differs from plan");
                }
                Ok(Preview {
                    event_type: kind,
                    modifier_flags: flags,
                    key_code: None,
                    synthetic_global_point: Some((p.x, p.y)),
                    synthetic_window_fields: window_tag.map(|tag| (tag, tag)),
                })
            }
        }
    }
}
/// Construct and immediately release a private-source event; never posts it.
#[cfg(target_os = "macos")]
pub fn preview(plan: &PlannedEvent, synthetic_global: (f64, f64)) -> Result<Preview, &'static str> {
    mac::preview(plan, synthetic_global, None)
}
/// Tests only synthetic pointer fields in memory; never sends or binds an event.
#[cfg(target_os = "macos")]
pub fn preview_tagged(
    plan: &PlannedEvent,
    synthetic_global: (f64, f64),
    synthetic_window_id: u32,
) -> Result<Preview, &'static str> {
    mac::preview(plan, synthetic_global, Some(synthetic_window_id))
}
#[cfg(not(target_os = "macos"))]
pub fn preview_tagged(_: &PlannedEvent, _: (f64, f64), _: u32) -> Result<Preview, &'static str> {
    Err("macOS required")
}
#[cfg(not(target_os = "macos"))]
pub fn preview(_: &PlannedEvent, _: (f64, f64)) -> Result<Preview, &'static str> {
    Err("macOS required")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{input_guard::WindowPoint, Target};
    fn plan(intent: Intent, button: Option<Button>) -> PlannedEvent {
        PlannedEvent {
            sequence: 1,
            target: Target {
                pid: 1,
                window_id: 2,
            },
            point_in_window_points: WindowPoint { x: 1.0, y: 2.0 },
            intent,
            button_before: button,
            button_after: button,
            keys_before: [0, 0],
            keys_after: [0, 0],
        }
    }
    #[test]
    fn maps_mouse_and_drag() {
        for (button, kind) in [
            (None, 5),
            (Some(Button::Left), 6),
            (Some(Button::Right), 7),
            (Some(Button::Middle), 27),
        ] {
            assert_eq!(
                spec(&plan(Intent::Move, button), (1.0, 2.0)).unwrap().0,
                kind
            );
        }
        assert_eq!(
            spec(&plan(Intent::ButtonDown(Button::Middle), None), (1.0, 2.0))
                .unwrap()
                .0,
            25
        );
        assert_eq!(
            spec(&plan(Intent::ButtonUp(Button::Middle), None), (1.0, 2.0))
                .unwrap()
                .0,
            26
        );
    }
    #[test]
    fn previews_modifier_flags_and_rejects_unsupported_keys() {
        let mut shift = plan(Intent::KeyDown(56), None);
        shift.keys_after[0] = 1 << 56;
        assert_eq!(
            spec(&shift, (1.0, 2.0)).unwrap(),
            (12, 0, Some((56, true)), 1 << 17)
        );
        let mut p = plan(Intent::KeyDown(0), None);
        p.keys_before[0] = 1 << 56;
        p.keys_after[0] = (1 << 56) | 1;
        assert_eq!(spec(&p, (1.0, 2.0)).unwrap().3, 1 << 17);
        let mut release = plan(Intent::KeyUp(56), None);
        release.keys_before[0] = (1 << 56) | (1 << 60);
        release.keys_after[0] = 1 << 60;
        assert_eq!(spec(&release, (1.0, 2.0)).unwrap().3, 1 << 17);
        release.keys_after[0] = 0;
        assert_eq!(spec(&release, (1.0, 2.0)).unwrap().3, 0);
        assert!(spec(&plan(Intent::KeyDown(57), None), (1.0, 2.0)).is_err());
        assert!(spec(&plan(Intent::KeyDown(63), None), (1.0, 2.0)).is_err());
        assert!(spec(&plan(Intent::Move, None), (f64::NAN, 2.0)).is_err());
        assert_eq!(
            spec(&plan(Intent::KeyUp(0), None), (1.0, 2.0)).unwrap().0,
            11
        );
    }
    #[test]
    #[ignore = "requires macOS window server; constructs but never posts events"]
    fn native_roundtrip_without_posting() {
        let tagged = preview_tagged(
            &plan(Intent::ButtonDown(Button::Left), None),
            (123.0, 456.0),
            0x123456,
        )
        .unwrap();
        assert_eq!(tagged.synthetic_window_fields, Some((0x123456, 0x123456)));
        assert!(!tagged.action_allowed());
        assert!(preview_tagged(&plan(Intent::KeyDown(0), None), (1.0, 2.0), 1).is_err());
        assert!(
            preview_tagged(&plan(Intent::ButtonDown(Button::Left), None), (1.0, 2.0), 0).is_err()
        );
        for (intent, button) in [
            (Intent::Move, None),
            (Intent::Move, Some(Button::Left)),
            (Intent::KeyDown(0), None),
            (Intent::KeyUp(0), None),
        ] {
            let result = preview(&plan(intent, button), (123.0, 456.0)).unwrap();
            assert!(!result.action_allowed());
        }
        let mut shift = plan(Intent::KeyDown(56), None);
        shift.keys_after[0] = 1 << 56;
        let result = preview(&shift, (123.0, 456.0)).unwrap();
        assert_eq!(result.event_type, 12);
        assert_eq!(result.modifier_flags, 1 << 17);
        let mut key = plan(Intent::KeyDown(0), None);
        key.keys_before[0] = 1 << 56;
        key.keys_after[0] = (1 << 56) | 1;
        assert_eq!(
            preview(&key, (123.0, 456.0)).unwrap().modifier_flags,
            1 << 17
        );
        for (code, flag) in [
            (54, 1 << 20),
            (55, 1 << 20),
            (56, 1 << 17),
            (60, 1 << 17),
            (58, 1 << 19),
            (61, 1 << 19),
            (59, 1 << 18),
            (62, 1 << 18),
        ] {
            let mut down = plan(Intent::KeyDown(code), None);
            down.keys_after[0] = 1 << code;
            let result = preview(&down, (123.0, 456.0)).unwrap();
            assert_eq!(
                (result.event_type, result.key_code, result.modifier_flags),
                (12, Some(code), flag)
            );
            let mut up = plan(Intent::KeyUp(code), None);
            up.keys_before[0] = 1 << code;
            let result = preview(&up, (123.0, 456.0)).unwrap();
            assert_eq!(
                (result.event_type, result.key_code, result.modifier_flags),
                (12, Some(code), 0)
            );
        }
    }
}
