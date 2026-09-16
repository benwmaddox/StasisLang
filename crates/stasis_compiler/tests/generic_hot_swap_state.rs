use serde_json::json;
use sha2::{Digest, Sha256};
use stasis_compiler::backend::development_swap::{
    commit_development_swap, DevelopmentSwapDescriptor, DevelopmentSwapHost, DevelopmentSwapStatus,
};
use stasis_compiler::backend::jit::{JitArtifact, JitProcess, JitScalarValue};
use stasis_compiler::backend::state_migration::{
    plan_state_migration, state_layout_version, StateMigrationStepKind,
};
use std::sync::{Mutex, MutexGuard};

const SOURCE_PATH: &str = "tests/stasis/seams/generic_hot_swap_state.stasis";

static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

fn jit_test_lock() -> MutexGuard<'static, ()> {
    JIT_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const SOURCE_TEMPLATE: &str = r#"
struct Lane {
    i32_lane: i32;
    u8_lane: u8;
    u16_lane: u16;
    u32_lane: u32;
    bool_lane: bool;
    f32_lane: f32;
    f64_lane: f64;
}

struct GenericOwner<T: type, N: i32> {
    values: T[N];
    lanes: Lane[N];
}

struct NestedGeneric<T: type, N: i32> {
    owner: GenericOwner<T, N>;
}

global left: GenericOwner<i32, __LEFT_CAPACITY__>;
global right: GenericOwner<i32, __RIGHT_CAPACITY__>;
global nested: NestedGeneric<i32, __NESTED_CAPACITY__>;
global scalar_i32: i32;
global scalar_u8: u8;
global scalar_u16: u16;
global scalar_u32: u32;
global scalar_bool: bool;
global scalar_f32: f32;
global scalar_f64: f64;

function mutate_i32_owner(self: GenericOwner<i32, N>): void {
    let index: i32 = 0;
    for (index = 0; index < N; index += 1) {
        self.values[index] = -700;
        self.lanes[index].i32_lane = -701;
        self.lanes[index].u8_lane = 1;
        self.lanes[index].u16_lane = 2;
        self.lanes[index].u32_lane = 3;
        self.lanes[index].bool_lane = false;
        self.lanes[index].f32_lane = -4.5;
        self.lanes[index].f64_lane = -6.5;
    }
}

function main(): i32 {
    return scalar_i32 + left.values[0] + right.values[0] + nested.owner.values[0] + __MAIN_BIAS__;
}

function tick(): i32 {
    return main();
}

function on_code_swap(): void {
    __HOOK_BODY__
    return;
}
"#;

fn source(
    left_capacity: i32,
    right_capacity: i32,
    nested_capacity: i32,
    main_bias: i32,
    hook_body: &str,
) -> String {
    SOURCE_TEMPLATE
        .replace("__LEFT_CAPACITY__", &left_capacity.to_string())
        .replace("__RIGHT_CAPACITY__", &right_capacity.to_string())
        .replace("__NESTED_CAPACITY__", &nested_capacity.to_string())
        .replace("__MAIN_BIAS__", &main_bias.to_string())
        .replace("__HOOK_BODY__", hook_body)
}

fn compile(source: &str) -> JitProcess {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["main".to_string(), "tick".to_string()]);
    process.upsert_file(SOURCE_PATH, source);
    process.compile().expect("compile generic hot-swap fixture");
    process
}

#[derive(Default)]
struct TestHost {
    current: u32,
    stages: usize,
    publishes: usize,
    restores: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StagedHost {
    previous: u32,
    next: u32,
}

impl DevelopmentSwapHost for TestHost {
    type Staged = StagedHost;

    fn stage(
        &mut self,
        _candidate: &JitProcess,
        _descriptor: &DevelopmentSwapDescriptor,
    ) -> Result<Self::Staged, String> {
        self.stages += 1;
        Ok(StagedHost {
            previous: self.current,
            next: self.current + 1,
        })
    }

    fn publish(&mut self, staged: &mut Self::Staged) -> Result<(), String> {
        self.current = staged.next;
        self.publishes += 1;
        Ok(())
    }

    fn restore(&mut self, staged: Self::Staged) -> Result<(), String> {
        self.current = staged.previous;
        self.restores += 1;
        Ok(())
    }
}

fn state_values(process: &JitProcess) -> Vec<(String, JitScalarValue)> {
    let layout = process.state_layout();
    let mut values = Vec::new();
    for scalar in layout.scalars {
        values.push((
            scalar.path.clone(),
            process.read_global_scalar(&scalar.path).unwrap(),
        ));
    }
    for collection in layout.collections {
        for field in collection.fields {
            for index in 0..collection.capacity {
                let value = process
                    .read_global_collection_scalar(&collection.path, &field.field, index)
                    .unwrap();
                values.push((
                    format!("{}[{}].{}", collection.path, index, field.field),
                    value,
                ));
            }
        }
    }
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values
}

fn state_digest(values: &[(String, JitScalarValue)]) -> String {
    let bytes = serde_json::to_vec(values).expect("serialize deterministic state values");
    format!("{:x}", Sha256::digest(bytes))
}

fn artifact_metadata(process: &JitProcess) -> Vec<serde_json::Value> {
    process
        .artifacts()
        .iter()
        .map(|artifact: &JitArtifact| {
            json!({
                "function_id": artifact.function_id,
                "symbol": artifact.function_key.display_name(),
                "slot": artifact.slot,
                "body_hash": artifact.body_hash,
                "executable_bytes": artifact.executable_bytes,
                "clif_bytes": artifact.clif.len(),
            })
        })
        .collect()
}

fn session_evidence(process: &JitProcess) -> serde_json::Value {
    let snapshot = process.program_snapshot().expect("program snapshot");
    let metadata = process.generation_metadata().expect("generation metadata");
    let values = state_values(process);
    json!({
        "state_digest": state_digest(&values),
        "layout_version": state_layout_version(&process.state_layout()).expect("layout version"),
        "source_revision": snapshot.source_revision(),
        "layout_digest": format!("{:x}", Sha256::digest(snapshot.layout_digest())),
        "artifact_count": process.artifacts().len(),
        "artifacts": artifact_metadata(process),
        "emitted_count": metadata.emitted_function_ids.len(),
        "reused_count": metadata.reused_function_ids.len(),
        "module_count": metadata.module_count,
        "retained_arena_count": metadata.retained_arena_count,
        "emitted_clif_bytes": metadata.emitted_clif_bytes,
        "executable_bytes": metadata.executable_bytes,
        "retained_jit_bytes": metadata.retained_jit_bytes,
        "total_jit_bytes": metadata.total_jit_bytes,
    })
}

fn write_scalar_state(process: &JitProcess) {
    process
        .write_global_scalar("scalar_i32", JitScalarValue::I32(101))
        .unwrap();
    process
        .write_global_scalar("scalar_u8", JitScalarValue::U8(102))
        .unwrap();
    process
        .write_global_scalar("scalar_u16", JitScalarValue::U16(103))
        .unwrap();
    process
        .write_global_scalar("scalar_u32", JitScalarValue::U32(104))
        .unwrap();
    process
        .write_global_scalar("scalar_bool", JitScalarValue::Bool(true))
        .unwrap();
    process
        .write_global_scalar("scalar_f32", JitScalarValue::F32(105.5))
        .unwrap();
    process
        .write_global_scalar("scalar_f64", JitScalarValue::F64(106.5))
        .unwrap();
}

fn write_owner_state(process: &JitProcess, path: &str, capacity: i32, base: i32) {
    for index in 0..capacity {
        process
            .write_global_collection_scalar(
                &format!("{path}.values"),
                "",
                index,
                JitScalarValue::I32(base + index),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "i32_lane",
                index,
                JitScalarValue::I32(base + 10 + index),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "u8_lane",
                index,
                JitScalarValue::U8((base + 20 + index) as u8),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "u16_lane",
                index,
                JitScalarValue::U16((base + 30 + index) as u16),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "u32_lane",
                index,
                JitScalarValue::U32((base + 40 + index) as u32),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "bool_lane",
                index,
                JitScalarValue::Bool(index % 2 == 0),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "f32_lane",
                index,
                JitScalarValue::F32(base as f32 + 50.5 + index as f32),
            )
            .unwrap();
        process
            .write_global_collection_scalar(
                &format!("{path}.lanes"),
                "f64_lane",
                index,
                JitScalarValue::F64(base as f64 + 60.5 + index as f64),
            )
            .unwrap();
    }
}

fn seed_active_state(process: &JitProcess) {
    write_scalar_state(process);
    write_owner_state(process, "left", 2, 10);
    write_owner_state(process, "right", 3, 20);
    write_owner_state(process, "nested.owner", 2, 30);
}

fn growth_source() -> String {
    source(
        4,
        5,
        3,
        900,
        "mutate_i32_owner(left); mutate_i32_owner(right); mutate_i32_owner(nested.owner); scalar_i32 = -901; scalar_u8 = 1; scalar_u16 = 2; scalar_u32 = 3; scalar_bool = false; scalar_f32 = -4.5; scalar_f64 = -6.5;",
    )
}

fn rejected_layout_source(kind: &str) -> String {
    let mut candidate = growth_source();
    match kind {
        "element" => {
            candidate = candidate.replace(
                "global left: GenericOwner<i32, 4>;",
                "global left: GenericOwner<f32, 4>;",
            );
            candidate = candidate.replace("mutate_i32_owner(left); ", "");
            candidate = candidate.replace(
                "return scalar_i32 + left.values[0] + right.values[0] + nested.owner.values[0] + 900;",
                "return 900;",
            );
        }
        "nested" => {
            candidate = candidate.replace(
                "struct NestedGeneric<T: type, N: i32> {",
                "struct NestedHistory { samples: f32[2]; }\n\nstruct NestedGeneric<T: type, N: i32> {",
            );
            candidate = candidate.replace(
                "owner: GenericOwner<T, N>;",
                "owner: GenericOwner<T, N>;\n    history: NestedHistory[2];",
            );
        }
        "soa" => {
            candidate = candidate.replace("f64_lane: f64;", "f64_lane: f32;");
        }
        _ => panic!("unknown rejected layout fixture {kind}"),
    }
    candidate
}

fn assert_state_unchanged(process: &JitProcess, values: &[(String, JitScalarValue)], digest: &str) {
    assert_eq!(
        state_values(process),
        values,
        "every scalar and lane remains intact"
    );
    assert_eq!(
        state_digest(values),
        digest,
        "state digest remains deterministic"
    );
}

#[test]
fn generic_development_swap_migrates_and_rolls_back_complete_soa_state() {
    let _guard = jit_test_lock();
    let mut active = compile(&source(2, 3, 2, 0, ""));
    seed_active_state(&active);
    let initial_values = state_values(&active);
    let initial_digest = state_digest(&initial_values);
    let initial_evidence = session_evidence(&active);
    let initial_snapshot = active.program_snapshot().expect("active snapshot");
    let initial_layout_digest = initial_snapshot.layout_digest();
    let initial_source_revision = initial_snapshot.source_revision();
    let initial_artifacts = artifact_metadata(&active);
    assert!(!initial_artifacts.is_empty(), "active executable artifacts");
    assert_eq!(initial_evidence["state_digest"], initial_digest);

    let mut candidate = active.staged_candidate();
    candidate.upsert_file(SOURCE_PATH, growth_source());
    candidate
        .compile_staged()
        .expect("capacity-growth candidate compiles");
    let preview = plan_state_migration(
        &active.state_layout(),
        &candidate.state_layout(),
        vec!["tick".to_string(), "main".to_string()],
        true,
        None,
    )
    .expect("preview capacity-growth migration");
    assert!(
        preview.state_layout_compatible,
        "growth preview is compatible"
    );
    assert!(preview.layout_changed);
    assert_eq!(preview.changed_functions, vec!["main", "tick"]);
    assert_eq!(
        preview.from_layout_version,
        state_layout_version(&active.state_layout()).unwrap()
    );
    assert_eq!(
        preview.to_layout_version,
        state_layout_version(&candidate.state_layout()).unwrap()
    );
    for (path, from, to) in [
        ("left.values", 2, 4),
        ("right.values", 3, 5),
        ("nested.owner.values", 2, 3),
    ] {
        assert!(
            preview.migration_steps.iter().any(|step| {
                step.path == path
                    && step.field.as_deref() == Some("")
                    && step.kind == StateMigrationStepKind::Copy
                    && step.from_capacity == Some(from)
                    && step.to_capacity == Some(to)
                    && step.elements == from
            }),
            "copy prefix step for {path}"
        );
        assert!(
            preview.migration_steps.iter().any(|step| {
                step.path == path
                    && step.field.as_deref() == Some("")
                    && step.kind == StateMigrationStepKind::Initialize
                    && step.start_index == from
                    && step.elements == to - from
            }),
            "initialize tail step for {path}"
        );
    }
    for field in [
        "i32_lane",
        "u8_lane",
        "u16_lane",
        "u32_lane",
        "bool_lane",
        "f32_lane",
        "f64_lane",
    ] {
        assert!(
            preview.migration_steps.iter().any(|step| {
                step.path == "left.lanes"
                    && step.field.as_deref() == Some(field)
                    && step.kind == StateMigrationStepKind::Initialize
                    && step.start_index == 2
                    && step.elements == 2
            }),
            "initialize left SoA tail for {field}"
        );
    }

    let candidate_layout_digest = candidate.program_snapshot().unwrap().layout_digest();
    let candidate_source_revision = candidate.program_snapshot().unwrap().source_revision();
    assert_ne!(candidate_layout_digest, initial_layout_digest);
    assert_ne!(candidate_source_revision, initial_source_revision);
    assert_eq!(candidate.global_collection_capacity("left.values"), Some(4));
    assert_eq!(candidate.global_collection_capacity("right.lanes"), Some(5));
    assert_eq!(
        candidate.global_collection_capacity("nested.owner.lanes"),
        Some(3)
    );

    let mut host = TestHost::default();
    let receipt = commit_development_swap(
        &mut active,
        candidate,
        DevelopmentSwapDescriptor::new(vec!["tick".to_string(), "main".to_string()], true),
        &mut host,
        |_| Ok::<(), String>(()),
    )
    .expect("compatible generic capacity growth commits");
    assert_eq!(receipt.status, DevelopmentSwapStatus::Accepted);
    assert!(receipt.state_layout_compatible);
    assert!(receipt.layout_changed);
    assert_eq!(
        receipt.from_layout_version,
        initial_evidence["layout_version"]
    );
    assert_eq!(
        receipt.to_layout_version,
        state_layout_version(&active.state_layout()).unwrap()
    );
    assert_eq!(host.stages, 1);
    assert_eq!(host.publishes, 1);
    assert_eq!(host.restores, 0);
    assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(1_061));

    for (path, capacity, base) in [("left", 4, 10), ("right", 5, 20), ("nested.owner", 3, 30)] {
        let values_path = format!("{path}.values");
        let lanes_path = format!("{path}.lanes");
        let old_capacity = match path {
            "left" | "nested.owner" => 2,
            "right" => 3,
            _ => unreachable!(),
        };
        for index in 0..old_capacity {
            assert_eq!(
                active.read_global_collection_scalar(&values_path, "", index),
                Ok(JitScalarValue::I32(base + index)),
                "{values_path}[{index}] prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "i32_lane", index),
                Ok(JitScalarValue::I32(base + 10 + index)),
                "{lanes_path}[{index}].i32_lane prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "u8_lane", index),
                Ok(JitScalarValue::U8((base + 20 + index) as u8)),
                "{lanes_path}[{index}].u8_lane prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "u16_lane", index),
                Ok(JitScalarValue::U16((base + 30 + index) as u16)),
                "{lanes_path}[{index}].u16_lane prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "u32_lane", index),
                Ok(JitScalarValue::U32((base + 40 + index) as u32)),
                "{lanes_path}[{index}].u32_lane prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "bool_lane", index),
                Ok(JitScalarValue::Bool(index % 2 == 0)),
                "{lanes_path}[{index}].bool_lane prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "f32_lane", index),
                Ok(JitScalarValue::F32(base as f32 + 50.5 + index as f32)),
                "{lanes_path}[{index}].f32_lane prefix"
            );
            assert_eq!(
                active.read_global_collection_scalar(&lanes_path, "f64_lane", index),
                Ok(JitScalarValue::F64(base as f64 + 60.5 + index as f64)),
                "{lanes_path}[{index}].f64_lane prefix"
            );
        }
        for index in old_capacity..capacity {
            assert_eq!(
                active.read_global_collection_scalar(&values_path, "", index),
                Ok(JitScalarValue::I32(0)),
                "{values_path}[{index}] default tail"
            );
            for (field, value) in [
                ("i32_lane", JitScalarValue::I32(0)),
                ("u8_lane", JitScalarValue::U8(0)),
                ("u16_lane", JitScalarValue::U16(0)),
                ("u32_lane", JitScalarValue::U32(0)),
                ("bool_lane", JitScalarValue::Bool(false)),
                ("f32_lane", JitScalarValue::F32(0.0)),
                ("f64_lane", JitScalarValue::F64(0.0)),
            ] {
                assert_eq!(
                    active.read_global_collection_scalar(&lanes_path, field, index),
                    Ok(value),
                    "{lanes_path}[{index}].{field} default tail"
                );
            }
        }
    }
    let accepted_evidence = session_evidence(&active);
    assert_ne!(accepted_evidence["state_digest"], initial_digest);
    assert_eq!(
        accepted_evidence["artifact_count"].as_u64().unwrap(),
        accepted_evidence["emitted_count"].as_u64().unwrap()
            + accepted_evidence["reused_count"].as_u64().unwrap()
    );
    assert!(
        accepted_evidence["module_count"].as_u64().unwrap()
            >= accepted_evidence["retained_arena_count"].as_u64().unwrap()
    );
    assert!(
        accepted_evidence["total_jit_bytes"].as_u64().unwrap()
            >= accepted_evidence["retained_jit_bytes"].as_u64().unwrap()
    );
    assert_ne!(artifact_metadata(&active), initial_artifacts);
}

#[test]
fn incompatible_generic_layouts_reject_before_host_publication() {
    let _guard = jit_test_lock();
    for kind in ["element", "nested", "soa"] {
        let mut active = compile(&source(2, 3, 2, 0, ""));
        seed_active_state(&active);
        let initial_values = state_values(&active);
        let initial_digest = state_digest(&initial_values);
        let initial_snapshot = active.program_snapshot().unwrap().clone();
        let initial_artifacts = artifact_metadata(&active);
        let mut candidate = active.staged_candidate();
        candidate.upsert_file(SOURCE_PATH, rejected_layout_source(kind));
        candidate
            .compile_staged()
            .expect("incompatible layout candidate compiles before migration rejection");
        let mut host = TestHost::default();
        let failure = commit_development_swap(
            &mut active,
            candidate,
            DevelopmentSwapDescriptor::new(vec!["main".to_string()], false),
            &mut host,
            |_| Ok::<(), String>(()),
        )
        .expect_err("{kind} incompatible layout rejected before publication");
        assert_eq!(failure.receipt.status, DevelopmentSwapStatus::Rejected);
        assert!(!failure.receipt.state_layout_compatible);
        assert_eq!(host.stages, 0, "{kind} rejects before host stage");
        assert_eq!(host.publishes, 0, "{kind} rejects before publication");
        assert_eq!(host.restores, 0, "{kind} has no staged host to restore");
        assert_eq!(
            active.program_snapshot().unwrap().clone().layout_digest(),
            initial_snapshot.layout_digest()
        );
        assert_eq!(artifact_metadata(&active), initial_artifacts);
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(161));
        assert_state_unchanged(&active, &initial_values, &initial_digest);
        match kind {
            "element" | "soa" => assert!(
                failure.error.contains("changed type"),
                "{kind}: {}",
                failure.error
            ),
            "nested" => assert!(
                failure.error.contains("non-migratable"),
                "{kind}: {}",
                failure.error
            ),
            _ => unreachable!(),
        }
    }
}

#[test]
fn failed_compile_type_and_effect_candidates_keep_active_snapshot_artifacts_executable_and_state() {
    let _guard = jit_test_lock();
    let active = compile(&source(2, 3, 2, 0, ""));
    seed_active_state(&active);
    let initial_values = state_values(&active);
    let initial_digest = state_digest(&initial_values);
    let initial_snapshot = active.program_snapshot().unwrap().clone();
    let initial_artifacts = artifact_metadata(&active);
    let initial_main = active.execute_i32_noarg_by_name("main").unwrap();
    let cases = [
        ("compile", source(2, 3, 2, 0, "")),
        (
            "type",
            source(2, 3, 2, 0, "").replace(
                "return scalar_i32 + left.values[0] + right.values[0] + nested.owner.values[0] + 0;",
                "return scalar_bool;",
            ),
        ),
        (
            "effect",
            source(2, 3, 2, 0, "").replace(
                "function on_code_swap(): void {",
                "function @effects() on_code_swap(): void { scalar_i32 += 1;",
            ),
        ),
    ];
    for (kind, mut candidate_source) in cases {
        if kind == "compile" {
            candidate_source =
                candidate_source.replace("\n}\n\nfunction tick", "\n\nfunction tick");
        }
        let mut candidate = active.staged_candidate();
        candidate.upsert_file(SOURCE_PATH, candidate_source);
        let error = candidate
            .compile_staged()
            .expect_err("invalid candidate must fail before swap");
        assert!(!format!("{error:?}").is_empty(), "{kind} diagnostic");
        assert_eq!(
            active.program_snapshot().unwrap().clone().layout_digest(),
            initial_snapshot.layout_digest()
        );
        assert_eq!(artifact_metadata(&active), initial_artifacts);
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(initial_main));
        assert_state_unchanged(&active, &initial_values, &initial_digest);
    }
}

#[test]
fn rejecting_on_code_swap_rolls_back_every_lane_and_valid_retry_commits() {
    let _guard = jit_test_lock();
    let mut active = compile(&source(2, 3, 2, 0, ""));
    seed_active_state(&active);
    let initial_values = state_values(&active);
    let initial_digest = state_digest(&initial_values);
    let initial_snapshot = active.program_snapshot().unwrap().clone();
    let initial_artifacts = artifact_metadata(&active);
    let mut host = TestHost::default();
    let mut rejected = active.staged_candidate();
    rejected.upsert_file(
        SOURCE_PATH,
        source(
            2,
            3,
            2,
            500,
            "mutate_i32_owner(left); mutate_i32_owner(right); mutate_i32_owner(nested.owner); scalar_i32 = -901; scalar_u8 = 1; scalar_u16 = 2; scalar_u32 = 3; scalar_bool = false; scalar_f32 = -4.5; scalar_f64 = -6.5;",
        ),
    );
    rejected
        .compile_staged()
        .expect("hook-rejection candidate compiles");
    let failure = commit_development_swap(
        &mut active,
        rejected,
        DevelopmentSwapDescriptor::new(vec!["main".to_string(), "on_code_swap".to_string()], true),
        &mut host,
        |candidate| {
            candidate.execute_void_noarg_by_name("on_code_swap")?;
            Err::<(), _>("deterministic hook rejection".to_string())
        },
    )
    .expect_err("on_code_swap rejection");
    assert_eq!(failure.receipt.status, DevelopmentSwapStatus::Rejected);
    assert_eq!(
        failure.hook_error.as_deref(),
        Some("deterministic hook rejection")
    );
    assert_eq!(host.stages, 1);
    assert_eq!(host.publishes, 1);
    assert_eq!(host.restores, 1);
    assert_eq!(
        active.program_snapshot().unwrap().clone().layout_digest(),
        initial_snapshot.layout_digest()
    );
    assert_eq!(artifact_metadata(&active), initial_artifacts);
    assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(161));
    assert_state_unchanged(&active, &initial_values, &initial_digest);

    let mut retry = active.staged_candidate();
    retry.upsert_file(SOURCE_PATH, source(2, 3, 2, 700, ""));
    retry.compile_staged().expect("valid retry compiles");
    let receipt = commit_development_swap(
        &mut active,
        retry,
        DevelopmentSwapDescriptor::new(vec!["main".to_string()], false),
        &mut host,
        |_| Ok::<(), String>(()),
    )
    .expect("valid retry after hook rejection");
    assert_eq!(receipt.status, DevelopmentSwapStatus::Accepted);
    assert!(!receipt.layout_changed);
    assert_eq!(host.stages, 2);
    assert_eq!(host.publishes, 2);
    assert_eq!(host.restores, 1);
    assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(861));
    assert_eq!(state_digest(&state_values(&active)), initial_digest);
}
