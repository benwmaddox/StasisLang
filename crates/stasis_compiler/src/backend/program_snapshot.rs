//! Immutable semantic program description shared by every compiler consumer.
//!
//! Code pointers and object paths are deliberately kept in `artifact_mappings`:
//! they describe a target build, not Stasis program semantics or state layout.

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::emit::{
    build_compile_analysis_cache, compute_files_fingerprint,
    resolve_preferred_extern_call_signatures, CompileAnalysisCache,
};
use crate::backend::reachability::compute_reachable_function_ids;
use crate::backend::state_layout::{build_state_layout, state_layout_digest, StateLayout};
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::data_flow::FunctionDataFlowSummary;
use crate::frontend::types::{TypeId, TypeInfo, TypeTable};
use crate::frontend::{
    lexer::{lex, TokenKind},
    parser::parse_string_literal_text,
};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramArtifactMapping {
    pub function_id: FunctionId,
    pub symbol: String,
    pub target_path: Option<String>,
    pub code_pointer: Option<u64>,
}

/// Stable semantic description of a function at an accepted compilation boundary.
///
/// Compiler work flags (such as `FunctionMeta::dirty`) are deliberately not retained:
/// they describe cache history, not the program that JIT/AOT consumers agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramFunction {
    pub id: FunctionId,
    pub name: String,
    pub name_hash: u64,
    pub file_id: u32,
    pub source_range: Range<u32>,
    pub signature_range: Range<u32>,
    pub signature_hash: u64,
    pub body_hash: u64,
    pub param_names: Vec<String>,
    pub params: Vec<TypeId>,
    pub return_type: TypeId,
    pub dependencies: Vec<FunctionId>,
    pub dependents: Vec<FunctionId>,
}

impl From<&FunctionMeta> for ProgramFunction {
    fn from(function: &FunctionMeta) -> Self {
        Self {
            id: function.id,
            name: function.name.clone(),
            name_hash: function.name_hash,
            file_id: function.file_id,
            source_range: function.source_range.clone(),
            signature_range: function.signature_range.clone(),
            signature_hash: function.signature_hash,
            body_hash: function.body_hash,
            param_names: function.param_names.clone(),
            params: function.params.clone(),
            return_type: function.return_type,
            dependencies: function.dependencies.clone(),
            dependents: function.dependents.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramCollectionMetadata {
    pub path: String,
    pub capacity: i32,
    pub element_type_id: Option<u16>,
    pub field_type_ids: BTreeMap<String, u16>,
}

#[derive(Debug, Clone)]
pub struct ProgramSnapshot {
    source_revision: u64,
    files: Vec<SourceFile>,
    functions: Vec<ProgramFunction>,
    accepted_diagnostics: Vec<crate::SourceDiagnostic>,
    reachable_function_ids: BTreeSet<FunctionId>,
    state_layout: StateLayout,
    layout_digest: [u8; 32],
    data_flow_summaries: Vec<FunctionDataFlowSummary>,
    global_type_ids: BTreeMap<String, u16>,
    collections: Vec<ProgramCollectionMetadata>,
    literal_table: BTreeMap<i32, String>,
    struct_field_type_ids: BTreeMap<u16, BTreeMap<String, u16>>,
    types: TypeTable,
    artifact_mappings: BTreeMap<FunctionId, ProgramArtifactMapping>,
    // Lowering-only detail remains private to the backend. Public consumers use the
    // typed snapshot records above instead of reconstructing parser metadata.
    pub(crate) analysis: CompileAnalysisCache,
}

impl ProgramSnapshot {
    pub(crate) fn build(
        source_revision: u64,
        files: &[SourceFile],
        functions: &[FunctionMeta],
        types: &TypeTable,
        data_flow_summaries: &[FunctionDataFlowSummary],
        required_emit_roots: &[String],
        analysis: CompileAnalysisCache,
    ) -> Result<Self, String> {
        let state_layout = build_state_layout(
            &analysis.global_path_types,
            &analysis.collection_infos,
            types,
        );
        let layout_digest = state_layout_digest(&state_layout)?;
        let collections = analysis
            .collection_infos
            .iter()
            .map(|(path, info)| ProgramCollectionMetadata {
                path: path.clone(),
                capacity: info.len,
                element_type_id: info.element_type,
                field_type_ids: info.field_types.clone(),
            })
            .collect();
        let literal_table = collect_program_literals(files)?;
        Ok(Self {
            source_revision,
            files: files.to_vec(),
            functions: functions.iter().map(ProgramFunction::from).collect(),
            accepted_diagnostics: Vec::new(),
            reachable_function_ids: compute_reachable_function_ids(functions, required_emit_roots),
            state_layout,
            layout_digest,
            data_flow_summaries: data_flow_summaries.to_vec(),
            global_type_ids: analysis.global_path_types.clone(),
            collections,
            literal_table,
            struct_field_type_ids: analysis.named_struct_field_types.clone(),
            types: types.clone(),
            artifact_mappings: BTreeMap::new(),
            analysis,
        })
    }

    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }
    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }
    pub fn functions(&self) -> &[ProgramFunction] {
        &self.functions
    }
    pub fn accepted_diagnostics(&self) -> &[crate::SourceDiagnostic] {
        &self.accepted_diagnostics
    }
    pub fn reachable_function_ids(&self) -> &BTreeSet<FunctionId> {
        &self.reachable_function_ids
    }
    pub fn state_layout(&self) -> &StateLayout {
        &self.state_layout
    }
    pub fn layout_digest(&self) -> [u8; 32] {
        self.layout_digest
    }
    pub fn data_flow_summaries(&self) -> &[FunctionDataFlowSummary] {
        &self.data_flow_summaries
    }
    pub fn global_type_ids(&self) -> &BTreeMap<String, u16> {
        &self.global_type_ids
    }
    pub fn collections(&self) -> &[ProgramCollectionMetadata] {
        &self.collections
    }
    pub fn literal_table(&self) -> &BTreeMap<i32, String> {
        &self.literal_table
    }
    pub fn string_literals(&self) -> Vec<String> {
        self.literal_table.values().cloned().collect()
    }
    pub fn struct_field_type_ids(&self) -> &BTreeMap<u16, BTreeMap<String, u16>> {
        &self.struct_field_type_ids
    }
    pub fn type_info(&self, id: TypeId) -> Option<&TypeInfo> {
        self.types.type_info(id)
    }
    pub fn types(&self) -> &TypeTable {
        &self.types
    }
    pub fn artifact_mappings(&self) -> &BTreeMap<FunctionId, ProgramArtifactMapping> {
        &self.artifact_mappings
    }

    pub(crate) fn set_artifact_mappings(
        &mut self,
        mappings: impl IntoIterator<Item = ProgramArtifactMapping>,
    ) {
        self.artifact_mappings = mappings
            .into_iter()
            .map(|mapping| (mapping.function_id, mapping))
            .collect();
    }

    pub(crate) fn set_artifact_paths(&mut self, paths: &BTreeMap<FunctionId, String>) {
        for (function_id, path) in paths {
            if let Some(mapping) = self.artifact_mappings.get_mut(function_id) {
                mapping.target_path = Some(path.clone());
            }
        }
    }
}

fn collect_program_literals(files: &[SourceFile]) -> Result<BTreeMap<i32, String>, String> {
    // Literal identity is file-semantic rather than reachability-dependent: tooling and target
    // runtimes see the same immutable table, while lowering decides which entries are referenced.
    let mut literals = BTreeMap::new();
    for file in files {
        for token in lex(&file.content)? {
            if token.kind != TokenKind::StringLiteral {
                continue;
            }
            let value = parse_string_literal_text(&file.content[token.start..token.end])?;
            let id = crate::backend::emit::hash_string_literal(&value);
            if let Some(previous) = literals.insert(id, value.clone()) {
                if previous != value {
                    return Err(format!(
                        "string literal hash collision for id {id}: '{previous}' vs '{value}'"
                    ));
                }
            }
        }
    }
    Ok(literals)
}

pub fn canonical_layout_digest_for_files(
    files: impl IntoIterator<Item = (String, String)>,
) -> Result<[u8; 32], String> {
    let mut compiler = crate::compiler::Compiler::new();
    for (path, source) in files {
        compiler.upsert_file(path, source);
    }
    compiler
        .index_pass()
        .map_err(|error| format!("canonical layout index failed: {error:?}"))?;
    compiler.types_mut().ensure_utf8_view_id()?;
    compiler.types_mut().ensure_ascii_view_id()?;
    let mut types = compiler.types().clone();
    let revision = compute_files_fingerprint(compiler.files());
    let analysis = build_compile_analysis_cache(
        compiler.files(),
        compiler.functions(),
        &mut types,
        revision,
        resolve_preferred_extern_call_signatures,
    )?;
    ProgramSnapshot::build(
        revision,
        compiler.files(),
        compiler.functions(),
        &types,
        compiler.function_data_flow_summaries(),
        &[],
        analysis,
    )
    .map(|snapshot| snapshot.layout_digest())
}

#[cfg(test)]
mod tests {
    use crate::backend::aot::AotProcess;
    use crate::backend::jit::JitProcess;
    use std::fs;

    const SOURCE: &str = "global score: i32;\nfunction main(): i32 { return score; }\n";

    #[test]
    fn jit_and_aot_publish_matching_immutable_semantic_snapshots() {
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", SOURCE);
        jit.compile().expect("compile JIT fixture");
        let jit_snapshot = jit.program_snapshot().expect("JIT snapshot");

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", SOURCE);
        aot.compile().expect("compile AOT fixture");
        let aot_snapshot = aot.program_snapshot().expect("AOT snapshot");

        assert_eq!(jit_snapshot.layout_digest(), aot_snapshot.layout_digest());
        assert_eq!(jit_snapshot.functions(), aot_snapshot.functions());
        assert_eq!(
            jit_snapshot.global_type_ids(),
            aot_snapshot.global_type_ids()
        );
        assert!(jit_snapshot.artifact_mappings().contains_key(&0));
        assert!(aot_snapshot.artifact_mappings().contains_key(&0));
    }

    #[test]
    fn body_only_change_preserves_layout_digest() {
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", SOURCE);
        jit.compile().expect("compile fixture");
        let original = jit.program_snapshot().expect("snapshot").layout_digest();
        jit.upsert_file(
            "main.stasis",
            "global score: i32;\nfunction main(): i32 { return score + 0; }\n",
        );
        jit.compile().expect("compile body-only change");
        assert_eq!(
            original,
            jit.program_snapshot().expect("snapshot").layout_digest()
        );
    }

    #[test]
    fn warm_jit_narrow_edit_matches_fresh_aot_semantic_functions() {
        let original = "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\nfunction unused(): i32 { return 7; }\n";
        let edited = "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper(); }\nfunction unused(): i32 { return 7; }\n";
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", original);
        jit.compile().expect("warm JIT baseline");
        jit.upsert_file("main.stasis", edited);
        jit.compile().expect("warm JIT narrow edit");

        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", edited);
        aot.compile().expect("fresh AOT compile");
        let jit_snapshot = jit.program_snapshot().expect("JIT snapshot");
        let aot_snapshot = aot.program_snapshot().expect("AOT snapshot");
        assert_eq!(jit_snapshot.functions(), aot_snapshot.functions());
        assert_eq!(jit_snapshot.layout_digest(), aot_snapshot.layout_digest());
        assert_eq!(
            jit_snapshot.accepted_diagnostics(),
            aot_snapshot.accepted_diagnostics()
        );
        assert!(jit_snapshot.accepted_diagnostics().is_empty());
    }

    #[test]
    fn comments_whitespace_and_signatures_have_deterministic_snapshot_identity() {
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", SOURCE);
        jit.compile().expect("compile fixture");
        let original = jit.program_snapshot().expect("snapshot").clone();

        jit.upsert_file(
            "main.stasis",
            "\n// formatting only\nglobal score: i32;\nfunction main(): i32 { return score; }\n",
        );
        jit.compile().expect("compile formatting-only change");
        let formatted = jit.program_snapshot().expect("snapshot");
        assert_eq!(original.layout_digest(), formatted.layout_digest());
        assert_eq!(
            original.functions()[0].signature_hash,
            formatted.functions()[0].signature_hash
        );

        jit.upsert_file(
            "main.stasis",
            "global score: i32;\nfunction main(value: i32): i32 { return value + score; }\n",
        );
        jit.compile().expect("compile signature change");
        assert_ne!(
            original.functions()[0].signature_hash,
            jit.program_snapshot().expect("snapshot").functions()[0].signature_hash
        );
    }

    #[test]
    fn state_layout_change_updates_digest_without_artifact_identity() {
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", SOURCE);
        jit.compile().expect("compile fixture");
        let original = jit.program_snapshot().expect("snapshot").layout_digest();
        jit.upsert_file(
            "main.stasis",
            "global score: i32; global lives: i32; function main(): i32 { return score; }\n",
        );
        jit.compile().expect("compile layout change");
        assert_ne!(
            original,
            jit.program_snapshot().expect("snapshot").layout_digest()
        );
    }

    #[test]
    fn multi_file_jit_and_aot_snapshots_match_semantic_metadata() {
        let main = "global score: i32; global values: i32[2]; function main(): i32 { return helper() + score; }";
        let helper = "function helper(): i32 { return values[0]; }";
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", main);
        jit.upsert_file("helper.stasis", helper);
        jit.compile().expect("compile multi-file JIT");
        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", main);
        aot.upsert_file("helper.stasis", helper);
        aot.compile().expect("compile multi-file AOT");
        let jit = jit.program_snapshot().expect("JIT snapshot");
        let aot = aot.program_snapshot().expect("AOT snapshot");
        assert_eq!(
            jit.files()
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>(),
            aot.files()
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
        assert_eq!(jit.functions(), aot.functions());
        assert_eq!(jit.reachable_function_ids(), aot.reachable_function_ids());
        assert_eq!(jit.state_layout(), aot.state_layout());
        assert_eq!(jit.layout_digest(), aot.layout_digest());
        assert_eq!(jit.global_type_ids(), aot.global_type_ids());
        assert_eq!(jit.collections(), aot.collections());
        assert_eq!(jit.struct_field_type_ids(), aot.struct_field_type_ids());
        assert_eq!(jit.string_literals(), aot.string_literals());
    }

    #[test]
    fn artifact_mappings_do_not_change_semantic_identity_and_aot_paths_are_materialized() {
        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", SOURCE);
        aot.compile().expect("compile AOT");
        let before = aot.program_snapshot().expect("snapshot").clone();
        let output =
            std::env::temp_dir().join(format!("stasis-snapshot-artifacts-{}", std::process::id()));
        let objects = aot.write_object_files(&output).expect("write objects");
        let after = aot.program_snapshot().expect("snapshot");
        assert_eq!(before.layout_digest(), after.layout_digest());
        assert_eq!(before.functions(), after.functions());
        for mapping in after.artifact_mappings().values() {
            assert!(after
                .functions()
                .iter()
                .any(|function| function.id == mapping.function_id));
            let path = mapping.target_path.as_ref().expect("materialized AOT path");
            assert!(std::path::Path::new(path).is_file());
        }
        assert_eq!(objects.len(), after.artifact_mappings().len());
        let _ = fs::remove_dir_all(output);
    }

    #[test]
    fn literals_are_stable_on_warm_aot_compile_and_match_unreachable_jit_source() {
        let source = "function main(): i32 { print_string(\"live\"); return 0; } function unused(): i32 { print_string(\"unreachable\"); return 0; }";
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source);
        jit.compile().expect("compile JIT");
        let mut aot = AotProcess::new();
        aot.upsert_file("main.stasis", source);
        aot.compile().expect("compile AOT");
        let first = aot
            .program_snapshot()
            .expect("snapshot")
            .literal_table()
            .clone();
        aot.compile().expect("warm AOT compile");
        assert_eq!(
            first,
            *aot.program_snapshot().expect("snapshot").literal_table()
        );
        assert_eq!(
            jit.program_snapshot().expect("snapshot").literal_table(),
            aot.program_snapshot().expect("snapshot").literal_table()
        );
        assert!(first.values().any(|value| value == "unreachable"));
    }
}
