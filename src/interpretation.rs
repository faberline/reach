//! Pure rules for combining image-adapter results. No file or process access.
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const SCHEMA: &str = "cu-interpretation-evidence-v1";
pub const MAX_ADAPTERS: usize = 4;

// Interpreter names and versions are short labels; answers may be longer phrases.
const MAX_LABEL: usize = 128;
const MAX_ANSWER: usize = 1024;

/// Non-blank, bounded, and free of control characters.
fn valid_text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

/// Classify one adapter result that already passed `recognize_image`.
pub fn classify(raw: &Value) -> Value {
    let (Some(name), Some(version)) = (
        raw["interpreter"]["name"]
            .as_str()
            .filter(|s| valid_text(s, MAX_LABEL)),
        raw["interpreter"]["version"]
            .as_str()
            .filter(|s| valid_text(s, MAX_LABEL)),
    ) else {
        return json!({"outcome":"invalid_format","reason":"missing or invalid interpreter name or version","interpreter":null,"answer":null});
    };
    // Output keeps only the two interpreter fields; anything else stays in `raw`.
    let interpreter = json!({"name":name,"version":version});
    // A missing key and a null value are different: only null means abstention.
    match raw.as_object().and_then(|o| o.get("answer")) {
        Some(Value::Null) => {
            json!({"outcome":"abstained","reason":null,"interpreter":interpreter,"answer":null})
        }
        Some(Value::String(answer)) if valid_text(answer, MAX_ANSWER) => {
            json!({"outcome":"answered","reason":null,"interpreter":interpreter,"answer":answer})
        }
        _ => {
            json!({"outcome":"invalid_format","reason":"missing or invalid answer","interpreter":null,"answer":null})
        }
    }
}

/// The entry fields for an adapter that produced no trusted result.
pub fn failed(reason: &str) -> Value {
    json!({"outcome":"failed","reason":reason,"interpreter":null,"answer":null})
}

/// "disagree", "incomplete", "single" or "agree".
pub fn agreement(entries: &[Value]) -> &'static str {
    let answers: BTreeSet<&str> = entries
        .iter()
        .filter(|e| e["outcome"] == "answered")
        .filter_map(|e| e["answer"].as_str())
        .collect();
    if answers.len() > 1 {
        "disagree"
    } else if entries.is_empty() || entries.iter().any(|e| e["outcome"] != "answered") {
        "incomplete"
    } else if entries.len() == 1 {
        "single"
    } else {
        "agree"
    }
}

/// True only when the image stayed the same and every adapter answered or abstained.
pub fn complete(entries: &[Value], image_stable: bool) -> bool {
    image_stable
        && !entries.is_empty()
        && entries
            .iter()
            .all(|e| e["outcome"] == "answered" || e["outcome"] == "abstained")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, version: &str, answer: Value) -> Value {
        json!({"interpreter":{"name":name,"version":version},"answer":answer})
    }

    #[test]
    fn classify_accepts_answer_and_abstention() {
        let input = json!({"interpreter":{"name":"a","version":"1","extra":true},"answer":"victory","score":0.9});
        let v = classify(&input);
        assert_eq!(v["outcome"], "answered");
        assert_eq!(v["answer"], "victory");
        assert!(v["reason"].is_null());
        assert_eq!(v["interpreter"], json!({"name":"a","version":"1"}));
        let abstain = json!({"interpreter":{"name":"a","version":"1","extra":true},"answer":null,"score":0.9});
        let v = classify(&abstain);
        assert_eq!(v["outcome"], "abstained");
        assert!(v["answer"].is_null());
        assert_eq!(v["interpreter"], json!({"name":"a","version":"1"}));
        assert_eq!(v.as_object().unwrap().len(), 4);
    }

    #[test]
    fn classify_rejects_missing_or_invalid_fields() {
        let long_name = "a".repeat(129);
        let long_answer = "a".repeat(1025);
        let cases = vec![
            json!({"answer":"victory"}),
            json!({"interpreter":{"name":"a","version":""},"answer":"victory"}),
            json!({"interpreter":{"name":long_name,"version":"1"},"answer":"victory"}),
            json!({"interpreter":{"name":1,"version":"1"},"answer":"victory"}),
            json!({"interpreter":{"name":"a","version":"1"}}),
            json!({"interpreter":{"name":"a","version":"1"},"answer":42}),
            json!({"interpreter":{"name":"a","version":"1"},"answer":" "}),
            json!({"interpreter":{"name":"a","version":"1"},"answer":long_answer}),
            json!({"interpreter":{"name":"a","version":"1"},"answer":"a\nb"}),
        ];
        for case in cases {
            let v = classify(&case);
            assert_eq!(v["outcome"], "invalid_format", "{case}");
            assert!(
                v["reason"].as_str().is_some_and(|r| !r.is_empty()),
                "{case}"
            );
            assert!(v["answer"].is_null(), "{case}");
            assert!(v["interpreter"].is_null(), "{case}");
        }
    }

    #[test]
    fn agreement_reports_single_agree_disagree_incomplete() {
        assert_eq!(agreement(&[]), "incomplete");
        let one = [classify(&raw("a", "1", json!("victory")))];
        assert_eq!(agreement(&one), "single");
        let same = [
            classify(&raw("a", "1", json!("victory"))),
            classify(&raw("b", "1", json!("victory"))),
        ];
        assert_eq!(agreement(&same), "agree");
        let different = [
            classify(&raw("a", "1", json!("victory"))),
            classify(&raw("b", "1", json!("defeat"))),
        ];
        assert_eq!(agreement(&different), "disagree");
        let abstained = [
            classify(&raw("a", "1", json!("victory"))),
            classify(&raw("b", "1", Value::Null)),
        ];
        assert_eq!(agreement(&abstained), "incomplete");
        let with_failure = [classify(&raw("a", "1", json!("victory"))), failed("x")];
        assert_eq!(agreement(&with_failure), "incomplete");
        let three = [
            classify(&raw("a", "1", json!("victory"))),
            classify(&raw("b", "1", json!("defeat"))),
            failed("x"),
        ];
        assert_eq!(agreement(&three), "disagree");
    }

    #[test]
    fn complete_requires_stable_image_and_valid_outcomes() {
        let answered_abstained = [
            classify(&raw("a", "1", json!("victory"))),
            classify(&raw("b", "1", Value::Null)),
        ];
        assert!(complete(&answered_abstained, true));
        assert!(!complete(&answered_abstained, false));
        let with_failure = [classify(&raw("a", "1", json!("victory"))), failed("x")];
        assert!(!complete(&with_failure, true));
        let with_invalid = [
            classify(&raw("a", "1", json!("victory"))),
            classify(&json!({"answer":"victory"})),
        ];
        assert!(!complete(&with_invalid, true));
        assert!(!complete(&[], true));
    }

    #[test]
    fn failed_entry_has_exactly_four_keys() {
        let v = failed("boom");
        assert_eq!(
            v,
            json!({"outcome":"failed","reason":"boom","interpreter":null,"answer":null})
        );
    }
}
