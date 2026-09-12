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
fn compact_catalog_lists_imports_and_names_but_loads_details_on_demand() {
    let root = super::super::tests::desktop_editor_fixture("compact_catalog");
    std::fs::write(root.join("src/main.stasis"), "import \"helper.stasis\";\nstruct Player { hp: i32; }\nglobal player: Player;\nconst LIMIT: i32 = 12;\nfunction main(): i32 { return 0; }\n").unwrap();
    std::fs::write(
        root.join("src/helper.stasis"),
        "function helper(): void {}\n",
    )
    .unwrap();
    let tools = ProposalTools {
        sources: super::super::desktop_source_context(&root).unwrap(),
        ..ProposalTools::default()
    };
    let catalog = tools.source_catalog().unwrap();
    let text = catalog.as_str().unwrap();
    assert!(text.contains("imports\n    helper.stasis\n"));
    assert!(text.contains("globals/constants player, LIMIT\n"));
    assert!(text.contains("struct Player\n"));
    assert!(text.contains("function main\n"));
    assert!(!text.contains("hp"));
    assert!(!text.contains("signature"));
    assert!(!text.contains("symbol_id"));
    for (index, original) in tools.sources.iter().enumerate() {
        let read = tools
            .read_source_symbol(&json!({"symbol_id": format!("s{index}")}))
            .unwrap();
        assert_eq!(&read, original);
        assert!(read["target"]["symbol_id"]
            .as_str()
            .unwrap()
            .starts_with("v1|"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compact_read_ids_disambiguate_duplicate_names_and_reject_invalid_ids() {
    let mut first = source("canonical-first", "function same(): void {}".into());
    let mut second = source("canonical-second", "function same(x: i32): void {}".into());
    first["target"]["name"] = json!("same");
    second["target"]["name"] = json!("same");
    let tools = ProposalTools {
        sources: vec![first.clone(), second.clone()],
        ..ProposalTools::default()
    };
    assert_eq!(
        tools.source_catalog().unwrap(),
        "src/main.stasis\n  s0 function same\n  s1 function same\n"
    );
    assert_eq!(
        tools
            .read_source_symbol(&json!({"symbol_id":"s0"}))
            .unwrap(),
        first
    );
    assert_eq!(
        tools
            .read_source_symbol(&json!({"symbol_id":"s1"}))
            .unwrap(),
        second
    );
    for id in ["s2", "s01", "s-1", "s9999999999999999999999999999"] {
        assert!(tools.read_source_symbol(&json!({"symbol_id":id})).is_err());
    }
}

#[test]
fn compact_catalog_escapes_newlines_in_names_without_creating_entries() {
    let mut item = source("canonical", String::new());
    item["target"]["name"] = json!("first\n  s1 function forged");
    let tools = ProposalTools {
        sources: vec![item],
        ..ProposalTools::default()
    };
    let catalog = tools.source_catalog().unwrap();
    assert_eq!(catalog.as_str().unwrap().lines().count(), 2);
    assert!(catalog.as_str().unwrap().contains("first\\n"));
}

#[test]
fn compact_catalog_defers_test_names_and_preserves_exact_read_targets() {
    let mut first = source(
        "first-test",
        "test `first case`(): bool { return true; }".into(),
    );
    first["target"]["kind"] = json!("test");
    first["target"]["name"] = json!("first case");
    let mut second = source(
        "second-test",
        "test `second case`(): bool { return false; }".into(),
    );
    second["target"]["kind"] = json!("test");
    second["target"]["name"] = json!("second case");
    let tools = ProposalTools {
        sources: vec![first.clone(), second.clone()],
        ..ProposalTools::default()
    };
    assert_eq!(
        tools.source_catalog().unwrap(),
        "src/main.stasis\n  t0 tests (2; read to list names)\n"
    );
    let names = tools
        .read_source_symbol(&json!({"symbol_id":"t0"}))
        .unwrap();
    assert_eq!(names["tests"], "  s0 first case\n  s1 second case\n");
    assert!(!names.to_string().contains("return"));
    assert_eq!(
        tools
            .read_source_symbol(&json!({"symbol_id":"s1"}))
            .unwrap(),
        second
    );
    assert!(tools
        .read_source_symbol(&json!({"symbol_id":"t2"}))
        .is_err());
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
    assert_eq!(
        catalog,
        "src/main.stasis\n  s0 function first\n  s1 function second\n"
    );
    assert!(serde_json::to_vec(&catalog).unwrap().len() < 1024);
    let observations = tools.execute(
        &[
            ToolCall {
                tool: "read_source_symbol".into(),
                args: json!({"symbol_id": "s0"}),
            },
            ToolCall {
                tool: "read_source_symbol".into(),
                args: json!({"symbol_id": "s1"}),
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
                    ("read_source_symbol", json!({"symbol_id": "s0"}))
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

#[test]
fn deferred_test_catalog_keeps_the_read_size_limit() {
    let mut item = source("test", String::new());
    item["target"]["kind"] = json!("test");
    item["target"]["name"] = json!("x".repeat(MAX_SOURCE_CONTEXT_BYTES));
    let tools = ProposalTools {
        sources: vec![item],
        ..ProposalTools::default()
    };
    assert!(tools.source_catalog().is_ok());
    assert!(tools
        .read_source_symbol(&json!({"symbol_id":"t0"}))
        .unwrap_err()
        .contains("256 KiB read limit"));
}
