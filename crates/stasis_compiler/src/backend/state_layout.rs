use super::emit::{CollectionInfoMap, GlobalPathTypeMap};
use crate::frontend::types::{
    TypeCategory, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32, TYPE_ID_U16,
    TYPE_ID_U32, TYPE_ID_U8,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateLayout {
    pub scalars: Vec<StateScalarLayout>,
    pub collections: Vec<StateCollectionLayout>,
    #[serde(default)]
    pub structs: Vec<StateStructLayout>,
    pub opaque: Vec<StateOpaqueLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateScalarLayout {
    pub path: String,
    pub type_name: String,
    #[serde(default)]
    pub storage_type_name: String,
}

impl StateScalarLayout {
    pub fn storage_type_name(&self) -> &str {
        if self.storage_type_name.is_empty() {
            &self.type_name
        } else {
            &self.storage_type_name
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateCollectionLayout {
    pub path: String,
    pub capacity: i32,
    pub element_shape: String,
    pub fully_migratable: bool,
    pub fields: Vec<StateCollectionFieldLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateCollectionFieldLayout {
    pub field: String,
    pub type_name: String,
    #[serde(default)]
    pub storage_type_name: String,
}

impl StateCollectionFieldLayout {
    pub fn storage_type_name(&self) -> &str {
        if self.storage_type_name.is_empty() {
            &self.type_name
        } else {
            &self.storage_type_name
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateStructLayout {
    pub path: String,
    pub type_name: String,
    pub fields: Vec<StateStructFieldLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateStructFieldLayout {
    pub field: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateOpaqueLayout {
    pub path: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateMemoryReport {
    pub storage_model: String,
    pub total_capacity_bytes: u64,
    pub projected_capacity_bytes: u64,
    pub snapshot_bytes: u64,
    pub mobile_budget_bytes: u64,
    pub entries: Vec<StateMemoryEntry>,
    pub structs: Vec<StateMemoryStructReport>,
    pub largest_pools: Vec<StateMemoryPoolReport>,
    pub command_buffers: Vec<StateMemoryPoolReport>,
    pub capacity_changes: Vec<StateCapacityChangeReport>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateMemoryEntry {
    pub path: String,
    pub field: String,
    pub kind: String,
    pub type_name: String,
    pub alignment_bytes: u64,
    pub element_bytes: u64,
    pub padding_bytes: u64,
    pub capacity: u64,
    pub active_count: Option<u64>,
    pub capacity_bytes: u64,
    pub active_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateMemoryStructReport {
    pub path: String,
    pub type_name: String,
    pub capacity_bytes: u64,
    pub fields: Vec<StateMemoryStructFieldReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateMemoryStructFieldReport {
    pub field: String,
    pub type_name: String,
    pub alignment_bytes: u64,
    pub padding_bytes: u64,
    pub capacity_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateMemoryPoolReport {
    pub path: String,
    pub element_shape: String,
    pub capacity: u64,
    pub active_count: Option<u64>,
    pub bytes_per_element: u64,
    pub capacity_bytes: u64,
    pub active_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateCapacityChangeReport {
    pub path: String,
    pub old_capacity: u64,
    pub new_capacity: u64,
    pub bytes_per_element: u64,
    pub delta_bytes: i64,
}

pub fn state_layout_digest(layout: &StateLayout) -> Result<[u8; 32], String> {
    let serialized = serde_json::to_vec(layout)
        .map_err(|error| format!("failed versioning compiler state layout: {error}"))?;
    let digest = Sha256::digest(serialized);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(bytes)
}

pub fn state_layout_version(layout: &StateLayout) -> Result<String, String> {
    Ok(state_layout_digest(layout)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn build_state_memory_report(
    layout: &StateLayout,
    active_counts: &BTreeMap<String, u64>,
    capacity_overrides: &BTreeMap<String, u64>,
    mobile_budget_bytes: u64,
) -> Result<StateMemoryReport, String> {
    for path in capacity_overrides.keys() {
        if !layout
            .collections
            .iter()
            .any(|collection| &collection.path == path)
        {
            return Err(format!(
                "capacity override path '{path}' was not found in compiler collection metadata"
            ));
        }
    }

    let mut warnings = Vec::new();
    let mut entries = layout
        .scalars
        .iter()
        .map(|scalar| {
            let storage_type_name = scalar.storage_type_name();
            let element_bytes = storage_type_bytes(storage_type_name).unwrap_or(0);
            if element_bytes == 0 {
                warnings.push(format!(
                    "state path '{}' has unsupported storage type '{}'",
                    scalar.path, storage_type_name
                ));
            }
            StateMemoryEntry {
                path: scalar.path.clone(),
                field: String::new(),
                kind: "scalar".to_string(),
                type_name: scalar.type_name.clone(),
                alignment_bytes: storage_type_alignment(storage_type_name).unwrap_or(1),
                element_bytes,
                padding_bytes: 0,
                capacity: 1,
                active_count: Some(1),
                capacity_bytes: element_bytes,
                active_bytes: Some(element_bytes),
            }
        })
        .collect::<Vec<_>>();

    let mut pools = Vec::new();
    let mut capacity_changes = Vec::new();
    for collection in &layout.collections {
        let old_capacity = u64::try_from(collection.capacity).map_err(|_| {
            format!(
                "collection '{}' has negative capacity {}",
                collection.path, collection.capacity
            )
        })?;
        let new_capacity = capacity_overrides
            .get(&collection.path)
            .copied()
            .unwrap_or(old_capacity);
        let active_count = active_counts
            .get(&collection.path)
            .copied()
            .map(|count| count.min(old_capacity));
        let mut bytes_per_element = 0u64;
        for field in &collection.fields {
            let storage_type_name = field.storage_type_name();
            let element_bytes = storage_type_bytes(storage_type_name).unwrap_or(0);
            if element_bytes == 0 {
                warnings.push(format!(
                    "collection path '{}' field '{}' has unsupported storage type '{}'",
                    collection.path, field.field, storage_type_name
                ));
            }
            bytes_per_element = bytes_per_element
                .checked_add(element_bytes)
                .ok_or_else(|| "state memory report byte count overflow".to_string())?;
            entries.push(StateMemoryEntry {
                path: collection.path.clone(),
                field: field.field.clone(),
                kind: "collection_field".to_string(),
                type_name: field.type_name.clone(),
                alignment_bytes: storage_type_alignment(storage_type_name).unwrap_or(1),
                element_bytes,
                padding_bytes: 0,
                capacity: old_capacity,
                active_count,
                capacity_bytes: checked_memory_bytes(old_capacity, element_bytes)?,
                active_bytes: active_count
                    .map(|count| checked_memory_bytes(count, element_bytes))
                    .transpose()?,
            });
        }
        let capacity_bytes = checked_memory_bytes(old_capacity, bytes_per_element)?;
        let projected_bytes = checked_memory_bytes(new_capacity, bytes_per_element)?;
        pools.push(StateMemoryPoolReport {
            path: collection.path.clone(),
            element_shape: collection.element_shape.clone(),
            capacity: old_capacity,
            active_count,
            bytes_per_element,
            capacity_bytes,
            active_bytes: active_count
                .map(|count| checked_memory_bytes(count, bytes_per_element))
                .transpose()?,
        });
        if new_capacity != old_capacity {
            let delta = i128::from(projected_bytes) - i128::from(capacity_bytes);
            let delta_bytes = i64::try_from(delta)
                .map_err(|_| "capacity change byte delta overflow".to_string())?;
            capacity_changes.push(StateCapacityChangeReport {
                path: collection.path.clone(),
                old_capacity,
                new_capacity,
                bytes_per_element,
                delta_bytes,
            });
        }
    }
    entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.field.cmp(&right.field))
    });

    let total_capacity_bytes = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.capacity_bytes)
            .ok_or_else(|| "state memory report byte count overflow".to_string())
    })?;
    let projected_delta = capacity_changes
        .iter()
        .map(|change| i128::from(change.delta_bytes))
        .sum::<i128>();
    let projected_capacity_bytes =
        u64::try_from(i128::from(total_capacity_bytes) + projected_delta)
            .map_err(|_| "projected state memory byte count overflow".to_string())?;

    let structs = layout
        .structs
        .iter()
        .map(|structure| build_struct_memory_report(structure, &entries))
        .collect();
    pools.sort_by(|left, right| {
        right
            .capacity_bytes
            .cmp(&left.capacity_bytes)
            .then_with(|| left.path.cmp(&right.path))
    });
    let largest_pools = pools.iter().take(8).cloned().collect();
    let command_buffers = pools
        .iter()
        .filter(|pool| is_command_buffer_path(&pool.path))
        .cloned()
        .collect();

    if projected_capacity_bytes > mobile_budget_bytes {
        warnings.push(format!(
            "projected state requires {projected_capacity_bytes} bytes, exceeding the {mobile_budget_bytes}-byte mobile snapshot budget"
        ));
    } else if mobile_budget_bytes > 0
        && projected_capacity_bytes.saturating_mul(4) >= mobile_budget_bytes.saturating_mul(3)
    {
        warnings.push(format!(
            "projected state uses at least 75% of the {mobile_budget_bytes}-byte mobile snapshot budget"
        ));
    }
    for opaque in &layout.opaque {
        warnings.push(format!(
            "opaque state path '{}' of type '{}' is excluded from the byte total",
            opaque.path, opaque.type_name
        ));
    }

    Ok(StateMemoryReport {
        storage_model: "soa_direct_bindings".to_string(),
        total_capacity_bytes,
        projected_capacity_bytes,
        snapshot_bytes: total_capacity_bytes,
        mobile_budget_bytes,
        entries,
        structs,
        largest_pools,
        command_buffers,
        capacity_changes,
        warnings,
    })
}

fn build_struct_memory_report(
    structure: &StateStructLayout,
    entries: &[StateMemoryEntry],
) -> StateMemoryStructReport {
    let fields = structure
        .fields
        .iter()
        .map(|field| {
            let path = format!("{}.{}", structure.path, field.field);
            let matching = entries
                .iter()
                .filter(|entry| entry.path == path || entry.path.starts_with(&format!("{path}.")));
            let (alignment_bytes, capacity_bytes) = matching.fold((1u64, 0u64), |sum, entry| {
                (
                    sum.0.max(entry.alignment_bytes),
                    sum.1.saturating_add(entry.capacity_bytes),
                )
            });
            StateMemoryStructFieldReport {
                field: field.field.clone(),
                type_name: field.type_name.clone(),
                alignment_bytes,
                padding_bytes: 0,
                capacity_bytes,
            }
        })
        .collect::<Vec<_>>();
    StateMemoryStructReport {
        path: structure.path.clone(),
        type_name: structure.type_name.clone(),
        capacity_bytes: fields.iter().map(|field| field.capacity_bytes).sum(),
        fields,
    }
}

fn storage_type_bytes(type_name: &str) -> Option<u64> {
    match type_name {
        "u8" => Some(1),
        "u16" => Some(2),
        "u32" | "i32" | "f32" | "bool" => Some(4),
        "f64" => Some(8),
        _ => None,
    }
}

fn storage_type_alignment(type_name: &str) -> Option<u64> {
    storage_type_bytes(type_name)
}

fn checked_memory_bytes(count: u64, element_bytes: u64) -> Result<u64, String> {
    count
        .checked_mul(element_bytes)
        .ok_or_else(|| "state memory report byte count overflow".to_string())
}

fn is_command_buffer_path(path: &str) -> bool {
    path.starts_with("gfx_cmd_")
        || path.starts_with("render_cmd_")
        || path.starts_with("audio_cmd_")
        || path.starts_with("cmd_")
        || path.contains(".gfx_cmd_")
        || path.contains(".render_cmd_")
        || path.contains(".audio_cmd_")
        || path.contains(".cmd_")
}

pub(crate) fn build_state_layout(
    global_path_types: &GlobalPathTypeMap,
    collection_infos: &CollectionInfoMap,
    type_table: &TypeTable,
) -> StateLayout {
    let scalars = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            let info = type_table.type_info(*type_id)?;
            let storage_type_name = scalar_storage_type_name(type_table, *type_id)?;
            if info.category == TypeCategory::Named
                && !is_named_scalar_state_path(path, *type_id, global_path_types, type_table)
            {
                return None;
            }
            Some(StateScalarLayout {
                path: path.clone(),
                type_name: info.name.clone(),
                storage_type_name: storage_type_name.to_string(),
            })
        })
        .collect();
    let mut collections: Vec<StateCollectionLayout> = collection_infos
        .iter()
        .map(|(path, info)| {
            let mut fields = Vec::new();
            if let Some((type_name, storage_type_name)) = info
                .element_type
                .and_then(|type_id| state_value_type_names(type_table, type_id))
            {
                fields.push(StateCollectionFieldLayout {
                    field: String::new(),
                    type_name,
                    storage_type_name,
                });
            }
            fields.extend(info.field_types.iter().filter_map(|(field, type_id)| {
                state_value_type_names(type_table, *type_id).map(
                    |(type_name, storage_type_name)| StateCollectionFieldLayout {
                        field: field.clone(),
                        type_name,
                        storage_type_name,
                    },
                )
            }));
            StateCollectionLayout {
                path: path.clone(),
                capacity: info.len,
                element_shape: info.element_shape.clone(),
                fully_migratable: info.fully_migratable,
                fields,
            }
        })
        .collect();
    for (path, type_id) in global_path_types {
        let Some(info) = type_table.type_info(*type_id) else {
            continue;
        };
        if !matches!(
            info.category,
            TypeCategory::AsciiFixed | TypeCategory::Utf8Fixed
        ) {
            continue;
        }
        let Some(capacity) = type_table.fixed_collection_len(*type_id) else {
            continue;
        };
        if let Some(collection) = collections
            .iter_mut()
            .find(|collection| collection.path == *path)
        {
            if !collection.fields.iter().any(|field| field.field.is_empty()) {
                collection.fields.push(StateCollectionFieldLayout {
                    field: String::new(),
                    type_name: "u8".to_string(),
                    storage_type_name: "u8".to_string(),
                });
            }
            collection.fully_migratable = true;
            continue;
        }
        collections.push(StateCollectionLayout {
            path: path.clone(),
            capacity,
            element_shape: info.name.clone(),
            fully_migratable: true,
            fields: vec![StateCollectionFieldLayout {
                field: String::new(),
                type_name: "u8".to_string(),
                storage_type_name: "u8".to_string(),
            }],
        });
    }
    collections.sort_by(|left, right| left.path.cmp(&right.path));
    let collection_paths = collections
        .iter()
        .map(|collection| collection.path.as_str())
        .collect::<BTreeSet<_>>();
    let opaque = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            if scalar_storage_type_name(type_table, *type_id).is_some()
                || collection_paths.contains(path.as_str())
            {
                return None;
            }
            let info = type_table.type_info(*type_id)?;
            let prefix = format!("{path}.");
            if info.category == TypeCategory::Named
                && global_path_types
                    .keys()
                    .any(|candidate| candidate.starts_with(&prefix))
            {
                return None;
            }
            Some(StateOpaqueLayout {
                path: path.clone(),
                type_name: info.name.clone(),
            })
        })
        .collect();
    let structs = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            let info = type_table.type_info(*type_id)?;
            if info.category != TypeCategory::Named {
                return None;
            }
            let prefix = format!("{path}.");
            let mut fields = BTreeMap::new();
            for (candidate, child_type_id) in global_path_types {
                let Some(rest) = candidate.strip_prefix(&prefix) else {
                    continue;
                };
                let Some(field) = rest.split('.').next() else {
                    continue;
                };
                let child_path = format!("{prefix}{field}");
                let child_type_id = global_path_types
                    .get(&child_path)
                    .copied()
                    .unwrap_or(*child_type_id);
                let child_type = type_table.type_info(child_type_id)?.name.clone();
                fields.entry(field.to_string()).or_insert(child_type);
            }
            Some(StateStructLayout {
                path: path.clone(),
                type_name: info.name.clone(),
                fields: fields
                    .into_iter()
                    .map(|(field, type_name)| StateStructFieldLayout { field, type_name })
                    .collect(),
            })
        })
        .collect();
    StateLayout {
        scalars,
        collections,
        structs,
        opaque,
    }
}

pub(crate) fn is_named_scalar_state_path(
    path: &str,
    type_id: u16,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> bool {
    type_table
        .type_info(type_id)
        .is_some_and(|info| info.category == TypeCategory::Named)
        && !global_path_types
            .keys()
            .any(|candidate| candidate.starts_with(&format!("{path}.")))
}

fn scalar_storage_type_name(type_table: &TypeTable, type_id: u16) -> Option<&'static str> {
    match type_id {
        TYPE_ID_I32 => Some("i32"),
        TYPE_ID_F32 => Some("f32"),
        TYPE_ID_F64 => Some("f64"),
        TYPE_ID_BOOL => Some("bool"),
        TYPE_ID_U8 => Some("u8"),
        TYPE_ID_U16 => Some("u16"),
        TYPE_ID_U32 => Some("u32"),
        _ if type_table
            .type_info(type_id)
            .is_some_and(|info| info.category == TypeCategory::Named) =>
        {
            Some("i32")
        }
        _ => None,
    }
}

fn state_value_type_names(type_table: &TypeTable, type_id: u16) -> Option<(String, String)> {
    let info = type_table.type_info(type_id)?;
    let storage_type_name = scalar_storage_type_name(type_table, type_id)?;
    Some((info.name.clone(), storage_type_name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::build_state_memory_report;
    use crate::backend::aot::AotProcess;
    use crate::backend::jit::JitProcess;
    use std::collections::BTreeMap;

    #[test]
    fn jit_and_aot_share_canonical_state_layout() {
        let source = "const SAMPLE_COUNT: i32 = 2;\n\
                      struct Row { value: i32; samples: f32[SAMPLE_COUNT]; }\n\
                      global rows: Row[4];\n\
                      global score: i32;\n\
                      global title: utf8[8];\n\
                      global samples: f32[4];\n\
                      function main(): i32 { return score; }\n";

        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile_staged().expect("compile JIT fixture");

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", source);
        aot.compile().expect("compile AOT fixture");

        let layout = jit.state_layout();
        assert_eq!(layout, aot.state_layout());
        assert!(layout.scalars.iter().any(|field| field.path == "score"));
        assert!(layout
            .collections
            .iter()
            .any(|collection| collection.path == "title" && collection.capacity == 8));
        assert!(layout
            .collections
            .iter()
            .any(|collection| collection.path == "samples" && collection.capacity == 4));
        let rows = layout
            .collections
            .iter()
            .find(|collection| collection.path == "rows")
            .expect("rows collection layout");
        assert!(!rows.fully_migratable);
        assert!(rows.element_shape.contains("samples:f32[2]"));

        let original_shape = rows.element_shape.clone();
        jit.upsert_file("main.stasis", source.replace("= 2", "= 3"));
        jit.compile_staged().expect("recompile changed extent");
        let changed_rows = jit
            .state_layout()
            .collections
            .into_iter()
            .find(|collection| collection.path == "rows")
            .expect("changed rows collection layout");
        assert!(changed_rows.element_shape.contains("samples:f32[3]"));
        assert_ne!(original_shape, changed_rows.element_shape);
    }

    #[test]
    fn compiler_layout_drives_bounded_memory_report_and_capacity_impact() {
        let source = "struct Enemy { hp: i32; speed: f64; alive: bool; }\n\
                      struct GameState { score: i32; enemies: Enemy[4]; }\n\
                      global state: GameState;\n\
                      global gfx_cmd_i32: i32[8];\n\
                      function main(): i32 { return state.score; }\n";
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile_staged().expect("compile report fixture");

        let active_counts = BTreeMap::from([("state.enemies".to_string(), 2)]);
        let overrides = BTreeMap::from([("state.enemies".to_string(), 10)]);
        let report =
            build_state_memory_report(&jit.state_layout(), &active_counts, &overrides, 128)
                .expect("build memory report");

        let enemies = report
            .largest_pools
            .iter()
            .find(|pool| pool.path == "state.enemies")
            .expect("enemy pool");
        assert_eq!(enemies.bytes_per_element, 16);
        assert_eq!(enemies.capacity_bytes, 64);
        assert_eq!(enemies.active_count, Some(2));
        assert_eq!(enemies.active_bytes, Some(32));
        assert!(report
            .structs
            .iter()
            .any(|structure| structure.path == "state"
                && structure.type_name == "GameState"
                && structure
                    .fields
                    .iter()
                    .any(|field| field.field == "enemies")));
        assert!(report
            .command_buffers
            .iter()
            .any(|pool| pool.path == "gfx_cmd_i32"));
        assert_eq!(report.capacity_changes[0].delta_bytes, 96);
        assert_eq!(
            report.projected_capacity_bytes,
            report.total_capacity_bytes + 96
        );
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("mobile snapshot budget")));
        assert!(report.entries.iter().all(|entry| entry.padding_bytes == 0));
    }

    #[test]
    fn narrow_unsigned_state_reports_true_element_widths() {
        let source = "global byte_value: u8;\nglobal word_value: u16;\nglobal wide_value: u32;\nglobal bytes: u8[2];\nglobal words: u16[2];\nglobal wides: u32[2];\nfunction main(): i32 { return 0; }\n";
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile_staged().expect("compile narrow layout fixture");
        let report = build_state_memory_report(
            &jit.state_layout(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            1024,
        )
        .expect("build narrow memory report");

        for (path, expected_bytes) in [
            ("byte_value", 1),
            ("word_value", 2),
            ("wide_value", 4),
            ("bytes", 1),
            ("words", 2),
            ("wides", 4),
        ] {
            let entry = report
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap_or_else(|| panic!("missing report entry for {path}"));
            assert_eq!(entry.element_bytes, expected_bytes, "{path}");
        }
    }

    #[test]
    fn enum_state_uses_i32_storage_lanes() {
        let source = "enum Phase { Waiting, Playing, }\n\
                      struct Enemy { phase: Phase; hp: i32; }\n\
                      struct Game { phase: Phase; enemies: Enemy[2]; }\n\
                      global game: Game;\n\
                      function main(): i32 { game.phase = Phase.Playing; game.enemies[0].phase = game.phase; return 0; }\n";
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile_staged().expect("compile enum state fixture");

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", source);
        aot.compile().expect("compile enum AOT fixture");

        let layout = jit.state_layout();
        assert_eq!(layout, aot.state_layout());
        assert!(layout.scalars.iter().any(|field| field.path == "game.phase"
            && field.type_name == "Phase"
            && field.storage_type_name() == "i32"));
        let enemies = layout
            .collections
            .iter()
            .find(|collection| collection.path == "game.enemies")
            .expect("enemy collection layout");
        assert!(enemies.fields.iter().any(|field| field.field == "phase"
            && field.type_name == "Phase"
            && field.storage_type_name() == "i32"));
    }

    #[test]
    fn state_inspection_sample_runs_through_cranelift_and_exposes_live_queries() {
        let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/state_inspection/src/main.stasis");
        let source = std::fs::read_to_string(&sample).expect("read state inspection sample");
        let mut jit = JitProcess::new();
        jit.set_project_root(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .to_string_lossy(),
        )
        .expect("set repository root");
        jit.set_required_emit_roots(&[
            "main".to_string(),
            "tick".to_string(),
            "render".to_string(),
        ]);
        jit.upsert_file(sample.to_string_lossy(), source);
        jit.compile().expect("compile state inspection sample");
        assert!(jit
            .artifacts()
            .iter()
            .all(|artifact| !artifact.clif.is_empty() && artifact.code_ptr != 0));
        assert_eq!(
            jit.execute_i32_noarg_by_name("main")
                .expect("run sample main"),
            0
        );
        assert_eq!(
            jit.execute_i32_noarg_by_name("tick")
                .expect("run sample tick"),
            0
        );
        assert_eq!(
            jit.execute_i32_noarg_by_name("render")
                .expect("run sample render"),
            0
        );
        assert_eq!(
            jit.inspect_state_query("state.score + state.enemies[1].hp")
                .expect("sample expression")["value"]["value"],
            18
        );
        let predicate = jit
            .inspect_state_query("state.enemies[?hp >= 8]")
            .expect("sample predicate");
        assert_eq!(predicate["total_matches"], 1);
        assert_eq!(predicate["matches"][0]["index"], 2);
        assert_eq!(
            jit.read_global_collection_scalar("render_cmd_i32", "", 0)
                .expect("sample rendered command"),
            crate::backend::jit::JitScalarValue::I32(11)
        );
    }
}
