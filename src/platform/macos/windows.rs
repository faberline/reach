//! Read-only Core Graphics window inventory, front to back.
use crate::{Rect, Target, Window};
use std::ffi::c_void;

type CFRef = *const c_void;
#[repr(C)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
struct CGRect {
    origin: Point,
    size: Size,
}
#[repr(C)]
struct CFRange {
    location: isize,
    length: isize,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative: u32) -> CFRef;
    fn CGWindowListCreateDescriptionFromArray(ids: CFRef) -> CFRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFRef, out: *mut CGRect) -> bool;
    static kCGWindowNumber: CFRef;
    static kCGWindowOwnerPID: CFRef;
    static kCGWindowBounds: CFRef;
    static kCGWindowLayer: CFRef;
    static kCGWindowName: CFRef;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayCreate(
        allocator: CFRef,
        values: *const CFRef,
        count: isize,
        callbacks: CFRef,
    ) -> CFRef;
    fn CFArrayGetCount(array: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
    fn CFDictionaryGetValue(dict: CFRef, key: CFRef) -> CFRef;
    fn CFNumberGetValue(number: CFRef, number_type: i32, value: *mut c_void) -> bool;
    fn CFGetTypeID(value: CFRef) -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(value: CFRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetBytes(
        value: CFRef,
        range: CFRange,
        encoding: u32,
        loss_byte: u8,
        external: bool,
        buffer: *mut u8,
        max_len: isize,
        used: *mut isize,
    ) -> isize;
    fn CFRelease(value: CFRef);
}

// Window names are optional. Missing, malformed or oversized names remain
// unknown; never use a truncated name as a window identity.
unsafe fn optional_title(value: CFRef) -> Option<String> {
    if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
        return None;
    }
    let length = CFStringGetLength(value);
    if !(0..=4096).contains(&length) {
        return None;
    }
    let max = CFStringGetMaximumSizeForEncoding(length, 0x08000100);
    if !(0..=16384).contains(&max) {
        return None;
    }
    let mut buf = vec![0_u8; max as usize + 1];
    let mut used = 0;
    let read = CFStringGetBytes(
        value,
        CFRange {
            location: 0,
            length,
        },
        0x08000100,
        0,
        false,
        buf.as_mut_ptr(),
        max,
        &mut used,
    );
    if read != length || used < 0 || used > max {
        return None;
    }
    buf.truncate(used as usize);
    String::from_utf8(buf).ok()
}

unsafe fn optional_layer(value: CFRef) -> Option<i32> {
    let mut layer = 0_i32;
    if !value.is_null()
        && CFGetTypeID(value) == CFNumberGetTypeID()
        && CFNumberGetValue(value, 3, &mut layer as *mut _ as *mut c_void)
    {
        Some(layer)
    } else {
        None
    }
}

pub fn onscreen_windows(pid: Option<i32>) -> Result<Vec<Window>, String> {
    window_inventory(pid, 1, false).map(|(windows, _)| windows)
}

/// All WindowServer candidates; this does not imply visibility or capture support.
pub fn all_windows(pid: i32) -> Result<(Vec<Window>, usize), String> {
    window_inventory(Some(pid), 0, true)
}

/// Query only the WindowServer candidate identified by the supplied window ID.
pub fn exact_window(target: Target) -> Result<Window, String> {
    Target::new(target.pid, target.window_id)?;
    unsafe {
        // Quartz window ID arrays contain IDs encoded as pointer-sized values,
        // not CFNumber objects. Null callbacks prevent retain/release of the ID.
        let raw_id = target.window_id as usize as CFRef;
        let ids = CFArrayCreate(std::ptr::null(), &raw_id, 1, std::ptr::null());
        if ids.is_null() {
            return Err("cannot create window ID array".into());
        }
        let array = CGWindowListCreateDescriptionFromArray(ids);
        CFRelease(ids);
        if array.is_null() {
            return Err("window server unavailable".into());
        }
        let result = parse(array, Some(target.pid), false);
        CFRelease(array);
        let (windows, _) = result?;
        if windows.len() != 1 || windows[0].target != target {
            return Err("target window missing or ambiguous".into());
        }
        Ok(windows.into_iter().next().unwrap())
    }
}

fn window_inventory(
    pid: Option<i32>,
    options: u32,
    omit_invalid: bool,
) -> Result<(Vec<Window>, usize), String> {
    if pid.is_some_and(|p| p <= 0) {
        return Err("PID must be positive".into());
    }
    unsafe {
        let array = CGWindowListCopyWindowInfo(options, 0);
        if array.is_null() {
            return Err("window server unavailable".into());
        }
        let result = parse(array, pid, omit_invalid);
        CFRelease(array);
        result
    }
}

unsafe fn parse(
    array: CFRef,
    pid: Option<i32>,
    omit_invalid: bool,
) -> Result<(Vec<Window>, usize), String> {
    if CFGetTypeID(array) != CFArrayGetTypeID() {
        return Err("invalid window list".into());
    }
    let mut result = Vec::new();
    let mut omitted = 0;
    for index in 0..CFArrayGetCount(array) {
        let dict = CFArrayGetValueAtIndex(array, index);
        if dict.is_null() || CFGetTypeID(dict) != CFDictionaryGetTypeID() {
            return Err("invalid window entry".into());
        }
        let id_ref = CFDictionaryGetValue(dict, kCGWindowNumber);
        let pid_ref = CFDictionaryGetValue(dict, kCGWindowOwnerPID);
        let bounds_ref = CFDictionaryGetValue(dict, kCGWindowBounds);
        let layer_ref = CFDictionaryGetValue(dict, kCGWindowLayer);
        let name_ref = CFDictionaryGetValue(dict, kCGWindowName);
        let (mut id, mut owner) = (0_i32, 0_i32);
        if pid_ref.is_null()
            || CFGetTypeID(pid_ref) != CFNumberGetTypeID()
            || !CFNumberGetValue(pid_ref, 3, &mut owner as *mut _ as *mut c_void)
            || owner <= 0
        {
            return Err("invalid window owner".into());
        }
        if pid.is_some_and(|wanted| wanted != owner) {
            continue;
        }
        let mut bounds = CGRect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 0.0,
                height: 0.0,
            },
        };
        if id_ref.is_null()
            || bounds_ref.is_null()
            || CFGetTypeID(id_ref) != CFNumberGetTypeID()
            || CFGetTypeID(bounds_ref) != CFDictionaryGetTypeID()
            || !CFNumberGetValue(id_ref, 3, &mut id as *mut _ as *mut c_void)
            || id <= 0
            || !CGRectMakeWithDictionaryRepresentation(bounds_ref, &mut bounds)
        {
            if omit_invalid {
                omitted += 1;
                continue;
            }
            return Err("invalid target window metadata".into());
        }
        let rect = Rect {
            x: bounds.origin.x,
            y: bounds.origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        };
        if !rect.valid() {
            if omit_invalid {
                omitted += 1;
                continue;
            }
            return Err("invalid target window bounds".into());
        }
        let layer = optional_layer(layer_ref);
        result.push(Window {
            target: Target::new(owner, id as u32)?,
            bounds: rect,
            layer,
            title: optional_title(name_ref),
        });
    }
    Ok((result, omitted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::CString, ptr};
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: CFRef, s: *const i8, encoding: u32) -> CFRef;
        fn CFNumberCreate(allocator: CFRef, number_type: i32, value: *const c_void) -> CFRef;
    }
    #[test]
    fn optional_fields_keep_unknown_and_signed_layer() {
        unsafe {
            assert_eq!(optional_layer(ptr::null()), None);
            assert_eq!(optional_title(ptr::null()), None);
            let text = CString::new("視窗標題 café").unwrap();
            let title = CFStringCreateWithCString(ptr::null(), text.as_ptr(), 0x08000100);
            assert!(!title.is_null());
            assert_eq!(optional_title(title).as_deref(), Some("視窗標題 café"));
            assert_eq!(optional_layer(title), None);
            let raw: i32 = -2147483603;
            let number = CFNumberCreate(ptr::null(), 3, &raw as *const _ as *const c_void);
            assert!(!number.is_null());
            assert_eq!(optional_layer(number), Some(raw));
            assert_eq!(optional_title(number), None);
            CFRelease(title);
            CFRelease(number);
            let long_text = CString::new("x".repeat(4097)).unwrap();
            let long_title = CFStringCreateWithCString(ptr::null(), long_text.as_ptr(), 0x08000100);
            assert!(!long_title.is_null());
            assert_eq!(optional_title(long_title), None);
            CFRelease(long_title);
        }
    }
}
