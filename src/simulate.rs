//! Synthetic input rehearsal for an agent. No OS window, screenshot or event is used.
use crate::{
    input_guard::{CurrentWindow, Delivery, Destination, InputRequest, Observation, WindowPoint},
    virtual_input::{Button, Intent, VirtualSession},
    Rect, Target, Window,
};
use serde_json::{json, Value};
use std::time::Instant;

fn keys(v: &Value, expected: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|o| o.len() == expected.len() && expected.iter().all(|k| o.contains_key(*k)))
}
fn coordinate(v: &Value, name: &str) -> Result<f64, String> {
    v[name]
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("invalid {name}"))
}
fn button(v: &Value) -> Result<Button, String> {
    match v["button"].as_str() {
        Some("left") => Ok(Button::Left),
        Some("right") => Ok(Button::Right),
        Some("middle") => Ok(Button::Middle),
        _ => Err("invalid button".into()),
    }
}
fn intent(v: &Value) -> Result<Intent, String> {
    match v["kind"].as_str() {
        Some("move") if keys(v, &["kind", "x", "y"]) => Ok(Intent::Move),
        Some("button_down") if keys(v, &["kind", "x", "y", "button"]) => {
            Ok(Intent::ButtonDown(button(v)?))
        }
        Some("button_up") if keys(v, &["kind", "x", "y", "button"]) => {
            Ok(Intent::ButtonUp(button(v)?))
        }
        Some(kind @ ("key_down" | "key_up")) if keys(v, &["kind", "x", "y", "keycode"]) => {
            let key = v["keycode"]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n <= 127)
                .ok_or("invalid keycode")?;
            Ok(if kind == "key_down" {
                Intent::KeyDown(key)
            } else {
                Intent::KeyUp(key)
            })
        }
        _ => Err("invalid simulated event".into()),
    }
}
/// Every field is fictional; a successful rehearsal never proves an operation safe.
pub fn run(width: f64, height: f64, events: &[Value]) -> Result<Value, String> {
    run_mode(width, height, events, false, None)
}

/// Uses synthetic coordinates to construct and immediately release native events.
/// No native event is posted, and native objects are created only after the
/// entire synthetic sequence has passed validation.
pub fn run_native(width: f64, height: f64, events: &[Value]) -> Result<Value, String> {
    run_mode(width, height, events, true, None)
}

/// Preview pointer events with a fictional window tag. Never posts an event.
pub fn run_native_tagged(
    width: f64,
    height: f64,
    events: &[Value],
    tag: u32,
) -> Result<Value, String> {
    if tag == 0
        || events.iter().any(|e| {
            !matches!(
                e["kind"].as_str(),
                Some("move" | "button_down" | "button_up")
            )
        })
    {
        return Err("synthetic tag requires a nonzero ID and pointer-only sequence".into());
    }
    run_mode(width, height, events, true, Some(tag))
}

fn run_mode(
    width: f64,
    height: f64,
    events: &[Value],
    native: bool,
    tag: Option<u32>,
) -> Result<Value, String> {
    if !width.is_finite()
        || !height.is_finite()
        || !(1.0..=32768.0).contains(&width)
        || !(1.0..=32768.0).contains(&height)
        || events.is_empty()
        || events.len() > 32
    {
        return Err("invalid synthetic dimensions or event count".into());
    }
    let target = Target::new(1, 1)?;
    let window = Window {
        target,
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        },
        layer: Some(0),
        title: None,
    };
    let now = Instant::now();
    let current = CurrentWindow {
        window: window.clone(),
        process_start_unix_seconds: 1.0,
    };
    let obs = Observation::new(window, 1.0, "0".repeat(64), now, 1)
        .map_err(|e| format!("synthetic observation: {e:?}"))?;
    let mut session = VirtualSession::new(current.clone(), now)
        .map_err(|e| format!("synthetic session: {e:?}"))?;
    let mut report = Vec::new();
    let mut plans = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let request = InputRequest {
            destination: Destination::Window(target),
            delivery: Delivery::Background,
            point: WindowPoint {
                x: coordinate(event, "x")?,
                y: coordinate(event, "y")?,
            },
            observation: obs.token(),
        };
        let action = intent(event)?;
        let plan = session
            .plan((index + 1) as u64, &request, &obs, &current, now, action)
            .map_err(|e| format!("event {} rejected: {e:?}", index + 1))?;
        report.push(json!({"sequence":plan.sequence,"kind":event["kind"],"button_before":format!("{:?}",plan.button_before),"button_after":format!("{:?}",plan.button_after),"held_key_bits_before":plan.keys_before,"held_key_bits_after":plan.keys_after}));
        plans.push(plan);
    }
    session
        .finish()
        .map_err(|e| format!("session rejected: {e:?}"))?;
    if native {
        for (item, plan) in report.iter_mut().zip(&plans) {
            let point = plan.point_in_window_points;
            let native = match tag {
                Some(window) => {
                    crate::native_event_preview::preview_tagged(plan, (point.x, point.y), window)
                }
                None => crate::native_event_preview::preview(plan, (point.x, point.y)),
            }
            .map_err(|e| format!("native preview failed: {e}"))?;
            item["native_type"] = native.event_type.into();
            item["native_modifier_flags"] = native.modifier_flags.into();
            if let Some((first, second)) = native.synthetic_window_fields {
                item["synthetic_window_fields"] = json!({"field_91":first,"field_92":second});
            }
        }
    }
    Ok(
        json!({"status":if tag.is_some() {"tagged_native_memory_only"} else if native {"native_memory_only"} else {"synthetic_only"},"action_allowed":false,"events":report,
        "note":"Fictional identity, observation and coordinates. Native mode only constructs and releases events. No event is posted. No real input is authorized."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn balanced_rehearsal_succeeds_without_authority() {
        let e = vec![
            json!({"kind":"move","x":10,"y":10}),
            json!({"kind":"button_down","button":"left","x":10,"y":10}),
            json!({"kind":"move","x":30,"y":20}),
            json!({"kind":"button_up","button":"left","x":30,"y":20}),
        ];
        let v = run(100.0, 80.0, &e).unwrap();
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["events"].as_array().unwrap().len(), 4);
    }
    #[test]
    fn unbalanced_and_outside_stop() {
        assert!(run(
            100.,
            80.,
            &[json!({"kind":"button_up","button":"left","x":1,"y":1})]
        )
        .is_err());
        assert!(run(100., 80., &[json!({"kind":"move","x":100,"y":1})]).is_err());
        assert!(run(
            100.,
            80.,
            &[json!({"kind":"key_down","keycode":0,"x":1,"y":1})]
        )
        .is_err());
    }
    #[test]
    fn invalid_and_extra_fields_rejected() {
        assert!(run(100., 80., &[json!({"kind":"move","x":1,"y":1,"extra":2})]).is_err());
        assert!(run(100., 80., &[]).is_err());
        assert!(run(100., 80., &vec![json!({"kind":"move","x":1,"y":1}); 33]).is_err());
        assert!(run_native_tagged(100., 80., &[json!({"kind":"move","x":1,"y":1})], 0).is_err());
        assert!(run_native_tagged(
            100.,
            80.,
            &[json!({"kind":"key_down","x":1,"y":1,"keycode":0})],
            123
        )
        .is_err());
    }
}
