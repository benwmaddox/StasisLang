use super::emit::{CollectionInfoMap, GlobalPathTypeMap};
use crate::frontend::types::{
    TypeCategory, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateLayout {
    pub scalars: Vec<StateScalarLayout>,
    pub collections: Vec<StateCollectionLayout>,
    pub opaque: Vec<StateOpaqueLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateScalarLayout {
    pub path: String,
    pub type_name: String,
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
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StateOpaqueLayout {
    pub path: String,
    pub type_name: String,
}

pub(crate) fn build_state_layout(
    global_path_types: &GlobalPathTypeMap,
    collection_infos: &CollectionInfoMap,
    type_table: &TypeTable,
) -> StateLayout {
    let scalars = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            scalar_type_name(*type_id).map(|type_name| StateScalarLayout {
                path: path.clone(),
                type_name: type_name.to_string(),
            })
        })
        .collect();
    let mut collections: Vec<StateCollectionLayout> = collection_infos
        .iter()
        .map(|(path, info)| {
            let mut fields = Vec::new();
            if let Some(type_name) = info
                .element_type
                .and_then(|type_id| collection_type_name(type_table, type_id))
            {
                fields.push(StateCollectionFieldLayout {
                    field: String::new(),
                    type_name: type_name.to_string(),
                });
            }
            fields.extend(info.field_types.iter().filter_map(|(field, type_id)| {
                collection_type_name(type_table, *type_id).map(|type_name| {
                    StateCollectionFieldLayout {
                        field: field.clone(),
                        type_name: type_name.to_string(),
                    }
                })
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
            if scalar_type_name(*type_id).is_some() || collection_paths.contains(path.as_str()) {
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
    StateLayout {
        scalars,
        collections,
        opaque,
    }
}

fn scalar_type_name(type_id: u16) -> Option<&'static str> {
    match type_id {
        TYPE_ID_I32 => Some("i32"),
        TYPE_ID_F32 => Some("f32"),
        TYPE_ID_F64 => Some("f64"),
        TYPE_ID_BOOL => Some("bool"),
        _ => None,
    }
}

fn collection_type_name(type_table: &TypeTable, type_id: u16) -> Option<&'static str> {
    if type_table
        .type_info(type_id)
        .is_some_and(|info| info.name == "u8")
    {
        Some("u8")
    } else {
        scalar_type_name(type_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::aot::AotProcess;
    use crate::backend::jit::JitProcess;

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
}
