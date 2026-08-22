//! Immutable semantic program description shared by every compiler consumer.
//!
//! Code pointers and object paths are deliberately kept in `artifact_mappings`:
//! they describe a target build, not Stasis program semantics or state layout.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::backend::assets::{discover_asset_references, AssetReference};
use crate::backend::emit::{
    build_compile_analysis_cache, compute_files_fingerprint,
    resolve_preferred_extern_call_signatures, CompileAnalysisCache,
};
use crate::backend::reachability::compute_reachable_function_ids;
use crate::backend::state_layout::{build_state_layout, state_layout_digest, StateLayout};
use crate::compiler::{FunctionId, FunctionMeta, SourceFile};
use crate::data_flow::FunctionDataFlowSummary;
use crate::frontend::module_graph::ModuleGraph;
use crate::frontend::types::{TypeId, TypeInfo, TypeTable};
use crate::frontend::{
    lexer::{lex, TokenKind},
    parser::parse_string_literal_text,
};
use crate::identity::SymbolId;
use sha2::{Digest, Sha256};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramArtifactMapping {
    pub function_id: FunctionId,
    pub symbol_id: SymbolId,
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
    pub symbol_id: SymbolId,
    pub name: String,
    pub module_alias: String,
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
            symbol_id: function.symbol_id.clone(),
            name: function.name.clone(),
            module_alias: function.module_alias.clone(),
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
    module_graph: ModuleGraph,
    functions: Vec<ProgramFunction>,
    reachable_function_ids: BTreeSet<FunctionId>,
    asset_references: Vec<AssetReference>,
    state_layout: StateLayout,
    layout_digest: [u8; 32],
    compiler_layout_digest: [u8; 32],
    data_flow_summaries: Arc<[FunctionDataFlowSummary]>,
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

fn compiler_layout_digest(
    analysis: &CompileAnalysisCache,
    functions: &[FunctionMeta],
    types: &TypeTable,
) -> [u8; 32] {
    let mut facts = Vec::new();
    let mut pending_type_ids = Vec::new();
    for (type_id, fields) in &analysis.named_struct_field_types {
        facts.push(format!("struct:{type_id}:{fields:?}"));
        pending_type_ids.push(*type_id);
        pending_type_ids.extend(fields.values().copied());
    }
    for (path, type_id) in &analysis.global_path_types {
        facts.push(format!("global:{path}:{type_id}"));
        pending_type_ids.push(*type_id);
    }
    for (path, info) in &analysis.collection_infos {
        facts.push(format!("collection:{path}:{info:?}"));
        pending_type_ids.extend(info.element_type);
        pending_type_ids.extend(info.field_types.values().copied());
    }
    for function in functions {
        pending_type_ids.extend(function.params.iter().copied());
        pending_type_ids.push(function.return_type);
    }
    for signature in &analysis.resolved_extern_signatures {
        pending_type_ids.extend(signature.params.iter().copied());
        pending_type_ids.push(signature.return_type);
    }
    let mut seen_type_ids = BTreeSet::new();
    while let Some(type_id) = pending_type_ids.pop() {
        if !seen_type_ids.insert(type_id) {
            continue;
        }
        if let Some(info) = types.type_info(type_id) {
            facts.push(format!("type:{type_id}:{info:?}"));
        }
        if let Some(element_type) = types.indexed_element_type_id(type_id) {
            pending_type_ids.push(element_type);
        }
    }
    facts.sort();
    let digest = Sha256::digest(facts.join("\n").as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    bytes
}

impl ProgramSnapshot {
    pub(crate) fn build(
        source_revision: u64,
        files: &[SourceFile],
        module_graph: &ModuleGraph,
        functions: &[FunctionMeta],
        types: &TypeTable,
        data_flow_summaries: Arc<[FunctionDataFlowSummary]>,
        required_emit_roots: &[String],
        analysis: CompileAnalysisCache,
    ) -> Result<Self, String> {
        let state_layout = build_state_layout(
            &analysis.global_path_types,
            &analysis.collection_infos,
            types,
        );
        let layout_digest = state_layout_digest(&state_layout)?;
        let compiler_layout_digest = compiler_layout_digest(&analysis, functions, types);
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
        let reachable_function_ids = compute_reachable_function_ids(functions, required_emit_roots);
        let asset_references = discover_asset_references(
            files,
            functions,
            &reachable_function_ids,
            &analysis.constant_values,
        )?;
        Ok(Self {
            source_revision,
            files: files.to_vec(),
            module_graph: module_graph.clone(),
            functions: functions.iter().map(ProgramFunction::from).collect(),
            reachable_function_ids,
            asset_references,
            state_layout,
            layout_digest,
            compiler_layout_digest,
            data_flow_summaries,
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
    pub fn module_graph(&self) -> &ModuleGraph {
        &self.module_graph
    }
    pub fn functions(&self) -> &[ProgramFunction] {
        &self.functions
    }
    pub fn function_by_id(&self, id: FunctionId) -> Option<&ProgramFunction> {
        self.functions.iter().find(|function| function.id == id)
    }
    pub fn function_by_symbol_id(&self, symbol_id: &SymbolId) -> Option<&ProgramFunction> {
        self.functions
            .iter()
            .find(|function| &function.symbol_id == symbol_id)
    }
    pub fn reachable_function_ids(&self) -> &BTreeSet<FunctionId> {
        &self.reachable_function_ids
    }
    pub fn asset_references(&self) -> &[AssetReference] {
        &self.asset_references
    }
    pub fn state_layout(&self) -> &StateLayout {
        &self.state_layout
    }
    pub fn layout_digest(&self) -> [u8; 32] {
        self.layout_digest
    }
    /// Digest of compiler-visible storage and type layouts that may be embedded in machine code.
    /// This is intentionally distinct from `layout_digest`, which versions persistent state and
    /// governs migration compatibility.
    pub(crate) fn compiler_layout_digest(&self) -> [u8; 32] {
        self.compiler_layout_digest
    }
    pub fn data_flow_summaries(&self) -> &[FunctionDataFlowSummary] {
        &self.data_flow_summaries
    }
    #[cfg(test)]
    pub(crate) fn data_flow_summaries_shared(&self) -> Arc<[FunctionDataFlowSummary]> {
        Arc::clone(&self.data_flow_summaries)
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
    ) -> Result<(), String> {
        let mut validated = BTreeMap::new();
        for mapping in mappings {
            if mapping.symbol_id.fn_id() != mapping.function_id {
                return Err(format!(
                    "artifact identity invariant failed: '{}' derives {:08x}, mapping carries {:08x}",
                    mapping.symbol_id,
                    mapping.symbol_id.fn_id(),
                    mapping.function_id
                ));
            }
            validated.insert(mapping.function_id, mapping);
        }
        self.artifact_mappings = validated;
        Ok(())
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
        // Most code-only hot edits have no quoted literals. Avoid a second full lexer pass
        // in that common path while retaining all-source (including unreachable) coverage.
        if !file.content.as_bytes().contains(&b'"') {
            continue;
        }
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
    canonical_layout_digest_with_root(None, files)
}

pub fn canonical_layout_digest_for_project(
    project_root: impl Into<String>,
    files: impl IntoIterator<Item = (String, String)>,
) -> Result<[u8; 32], String> {
    canonical_layout_digest_with_root(Some(project_root.into()), files)
}

fn canonical_layout_digest_with_root(
    project_root: Option<String>,
    files: impl IntoIterator<Item = (String, String)>,
) -> Result<[u8; 32], String> {
    let mut compiler = crate::compiler::Compiler::new();
    if let Some(project_root) = project_root {
        compiler.set_project_root(project_root)?;
    }
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
        compiler.module_graph(),
        compiler.functions(),
        &types,
        compiler.data_flow_summaries_shared(),
        &[],
        analysis,
    )
    .map(|snapshot| snapshot.layout_digest())
}

/// Canonical state-layout identity for legacy/tooling callers.  Unlike a full
/// ProgramSnapshot this intentionally does not resolve call signatures or
/// extern symbols: function bodies cannot affect state layout.
pub fn canonical_state_layout_digest_for_files(
    files: impl IntoIterator<Item = (String, String)>,
) -> Result<[u8; 32], String> {
    let files = files
        .into_iter()
        .map(|(path, content)| crate::compiler::SourceFile {
            hash: crate::frontend::indexer::hash_text(&content),
            path,
            content,
            functions: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut types = TypeTable::new();
    types.ensure_utf8_view_id()?;
    types.ensure_ascii_view_id()?;
    let constants = crate::backend::emit::collect_top_level_constant_values(&files, &mut types)?;
    let globals = crate::backend::emit::collect_global_path_types(&files, &mut types, &constants)?;
    let collections =
        crate::backend::emit::collect_foreach_collection_infos(&files, &mut types, &constants)?;
    let layout = build_state_layout(&globals, &collections, &types);
    state_layout_digest(&layout)
}

pub fn semantic_revision_with_required_roots(
    files_fingerprint: u64,
    required_roots: &[String],
) -> u64 {
    let mut roots = required_roots.to_vec();
    roots.sort();
    roots.dedup();
    let mut revision = files_fingerprint ^ 0x5354_4153_4953_524f;
    revision = revision.wrapping_mul(1_099_511_628_211) ^ roots.len() as u64;
    for root in roots {
        revision = revision.wrapping_mul(1_099_511_628_211);
        revision ^= root.len() as u64;
        for byte in root.bytes() {
            revision ^= u64::from(byte);
            revision = revision.wrapping_mul(1_099_511_628_211);
        }
    }
    revision
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let main_id = jit_snapshot.functions()[0].id;
        assert!(jit_snapshot.artifact_mappings().contains_key(&main_id));
        assert!(aot_snapshot.artifact_mappings().contains_key(&main_id));
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
    fn non_state_struct_change_only_changes_compiler_layout_digest() {
        fn source(extra_field: bool) -> String {
            let fields = if extra_field {
                "value: i32; extra: f32;"
            } else {
                "value: i32;"
            };
            format!(
                "struct LocalOnly {{ {fields} }}\nglobal score: i32;\nfunction main(): i32 {{ return score; }}\n"
            )
        }

        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", source(false));
        jit.compile().expect("compile original local layout");
        let original_state = jit.program_snapshot().expect("snapshot").layout_digest();
        let original_compiler = jit
            .program_snapshot()
            .expect("snapshot")
            .compiler_layout_digest();

        jit.upsert_file("main.stasis", source(true));
        jit.compile().expect("compile changed local layout");
        let changed = jit.program_snapshot().expect("snapshot");
        assert_eq!(original_state, changed.layout_digest());
        assert_ne!(original_compiler, changed.compiler_layout_digest());
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
    }

    #[test]
    fn required_roots_refresh_jit_and_aot_snapshots_without_source_edits() {
        let source = "function main(): i32 { return 1; }\nfunction extra(): i32 { return 2; }\n";
        let roots = vec!["extra".to_string()];
        let mut jit = JitProcess::new();
        let mut aot = AotProcess::new();
        jit.upsert_file("main.stasis", source);
        aot.upsert_file("main.stasis", source);
        jit.compile().expect("baseline JIT");
        aot.compile().expect("baseline AOT");
        let baseline_jit_revision = jit
            .program_snapshot()
            .expect("JIT snapshot")
            .source_revision();
        let baseline_aot_revision = aot
            .program_snapshot()
            .expect("AOT snapshot")
            .source_revision();
        jit.set_required_emit_roots(&roots);
        aot.set_required_emit_roots(&roots);
        jit.compile().expect("rooted JIT");
        aot.compile().expect("rooted AOT");
        for snapshot in [
            jit.program_snapshot().expect("JIT snapshot"),
            aot.program_snapshot().expect("AOT snapshot"),
        ] {
            let extra_id = snapshot
                .functions()
                .iter()
                .find(|function| function.name == "extra")
                .expect("extra function")
                .id;
            assert!(snapshot
                .functions()
                .iter()
                .any(|function| function.name == "extra"));
            assert!(snapshot.reachable_function_ids().contains(&extra_id));
            assert!(snapshot.artifact_mappings().contains_key(&extra_id));
        }
        assert_ne!(
            jit.program_snapshot()
                .expect("JIT snapshot")
                .source_revision(),
            baseline_jit_revision
        );
        assert_ne!(
            aot.program_snapshot()
                .expect("AOT snapshot")
                .source_revision(),
            baseline_aot_revision
        );
        jit.set_required_emit_roots(&[]);
        aot.set_required_emit_roots(&[]);
        jit.compile().expect("unrooted JIT");
        aot.compile().expect("unrooted AOT");
        for snapshot in [
            jit.program_snapshot().expect("JIT snapshot"),
            aot.program_snapshot().expect("AOT snapshot"),
        ] {
            let extra_id = snapshot
                .functions()
                .iter()
                .find(|function| function.name == "extra")
                .expect("extra function")
                .id;
            assert!(!snapshot.artifact_mappings().contains_key(&extra_id));
        }
    }

    #[test]
    fn required_root_revision_is_order_independent_and_boundary_safe() {
        let files = 42;
        assert_eq!(
            semantic_revision_with_required_roots(files, &["b".into(), "a".into(), "a".into()]),
            semantic_revision_with_required_roots(files, &["a".into(), "b".into()])
        );
        assert_ne!(
            semantic_revision_with_required_roots(files, &["ab".into()]),
            semantic_revision_with_required_roots(files, &["a".into(), "b".into()])
        );
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
        let main = "import \"helper.stasis\"; global score: i32; global values: i32[2]; function main(): i32 { return helper() + score; }";
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
    fn artifact_mapping_rejects_symbol_id_mismatch_without_partial_publication() {
        let mut jit = JitProcess::new();
        jit.upsert_file("main.stasis", SOURCE);
        jit.compile().expect("compile JIT fixture");
        let mut snapshot = jit.program_snapshot().expect("snapshot").clone();
        let accepted = snapshot.artifact_mappings().clone();
        let mut mismatched = accepted.values().next().expect("main mapping").clone();
        mismatched.symbol_id = SymbolId::function(
            &crate::identity::CanonicalSourcePath::project_relative("other.stasis")
                .expect("canonical fixture path"),
            "main",
            "()",
        );

        let error = snapshot
            .set_artifact_mappings([mismatched])
            .expect_err("mismatched identity must fail");

        assert!(error.contains("artifact identity invariant failed"));
        assert_eq!(snapshot.artifact_mappings(), &accepted);
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
