//! Report structural consistency of capture geometry; never an input mapping.
use serde_json::{json, Value};
fn check(v: &Value) -> Result<f64, &'static str> {
    if v["capture_type"] != "desktop_independent_window" {
        return Err("unsupported_capture_type");
    }
    let scale = v["point_pixel_scale"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0 && *n <= 16.0)
        .ok_or("invalid_scale")?;
    let frame = &v["window_frame_points"];
    let content = &v["filter_content_rect_points"];
    for key in ["x", "y", "width", "height"] {
        let a = frame[key]
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or("invalid_frame")?;
        let b = content[key]
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or("invalid_content_rect")?;
        if a != b {
            return Err("content_rect_differs_from_window");
        }
    }
    for (side, pixels) in [("width", "image_width"), ("height", "image_height")] {
        let n = content[side]
            .as_f64()
            .filter(|n| *n > 0.0)
            .ok_or("invalid_dimension")?;
        let computed = (n * scale).ceil();
        if !computed.is_finite()
            || !(1.0..=8192.0).contains(&computed)
            || v[pixels].as_u64() != Some(computed as u64)
        {
            return Err("scaled_dimensions_mismatch");
        }
    }
    Ok(scale)
}
/// Metadata may be stale, incomplete or wrong; no result provides usable coordinates.
pub fn inspect(v: &Value) -> Value {
    match check(v) {
        Ok(scale) => {
            json!({"status":"metadata_consistent_unverified","point_pixel_scale":scale,"action_allowed":false,"note":"Metadata agreement does not calibrate coordinates, prove image content or authorize input."})
        }
        Err(reason) => json!({"status":"unknown","reason":reason,"action_allowed":false}),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> Value {
        json!({"capture_type":"desktop_independent_window","point_pixel_scale":2,"window_frame_points":{"x":-1102,"y":0,"width":1102,"height":742},"filter_content_rect_points":{"x":-1102,"y":0,"width":1102,"height":742},"image_width":2204,"image_height":1484})
    }
    #[test]
    fn matches_metadata_only() {
        let v = inspect(&example());
        assert_eq!(v["status"], "metadata_consistent_unverified");
        assert_eq!(v["action_allowed"], false)
    }
    #[test]
    fn mismatches_and_missing_data_are_unknown() {
        let mut v = example();
        v["image_width"] = 2203.into();
        assert_eq!(inspect(&v)["reason"], "scaled_dimensions_mismatch");
        let mut v = example();
        v["filter_content_rect_points"]["x"] = 0.into();
        assert_eq!(inspect(&v)["reason"], "content_rect_differs_from_window");
        let mut v = example();
        v["point_pixel_scale"] = Value::Null;
        assert_eq!(inspect(&v)["reason"], "invalid_scale");
    }
    #[test]
    fn fractional_scale_uses_capture_rounding() {
        let mut v = example();
        v["point_pixel_scale"] = 1.5.into();
        v["window_frame_points"]["width"] = 101.into();
        v["filter_content_rect_points"]["width"] = 101.into();
        v["image_width"] = 152.into();
        v["image_height"] = 1113.into();
        assert_eq!(inspect(&v)["status"], "metadata_consistent_unverified");
    }
}
