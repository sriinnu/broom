//! Laya: the backup when Jev is unavailable, unreachable or unsure — and it needs no
//! key from the user: the request goes out with no Authorization header unless an
//! optional one happens to be configured (env `LAYA_API_KEY`, or the keychain slot,
//! or `AI_GATEWAY_API_KEY` for Vercel's gateway). It answers in chat text rather than
//! typed decisions, so the ask is stricter — a fixed JSON shape parsed strictly — and
//! it self-reports match strength instead of calibrated probabilities. Any drift
//! from the shape is a `BadResponse`, which leaves the exact matches in charge.
//! Endpoint and model follow the local kosha registry's route and both are
//! env-overridable.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{AskContext, CandidateGroup, Http, Intent, Provider, ProviderError, Scope, Understanding, laya_key};

pub const BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";
pub const MODEL: &str = "convaiinnovations/laya";

pub struct Laya {
    http: Arc<dyn Http>,
    base_url: String,
    model: String,
}

impl Laya {
    pub fn new(http: Arc<dyn Http>) -> Self {
        Self {
            http,
            base_url: std::env::var("LAYA_BASE_URL").unwrap_or_else(|_| BASE_URL.into()),
            model: std::env::var("LAYA_MODEL").unwrap_or_else(|_| MODEL.into()),
        }
    }
}

impl Provider for Laya {
    /// Always: the backup asks nothing of the user. If the route turns us away,
    /// that surfaces as `Unauthorized` and the exact matches stay in charge.
    fn configured(&self) -> bool {
        true
    }

    fn understand(&self, query: &str, ctx: &AskContext, groups: &[CandidateGroup]) -> Result<Understanding, ProviderError> {
        if groups.is_empty() {
            return Err(ProviderError::BadResponse("nothing to choose between".into()));
        }
        let reply = self
            .http
            .post_json(&format!("{}/chat/completions", self.base_url), laya_key().as_deref(), &body(query, ctx, groups, &self.model))?;
        let content = reply
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::BadResponse("no reply text".into()))?;
        parse(content, groups.len())
    }
}

fn body(query: &str, ctx: &AskContext, groups: &[CandidateGroup], model: &str) -> Value {
    let listed = groups
        .iter()
        .enumerate()
        .map(|(i, g)| format!("{i}. {} — {}", g.label, g.detail))
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "model": model,
        "temperature": 0.0,
        "messages": [
            {
                "role": "system",
                "content": "You map a disk-cleanup request onto groups of files that were found by exact matching. Reply with ONLY a JSON object, no prose, no code fences: {\"intent\": \"delete\" or \"locate\", \"target\": <group number as a string, or \"none\">, \"targets\": [every group number the request refers to, as strings], \"scope\": \"focused\" or \"whole\", \"match\": \"high\", \"medium\" or \"none\"}. Judge by what the words mean, not by group size."
            },
            {
                "role": "user",
                "content": format!(
                    "Request: {query}\nScan root: {}\nFolder in view: {}\n\nGroups:\n{listed}\n\nintent: delete if the request asks to remove, clean up or free space; otherwise locate. target: the group the request is about, or \"none\". targets: all groups the request is about, in case it names several. scope: focused if the request is about the folder in view, otherwise whole. match: high if the words clearly point at one group, medium if they only might, none if nothing fits.",
                    ctx.root_name(),
                    ctx.focused_name().unwrap_or_else(|| "(the scan root)".into()),
                )
            }
        ]
    })
}

fn parse(content: &str, groups_len: usize) -> Result<Understanding, ProviderError> {
    let Some(json_text) = json_part(content) else {
        return Err(ProviderError::BadResponse("reply was not JSON".into()));
    };
    let parsed: Value = serde_json::from_str(json_text).map_err(|e| ProviderError::BadResponse(short(&e.to_string())))?;
    let word = |key: &str| parsed.get(key).and_then(Value::as_str).unwrap_or("");
    let intent = match word("intent") {
        "delete" => Intent::Delete,
        "locate" => Intent::Locate,
        _ => return Err(ProviderError::BadResponse("no intent in answer".into())),
    };
    let scope = match word("scope") {
        "focused" => Scope::FocusedFolder,
        "whole" => Scope::WholeTree,
        _ => return Err(ProviderError::BadResponse("no scope in answer".into())),
    };
    let target = word("target");
    let group = target
        .parse::<usize>()
        .map_or(usize::MAX, |ix| if ix < groups_len { ix } else { usize::MAX });
    // Self-reported match strength stands in for Jev's calibrated numbers: only "high"
    // clears the bar that lets a delete pre-select anything.
    let strength = match word("match") {
        "high" => 0.9,
        "medium" => 0.5,
        _ => 0.0,
    };
    let mut groups: Vec<usize> = parsed
        .get("targets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().and_then(|s| s.parse::<usize>().ok()).or_else(|| v.as_u64().map(|n| n as usize)))
                .filter(|&ix| ix < groups_len)
                .collect()
        })
        .unwrap_or_default();
    if group != usize::MAX && !groups.contains(&group) {
        groups.insert(0, group);
    }
    Ok(Understanding { intent, group, groups, scope, confidence: strength, top_probability: strength, provider: "Laya" })
}

/// The JSON object in a reply that may be wrapped in prose or code fences.
fn json_part(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start < end).then(|| &text[start..=end])
}

fn short(message: &str) -> String {
    message.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Safety, Source};

    #[test]
    fn parses_a_strict_answer() {
        let content = r#"{"intent": "delete", "target": "0", "scope": "whole", "match": "high"}"#;
        let u = parse(content, 2).unwrap();
        assert_eq!(u.intent, Intent::Delete);
        assert_eq!(u.scope, Scope::WholeTree);
        assert_eq!(u.group, 0);
        assert!(u.confidence >= 0.65 && u.top_probability >= 0.55, "high is actionable");

        let medium = parse(r#"{"intent": "delete", "target": "0", "scope": "whole", "match": "medium"}"#, 2).unwrap();
        assert!(medium.confidence < 0.65, "medium only ever suggests");
    }

    #[test]
    fn tolerates_wrapping_and_rejects_drift() {
        let fenced = "```json\n{\"intent\": \"locate\", \"target\": \"1\", \"scope\": \"focused\", \"match\": \"high\"}\n```";
        let u = parse(fenced, 2).unwrap();
        assert_eq!(u.intent, Intent::Locate);
        assert_eq!(u.group, 1);
        assert_eq!(u.scope, Scope::FocusedFolder);

        assert_eq!(parse(r#"{"intent": "delete", "target": "none", "scope": "whole", "match": "none"}"#, 2).unwrap().group, usize::MAX);
        assert_eq!(parse(r#"{"intent": "delete", "target": "9", "scope": "whole", "match": "high"}"#, 2).unwrap().group, usize::MAX);
        assert!(parse("I think you want node_modules", 2).is_err());
        assert!(parse(r#"{"intent": "zap"}"#, 2).is_err());
    }

    #[test]
    fn body_lists_groups_and_pins_the_shape() {
        let ctx = AskContext { scan_root: "/".into(), focused: Some("/Users/sriinnu/Downloads".into()) };
        let groups = vec![CandidateGroup {
            source: Source::Folder,
            label: ".dmg files · 3 GB in 2 places".into(),
            detail: "Files by their extension".into(),
            nodes: vec![1, 2],
            total: 3_000_000_000,
            safety: Safety::Review,
        }];
        let body = body("delete the installers here", &ctx, &groups, "test-model");
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["temperature"], 0.0);
        let user = body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("0. .dmg files"), "{user}");
        // The provider sees names relative to the root, never an absolute path.
        assert!(user.contains("Users/sriinnu/Downloads"), "{user}");
        assert!(!user.contains("/Users/sriinnu/Downloads"), "{user}");
    }

    /// One real request through the gateway, keyless if nothing is configured,
    /// contract-checking what parsing accepts. Prints the parsed understanding
    /// only. `cargo test -- --ignored laya_live`.
    #[test]
    #[ignore = "spends a real API call"]
    fn laya_live_roundtrip() {
        use super::super::UreqHttp;
        let laya = Laya::new(Arc::new(UreqHttp));
        let ctx = AskContext { scan_root: "/".into(), focused: None };
        let groups = vec![CandidateGroup {
            source: Source::Folder,
            label: "node_modules · 4 GB in 2 places".into(),
            detail: "JavaScript dependencies".into(),
            nodes: vec![1],
            total: 4_000_000_000,
            safety: Safety::Review,
        }];
        match laya.understand("delete node_modules", &ctx, &groups) {
            Ok(u) => println!("Laya answered: intent={:?} group={} match-strength={:.2}", u.intent, u.group, u.confidence),
            Err(ProviderError::Unauthorized) => eprintln!("keyless: the gateway wants a key right now"),
            Err(error) => panic!("live Laya call failed: {error}"),
        }
    }
}
