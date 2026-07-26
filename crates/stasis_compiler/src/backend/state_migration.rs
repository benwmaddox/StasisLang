use super::jit::{JitProcess, JitScalarValue, JitStateLayout};
pub use super::state_layout::state_layout_version;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_STATE_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StateMigrationPreview {
    pub schema_version: u16,
    pub changed_functions: Vec<String>,
    pub state_layout_compatible: bool,
    pub layout_changed: bool,
    pub from_layout_version: String,
    pub to_layout_version: String,
    pub migration_scope: StateMigrationScope,
    pub migration_steps: Vec<StateMigrationStep>,
    pub warnings: Vec<String>,
    pub rejection: Option<String>,
    pub estimated_commit_cost_us: u64,
    pub requires_explicit_apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StateMigrationScope {
    pub kind: &'static str,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateMigrationStepKind {
    Copy,
    Initialize,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StateMigrationStep {
    pub kind: StateMigrationStepKind,
    pub path: String,
    pub field: Option<String>,
    pub type_name: String,
    pub elements: u32,
    pub start_index: u32,
    pub from_capacity: Option<u32>,
    pub to_capacity: Option<u32>,
}

#[derive(Debug)]
struct CapturedMigrationState {
    scalars: Vec<(String, JitScalarValue)>,
    collections: Vec<(String, String, Vec<JitScalarValue>)>,
}

#[derive(Debug, Clone)]
struct CollectionFieldLayout {
    type_name: String,
    capacity: u32,
}

pub fn plan_state_migration(
    active: &JitStateLayout,
    incoming: &JitStateLayout,
    mut changed_functions: Vec<String>,
    requires_explicit_apply: bool,
    abi_rejection: Option<String>,
) -> Result<StateMigrationPreview, String> {
    let from_layout_version = state_layout_version(active)?;
    let to_layout_version = state_layout_version(incoming)?;
    let layout_changed = from_layout_version != to_layout_version;
    changed_functions.sort();
    changed_functions.dedup();

    let active_scalars = active
        .scalars
        .iter()
        .map(|entry| (entry.path.clone(), entry.type_name.clone()))
        .collect::<BTreeMap<_, _>>();
    let incoming_scalars = incoming
        .scalars
        .iter()
        .map(|entry| (entry.path.clone(), entry.type_name.clone()))
        .collect::<BTreeMap<_, _>>();
    let active_collections = collection_field_layouts(active)?;
    let incoming_collections = collection_field_layouts(incoming)?;
    let active_collection_layouts = active
        .collections
        .iter()
        .map(|collection| (collection.path.as_str(), collection))
        .collect::<BTreeMap<_, _>>();
    let incoming_collection_layouts = incoming
        .collections
        .iter()
        .map(|collection| (collection.path.as_str(), collection))
        .collect::<BTreeMap<_, _>>();
    let active_opaque = active
        .opaque
        .iter()
        .map(|entry| (entry.path.clone(), entry.type_name.clone()))
        .collect::<BTreeMap<_, _>>();
    let incoming_opaque = incoming
        .opaque
        .iter()
        .map(|entry| (entry.path.clone(), entry.type_name.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let mut rejections = Vec::new();
    let mut affected_roots = BTreeSet::new();

    if layout_changed {
        for (path, incoming_collection) in &incoming_collection_layouts {
            let active_collection = active_collection_layouts.get(path).copied();
            if incoming_collection.fully_migratable
                && active_collection.is_none_or(|collection| collection.fully_migratable)
            {
                continue;
            }
            let detail = active_collection.map_or_else(
                || format!("new shape '{}'", incoming_collection.element_shape),
                |collection| {
                    if collection.element_shape == incoming_collection.element_shape
                        && collection.capacity == incoming_collection.capacity
                    {
                        format!("shape '{}'", incoming_collection.element_shape)
                    } else {
                        format!(
                            "shape '{}' capacity {} -> shape '{}' capacity {}",
                            collection.element_shape,
                            collection.capacity,
                            incoming_collection.element_shape,
                            incoming_collection.capacity
                        )
                    }
                },
            );
            rejections.push(format!(
                "collection state path '{path}' has non-migratable {detail}"
            ));
            affected_roots.insert(migration_root(path));
        }
        for (path, active_collection) in &active_collection_layouts {
            if incoming_collection_layouts.contains_key(path) || active_collection.fully_migratable
            {
                continue;
            }
            rejections.push(format!(
                "removed collection state path '{path}' has non-migratable shape '{}'",
                active_collection.element_shape
            ));
            affected_roots.insert(migration_root(path));
        }
        for (path, incoming_type) in &incoming_scalars {
            if path.ends_with(".max_length") {
                continue;
            }
            match active_scalars.get(path) {
                Some(active_type) if active_type == incoming_type => steps.push(scalar_step(
                    StateMigrationStepKind::Copy,
                    path,
                    incoming_type,
                )),
                Some(active_type) => {
                    rejections.push(format!(
                        "state path '{path}' changed type '{active_type}' -> '{incoming_type}'"
                    ));
                    affected_roots.insert(migration_root(path));
                }
                None => {
                    steps.push(scalar_step(
                        StateMigrationStepKind::Initialize,
                        path,
                        incoming_type,
                    ));
                    affected_roots.insert(migration_root(path));
                }
            }
        }
        for (path, active_type) in &active_scalars {
            if path.ends_with(".max_length") || incoming_scalars.contains_key(path) {
                continue;
            }
            steps.push(scalar_step(
                StateMigrationStepKind::Remove,
                path,
                active_type,
            ));
            warnings.push(format!("removed state path '{path}' will be discarded"));
            affected_roots.insert(migration_root(path));
        }

        for ((path, field), incoming_field) in &incoming_collections {
            match active_collections.get(&(path.clone(), field.clone())) {
                Some(active_field) if active_field.type_name == incoming_field.type_name => {
                    let elements = active_field.capacity.min(incoming_field.capacity);
                    steps.push(collection_step(
                        StateMigrationStepKind::Copy,
                        path,
                        field,
                        &incoming_field.type_name,
                        0,
                        elements,
                        active_field.capacity,
                        incoming_field.capacity,
                    ));
                    if incoming_field.capacity < active_field.capacity {
                        warnings.push(format!(
                            "collection '{}' shrinks from {} to {}; elements [{}..{}) will be discarded",
                            display_collection_path(path, field),
                            active_field.capacity,
                            incoming_field.capacity,
                            incoming_field.capacity,
                            active_field.capacity,
                        ));
                        affected_roots.insert(migration_root(path));
                    } else if incoming_field.capacity > active_field.capacity {
                        steps.push(collection_step(
                            StateMigrationStepKind::Initialize,
                            path,
                            field,
                            &incoming_field.type_name,
                            active_field.capacity,
                            incoming_field.capacity - active_field.capacity,
                            active_field.capacity,
                            incoming_field.capacity,
                        ));
                        affected_roots.insert(migration_root(path));
                    }
                }
                Some(active_field) => {
                    rejections.push(format!(
                        "collection state path '{}' changed type '{}' -> '{}'",
                        display_collection_path(path, field),
                        active_field.type_name,
                        incoming_field.type_name,
                    ));
                    affected_roots.insert(migration_root(path));
                }
                None => {
                    steps.push(collection_step(
                        StateMigrationStepKind::Initialize,
                        path,
                        field,
                        &incoming_field.type_name,
                        0,
                        incoming_field.capacity,
                        0,
                        incoming_field.capacity,
                    ));
                    affected_roots.insert(migration_root(path));
                }
            }
        }
        for ((path, field), active_field) in &active_collections {
            if incoming_collections.contains_key(&(path.clone(), field.clone())) {
                continue;
            }
            steps.push(collection_step(
                StateMigrationStepKind::Remove,
                path,
                field,
                &active_field.type_name,
                0,
                active_field.capacity,
                active_field.capacity,
                0,
            ));
            warnings.push(format!(
                "removed collection state path '{}' will be discarded",
                display_collection_path(path, field)
            ));
            affected_roots.insert(migration_root(path));
        }
        for (path, incoming_type) in &incoming_opaque {
            match active_opaque.get(path) {
                Some(active_type) if active_type == incoming_type => {}
                Some(active_type) => {
                    rejections.push(format!(
                        "state path '{path}' changed non-migratable type '{active_type}' -> '{incoming_type}'"
                    ));
                    affected_roots.insert(migration_root(path));
                }
                None => {
                    rejections.push(format!(
                        "new state path '{path}' uses non-migratable type '{incoming_type}'"
                    ));
                    affected_roots.insert(migration_root(path));
                }
            }
        }
        for (path, active_type) in &active_opaque {
            if incoming_opaque.contains_key(path) {
                continue;
            }
            rejections.push(format!(
                "removed state path '{path}' uses non-migratable type '{active_type}'"
            ));
            affected_roots.insert(migration_root(path));
        }
    }
    if let Some(rejection) = abi_rejection {
        rejections.push(rejection);
    }

    let state_layout_compatible = rejections.is_empty();
    let rejection = (!state_layout_compatible).then(|| {
        format!(
            "hot reload rejected: {}. Old code and state remain active.",
            rejections.join("; ")
        )
    });
    let migration_scope = if affected_roots.len() == 1 {
        let path = affected_roots
            .into_iter()
            .next()
            .expect("one affected root");
        let top_level_global = active_scalars.contains_key(&path)
            || incoming_scalars.contains_key(&path)
            || active_opaque.contains_key(&path)
            || incoming_opaque.contains_key(&path)
            || active_collections
                .keys()
                .any(|(collection, _)| collection == &path)
            || incoming_collections
                .keys()
                .any(|(collection, _)| collection == &path);
        StateMigrationScope {
            kind: if top_level_global {
                "whole_state"
            } else {
                "struct"
            },
            path: (!top_level_global).then_some(path),
        }
    } else if affected_roots.is_empty() {
        StateMigrationScope {
            kind: "none",
            path: None,
        }
    } else {
        StateMigrationScope {
            kind: "whole_state",
            path: None,
        }
    };
    let state_values = steps
        .iter()
        .filter(|step| step.kind != StateMigrationStepKind::Remove)
        .map(|step| u64::from(step.elements))
        .sum::<u64>();
    let estimated_commit_cost_us = 20u64
        .saturating_add((changed_functions.len() as u64).saturating_mul(5))
        .saturating_add(state_values.saturating_mul(8).div_ceil(256));

    Ok(StateMigrationPreview {
        schema_version: 1,
        changed_functions,
        state_layout_compatible,
        layout_changed,
        from_layout_version,
        to_layout_version,
        migration_scope,
        migration_steps: steps,
        warnings,
        rejection,
        estimated_commit_cost_us,
        requires_explicit_apply,
    })
}

pub fn finalize_runtime_preview(candidate: &JitProcess, preview: &mut StateMigrationPreview) {
    if !preview.state_layout_compatible {
        return;
    }
    if let Err(error) = preflight_collection_growth(candidate, preview) {
        preview.state_layout_compatible = false;
        preview.rejection = Some(format!(
            "hot reload rejected: {error}. Old code and state remain active."
        ));
    }
}

pub fn activate_candidate_transactionally<T>(
    active: Option<&JitProcess>,
    candidate: &JitProcess,
    preview: &StateMigrationPreview,
    hook_may_mutate_state: bool,
    apply: impl FnOnce() -> T,
    accepted: impl FnOnce(&T) -> bool,
) -> Result<T, String> {
    if !preview.state_layout_compatible {
        return Err(preview
            .rejection
            .clone()
            .unwrap_or_else(|| "incoming state layout is incompatible".to_string()));
    }
    preflight_collection_growth(candidate, preview)?;
    let needs_snapshot = preview.layout_changed || hook_may_mutate_state;
    let snapshot = needs_snapshot
        .then(|| stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES))
        .transpose()?;
    let migration_state = if preview.layout_changed {
        let active = active.ok_or_else(|| {
            "layout migration requires an active JIT candidate at the safe point".to_string()
        })?;
        Some(capture_migration_state(
            active,
            preview,
            MAX_STATE_SNAPSHOT_BYTES,
        )?)
    } else {
        None
    };

    let transaction = (|| {
        candidate.activate_staged_runtime()?;
        if let Some(snapshot) = snapshot.as_ref() {
            if preview.layout_changed {
                stasis_dynload::restore_jit_runtime_state(snapshot);
                prepare_collection_growth(candidate, preview)?;
                candidate.activate_staged_runtime()?;
                apply_migration_state(
                    candidate,
                    preview,
                    migration_state
                        .as_ref()
                        .expect("layout changes capture migration state"),
                )?;
            }
        }
        Ok(apply())
    })();
    let result = match transaction {
        Ok(result) => result,
        Err(error) => {
            rollback_runtime(active, snapshot.as_ref())?;
            return Err(error);
        }
    };
    if !accepted(&result) {
        rollback_runtime(active, snapshot.as_ref())?;
    }
    Ok(result)
}

fn rollback_runtime(
    active: Option<&JitProcess>,
    snapshot: Option<&stasis_dynload::JitRuntimeStateSnapshot>,
) -> Result<(), String> {
    if let Some(active) = active {
        active.activate_staged_runtime().map_err(|error| {
            format!("failed restoring active JIT runtime after rejected candidate: {error}")
        })?;
    }
    if let Some(snapshot) = snapshot {
        stasis_dynload::restore_jit_runtime_state(snapshot);
    }
    Ok(())
}

fn collection_field_layouts(
    layout: &JitStateLayout,
) -> Result<BTreeMap<(String, String), CollectionFieldLayout>, String> {
    let mut fields = BTreeMap::new();
    for collection in &layout.collections {
        let capacity = u32::try_from(collection.capacity).map_err(|_| {
            format!(
                "collection '{}' has negative capacity {}",
                collection.path, collection.capacity
            )
        })?;
        for field in &collection.fields {
            fields.insert(
                (collection.path.clone(), field.field.clone()),
                CollectionFieldLayout {
                    type_name: field.type_name.clone(),
                    capacity,
                },
            );
        }
    }
    Ok(fields)
}

fn scalar_step(kind: StateMigrationStepKind, path: &str, type_name: &str) -> StateMigrationStep {
    StateMigrationStep {
        kind,
        path: path.to_string(),
        field: None,
        type_name: type_name.to_string(),
        elements: 1,
        start_index: 0,
        from_capacity: None,
        to_capacity: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn collection_step(
    kind: StateMigrationStepKind,
    path: &str,
    field: &str,
    type_name: &str,
    start_index: u32,
    elements: u32,
    from_capacity: u32,
    to_capacity: u32,
) -> StateMigrationStep {
    StateMigrationStep {
        kind,
        path: path.to_string(),
        field: Some(field.to_string()),
        type_name: type_name.to_string(),
        elements,
        start_index,
        from_capacity: Some(from_capacity),
        to_capacity: Some(to_capacity),
    }
}

fn migration_root(path: &str) -> String {
    path.split('.').next().unwrap_or(path).to_string()
}

fn display_collection_path(path: &str, field: &str) -> String {
    if field.is_empty() {
        format!("{path}[]")
    } else {
        format!("{path}[].{field}")
    }
}

fn capture_migration_state(
    active: &JitProcess,
    preview: &StateMigrationPreview,
    max_bytes: usize,
) -> Result<CapturedMigrationState, String> {
    let copied_values = preview
        .migration_steps
        .iter()
        .filter(|step| step.kind == StateMigrationStepKind::Copy)
        .map(|step| step.elements as usize)
        .try_fold(0usize, |total, count| total.checked_add(count))
        .ok_or_else(|| "state migration value count overflow".to_string())?;
    let required_bytes = copied_values
        .checked_mul(std::mem::size_of::<JitScalarValue>())
        .ok_or_else(|| "state migration snapshot size overflow".to_string())?;
    if required_bytes > max_bytes {
        return Err(format!(
            "state migration requires {required_bytes} bytes, exceeding the {max_bytes}-byte live limit"
        ));
    }

    let mut scalars = Vec::new();
    let mut collections = Vec::new();
    for step in &preview.migration_steps {
        if step.kind != StateMigrationStepKind::Copy {
            continue;
        }
        if let Some(field) = step.field.as_deref() {
            let mut values = Vec::with_capacity(step.elements as usize);
            for index in step.start_index..step.start_index.saturating_add(step.elements) {
                values.push(active.read_global_collection_scalar(
                    &step.path,
                    field,
                    index as i32,
                )?);
            }
            collections.push((step.path.clone(), field.to_string(), values));
        } else {
            scalars.push((step.path.clone(), active.read_global_scalar(&step.path)?));
        }
    }
    Ok(CapturedMigrationState {
        scalars,
        collections,
    })
}

fn apply_migration_state(
    incoming: &JitProcess,
    preview: &StateMigrationPreview,
    captured: &CapturedMigrationState,
) -> Result<(), String> {
    let scalars = captured
        .scalars
        .iter()
        .map(|(path, value)| (path.as_str(), *value))
        .collect::<BTreeMap<_, _>>();
    let collections = captured
        .collections
        .iter()
        .map(|(path, field, values)| ((path.as_str(), field.as_str()), values.as_slice()))
        .collect::<BTreeMap<_, _>>();

    for step in &preview.migration_steps {
        match (step.kind, step.field.as_deref()) {
            (StateMigrationStepKind::Copy, None) => {
                let value = scalars.get(step.path.as_str()).copied().ok_or_else(|| {
                    format!("migration copy source '{}' was not captured", step.path)
                })?;
                incoming.write_global_scalar(&step.path, value)?;
            }
            (StateMigrationStepKind::Initialize, None) => {
                incoming.write_global_scalar(&step.path, default_scalar_value(&step.type_name)?)?;
            }
            (StateMigrationStepKind::Copy, Some(field)) => {
                let values = collections
                    .get(&(step.path.as_str(), field))
                    .copied()
                    .ok_or_else(|| {
                        format!(
                            "migration copy source '{}' was not captured",
                            display_collection_path(&step.path, field)
                        )
                    })?;
                if values.len() != step.elements as usize {
                    return Err(format!(
                        "migration copy source '{}' expected {} elements, captured {}",
                        display_collection_path(&step.path, field),
                        step.elements,
                        values.len()
                    ));
                }
                for (offset, value) in values.iter().copied().enumerate() {
                    incoming.write_global_collection_scalar(
                        &step.path,
                        field,
                        step.start_index as i32 + offset as i32,
                        value,
                    )?;
                }
            }
            (StateMigrationStepKind::Initialize, Some(field)) => {
                let value = default_scalar_value(&step.type_name)?;
                for index in step.start_index..step.start_index.saturating_add(step.elements) {
                    incoming.write_global_collection_scalar(
                        &step.path,
                        field,
                        index as i32,
                        value,
                    )?;
                }
            }
            (StateMigrationStepKind::Remove, _) => {}
        }
    }

    let mut shrunken = BTreeSet::new();
    for step in &preview.migration_steps {
        let (Some(from_capacity), Some(to_capacity)) = (step.from_capacity, step.to_capacity)
        else {
            continue;
        };
        if to_capacity < from_capacity {
            shrunken.insert((step.path.as_str(), to_capacity));
        }
    }
    for (path, capacity) in shrunken {
        normalize_collection_lengths(incoming, path, capacity)?;
    }
    Ok(())
}

fn normalize_collection_lengths(
    incoming: &JitProcess,
    path: &str,
    capacity: u32,
) -> Result<(), String> {
    let capacity = capacity as i32;
    if incoming.global_fixed_text_encoding(path) == Some("utf8") {
        let byte_length = read_i32_header(incoming, path, "length")?
            .or(read_i32_header(incoming, path, "byte_length")?)
            .unwrap_or(0)
            .clamp(0, capacity);
        let mut bytes = Vec::with_capacity(byte_length as usize);
        for index in 0..byte_length {
            let JitScalarValue::U8(value) =
                incoming.read_global_collection_scalar(path, "", index)?
            else {
                return Err(format!("UTF-8 collection '{path}' payload is not u8"));
            };
            bytes.push(value);
        }
        let valid_bytes = std::str::from_utf8(&bytes)
            .map(|_| bytes.len())
            .unwrap_or_else(|error| error.valid_up_to());
        let char_length = std::str::from_utf8(&bytes[..valid_bytes])
            .expect("valid UTF-8 prefix")
            .chars()
            .count() as i32;
        for index in valid_bytes..bytes.len() {
            incoming.write_global_collection_scalar(
                path,
                "",
                index as i32,
                JitScalarValue::U8(0),
            )?;
        }
        write_i32_header(incoming, path, "length", valid_bytes as i32)?;
        write_i32_header(incoming, path, "byte_length", valid_bytes as i32)?;
        write_i32_header(incoming, path, "char_length", char_length)?;
        return Ok(());
    }
    for suffix in ["length", "byte_length", "char_length"] {
        if let Some(length) = read_i32_header(incoming, path, suffix)? {
            write_i32_header(incoming, path, suffix, length.clamp(0, capacity))?;
        }
    }
    Ok(())
}

fn read_i32_header(incoming: &JitProcess, path: &str, suffix: &str) -> Result<Option<i32>, String> {
    let header = format!("{path}.{suffix}");
    if incoming.global_scalar_type(&header) != Some("i32") {
        return Ok(None);
    }
    let JitScalarValue::I32(value) = incoming.read_global_scalar(&header)? else {
        return Err(format!("collection length path '{header}' is not i32"));
    };
    Ok(Some(value))
}

fn write_i32_header(
    incoming: &JitProcess,
    path: &str,
    suffix: &str,
    value: i32,
) -> Result<(), String> {
    let header = format!("{path}.{suffix}");
    if incoming.global_scalar_type(&header) == Some("i32") {
        incoming.write_global_scalar(&header, JitScalarValue::I32(value))?;
    }
    Ok(())
}

fn default_scalar_value(type_name: &str) -> Result<JitScalarValue, String> {
    match type_name {
        "i32" => Ok(JitScalarValue::I32(0)),
        "f32" => Ok(JitScalarValue::F32(0.0)),
        "f64" => Ok(JitScalarValue::F64(0.0)),
        "bool" => Ok(JitScalarValue::Bool(false)),
        "u8" => Ok(JitScalarValue::U8(0)),
        "u16" => Ok(JitScalarValue::U16(0)),
        "u32" => Ok(JitScalarValue::U32(0)),
        _ => Err(format!(
            "state migration cannot initialize unsupported scalar type '{type_name}'"
        )),
    }
}

fn preflight_collection_growth(
    candidate: &JitProcess,
    preview: &StateMigrationPreview,
) -> Result<Vec<(String, String, u32)>, String> {
    let mut prepared = BTreeSet::new();
    let mut total_bytes = 0usize;
    for step in &preview.migration_steps {
        let (Some(field), Some(from_capacity), Some(to_capacity)) =
            (step.field.as_deref(), step.from_capacity, step.to_capacity)
        else {
            continue;
        };
        if to_capacity <= from_capacity
            || !prepared.insert((step.path.clone(), field.to_string(), to_capacity))
        {
            continue;
        }
        let value_bytes = migration_type_bytes(&step.type_name)?;
        total_bytes = total_bytes
            .checked_add(
                (to_capacity as usize)
                    .checked_mul(value_bytes)
                    .ok_or_else(|| "collection growth allocation size overflow".to_string())?,
            )
            .ok_or_else(|| "collection growth allocation size overflow".to_string())?;
        if total_bytes > MAX_STATE_SNAPSHOT_BYTES {
            return Err(format!(
                "collection growth requires {total_bytes} bytes, exceeding the {MAX_STATE_SNAPSHOT_BYTES}-byte live limit"
            ));
        }
    }
    for (path, field, capacity) in &prepared {
        candidate
            .preflight_global_collection_capacity(path, field, *capacity)
            .map_err(|error| {
                format!(
                    "failed preparing collection growth for '{}': {error}",
                    display_collection_path(path, field)
                )
            })?;
    }
    Ok(prepared.into_iter().collect())
}

fn prepare_collection_growth(
    candidate: &JitProcess,
    preview: &StateMigrationPreview,
) -> Result<(), String> {
    for (path, field, capacity) in preflight_collection_growth(candidate, preview)? {
        candidate
            .ensure_global_collection_capacity(&path, &field, capacity)
            .map_err(|error| {
                format!(
                    "failed preparing collection growth for '{}': {error}",
                    display_collection_path(&path, &field)
                )
            })?;
    }
    Ok(())
}

fn migration_type_bytes(type_name: &str) -> Result<usize, String> {
    match type_name {
        "u8" => Ok(1),
        "u16" => Ok(2),
        "u32" | "i32" | "f32" | "bool" => Ok(4),
        "f64" => Ok(8),
        _ => Err(format!(
            "unsupported collection migration type '{type_name}'"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::jit::{JitStateCollectionFieldLayout, JitStateCollectionLayout};

    #[test]
    fn non_migratable_collection_shape_rejects_before_activation() {
        let collection = |capacity| JitStateCollectionLayout {
            path: "rows".to_string(),
            capacity,
            element_shape: "{value:i32,samples:f32[2]}".to_string(),
            fully_migratable: false,
            fields: vec![JitStateCollectionFieldLayout {
                field: "value".to_string(),
                type_name: "i32".to_string(),
            }],
        };
        let active = JitStateLayout {
            scalars: Vec::new(),
            collections: vec![collection(4)],
            structs: Vec::new(),
            opaque: Vec::new(),
        };
        let incoming = JitStateLayout {
            scalars: Vec::new(),
            collections: vec![collection(8)],
            structs: Vec::new(),
            opaque: Vec::new(),
        };

        let preview = plan_state_migration(&active, &incoming, Vec::new(), false, None)
            .expect("plan deterministic rejection");

        assert!(!preview.state_layout_compatible);
        assert!(preview
            .rejection
            .as_deref()
            .is_some_and(|message| message.contains("non-migratable")));
    }
}
