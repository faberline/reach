//! Pure in-memory Core Graphics construction and readback. No events are posted.
#![cfg(target_os = "macos")]
use std::ffi::c_void;

type CfRef = *mut c_void;
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Point {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state: i32) -> CfRef;
    fn CGEventCreateMouseEvent(source: CfRef, kind: u32, point: Point, button: u32) -> CfRef;
    fn CGEventCreateKeyboardEvent(source: CfRef, code: u16, down: bool) -> CfRef;
    fn CGEventGetType(event: CfRef) -> u32;
    fn CGEventGetLocation(event: CfRef) -> Point;
    fn CGEventGetIntegerValueField(event: CfRef, field: u32) -> i64;
    fn CGEventSetIntegerValueField(event: CfRef, field: u32, value: i64);
    fn CGEventGetFlags(event: CfRef) -> u64;
}

#[test]
#[ignore = "explicit native memory test; constructs only and never posts events"]
fn synthetic_window_fields_roundtrip_without_posting() {
    unsafe {
        let source = Owned::new(CGEventSourceCreate(-1));
        let event = Owned::new(CGEventCreateMouseEvent(
            source.0,
            1,
            Point { x: 123.0, y: 456.0 },
            0,
        ));
        // Test values are synthetic and are not looked up as real window IDs.
        let target = 0x123456_i64;
        CGEventSetIntegerValueField(event.0, 91, target);
        CGEventSetIntegerValueField(event.0, 92, target);
        assert_eq!(CGEventGetIntegerValueField(event.0, 91), target);
        assert_eq!(CGEventGetIntegerValueField(event.0, 92), target);
        CGEventSetIntegerValueField(event.0, 91, target + 1);
        assert_eq!(CGEventGetIntegerValueField(event.0, 91), target + 1);
        assert_eq!(CGEventGetIntegerValueField(event.0, 92), target);
        assert_eq!(CGEventGetType(event.0), 1);
    }
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: CfRef);
}

struct Owned(CfRef);
impl Owned {
    fn new(value: CfRef) -> Self {
        assert!(!value.is_null());
        Self(value)
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

#[test]
#[ignore = "explicit native memory test; private CGEvent source may be unavailable in sandbox"]
fn construct_and_read_mouse_events_without_posting() {
    unsafe {
        // -1 is kCGEventSourceStatePrivate. Coordinates below are synthetic
        // Core Graphics global coordinates, not a validated window mapping.
        let source = Owned::new(CGEventSourceCreate(-1));
        for (kind, button) in [
            (5, 0),
            (1, 0),
            (6, 0),
            (2, 0),
            (3, 1),
            (7, 1),
            (4, 1),
            (25, 2),
            (27, 2),
            (26, 2),
        ] {
            let p = Point { x: 123.0, y: 456.0 };
            let event = Owned::new(CGEventCreateMouseEvent(source.0, kind, p, button));
            assert_eq!(CGEventGetType(event.0), kind);
            let returned = CGEventGetLocation(event.0);
            assert_eq!((returned.x, returned.y), (p.x, p.y));
            if matches!(kind, 25..=27) {
                assert_eq!(CGEventGetIntegerValueField(event.0, 3), 2);
            }
        }
    }
}

#[test]
#[ignore = "explicit native memory test; private CGEvent source may be unavailable in sandbox"]
fn construct_and_read_virtual_keycodes_without_posting() {
    unsafe {
        let source = Owned::new(CGEventSourceCreate(-1));
        for (code, down, kind) in [
            (0, true, 10),
            (0, false, 11),
            (127, true, 10),
            (127, false, 11),
        ] {
            let event = Owned::new(CGEventCreateKeyboardEvent(source.0, code, down));
            assert_eq!(CGEventGetType(event.0), kind);
            assert_eq!(CGEventGetIntegerValueField(event.0, 9), i64::from(code));
        }
        // Modifier handling differs from ordinary text keys. Record what the
        // local OS constructs, but do not infer correct modifier delivery.
        for down in [true, false] {
            let event = Owned::new(CGEventCreateKeyboardEvent(source.0, 56, down));
            let kind = CGEventGetType(event.0);
            assert!([10, 11, 12].contains(&kind));
            assert_eq!(CGEventGetIntegerValueField(event.0, 9), 56);
            println!(
                "shift down={down}, event_type={kind}, flags={:#x}",
                CGEventGetFlags(event.0)
            );
        }
        // Constructing a modifier and then a regular key is still only a
        // data test; no assumption is made that modifier state is propagated.
        let _shift = Owned::new(CGEventCreateKeyboardEvent(source.0, 56, true));
        let key = Owned::new(CGEventCreateKeyboardEvent(source.0, 0, true));
        println!("subsequent key event flags={:#x}", CGEventGetFlags(key.0));
    }
}
