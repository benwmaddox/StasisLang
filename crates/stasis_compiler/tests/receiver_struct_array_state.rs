use stasis_compiler::backend::development_swap::{
    commit_development_swap, DevelopmentSwapDescriptor, DevelopmentSwapHost, DevelopmentSwapStatus,
};
use stasis_compiler::backend::jit::{JitProcess, JitScalarValue};
use std::collections::BTreeMap;

const SOURCE_PATH: &str = "tests/stasis/seams/receiver_struct_array_probe.stasis";
const SOURCE: &str = include_str!("../../../tests/stasis/seams/receiver_struct_array_probe.stasis");
const ROOT: &str = "receiver_struct_array_probe";

fn compile(source: &str) -> JitProcess {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&[ROOT.to_string(), "receiver_dynamic_parent".to_string()]);
    process.upsert_file(SOURCE_PATH, source);
    process
        .compile()
        .expect("compile receiver struct-array probe");
    process
}

#[derive(Default)]
struct NoopHost {
    stages: usize,
}

impl DevelopmentSwapHost for NoopHost {
    type Staged = ();

    fn stage(
        &mut self,
        _candidate: &JitProcess,
        _descriptor: &DevelopmentSwapDescriptor,
    ) -> Result<Self::Staged, String> {
        self.stages += 1;
        Ok(())
    }

    fn publish(&mut self, _staged: &mut Self::Staged) -> Result<(), String> {
        Ok(())
    }

    fn restore(&mut self, _staged: Self::Staged) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn nested_receiver_calls_use_each_owners_struct_array_storage() {
    let process = compile(SOURCE);
    assert_eq!(process.execute_i32_noarg_by_name(ROOT), Ok(0));

    for (path, index, parent, local_x) in [
        ("receiver_left.bones", 0, -1, 1.25),
        ("receiver_left.bones", 23, 22, 23.25),
        ("receiver_right.bones", 0, 7, 91.5),
        ("receiver_right.bones", 23, 8, 92.5),
    ] {
        assert_eq!(
            process.read_global_collection_scalar(path, "parent", index),
            Ok(JitScalarValue::I32(parent)),
            "{path}.parent"
        );
        assert_eq!(
            process.read_global_collection_scalar(path, "local_x", index),
            Ok(JitScalarValue::F32(local_x)),
            "{path}.local_x"
        );
    }

    let summaries = process.function_data_flow_summaries();
    let setter = summaries
        .iter()
        .find(|summary| summary.function == "receiver_set_bone")
        .expect("receiver setter data-flow summary");
    assert!(setter
        .direct
        .parameter_writes
        .contains(&"self.bones[*].parent".to_string()));
    assert!(setter
        .direct
        .parameter_writes
        .contains(&"self.bones[*].local_x".to_string()));
    let getter = summaries
        .iter()
        .find(|summary| summary.function == "receiver_parent")
        .expect("receiver getter data-flow summary");
    assert_eq!(getter.direct.parameter_reads, vec!["self.bones[*].parent"]);

    let root = summaries
        .iter()
        .find(|summary| summary.function == ROOT)
        .expect("root data-flow summary");
    for path in [
        "receiver_left.bones[*].parent",
        "receiver_left.bones[*].local_x",
        "receiver_right.bones[*].parent",
        "receiver_right.bones[*].local_x",
    ] {
        assert!(
            root.aggregate.writes.contains(&path.to_string()),
            "aggregate writes contain {path}: {:?}",
            root.aggregate.writes
        );
        assert!(
            root.aggregate.reads.contains(&path.to_string()),
            "aggregate reads contain {path}: {:?}",
            root.aggregate.reads
        );
    }
}

#[test]
fn state_layout_has_two_owner_collections_with_scalar_field_lanes() {
    let process = compile(SOURCE);
    let layout = process.state_layout();

    let state_paths = layout
        .scalars
        .iter()
        .map(|entry| entry.path.as_str())
        .chain(layout.collections.iter().map(|entry| entry.path.as_str()))
        .chain(layout.structs.iter().map(|entry| entry.path.as_str()))
        .chain(layout.opaque.iter().map(|entry| entry.path.as_str()))
        .collect::<Vec<_>>();
    assert!(
        state_paths.iter().all(|path| !path.starts_with("self")),
        "receiver parameter must not become state: {state_paths:?}"
    );

    let expected_fields = [
        ("active", "bool", 4),
        ("byte_lane", "u8", 1),
        ("local_x", "f32", 4),
        ("parent", "i32", 4),
        ("precise", "f64", 8),
        ("short_lane", "u16", 2),
        ("wide_lane", "u32", 4),
        ("world_angle", "f32", 4),
    ];
    let report = process
        .state_memory_report(&BTreeMap::new(), u64::MAX)
        .expect("state memory report");
    for path in ["receiver_left.bones", "receiver_right.bones"] {
        let collection = layout
            .collections
            .iter()
            .find(|collection| collection.path == path)
            .unwrap_or_else(|| panic!("layout collection {path}"));
        assert_eq!(collection.capacity, 24, "{path} capacity");
        assert!(collection.fully_migratable, "{path} migration support");
        assert_eq!(
            collection
                .fields
                .iter()
                .map(|field| (field.field.as_str(), field.storage_type_name()))
                .collect::<Vec<_>>(),
            expected_fields
                .iter()
                .map(|(field, storage, _)| (*field, *storage))
                .collect::<Vec<_>>(),
            "{path} scalar lanes"
        );
        for (field, _, element_bytes) in expected_fields {
            let entry = report
                .entries
                .iter()
                .find(|entry| entry.path == path && entry.field == field)
                .unwrap_or_else(|| panic!("memory entry {path}.{field}"));
            assert_eq!(entry.capacity, 24);
            assert_eq!(entry.element_bytes, element_bytes);
            assert_eq!(entry.capacity_bytes, 24 * element_bytes);
        }
    }
}

#[test]
fn capacity_change_reemits_unchanged_receiver_accessor() {
    let active = compile(SOURCE);
    let active_accessor = active
        .program_snapshot()
        .expect("active program snapshot")
        .functions()
        .iter()
        .find(|function| function.name == "receiver_parent")
        .expect("active receiver accessor");
    let active_id = active_accessor.id;
    let active_body_hash = active_accessor.body_hash;

    let capacity_source = SOURCE.replace(
        "const RECEIVER_BONE_CAPACITY: i32 = 24;",
        "const RECEIVER_BONE_CAPACITY: i32 = 25;",
    );
    let mut candidate = active.staged_candidate();
    candidate.upsert_file(SOURCE_PATH, capacity_source);
    candidate
        .compile_staged()
        .expect("compile changed-capacity receiver candidate");

    let candidate_accessor = candidate
        .program_snapshot()
        .expect("candidate program snapshot")
        .functions()
        .iter()
        .find(|function| function.name == "receiver_parent")
        .expect("candidate receiver accessor");
    assert_eq!(candidate_accessor.id, active_id, "stable accessor identity");
    assert_eq!(
        candidate_accessor.body_hash, active_body_hash,
        "capacity-only edit leaves accessor source unchanged"
    );
    assert_eq!(
        candidate.global_collection_capacity("receiver_left.bones"),
        Some(25)
    );
    let generation = candidate
        .generation_metadata()
        .expect("candidate generation metadata");
    assert!(
        generation.emitted_function_ids.contains(&active_id),
        "capacity-dependent receiver accessor must be rebuilt: {generation:#?}"
    );
    assert!(
        !generation.reused_function_ids.contains(&active_id),
        "capacity-dependent receiver accessor must not reuse stale code: {generation:#?}"
    );
}

#[test]
fn body_only_swap_preserves_state_and_field_type_change_is_rejected() {
    let mut active = compile(SOURCE);
    assert_eq!(active.execute_i32_noarg_by_name(ROOT), Ok(0));

    let body_only_source = SOURCE.replace(
        "return receiver_left.receiver_parent(index);",
        "return receiver_left.receiver_parent(index) + 0;",
    );
    let mut candidate = active.staged_candidate();
    candidate.upsert_file(SOURCE_PATH, &body_only_source);
    candidate
        .compile_staged()
        .expect("compile body-only receiver candidate");
    let mut host = NoopHost::default();
    let receipt = commit_development_swap(
        &mut active,
        candidate,
        DevelopmentSwapDescriptor::new(vec!["receiver_dynamic_parent".to_string()], false),
        &mut host,
        |_| Ok::<(), String>(()),
    )
    .expect("accept unchanged receiver state layout");
    assert_eq!(receipt.status, DevelopmentSwapStatus::Accepted);
    assert!(receipt.state_layout_compatible);
    assert!(!receipt.layout_changed);
    assert_eq!(host.stages, 1);
    assert_eq!(
        active.read_global_collection_scalar("receiver_left.bones", "parent", 23),
        Ok(JitScalarValue::I32(22))
    );
    assert_eq!(
        active.read_global_collection_scalar("receiver_right.bones", "world_angle", 23),
        Ok(JitScalarValue::F32(272.25))
    );

    let incompatible_source = body_only_source.replace("wide_lane: u32;", "wide_lane: i32;");
    let mut candidate = active.staged_candidate();
    candidate.upsert_file(SOURCE_PATH, incompatible_source);
    candidate
        .compile_staged()
        .expect("compile incompatible field-lane candidate");
    let failure = commit_development_swap(
        &mut active,
        candidate,
        DevelopmentSwapDescriptor::new(Vec::<String>::new(), false),
        &mut host,
        |_| Ok::<(), String>(()),
    )
    .expect_err("reject changed receiver field storage type");
    assert_eq!(failure.receipt.status, DevelopmentSwapStatus::Rejected);
    assert!(!failure.receipt.state_layout_compatible);
    assert!(failure.receipt.layout_changed);
    assert_eq!(host.stages, 1, "incompatible layout rejects before staging");
    assert_eq!(
        active.read_global_collection_scalar("receiver_left.bones", "parent", 23),
        Ok(JitScalarValue::I32(22))
    );
    assert_eq!(
        active.read_global_collection_scalar("receiver_right.bones", "world_angle", 23),
        Ok(JitScalarValue::F32(272.25))
    );
}
