//! Read-only Core Graphics display inventory. This does not calibrate input.
use serde_json::{json, Value};
use std::collections::HashSet;
#[cfg(target_os = "macos")]
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
struct Display {
    id: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    pixels_wide: usize,
    pixels_high: usize,
    rotation: f64,
    mirrors: u32,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only used by the macOS sampler and tests
fn validate(main: u32, displays: &[Display]) -> Result<(), &'static str> {
    if main == 0 || displays.is_empty() || displays.len() > 32 {
        return Err("invalid_display_count_or_main");
    }
    let mut ids = HashSet::new();
    for d in displays {
        if d.id == 0
            || !ids.insert(d.id)
            || !d.x.is_finite()
            || !d.y.is_finite()
            || !d.width.is_finite()
            || d.width <= 0.0
            || !d.height.is_finite()
            || d.height <= 0.0
            || !(d.x + d.width).is_finite()
            || !(d.y + d.height).is_finite()
            || d.pixels_wide == 0
            || d.pixels_high == 0
            || !d.rotation.is_finite()
            || d.mirrors == d.id
        {
            return Err("invalid_display_metadata");
        }
    }
    if !ids.contains(&main) {
        return Err("main_display_missing");
    }
    Ok(())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only used by the macOS sampler and tests
fn compare(
    main_before: u32,
    before: &[Display],
    main_after: u32,
    after: &[Display],
) -> Result<Value, &'static str> {
    validate(main_before, before)?;
    validate(main_after, after)?;
    if main_before != main_after || before != after {
        return Err("display_inventory_changed");
    }
    let displays = after.iter().map(|d| json!({
        "display_id":d.id,"bounds_cg_global":{"x":d.x,"y":d.y,"width":d.width,"height":d.height},
        "pixels_wide":d.pixels_wide,"pixels_high":d.pixels_high,"rotation_degrees":d.rotation,
        "mirrors_display_id":if d.mirrors == 0 { None } else { Some(d.mirrors) }
    })).collect::<Vec<_>>();
    Ok(
        json!({"status":"observed","action_allowed":false,"main_display_id":main_after,
        "coordinate_system":"core_graphics_global", "active_displays":displays,
        "scope":"active_displays_only_online_mirrors_may_be_omitted",
        "inventory":"two_matching_sequential_samples_not_atomic",
        "coordinate_calibration":"unverified", "pixel_mapping":"unverified",
        "display_identity_persistent":"unverified"}),
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Display;
    use std::ptr;
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
    struct Rect {
        origin: Point,
        size: Size,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(id: u32) -> Rect;
        fn CGDisplayPixelsWide(id: u32) -> usize;
        fn CGDisplayPixelsHigh(id: u32) -> usize;
        fn CGDisplayRotation(id: u32) -> f64;
        fn CGDisplayMirrorsDisplay(id: u32) -> u32;
    }
    pub fn sample() -> Result<(u32, Vec<Display>), &'static str> {
        unsafe {
            let mut count = 0;
            if CGGetActiveDisplayList(0, ptr::null_mut(), &mut count) != 0
                || count == 0
                || count > 32
            {
                return Err("active_display_count_unavailable");
            }
            // The header allows no more than max entries. Recheck count afterwards.
            let mut ids = vec![0u32; count as usize];
            let mut received = 0;
            if CGGetActiveDisplayList(count, ids.as_mut_ptr(), &mut received) != 0
                || received != count
            {
                return Err("active_display_list_changed");
            }
            let mut count_after = 0;
            if CGGetActiveDisplayList(0, ptr::null_mut(), &mut count_after) != 0
                || count_after != count
            {
                return Err("active_display_count_changed");
            }
            let main = CGMainDisplayID();
            let mut displays = ids
                .into_iter()
                .map(|id| {
                    let b = CGDisplayBounds(id);
                    Display {
                        id,
                        x: b.origin.x,
                        y: b.origin.y,
                        width: b.size.width,
                        height: b.size.height,
                        pixels_wide: CGDisplayPixelsWide(id),
                        pixels_high: CGDisplayPixelsHigh(id),
                        rotation: CGDisplayRotation(id),
                        mirrors: CGDisplayMirrorsDisplay(id),
                    }
                })
                .collect::<Vec<_>>();
            displays.sort_by_key(|d| d.id);
            Ok((main, displays))
        }
    }
}

pub fn observe() -> Result<Value, &'static str> {
    #[cfg(target_os = "macos")]
    {
        let monotonic_start = Instant::now();
        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|v| v.as_millis());
        let (main_before, before) = macos::sample()?;
        let (main_after, after) = macos::sample()?;
        let mut result = compare(main_before, &before, main_after, &after)?;
        result["started_unix_ms"] = json!(start);
        result["completed_unix_ms"] = json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|v| v.as_millis()));
        result["wall_clock"] = json!("system_unverified");
        result["elapsed_ms"] = json!(monotonic_start.elapsed().as_millis());
        result["duration_clock"] = json!("monotonic");
        Ok(result)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("macos_required")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn one() -> Display {
        Display {
            id: 1,
            x: -1200.0,
            y: 0.0,
            width: 1200.0,
            height: 800.0,
            pixels_wide: 2400,
            pixels_high: 1600,
            rotation: 0.0,
            mirrors: 0,
        }
    }
    #[test]
    fn stable_inventory_never_authorizes_input() {
        let d = one();
        let value = compare(1, std::slice::from_ref(&d), 1, std::slice::from_ref(&d)).unwrap();
        assert_eq!(value["action_allowed"], false);
        assert_eq!(value["coordinate_calibration"], "unverified");
    }
    #[test]
    fn changes_and_invalid_data_fail_closed() {
        let a = one();
        let mut b = a.clone();
        b.x += 1.0;
        assert_eq!(
            compare(1, std::slice::from_ref(&a), 1, &[b]),
            Err("display_inventory_changed")
        );
        assert_eq!(
            validate(2, std::slice::from_ref(&a)),
            Err("main_display_missing")
        );
        assert_eq!(
            validate(1, &[a.clone(), a]),
            Err("invalid_display_metadata")
        );
        let mut bad = one();
        bad.width = f64::NAN;
        assert_eq!(validate(1, &[bad]), Err("invalid_display_metadata"));
    }
}
