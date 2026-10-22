//! Jev (typesafe.ai System One): typed answers with probabilities, one request. It
//! judges only what the query *means* — delete or locate, which candidate group, whole
//! scan or folder in view. Finding the files and every safety rule stay in code.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{AskContext, CandidateGroup, Http, Intent, Provider, ProviderError, Scope, Understanding, jev_key};

pub const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

pub struct Jev {
    pub http: Arc<dyn Http>,
}

impl Provider for Jev {
    fn configured(&self) -> bool {
        jev_key().is_some()
    }

    fn understand(&self, query: &str, ctx: &AskContext, groups: &[CandidateGroup]) -> Result<Understanding, ProviderError> {
        let key = jev_key().ok_or(ProviderError::NoKey)?;
        if groups.is_empty() {
            return Err(ProviderError::BadResponse("nothing to choose between".into()));
        }
        let reply = self.http.post_json(ENDPOINT, Some(&key), &body(query, ctx, groups))?;
        parse(&reply, groups.len())
    }
}

fn body(query: &str, ctx: &AskContext, groups: &[CandidateGroup]) -> Value {
    let mut body = body_core(query, ctx, groups);
    if let Some(questions) = body.get_mut("questions").and_then(Value::as_object_mut) {
        for (i, g) in groups.iter().enumerate() {
            questions.insert(
                format!("g{i}"),
                json!({
                    "type": "choice",
                    "instructions": format!("Does the request refer to this group of files: {} ({})?", g.label, g.detail),
                    "criteria": {
                        "yes": "The user's words name or describe these files.",
                        "no": "The user's words do not refer to these files."
                    }
                }),
            );
        }
    }
    body
}

fn body_core(query: &str, ctx: &AskContext, groups: &[CandidateGroup]) -> Value {
    let mut target: serde_json::Map<String, Value> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| (i.to_string(), json!(format!("{} — {}", g.label, g.detail))))
        .collect();
    target.insert("none".into(), json!("No group matches the query"));

    json!({
        "state": {
            "query": query,
            "scan_root": ctx.root_name(),
            "focused_folder": ctx.focused_name().unwrap_or_default(),
            "note": "Groups were found by exact matching in the app. Judge only what the words refer to.",
            "groups": groups.iter().enumerate().map(|(i, g)| json!({
                "id": i.to_string(),
                "label": g.label,
                "detail": g.detail,
            })).collect::<Vec<_>>(),
        },
        "model": "jev-latest",
        "questions": {
            "intent": {
                "type": "choice",
                "instructions": "Does the user want the matched files deleted, or only found and listed?",
                "criteria": {
                    "delete": "The user's words ask for removal, cleanup, clearing or freeing of disk space.",
                    "locate": "The user's words ask to find, list, show or measure files, with nothing about removing them."
                }
            },
            "target": {
                "type": "choice",
                "instructions": "Which group of files does the query refer to? Judge by the names, file types and places the user wrote, not by group size. Pick \"none\" if no group matches.",
                "criteria": target
            },
            "scope": {
                "type": "choice",
                "instructions": "Should this cover the whole scan, or only the folder currently in view?",
                "criteria": {
                    "focused": "Only inside the folder in view; the query names that folder or says here / this folder.",
                    "whole": "Anywhere in the scan; the query names no folder, or a folder other than the one in view."
                }
            }
        }
    })
}

fn parse(reply: &Value, groups_len: usize) -> Result<Understanding, ProviderError> {
    let answers = reply.get("answers").ok_or_else(|| ProviderError::BadResponse("no answers".into()))?;
    let choice = |id: &str| {
        answers
            .pointer(&format!("/{id}/choice"))
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::BadResponse(format!("no {id} answer")))
    };
    let intent = match choice("intent")? {
        "delete" => Intent::Delete,
        "locate" => Intent::Locate,
        other => return Err(ProviderError::BadResponse(format!("unknown intent {other:?}"))),
    };
    let scope = match choice("scope")? {
        "focused" => Scope::FocusedFolder,
        "whole" => Scope::WholeTree,
        other => return Err(ProviderError::BadResponse(format!("unknown scope {other:?}"))),
    };
    let chosen = choice("target")?;
    // An id outside the offered groups counts as no match, not an error.
    let group = chosen
        .parse::<usize>()
        .map_or(usize::MAX, |ix| if ix < groups_len { ix } else { usize::MAX });
    let confidence = answers.pointer("/target/confidence").and_then(Value::as_f64).unwrap_or(0.0);
    let top_probability = answers
        .pointer("/target/probabilities")
        .and_then(|p| p.get(chosen))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let mut groups: Vec<usize> = (0..groups_len)
        .filter(|i| {
            let yes = answers.pointer(&format!("/g{i}/choice")).and_then(Value::as_str) == Some("yes");
            let p = answers.pointer(&format!("/g{i}/probabilities/yes")).and_then(Value::as_f64).unwrap_or(0.0);
            yes && p >= super::ACT_PROBABILITY
        })
        .collect();
    if group != usize::MAX && !groups.contains(&group) {
        groups.insert(0, group);
    }
    Ok(Understanding { intent, group, groups, scope, confidence, top_probability, provider: "Jev" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Safety, Source};
    use serde_json::json;

    #[test]
    fn body_lists_groups_with_a_none_choice() {
        let ctx = AskContext { scan_root: "/".into(), focused: None };
        let groups = vec![CandidateGroup {
            source: Source::Folder,
            label: "node_modules · 4 GB in 2 places".into(),
            detail: "JavaScript dependencies".into(),
            nodes: vec![1],
            total: 4_000_000_000,
            safety: Safety::Review,
        }];
        let body = body("delete node_modules", &ctx, &groups);
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"]["groups"][0]["id"], "0");
        let criteria = &body["questions"]["target"]["criteria"];
        assert_eq!(criteria["0"], "node_modules · 4 GB in 2 places — JavaScript dependencies");
        assert!(criteria.get("none").is_some());
    }

    #[test]
    fn parses_answers_with_probabilities() {
        let reply = json!({
            "answers": {
                "intent": { "choice": "delete", "confidence": 0.9 },
                "target": { "choice": "1", "confidence": 0.8, "probabilities": { "0": 0.1, "1": 0.8, "none": 0.1 } },
                "scope": { "choice": "whole" }
            }
        });
        let u = parse(&reply, 2).unwrap();
        assert_eq!(u.intent, Intent::Delete);
        assert_eq!(u.scope, Scope::WholeTree);
        assert_eq!(u.group, 1);
        assert_eq!(u.confidence, 0.8);
        assert_eq!(u.top_probability, 0.8);
        assert_eq!(u.provider, "Jev");
    }

    #[test]
    fn none_and_out_of_range_are_no_match() {
        let base = json!({
            "answers": {
                "intent": { "choice": "locate" },
                "scope": { "choice": "focused" },
                "target": { "choice": "none", "probabilities": { "0": 0.2, "1": 0.3, "none": 0.5 } }
            }
        });
        assert_eq!(parse(&base, 2).unwrap().group, usize::MAX);

        let out_of_range = json!({
            "answers": {
                "intent": { "choice": "locate" },
                "scope": { "choice": "focused" },
                "target": { "choice": "7", "probabilities": { "0": 0.2, "1": 0.3, "none": 0.5 } }
            }
        });
        assert_eq!(parse(&out_of_range, 2).unwrap().group, usize::MAX, "an id past the end is treated as none");
    }

    #[test]
    fn a_broken_answer_is_an_error_not_a_guess() {
        assert!(parse(&json!({ "answers": {} }), 2).is_err());
        assert!(parse(&json!({}), 2).is_err());
        let unknown = json!({
            "answers": {
                "intent": { "choice": "zap" },
                "scope": { "choice": "whole" },
                "target": { "choice": "0", "probabilities": { "0": 1.0 } }
            }
        });
        assert!(parse(&unknown, 2).is_err());
    }

    #[test]
    fn parses_with_group_count() {
        // The clamped parse: same answer, different group counts offered.
        let reply = json!({
            "answers": {
                "intent": { "choice": "delete" },
                "scope": { "choice": "whole" },
                "target": { "choice": "1", "probabilities": { "0": 0.1, "1": 0.9 } }
            }
        });
        assert_eq!(parse(&reply, 2).unwrap().group, 1);
        assert_eq!(parse(&reply, 1).unwrap().group, usize::MAX, "id past the end is none");
    }
}
