use super::*;

#[test]
fn source_context_loads_full_project_larger_than_prompt_limit() {
    let root = super::super::tests::desktop_editor_fixture("large_source_snapshot");
    let body = format!(
        "function main(): i32 {{ // {}\n return 0; }}\n",
        "x".repeat(MAX_SOURCE_CONTEXT_BYTES)
    );
    std::fs::write(root.join("src/main.stasis"), &body).unwrap();
    let sources = super::super::desktop_source_context(&root).unwrap();
    assert!(serde_json::to_vec(&sources).unwrap().len() > MAX_SOURCE_CONTEXT_BYTES);
    let tools = ProposalTools {
        sources,
        ..ProposalTools::default()
    };
    assert!(tools.source_catalog().is_ok());
    assert!(tools.sources.iter().any(|item| item["source"]
        .as_str()
        .is_some_and(|source| source.contains(&"x".repeat(MAX_SOURCE_CONTEXT_BYTES)))));
    std::fs::remove_dir_all(root).unwrap();
}

fn source(symbol_id: &str, body: String) -> Value {
    json!({"target": {"file": "src/main.stasis", "name": symbol_id,
        "kind": "function", "owner": null, "signature": "() -> void",
        "symbol_id": symbol_id}, "source": body})
}

#[test]
fn source_context_catalog_bounds_large_snapshot_without_discarding_source() {
    let sources = vec![
        source("first", "a".repeat(140_000)),
        source("second", "b".repeat(140_000)),
    ];
    assert!(serde_json::to_vec(&sources).unwrap().len() > MAX_SOURCE_CONTEXT_BYTES);
    let mut tools = ProposalTools {
        sources: sources.clone(),
        ..ProposalTools::default()
    };
    let catalog = tools.source_catalog().unwrap();
    assert_eq!(catalog, json!([sources[0]["target"], sources[1]["target"]]));
    assert!(serde_json::to_vec(&catalog).unwrap().len() < 1024);
    let observations = tools.execute(
        &[
            ToolCall {
                tool: "read_source_symbol".into(),
                args: json!({"symbol_id": "first"}),
            },
            ToolCall {
                tool: "read_source_symbol".into(),
                args: json!({"symbol_id": "second"}),
            },
        ],
        &AtomicBool::new(false),
    );
    assert_eq!(observations[0].result.as_ref(), Some(&sources[0]));
    assert_eq!(observations[1].result.as_ref(), Some(&sources[1]));
    assert!(tools.proposals.is_empty());
}

#[test]
fn source_context_reads_reject_unknown_oversized_and_canceled_calls() {
    let mut tools = ProposalTools {
        sources: vec![source("large", "x".repeat(MAX_SOURCE_CONTEXT_BYTES))],
        ..ProposalTools::default()
    };
    for (tool, args, expected) in [
        (
            "read_source_symbol",
            json!({"symbol_id": "missing"}),
            "Unknown source symbol",
        ),
        (
            "read_source_symbol",
            json!({"symbol_id": "large"}),
            "256 KiB read limit",
        ),
        (
            "read_source_symbol",
            json!({}),
            "symbol_id must be a string",
        ),
        ("unknown", json!({}), "Unknown desktop editor tool"),
    ] {
        let result = tools.execute(
            &[ToolCall {
                tool: tool.into(),
                args,
            }],
            &AtomicBool::new(false),
        );
        assert!(result[0].error.as_ref().unwrap().contains(expected));
        assert!(result[0].result.is_none());
    }
    let result = tools.execute(
        &[ToolCall {
            tool: "read_source_symbol".into(),
            args: json!({"symbol_id": "large"}),
        }],
        &AtomicBool::new(true),
    );
    assert_eq!(result[0].error.as_deref(), Some("AI request canceled"));
    assert!(tools.proposals.is_empty());
}

#[test]
fn source_context_catalog_rejects_oversized_metadata_explicitly() {
    let tools = ProposalTools {
        sources: vec![source(&"x".repeat(MAX_SOURCE_CONTEXT_BYTES), String::new())],
        ..ProposalTools::default()
    };
    assert!(tools
        .source_catalog()
        .unwrap_err()
        .contains("symbol catalog exceeds 256 KiB"));
}

#[test]
fn source_context_agent_reads_before_proposing_without_applying() {
    use stasis_ai::{ModelProvider, ModelResponse};
    struct Provider {
        turn: usize,
    }
    impl ModelProvider for Provider {
        fn respond(&mut self, prompt: &str, _: &AtomicBool) -> Result<ModelResponse, String> {
            self.turn += 1;
            let (tool, args) = match self.turn {
                1 => {
                    assert!(prompt.contains("editable_symbols"));
                    assert!(!prompt.contains("return 7;"));
                    ("read_source_symbol", json!({"symbol_id": "value"}))
                }
                2 => {
                    assert!(prompt.contains("return 7;"));
                    (
                        "propose_semantic_edit",
                        json!({"proposal_id": "change", "description": "Update value", "batch": {"schema_version": 1, "edits": [{"operation": "update", "target": {"symbol_id": "value", "name": "value"}, "new_source": "function value(): i32 { return 8; }"}]}}),
                    )
                }
                3 => {
                    return Ok(ModelResponse::Done {
                        working_notes: "Inspect the exact source before proposing.".into(),
                        summary: "Ready for acceptance.".into(),
                    })
                }
                _ => panic!("unexpected provider turn"),
            };
            Ok(ModelResponse::ToolCalls {
                working_notes: "Inspect the exact source before proposing.".into(),
                summary: String::new(),
                tool_calls: vec![ToolCall {
                    tool: action_id_for_tool(tool),
                    args,
                }],
            })
        }
    }
    let original = source("value", "function value(): i32 { return 7; }".into());
    let mut tools = ProposalTools {
        sources: vec![original.clone()],
        ..ProposalTools::default()
    };
    let catalog = tools.source_catalog().unwrap();
    let result = run_agent_with_profile(
        &mut Provider { turn: 0 },
        &mut tools,
        &AgentProfile {
            max_turns: 4,
            ..AgentProfile::default()
        },
        "Update value",
        json!({"editable_symbols": catalog}),
        proposal_tool_specs(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(result, "Ready for acceptance.");
    assert_eq!(tools.proposals.len(), 1);
    assert_eq!(tools.sources, vec![original]);
}
