use super::compile_analysis::{CollectionInfoMap, GlobalPathTypeMap, TypedCollectionInfoMap};
use crate::frontend::types::{
    TypeCategory, TypeTable, TypedCollectionDescriptor, TypedCollectionKind,
    TypedCollectionOverflowPolicy, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
    TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8,
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

/// Identifies the kind of native storage represented by a generated symbol.
///
/// Scalar metadata (including collection `.length`/`.max_length` paths) and
/// collection backing lanes intentionally live in separate namespaces.  The
/// kind is part of the symbol identity rather than an incidental prefix on one
/// side, so a user path cannot recreate the other kind's generated symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AotStorageSymbolKind {
    Scalar,
    Array,
}

/// Return the canonical C/native storage symbol for a state path.
///
/// The path/field flattening remains the existing ABI spelling.  The explicit
/// kind namespace is new and is shared by native object storage and generated
/// C bridges so scalar metadata can never collide with an array field such as
/// a user-defined `length` field.
pub fn aot_storage_symbol(kind: AotStorageSymbolKind, path: &str, field: &str) -> String {
    let namespace = match kind {
        AotStorageSymbolKind::Scalar => "stasis_state_scalar__",
        AotStorageSymbolKind::Array => "stasis_state_array__",
    };
    let mut symbol = String::with_capacity(namespace.len() + path.len() + field.len() + 2);
    symbol.push_str(namespace);
    symbol.push_str(&path.replace('.', "__"));
    if !field.is_empty() {
        symbol.push_str("__");
        symbol.push_str(&field.replace('.', "__"));
    }
    symbol
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

pub fn is_command_buffer_path(path: &str) -> bool {
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
    typed_collection_descriptors: &TypedCollectionInfoMap,
    type_table: &TypeTable,
) -> Result<StateLayout, String> {
    let typed_paths = typed_collection_descriptors
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    for (path, descriptor) in typed_collection_descriptors {
        validate_typed_collection_placement(path, descriptor, global_path_types, type_table)?;
    }
    let mut scalars: Vec<StateScalarLayout> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            if typed_paths.contains(path) {
                return None;
            }
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
    for (path, descriptor) in typed_collection_descriptors {
        let mut fields = Vec::new();
        for lane in &descriptor.lanes {
            let type_name = type_table
                .type_info(lane.type_id)
                .map(|info| info.name.clone())
                .unwrap_or_else(|| format!("type{}", lane.type_id));
            let storage_type_name = typed_lane_storage_type_name(lane.type_id)
                .unwrap_or_default()
                .to_string();
            if typed_metadata_lane(lane.name.as_str()) {
                scalars.push(StateScalarLayout {
                    path: format!("{path}.{}", lane.name),
                    type_name,
                    storage_type_name,
                });
            } else {
                fields.push(StateCollectionFieldLayout {
                    field: lane.name.clone(),
                    type_name,
                    storage_type_name,
                });
            }
        }
        collections.push(StateCollectionLayout {
            path: path.clone(),
            capacity: i32::try_from(descriptor.capacity).map_err(|_| {
                format!(
                    "typed collection state path '{}' capacity {} exceeds i32 layout capacity",
                    path, descriptor.capacity
                )
            })?,
            element_shape: typed_collection_layout_metadata(descriptor, type_table),
            fully_migratable: false,
            fields,
        });
    }
    scalars.sort_by(|left, right| left.path.cmp(&right.path));
    for (path, type_id) in global_path_types {
        if typed_paths.contains(path) {
            continue;
        }
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
            if let Some(field) = collection
                .fields
                .iter_mut()
                .find(|field| field.field.is_empty())
            {
                field.type_name = "u8".to_string();
                field.storage_type_name = "u8".to_string();
            } else {
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
            if typed_paths.contains(path) {
                return None;
            }
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
            if typed_paths.contains(path) {
                return None;
            }
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
    Ok(StateLayout {
        scalars,
        collections,
        structs,
        opaque,
    })
}

/// Stable metadata spelling for compiler-owned collection state.  The
/// existing `element_shape` slot is used deliberately: widening the public
/// layout records would break app-side fixtures that construct those records
/// directly.  This spelling includes every descriptor fact that can affect
/// migration identity, including lane order and byte ranges.
pub(crate) fn typed_collection_layout_metadata(
    descriptor: &TypedCollectionDescriptor,
    type_table: &TypeTable,
) -> String {
    let lanes = descriptor
        .lanes
        .iter()
        .map(|lane| {
            let type_name = type_table
                .type_info(lane.type_id)
                .map(|info| info.name.as_str())
                .unwrap_or("?");
            format!(
                "{}:{}:{}:{}:{}:{}",
                lane.name,
                type_name,
                lane.element_count,
                lane.offset_bytes,
                lane.byte_size,
                lane.alignment_bytes
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "typed_collection{{type={};kind={};policy={};capacity={};width={};height={};static_size={};lanes=[{}]}}",
        descriptor.canonical_type_name(type_table),
        descriptor.kind_name(),
        descriptor.policy_name(),
        descriptor.capacity,
        descriptor.width.map_or_else(|| "-".to_string(), |value| value.to_string()),
        descriptor.height.map_or_else(|| "-".to_string(), |value| value.to_string()),
        descriptor.static_size_bytes,
        lanes,
    )
}

fn validate_typed_collection_placement(
    path: &str,
    descriptor: &TypedCollectionDescriptor,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<(), String> {
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "typed collection descriptor '{path}' has unsupported placement; only persistent global paths are supported"
        )
    })?;
    if !type_table.is_typed_collection_type(type_id) {
        return Err(format!(
            "typed collection descriptor '{path}' is not backed by its compiler-owned type identity"
        ));
    }
    let info = type_table.type_info(type_id).ok_or_else(|| {
        format!("typed collection descriptor '{path}' has no type metadata for type id {type_id}")
    })?;
    let canonical = descriptor.canonical_type_name(type_table);
    if info.name != canonical {
        return Err(format!(
            "typed collection descriptor '{path}' identity mismatch: type table has '{}', descriptor has '{canonical}'",
            info.name
        ));
    }
    if descriptor.capacity > i32::MAX as u32 {
        return Err(format!(
            "typed collection state path '{path}' capacity {} exceeds i32 layout capacity",
            descriptor.capacity
        ));
    }
    let supported_kind = matches!(
        descriptor.kind,
        TypedCollectionKind::Pool
            | TypedCollectionKind::StablePool
            | TypedCollectionKind::Queue
            | TypedCollectionKind::RingBuffer
            | TypedCollectionKind::PriorityQueue
            | TypedCollectionKind::Map
            | TypedCollectionKind::Set
    );
    let supported_policy = match descriptor.kind {
        TypedCollectionKind::Pool
        | TypedCollectionKind::StablePool
        | TypedCollectionKind::PriorityQueue
        | TypedCollectionKind::Map
        | TypedCollectionKind::Set => matches!(
            descriptor.policy,
            TypedCollectionOverflowPolicy::Error | TypedCollectionOverflowPolicy::DropNewest
        ),
        TypedCollectionKind::Queue | TypedCollectionKind::RingBuffer => matches!(
            descriptor.policy,
            TypedCollectionOverflowPolicy::Error
                | TypedCollectionOverflowPolicy::DropNewest
                | TypedCollectionOverflowPolicy::OverwriteOldest
        ),
        _ => false,
    };
    let supported_payload = match descriptor.kind {
        TypedCollectionKind::Pool
        | TypedCollectionKind::StablePool
        | TypedCollectionKind::Queue
        | TypedCollectionKind::RingBuffer => descriptor.element_type == Some(TYPE_ID_I32),
        TypedCollectionKind::PriorityQueue => descriptor.element_type == Some(TYPE_ID_I32),
        TypedCollectionKind::Map => {
            descriptor.key_type == Some(TYPE_ID_I32) && descriptor.value_type == Some(TYPE_ID_I32)
        }
        TypedCollectionKind::Set => descriptor.key_type == Some(TYPE_ID_I32),
        _ => false,
    };
    if !supported_kind || !supported_payload || !supported_policy {
        return Err(format!(
            "typed collection state path '{path}' is not yet supported for production layout: only pool<i32,N,error|drop_newest>, stable_pool<i32,N,error|drop_newest>, queue<i32,N,error|drop_newest|overwrite_oldest>, ring_buffer<i32,N,error|drop_newest|overwrite_oldest>, priority_queue<i32,N,error|drop_newest>, map<i32,i32,N,error|drop_newest>, and set<i32,N,error|drop_newest> have a descriptor-defined state contract; got {}",
            descriptor.canonical_type_name(type_table)
        ));
    }

    if descriptor.kind == TypedCollectionKind::StablePool
        && !stable_pool_lane_schema_matches(descriptor)
    {
        return Err(format!(
            "typed collection state path '{path}' stable_pool descriptor must use exact count:i32@0, occupied:u8[N], and aligned values:i32[N] lanes"
        ));
    }
    if descriptor.kind == TypedCollectionKind::Map && !map_lane_schema_matches(descriptor) {
        return Err(format!(
            "typed collection state path '{path}' map descriptor must use exact count:i32@0, occupied:u8[N], aligned keys:i32[N], and aligned values:i32[N] lanes"
        ));
    }
    if descriptor.kind == TypedCollectionKind::Set && !set_lane_schema_matches(descriptor) {
        return Err(format!(
            "typed collection state path '{path}' set descriptor must use exact count:i32@0, occupied:u8[N], and aligned keys:i32[N] lanes"
        ));
    }
    if descriptor.kind == TypedCollectionKind::PriorityQueue
        && !priority_queue_lane_schema_matches(descriptor)
    {
        return Err(format!(
            "typed collection state path '{path}' priority_queue descriptor must use exact count:i32@0, next_order:u32@4, priority:i32[N]@8, order:u32[N], and values:i32[N] lanes"
        ));
    }

    let mut names = BTreeSet::new();
    let mut previous_end = 0u64;
    let mut max_alignment = 1u64;
    for lane in &descriptor.lanes {
        if !names.insert(lane.name.as_str()) {
            return Err(format!(
                "typed collection state path '{path}' contains duplicate lane '{}'",
                lane.name
            ));
        }
        let storage_type = typed_lane_storage_type_name(lane.type_id).ok_or_else(|| {
            format!(
                "typed collection state path '{path}' lane '{}' has unsupported payload type id {}",
                lane.name, lane.type_id
            )
        })?;
        let element_bytes = storage_type_bytes(storage_type).ok_or_else(|| {
            format!(
                "typed collection state path '{path}' lane '{}' has unsupported storage type '{storage_type}'",
                lane.name
            )
        })?;
        let alignment = storage_type_alignment(storage_type).ok_or_else(|| {
            format!(
                "typed collection state path '{path}' lane '{}' has unsupported alignment",
                lane.name
            )
        })?;
        if lane.alignment_bytes != alignment {
            return Err(format!(
                "typed collection state path '{path}' lane '{}' alignment {} does not match scalar alignment {alignment}",
                lane.name, lane.alignment_bytes
            ));
        }
        if lane.offset_bytes % alignment != 0 {
            return Err(format!(
                "typed collection state path '{path}' lane '{}' offset {} is not aligned to {alignment}",
                lane.name, lane.offset_bytes
            ));
        }
        let expected_size = element_bytes
            .checked_mul(lane.element_count)
            .ok_or_else(|| {
                format!(
                    "typed collection state path '{path}' lane '{}' byte size overflow",
                    lane.name
                )
            })?;
        if lane.byte_size != expected_size {
            return Err(format!(
                "typed collection state path '{path}' lane '{}' byte size {} does not match {}",
                lane.name, lane.byte_size, expected_size
            ));
        }
        if lane.byte_size == 0 {
            if lane.offset_bytes < previous_end {
                return Err(format!(
                    "typed collection state path '{path}' lane '{}' overlaps a previous lane",
                    lane.name
                ));
            }
            continue;
        }
        if lane.offset_bytes < previous_end {
            return Err(format!(
                "typed collection state path '{path}' lane '{}' overlaps a previous lane",
                lane.name
            ));
        }
        previous_end = lane
            .offset_bytes
            .checked_add(lane.byte_size)
            .ok_or_else(|| {
                format!(
                    "typed collection state path '{path}' lane '{}' offset overflow",
                    lane.name
                )
            })?;
        max_alignment = max_alignment.max(alignment);
    }
    let expected_total = align_state_layout_offset(previous_end, max_alignment)?;
    if descriptor.static_size_bytes != expected_total {
        return Err(format!(
            "typed collection state path '{path}' total size {} does not match lane total {expected_total}",
            descriptor.static_size_bytes
        ));
    }
    Ok(())
}

fn stable_pool_lane_schema_matches(descriptor: &TypedCollectionDescriptor) -> bool {
    if descriptor.lanes.len() != 3 {
        return false;
    }

    let [count, occupied, values] = descriptor.lanes.as_slice() else {
        return false;
    };
    let capacity = u64::from(descriptor.capacity);
    let values_offset = if capacity == 0 {
        count.offset_bytes.checked_add(4).unwrap_or(u64::MAX)
    } else {
        count
            .offset_bytes
            .checked_add(4)
            .and_then(|offset| align_state_layout_offset(offset + capacity, 4).ok())
            .unwrap_or(u64::MAX)
    };

    count.name == "count"
        && count.type_id == TYPE_ID_I32
        && count.element_count == 1
        && count.offset_bytes == 0
        && count.byte_size == 4
        && count.alignment_bytes == 4
        && occupied.name == "occupied"
        && occupied.type_id == TYPE_ID_U8
        && occupied.element_count == capacity
        && occupied.offset_bytes == 4
        && occupied.byte_size == capacity
        && occupied.alignment_bytes == 1
        && values.name == "values"
        && values.type_id == TYPE_ID_I32
        && values.element_count == capacity
        && values.offset_bytes == values_offset
        && values.byte_size == capacity.saturating_mul(4)
        && values.alignment_bytes == 4
}

fn map_lane_schema_matches(descriptor: &TypedCollectionDescriptor) -> bool {
    if descriptor.lanes.len() != 4 {
        return false;
    }

    let [count, occupied, keys, values] = descriptor.lanes.as_slice() else {
        return false;
    };
    let capacity = u64::from(descriptor.capacity);
    let Some(keys_offset) = map_set_array_offset(capacity) else {
        return false;
    };
    let Some(values_offset) = keys_offset.checked_add(capacity.checked_mul(4).unwrap_or(u64::MAX))
    else {
        return false;
    };

    count_lane_schema_matches(count)
        && occupied_lane_schema_matches(occupied, capacity)
        && i32_array_lane_schema_matches(keys, "keys", capacity, keys_offset)
        && i32_array_lane_schema_matches(values, "values", capacity, values_offset)
}

fn set_lane_schema_matches(descriptor: &TypedCollectionDescriptor) -> bool {
    if descriptor.lanes.len() != 3 {
        return false;
    }

    let [count, occupied, keys] = descriptor.lanes.as_slice() else {
        return false;
    };
    let capacity = u64::from(descriptor.capacity);
    let Some(keys_offset) = map_set_array_offset(capacity) else {
        return false;
    };

    count_lane_schema_matches(count)
        && occupied_lane_schema_matches(occupied, capacity)
        && i32_array_lane_schema_matches(keys, "keys", capacity, keys_offset)
}

fn priority_queue_lane_schema_matches(descriptor: &TypedCollectionDescriptor) -> bool {
    if descriptor.lanes.len() != 5 {
        return false;
    }

    let [count, next_order, priority, order, values] = descriptor.lanes.as_slice() else {
        return false;
    };
    let capacity = u64::from(descriptor.capacity);
    let Some(order_offset) = 8u64.checked_add(capacity.checked_mul(4).unwrap_or(u64::MAX)) else {
        return false;
    };
    let Some(values_offset) = 8u64.checked_add(capacity.checked_mul(8).unwrap_or(u64::MAX)) else {
        return false;
    };

    count_lane_schema_matches(count)
        && next_order_lane_schema_matches(next_order)
        && i32_array_lane_schema_matches(priority, "priority", capacity, 8)
        && u32_array_lane_schema_matches(order, "order", capacity, order_offset)
        && i32_array_lane_schema_matches(values, "values", capacity, values_offset)
}

fn map_set_array_offset(capacity: u64) -> Option<u64> {
    let end_of_occupied = 4u64.checked_add(capacity)?;
    align_state_layout_offset(end_of_occupied, 4).ok()
}

fn count_lane_schema_matches(lane: &crate::frontend::types::TypedCollectionLane) -> bool {
    lane.name == "count"
        && lane.type_id == TYPE_ID_I32
        && lane.element_count == 1
        && lane.offset_bytes == 0
        && lane.byte_size == 4
        && lane.alignment_bytes == 4
}

fn occupied_lane_schema_matches(
    lane: &crate::frontend::types::TypedCollectionLane,
    capacity: u64,
) -> bool {
    lane.name == "occupied"
        && lane.type_id == TYPE_ID_U8
        && lane.element_count == capacity
        && lane.offset_bytes == 4
        && lane.byte_size == capacity
        && lane.alignment_bytes == 1
}

fn i32_array_lane_schema_matches(
    lane: &crate::frontend::types::TypedCollectionLane,
    name: &str,
    capacity: u64,
    offset: u64,
) -> bool {
    lane.name == name
        && lane.type_id == TYPE_ID_I32
        && lane.element_count == capacity
        && lane.offset_bytes == offset
        && lane.byte_size == capacity.saturating_mul(4)
        && lane.alignment_bytes == 4
}

fn next_order_lane_schema_matches(lane: &crate::frontend::types::TypedCollectionLane) -> bool {
    lane.name == "next_order"
        && lane.type_id == TYPE_ID_U32
        && lane.element_count == 1
        && lane.offset_bytes == 4
        && lane.byte_size == 4
        && lane.alignment_bytes == 4
}

fn u32_array_lane_schema_matches(
    lane: &crate::frontend::types::TypedCollectionLane,
    name: &str,
    capacity: u64,
    offset: u64,
) -> bool {
    lane.name == name
        && lane.type_id == TYPE_ID_U32
        && lane.element_count == capacity
        && lane.offset_bytes == offset
        && lane.byte_size == capacity.saturating_mul(4)
        && lane.alignment_bytes == 4
}

fn typed_lane_storage_type_name(type_id: u16) -> Option<&'static str> {
    match type_id {
        TYPE_ID_I32 => Some("i32"),
        TYPE_ID_F32 => Some("f32"),
        TYPE_ID_F64 => Some("f64"),
        TYPE_ID_BOOL => Some("bool"),
        TYPE_ID_U8 => Some("u8"),
        TYPE_ID_U16 => Some("u16"),
        TYPE_ID_U32 => Some("u32"),
        _ => None,
    }
}

fn typed_metadata_lane(name: &str) -> bool {
    matches!(name, "count" | "head" | "next_order")
}

fn align_state_layout_offset(offset: u64, alignment: u64) -> Result<u64, String> {
    let adjustment = alignment
        .checked_sub(1)
        .ok_or_else(|| "typed collection alignment cannot be zero".to_string())?;
    offset
        .checked_add(adjustment)
        .map(|value| value / alignment * alignment)
        .ok_or_else(|| "typed collection alignment arithmetic overflow".to_string())
}

pub(crate) fn is_named_scalar_state_path(
    path: &str,
    type_id: u16,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> bool {
    type_table.type_info(type_id).is_some_and(|info| {
        info.category == TypeCategory::Named && !type_table.is_typed_collection_type(type_id)
    }) && !global_path_types
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
    use super::{
        aot_storage_symbol, build_state_layout, build_state_memory_report,
        is_named_scalar_state_path, state_layout_digest, AotStorageSymbolKind,
    };
    use crate::backend::aot::AotProcess;
    use crate::backend::compile_analysis::{
        collect_foreach_collection_infos, collect_global_path_types,
        collect_top_level_constant_values, collect_typed_collection_descriptors,
    };
    use crate::backend::jit::JitProcess;
    use crate::compiler::SourceFile;
    use crate::frontend::types::TypeTable;
    use std::collections::BTreeMap;

    fn typed_layout_result(type_name: &str) -> Result<super::StateLayout, String> {
        let content =
            format!("global actors: {type_name};\nfunction main(): i32 {{ return 0; }}\n");
        let files = vec![SourceFile {
            path: "typed_pool.stasis".to_string(),
            hash: 0,
            original_content: content.clone(),
            content,
            functions: Vec::new(),
        }];
        let mut types = TypeTable::new();
        types.ensure_utf8_view_id().expect("utf8 view");
        types.ensure_ascii_view_id().expect("ascii view");
        let constants = collect_top_level_constant_values(&files, &mut types).expect("constants");
        let globals =
            collect_global_path_types(&files, &mut types, &constants).expect("global paths");
        let collections =
            collect_foreach_collection_infos(&files, &mut types, &constants).expect("arrays");
        let typed =
            collect_typed_collection_descriptors(&files, &mut types).expect("typed descriptors");
        build_state_layout(&globals, &collections, &typed, &types)
    }

    fn typed_layout(type_name: &str) -> super::StateLayout {
        typed_layout_result(type_name).expect("state layout")
    }

    #[test]
    fn aot_storage_symbol_namespaces_scalar_metadata_and_array_fields() {
        let scalar = aot_storage_symbol(
            AotStorageSymbolKind::Scalar,
            "app.maze_wall_cache.horizontal.length",
            "",
        );
        let array = aot_storage_symbol(
            AotStorageSymbolKind::Array,
            "app.maze_wall_cache.horizontal",
            "length",
        );

        assert_eq!(
            scalar,
            "stasis_state_scalar__app__maze_wall_cache__horizontal__length"
        );
        assert_eq!(
            array,
            "stasis_state_array__app__maze_wall_cache__horizontal__length"
        );
        assert_ne!(scalar, array);
        assert!(scalar.starts_with("stasis_state_scalar__"));
        assert!(array.starts_with("stasis_state_array__"));
        assert_ne!(
            aot_storage_symbol(
                AotStorageSymbolKind::Scalar,
                "stasis_state_array__app.maze_wall_cache.horizontal",
                "length",
            ),
            array,
            "a scalar user path cannot recreate an array namespace symbol"
        );
    }

    #[test]
    fn typed_pool_layout_is_descriptor_owned_and_has_count_then_values_lanes() {
        let layout = typed_layout("pool<i32, 2, error>");
        assert!(layout.scalars.iter().all(|scalar| scalar.path != "actors"));
        let count = layout
            .scalars
            .iter()
            .find(|scalar| scalar.path == "actors.count")
            .expect("typed pool count scalar");
        assert_eq!(count.type_name, "i32");
        assert_eq!(count.storage_type_name(), "i32");
        let pool = layout
            .collections
            .iter()
            .find(|collection| collection.path == "actors")
            .expect("typed pool collection layout");
        assert_eq!(pool.capacity, 2);
        assert!(!pool.fully_migratable);
        assert_eq!(
            pool.fields
                .iter()
                .map(|field| field.field.as_str())
                .collect::<Vec<_>>(),
            ["values"]
        );
        assert_eq!(pool.fields[0].storage_type_name(), "i32");
        assert!(pool
            .element_shape
            .contains("typed_collection{type=pool<i32, 2, error>"));
        assert!(pool
            .element_shape
            .contains("lanes=[count:i32:1:0:4:4,values:i32:2:4:8:4]"));
    }

    #[test]
    fn typed_collection_root_is_not_a_named_scalar_state_path() {
        let mut types = TypeTable::new();
        let typed = types
            .resolve_or_intern("pool<i32, 2, error>")
            .expect("typed pool type");
        let nominal = types
            .resolve_or_intern("Entity")
            .expect("nominal scalar type");
        let global_path_types = BTreeMap::from([
            ("actors".to_string(), typed),
            ("entity".to_string(), nominal),
        ]);

        assert_eq!(
            types.type_info(typed).map(|info| info.category),
            Some(crate::frontend::types::TypeCategory::Named)
        );
        assert!(!is_named_scalar_state_path(
            "actors",
            typed,
            &global_path_types,
            &types
        ));
        assert!(is_named_scalar_state_path(
            "entity",
            nominal,
            &global_path_types,
            &types
        ));
    }

    #[test]
    fn typed_pool_zero_capacity_keeps_metadata_lane_without_payload_bytes() {
        let layout = typed_layout("pool<i32, 0, drop_newest>");
        assert!(layout.scalars.iter().all(|scalar| scalar.path != "actors"));
        assert!(layout
            .scalars
            .iter()
            .any(|scalar| scalar.path == "actors.count"));
        let pool = layout
            .collections
            .iter()
            .find(|collection| collection.path == "actors")
            .expect("zero-capacity typed pool layout");
        assert_eq!(pool.capacity, 0);
        assert_eq!(
            pool.fields
                .iter()
                .map(|field| field.field.as_str())
                .collect::<Vec<_>>(),
            ["values"]
        );
        assert!(pool.element_shape.contains("type=pool<i32, 0, drop_newest"));
        assert!(pool
            .element_shape
            .contains("lanes=[count:i32:1:0:4:4,values:i32:0:4:0:4]"));
    }

    #[test]
    fn typed_stable_pool_layout_has_exact_lanes_for_every_supported_policy() {
        for policy in ["error", "drop_newest"] {
            let layout = typed_layout(&format!("stable_pool<i32, 2, {policy}>"));
            assert!(layout.scalars.iter().all(|scalar| scalar.path != "actors"));
            let count = layout
                .scalars
                .iter()
                .find(|scalar| scalar.path == "actors.count")
                .expect("typed stable pool count scalar");
            assert_eq!(count.type_name, "i32");
            assert_eq!(count.storage_type_name(), "i32");

            let pool = layout
                .collections
                .iter()
                .find(|collection| collection.path == "actors")
                .expect("typed stable pool collection layout");
            assert_eq!(pool.capacity, 2);
            assert!(!pool.fully_migratable);
            assert_eq!(
                pool.fields
                    .iter()
                    .map(|field| field.field.as_str())
                    .collect::<Vec<_>>(),
                ["occupied", "values"]
            );
            assert_eq!(pool.fields[0].type_name, "u8");
            assert_eq!(pool.fields[0].storage_type_name(), "u8");
            assert_eq!(pool.fields[1].type_name, "i32");
            assert_eq!(pool.fields[1].storage_type_name(), "i32");
            assert_eq!(
                pool.element_shape,
                format!(
                    "typed_collection{{type=stable_pool<i32, 2, {policy}>;kind=stable_pool;policy={policy};capacity=2;width=-;height=-;static_size=16;lanes=[count:i32:1:0:4:4,occupied:u8:2:4:2:1,values:i32:2:8:8:4]}}"
                )
            );
        }
    }

    #[test]
    fn typed_stable_pool_zero_capacity_keeps_occupied_and_values_lanes_empty() {
        for policy in ["error", "drop_newest"] {
            let layout = typed_layout(&format!("stable_pool<i32, 0, {policy}>"));
            let count = layout
                .scalars
                .iter()
                .find(|scalar| scalar.path == "actors.count")
                .expect("zero-capacity typed stable pool count scalar");
            assert_eq!(count.storage_type_name(), "i32");
            let pool = layout
                .collections
                .iter()
                .find(|collection| collection.path == "actors")
                .expect("zero-capacity typed stable pool collection layout");
            assert_eq!(pool.capacity, 0);
            assert_eq!(
                pool.fields
                    .iter()
                    .map(|field| field.field.as_str())
                    .collect::<Vec<_>>(),
                ["occupied", "values"]
            );
            assert_eq!(
                pool.element_shape,
                format!(
                    "typed_collection{{type=stable_pool<i32, 0, {policy}>;kind=stable_pool;policy={policy};capacity=0;width=-;height=-;static_size=4;lanes=[count:i32:1:0:4:4,occupied:u8:0:4:0:1,values:i32:0:4:0:4]}}"
                )
            );
        }
    }

    #[test]
    fn typed_map_and_set_layouts_have_exact_i32_lanes_for_every_policy_and_capacity() {
        for policy in ["error", "drop_newest"] {
            for capacity in [0, 1, 3] {
                let map = typed_layout(&format!("map<i32, i32, {capacity}, {policy}>"));
                assert_eq!(
                    map.scalars
                        .iter()
                        .map(|scalar| scalar.path.as_str())
                        .collect::<Vec<_>>(),
                    ["actors.count"]
                );
                let map_layout = map
                    .collections
                    .iter()
                    .find(|collection| collection.path == "actors")
                    .expect("typed map collection layout");
                assert_eq!(map_layout.capacity, capacity);
                assert!(!map_layout.fully_migratable);
                assert_eq!(
                    map_layout
                        .fields
                        .iter()
                        .map(|field| field.field.as_str())
                        .collect::<Vec<_>>(),
                    ["occupied", "keys", "values"]
                );
                assert_eq!(
                    map_layout.element_shape,
                    format!(
                        "typed_collection{{type=map<i32, i32, {capacity}, {policy}>;kind=map;policy={policy};capacity={capacity};width=-;height=-;static_size={};lanes=[{}]}}",
                        match capacity {
                            0 => 4,
                            1 => 16,
                            3 => 32,
                            _ => unreachable!(),
                        },
                        match capacity {
                            0 => "count:i32:1:0:4:4,occupied:u8:0:4:0:1,keys:i32:0:4:0:4,values:i32:0:4:0:4",
                            1 => "count:i32:1:0:4:4,occupied:u8:1:4:1:1,keys:i32:1:8:4:4,values:i32:1:12:4:4",
                            3 => "count:i32:1:0:4:4,occupied:u8:3:4:3:1,keys:i32:3:8:12:4,values:i32:3:20:12:4",
                            _ => unreachable!(),
                        }
                    )
                );

                let set = typed_layout(&format!("set<i32, {capacity}, {policy}>"));
                assert_eq!(
                    set.scalars
                        .iter()
                        .map(|scalar| scalar.path.as_str())
                        .collect::<Vec<_>>(),
                    ["actors.count"]
                );
                let set_layout = set
                    .collections
                    .iter()
                    .find(|collection| collection.path == "actors")
                    .expect("typed set collection layout");
                assert_eq!(set_layout.capacity, capacity);
                assert!(!set_layout.fully_migratable);
                assert_eq!(
                    set_layout
                        .fields
                        .iter()
                        .map(|field| field.field.as_str())
                        .collect::<Vec<_>>(),
                    ["occupied", "keys"]
                );
                assert_eq!(
                    set_layout.element_shape,
                    format!(
                        "typed_collection{{type=set<i32, {capacity}, {policy}>;kind=set;policy={policy};capacity={capacity};width=-;height=-;static_size={};lanes=[{}]}}",
                        match capacity {
                            0 => 4,
                            1 => 12,
                            3 => 20,
                            _ => unreachable!(),
                        },
                        match capacity {
                            0 => "count:i32:1:0:4:4,occupied:u8:0:4:0:1,keys:i32:0:4:0:4",
                            1 => "count:i32:1:0:4:4,occupied:u8:1:4:1:1,keys:i32:1:8:4:4",
                            3 => "count:i32:1:0:4:4,occupied:u8:3:4:3:1,keys:i32:3:8:12:4",
                            _ => unreachable!(),
                        }
                    )
                );
            }
        }
    }

    #[test]
    fn typed_priority_queue_layout_has_exact_metadata_and_soa_lanes() {
        for policy in ["error", "drop_newest"] {
            for capacity in [0, 1, 3] {
                let layout = typed_layout(&format!("priority_queue<i32, {capacity}, {policy}>"));
                assert_eq!(
                    layout
                        .scalars
                        .iter()
                        .map(|scalar| scalar.path.as_str())
                        .collect::<Vec<_>>(),
                    ["actors.count", "actors.next_order"]
                );
                let count = layout
                    .scalars
                    .iter()
                    .find(|scalar| scalar.path == "actors.count")
                    .expect("priority queue count scalar");
                assert_eq!(count.storage_type_name(), "i32");
                let next_order = layout
                    .scalars
                    .iter()
                    .find(|scalar| scalar.path == "actors.next_order")
                    .expect("priority queue next_order scalar");
                assert_eq!(next_order.storage_type_name(), "u32");

                let queue = layout
                    .collections
                    .iter()
                    .find(|collection| collection.path == "actors")
                    .expect("priority queue collection layout");
                assert_eq!(queue.capacity, capacity);
                assert!(!queue.fully_migratable);
                assert_eq!(
                    queue
                        .fields
                        .iter()
                        .map(|field| field.field.as_str())
                        .collect::<Vec<_>>(),
                    ["priority", "order", "values"]
                );
                let (static_size, lanes) = match capacity {
                    0 => (
                        8,
                        "count:i32:1:0:4:4,next_order:u32:1:4:4:4,priority:i32:0:8:0:4,order:u32:0:8:0:4,values:i32:0:8:0:4",
                    ),
                    1 => (
                        20,
                        "count:i32:1:0:4:4,next_order:u32:1:4:4:4,priority:i32:1:8:4:4,order:u32:1:12:4:4,values:i32:1:16:4:4",
                    ),
                    3 => (
                        44,
                        "count:i32:1:0:4:4,next_order:u32:1:4:4:4,priority:i32:3:8:12:4,order:u32:3:20:12:4,values:i32:3:32:12:4",
                    ),
                    _ => unreachable!(),
                };
                assert_eq!(
                    queue.element_shape,
                    format!(
                        "typed_collection{{type=priority_queue<i32, {capacity}, {policy}>;kind=priority_queue;policy={policy};capacity={capacity};width=-;height=-;static_size={static_size};lanes=[{lanes}]}}"
                    )
                );
            }
        }
    }

    #[test]
    fn typed_queue_layout_keeps_count_and_head_metadata_for_every_policy() {
        for policy in ["error", "drop_newest", "overwrite_oldest"] {
            let layout = typed_layout(&format!("queue<i32, 2, {policy}>"));
            let queue = layout
                .collections
                .iter()
                .find(|collection| collection.path == "actors")
                .expect("typed queue collection layout");
            assert_eq!(queue.capacity, 2);
            assert_eq!(
                queue
                    .fields
                    .iter()
                    .map(|field| field.field.as_str())
                    .collect::<Vec<_>>(),
                ["values"]
            );
            assert!(queue
                .element_shape
                .contains(&format!("typed_collection{{type=queue<i32, 2, {policy}>")));
            assert!(queue
                .element_shape
                .contains("lanes=[count:i32:1:0:4:4,head:i32:1:4:4:4,values:i32:2:8:8:4]"));
            for lane in ["count", "head"] {
                let scalar = layout
                    .scalars
                    .iter()
                    .find(|scalar| scalar.path == format!("actors.{lane}"))
                    .unwrap_or_else(|| panic!("typed queue {lane} scalar"));
                assert_eq!(scalar.storage_type_name(), "i32");
            }
        }
    }

    #[test]
    fn typed_queue_zero_capacity_retains_count_head_and_empty_values_lane() {
        let layout = typed_layout("queue<i32, 0, overwrite_oldest>");
        for lane in ["count", "head"] {
            assert!(layout
                .scalars
                .iter()
                .any(|scalar| scalar.path == format!("actors.{lane}")));
        }
        let queue = layout
            .collections
            .iter()
            .find(|collection| collection.path == "actors")
            .expect("zero-capacity typed queue layout");
        assert_eq!(queue.capacity, 0);
        assert_eq!(
            queue
                .fields
                .iter()
                .map(|field| field.field.as_str())
                .collect::<Vec<_>>(),
            ["values"]
        );
        assert!(queue
            .element_shape
            .contains("type=queue<i32, 0, overwrite_oldest"));
        assert!(queue
            .element_shape
            .contains("lanes=[count:i32:1:0:4:4,head:i32:1:4:4:4,values:i32:0:8:0:4]"));
    }

    #[test]
    fn typed_ring_buffer_layout_has_exact_count_head_and_values_lanes_for_every_policy() {
        for policy in ["error", "drop_newest", "overwrite_oldest"] {
            let layout = typed_layout(&format!("ring_buffer<i32, 2, {policy}>"));
            assert!(layout.scalars.iter().all(|scalar| scalar.path != "actors"));
            let count = layout
                .scalars
                .iter()
                .find(|scalar| scalar.path == "actors.count")
                .expect("typed ring buffer count scalar");
            assert_eq!(count.type_name, "i32");
            assert_eq!(count.storage_type_name(), "i32");
            let head = layout
                .scalars
                .iter()
                .find(|scalar| scalar.path == "actors.head")
                .expect("typed ring buffer head scalar");
            assert_eq!(head.type_name, "i32");
            assert_eq!(head.storage_type_name(), "i32");

            let ring = layout
                .collections
                .iter()
                .find(|collection| collection.path == "actors")
                .expect("typed ring buffer collection layout");
            assert_eq!(ring.capacity, 2);
            assert!(!ring.fully_migratable);
            assert_eq!(
                ring.fields
                    .iter()
                    .map(|field| field.field.as_str())
                    .collect::<Vec<_>>(),
                ["values"]
            );
            assert_eq!(ring.fields[0].type_name, "i32");
            assert_eq!(ring.fields[0].storage_type_name(), "i32");
            assert_eq!(
                ring.element_shape,
                format!(
                    "typed_collection{{type=ring_buffer<i32, 2, {policy}>;kind=ring_buffer;policy={policy};capacity=2;width=-;height=-;static_size=16;lanes=[count:i32:1:0:4:4,head:i32:1:4:4:4,values:i32:2:8:8:4]}}"
                )
            );
        }
    }

    #[test]
    fn typed_ring_buffer_zero_capacity_has_exact_metadata_and_empty_values_lane() {
        let layout = typed_layout("ring_buffer<i32, 0, overwrite_oldest>");
        for lane in ["count", "head"] {
            let scalar = layout
                .scalars
                .iter()
                .find(|scalar| scalar.path == format!("actors.{lane}"))
                .unwrap_or_else(|| panic!("typed ring buffer {lane} scalar"));
            assert_eq!(scalar.type_name, "i32");
            assert_eq!(scalar.storage_type_name(), "i32");
        }
        let ring = layout
            .collections
            .iter()
            .find(|collection| collection.path == "actors")
            .expect("zero-capacity typed ring buffer layout");
        assert_eq!(ring.capacity, 0);
        assert!(!ring.fully_migratable);
        assert_eq!(
            ring.fields
                .iter()
                .map(|field| field.field.as_str())
                .collect::<Vec<_>>(),
            ["values"]
        );
        assert_eq!(ring.fields[0].type_name, "i32");
        assert_eq!(ring.fields[0].storage_type_name(), "i32");
        assert_eq!(
            ring.element_shape,
            "typed_collection{type=ring_buffer<i32, 0, overwrite_oldest>;kind=ring_buffer;policy=overwrite_oldest;capacity=0;width=-;height=-;static_size=8;lanes=[count:i32:1:0:4:4,head:i32:1:4:4:4,values:i32:0:8:0:4]}"
        );
    }

    #[test]
    fn typed_pool_policy_and_capacity_are_state_layout_identity() {
        let error = state_layout_digest(&typed_layout("pool<i32, 2, error>")).expect("digest");
        let drop = state_layout_digest(&typed_layout("pool<i32, 2, drop_newest>")).expect("digest");
        let larger = state_layout_digest(&typed_layout("pool<i32, 3, error>")).expect("digest");
        assert_ne!(error, drop, "overflow policy is part of layout identity");
        assert_ne!(error, larger, "capacity is part of layout identity");
    }

    #[test]
    fn typed_stable_pool_memory_report_counts_each_soa_lane_exactly() {
        let layout = typed_layout("stable_pool<i32, 2, error>");
        let active_counts = BTreeMap::from([("actors".to_string(), 1)]);
        let capacity_overrides = BTreeMap::from([("actors".to_string(), 4)]);
        let report =
            build_state_memory_report(&layout, &active_counts, &capacity_overrides, u64::MAX)
                .expect("stable pool memory report");

        let count = report
            .entries
            .iter()
            .find(|entry| entry.path == "actors.count")
            .expect("stable pool count report entry");
        assert_eq!(count.element_bytes, 4);
        assert_eq!(count.capacity, 1);
        assert_eq!(count.capacity_bytes, 4);

        let occupied = report
            .entries
            .iter()
            .find(|entry| entry.path == "actors" && entry.field == "occupied")
            .expect("stable pool occupied report entry");
        assert_eq!(occupied.element_bytes, 1);
        assert_eq!(occupied.capacity, 2);
        assert_eq!(occupied.active_count, Some(1));
        assert_eq!(occupied.capacity_bytes, 2);
        assert_eq!(occupied.active_bytes, Some(1));

        let values = report
            .entries
            .iter()
            .find(|entry| entry.path == "actors" && entry.field == "values")
            .expect("stable pool values report entry");
        assert_eq!(values.element_bytes, 4);
        assert_eq!(values.capacity, 2);
        assert_eq!(values.capacity_bytes, 8);
        assert_eq!(values.active_bytes, Some(4));

        let pool = report
            .largest_pools
            .iter()
            .find(|pool| pool.path == "actors")
            .expect("stable pool report");
        assert_eq!(pool.bytes_per_element, 5);
        assert_eq!(pool.capacity_bytes, 10);
        assert_eq!(pool.active_count, Some(1));
        assert_eq!(pool.active_bytes, Some(5));
        assert_eq!(report.total_capacity_bytes, 14);
        assert_eq!(report.projected_capacity_bytes, 24);
        assert_eq!(report.capacity_changes[0].delta_bytes, 10);
    }

    #[test]
    fn typed_map_and_set_memory_reports_count_each_soa_lane_exactly() {
        for policy in ["error", "drop_newest"] {
            for kind in ["map", "set"] {
                for capacity in [0_u64, 1, 3] {
                    let type_name = if kind == "map" {
                        format!("map<i32, i32, {capacity}, {policy}>")
                    } else {
                        format!("set<i32, {capacity}, {policy}>")
                    };
                    let layout = typed_layout(&type_name);
                    let active_counts = BTreeMap::from([("actors".to_string(), 1)]);
                    let capacity_overrides = BTreeMap::from([("actors".to_string(), capacity + 1)]);
                    let report = build_state_memory_report(
                        &layout,
                        &active_counts,
                        &capacity_overrides,
                        u64::MAX,
                    )
                    .expect("typed map/set memory report");

                    let fields = if kind == "map" {
                        vec![("occupied", 1_u64), ("keys", 4), ("values", 4)]
                    } else {
                        vec![("occupied", 1_u64), ("keys", 4)]
                    };
                    assert_eq!(report.entries.len(), fields.len() + 1);
                    let count = report
                        .entries
                        .iter()
                        .find(|entry| entry.path == "actors.count")
                        .expect("typed map/set count report entry");
                    assert_eq!(count.element_bytes, 4);
                    assert_eq!(count.capacity, 1);
                    assert_eq!(count.capacity_bytes, 4);

                    let active_capacity = capacity.min(1);
                    for (field, element_bytes) in &fields {
                        let entry = report
                            .entries
                            .iter()
                            .find(|entry| entry.path == "actors" && entry.field == *field)
                            .unwrap_or_else(|| panic!("typed {kind} {field} report entry"));
                        assert_eq!(entry.element_bytes, *element_bytes);
                        assert_eq!(entry.capacity, capacity);
                        assert_eq!(entry.active_count, Some(active_capacity));
                        assert_eq!(entry.capacity_bytes, capacity * element_bytes);
                        assert_eq!(entry.active_bytes, Some(active_capacity * element_bytes));
                    }

                    let bytes_per_element = fields
                        .iter()
                        .map(|(_, element_bytes)| element_bytes)
                        .sum::<u64>();
                    let pool = report
                        .largest_pools
                        .iter()
                        .find(|pool| pool.path == "actors")
                        .expect("typed map/set pool report");
                    assert_eq!(pool.capacity, capacity);
                    assert_eq!(pool.active_count, Some(active_capacity));
                    assert_eq!(pool.bytes_per_element, bytes_per_element);
                    assert_eq!(pool.capacity_bytes, capacity * bytes_per_element);
                    assert_eq!(pool.active_bytes, Some(active_capacity * bytes_per_element));
                    assert_eq!(
                        report.total_capacity_bytes,
                        4 + capacity * bytes_per_element
                    );
                    assert_eq!(
                        report.projected_capacity_bytes,
                        4 + (capacity + 1) * bytes_per_element
                    );
                    assert_eq!(report.capacity_changes.len(), 1);
                    assert_eq!(report.capacity_changes[0].old_capacity, capacity);
                    assert_eq!(report.capacity_changes[0].new_capacity, capacity + 1);
                    assert_eq!(
                        report.capacity_changes[0].bytes_per_element,
                        bytes_per_element
                    );
                    assert_eq!(
                        report.capacity_changes[0].delta_bytes,
                        i64::try_from(bytes_per_element).expect("report delta")
                    );
                }
            }
        }
    }

    #[test]
    fn typed_priority_queue_memory_reports_each_lane_and_metadata_exactly() {
        for policy in ["error", "drop_newest"] {
            for capacity in [0_u64, 1, 3] {
                let layout = typed_layout(&format!("priority_queue<i32, {capacity}, {policy}>"));
                let active_counts = BTreeMap::from([("actors".to_string(), 1)]);
                let capacity_overrides = BTreeMap::from([("actors".to_string(), capacity + 1)]);
                let report = build_state_memory_report(
                    &layout,
                    &active_counts,
                    &capacity_overrides,
                    u64::MAX,
                )
                .expect("priority queue memory report");

                assert_eq!(report.entries.len(), 5);
                for path in ["actors.count", "actors.next_order"] {
                    let scalar = report
                        .entries
                        .iter()
                        .find(|entry| entry.path == path)
                        .unwrap_or_else(|| panic!("priority queue scalar report entry {path}"));
                    assert_eq!(scalar.element_bytes, 4);
                    assert_eq!(scalar.capacity, 1);
                    assert_eq!(scalar.capacity_bytes, 4);
                }

                let active_capacity = capacity.min(1);
                for field in ["priority", "order", "values"] {
                    let entry = report
                        .entries
                        .iter()
                        .find(|entry| entry.path == "actors" && entry.field == field)
                        .unwrap_or_else(|| panic!("priority queue {field} report entry"));
                    assert_eq!(entry.element_bytes, 4);
                    assert_eq!(entry.capacity, capacity);
                    assert_eq!(entry.active_count, Some(active_capacity));
                    assert_eq!(entry.capacity_bytes, capacity * 4);
                    assert_eq!(entry.active_bytes, Some(active_capacity * 4));
                }

                let pool = report
                    .largest_pools
                    .iter()
                    .find(|pool| pool.path == "actors")
                    .expect("priority queue pool report");
                assert_eq!(pool.capacity, capacity);
                assert_eq!(pool.active_count, Some(active_capacity));
                assert_eq!(pool.bytes_per_element, 12);
                assert_eq!(pool.capacity_bytes, capacity * 12);
                assert_eq!(pool.active_bytes, Some(active_capacity * 12));
                assert_eq!(report.total_capacity_bytes, 8 + capacity * 12);
                assert_eq!(report.projected_capacity_bytes, 8 + (capacity + 1) * 12);
                assert_eq!(report.capacity_changes.len(), 1);
                assert_eq!(report.capacity_changes[0].old_capacity, capacity);
                assert_eq!(report.capacity_changes[0].new_capacity, capacity + 1);
                assert_eq!(report.capacity_changes[0].bytes_per_element, 12);
                assert_eq!(report.capacity_changes[0].delta_bytes, 12);
            }
        }
    }

    #[test]
    fn unsupported_typed_collection_kinds_are_rejected_at_state_layout_boundary() {
        let bitset = typed_layout_result("bitset<8, error>")
            .expect_err("bitset layout must not claim pool-shaped storage");
        assert!(
            bitset.contains(
                "only pool<i32,N,error|drop_newest>, stable_pool<i32,N,error|drop_newest>, queue<i32,N,error|drop_newest|overwrite_oldest>, ring_buffer<i32,N,error|drop_newest|overwrite_oldest>, priority_queue<i32,N,error|drop_newest>, map<i32,i32,N,error|drop_newest>, and set<i32,N,error|drop_newest>"
            ),
            "{bitset}"
        );

        let wide_pool = typed_layout_result("pool<f64, 2, error>")
            .expect_err("non-i32 pool payload layout must be rejected");
        assert!(
            wide_pool.contains(
                "only pool<i32,N,error|drop_newest>, stable_pool<i32,N,error|drop_newest>, queue<i32,N,error|drop_newest|overwrite_oldest>, ring_buffer<i32,N,error|drop_newest|overwrite_oldest>, priority_queue<i32,N,error|drop_newest>, map<i32,i32,N,error|drop_newest>, and set<i32,N,error|drop_newest>"
            ),
            "{wide_pool}"
        );
    }

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
        let title = layout
            .collections
            .iter()
            .find(|collection| collection.path == "title")
            .expect("fixed UTF-8 collection layout");
        assert_eq!(title.capacity, 8);
        assert_eq!(title.fields[0].storage_type_name(), "u8");
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
        jit.upsert_file("tests/stasis/seams/state_layout.stasis", source);
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
