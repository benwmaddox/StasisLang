use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct FnId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct CodeGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayoutHash(pub [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextSource {
    FileWatcher,
    EditorBuffer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChangeEvent {
    pub contract_version: u16,
    pub path: PathBuf,
    pub revision: u64,
    pub text_source: TextSource,
    pub change_kind: FileChangeKind,
}

impl FileChangeEvent {
    pub fn new(
        path: PathBuf,
        revision: u64,
        text_source: TextSource,
        change_kind: FileChangeKind,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            path,
            revision,
            text_source,
            change_kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetMode {
    JitDev,
    AotProd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileRequest {
    pub contract_version: u16,
    pub request_id: RequestId,
    pub changed_files: Vec<PathBuf>,
    pub target_mode: TargetMode,
}

impl CompileRequest {
    pub fn new(
        request_id: RequestId,
        changed_files: Vec<PathBuf>,
        target_mode: TargetMode,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            request_id,
            changed_files,
            target_mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub path: Option<PathBuf>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionPatch {
    pub fn_id: FnId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionPatchSet {
    pub functions: Vec<FunctionPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompileStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileResult {
    pub contract_version: u16,
    pub request_id: RequestId,
    pub status: CompileStatus,
    pub diagnostics: Vec<Diagnostic>,
    pub layout_hash: Option<LayoutHash>,
    pub fn_patch_set: Option<FunctionPatchSet>,
    pub hook_symbol: Option<String>,
}

impl CompileResult {
    pub fn success(
        request_id: RequestId,
        layout_hash: LayoutHash,
        fn_patch_set: FunctionPatchSet,
    ) -> Self {
        Self::success_with_hook_symbol(request_id, layout_hash, fn_patch_set, None)
    }

    pub fn success_with_hook_symbol(
        request_id: RequestId,
        layout_hash: LayoutHash,
        fn_patch_set: FunctionPatchSet,
        hook_symbol: Option<String>,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            request_id,
            status: CompileStatus::Success,
            diagnostics: Vec::new(),
            layout_hash: Some(layout_hash),
            fn_patch_set: Some(fn_patch_set),
            hook_symbol,
        }
    }

    pub fn failed(request_id: RequestId, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            request_id,
            status: CompileStatus::Failed,
            diagnostics,
            layout_hash: None,
            fn_patch_set: None,
            hook_symbol: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapCommitRequest {
    pub contract_version: u16,
    pub request_id: RequestId,
    pub layout_hash: LayoutHash,
    pub fn_patch_set: FunctionPatchSet,
    pub hook_symbol: Option<String>,
}

impl SwapCommitRequest {
    pub fn new(
        request_id: RequestId,
        layout_hash: LayoutHash,
        fn_patch_set: FunctionPatchSet,
        hook_symbol: Option<String>,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            request_id,
            layout_hash,
            fn_patch_set,
            hook_symbol,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapCommitStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapCommitResult {
    pub contract_version: u16,
    pub request_id: RequestId,
    pub status: SwapCommitStatus,
    pub swapped_fn_ids: Vec<FnId>,
    pub new_generation: Option<CodeGeneration>,
    pub error: Option<String>,
}

impl SwapCommitResult {
    pub fn success(
        request_id: RequestId,
        swapped_fn_ids: Vec<FnId>,
        new_generation: CodeGeneration,
    ) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            request_id,
            status: SwapCommitStatus::Success,
            swapped_fn_ids,
            new_generation: Some(new_generation),
            error: None,
        }
    }

    pub fn failed(request_id: RequestId, error: impl Into<String>) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            request_id,
            status: SwapCommitStatus::Failed,
            swapped_fn_ids: Vec::new(),
            new_generation: None,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_success_contract_shape_matches_s9_names() {
        let request_id = RequestId(7);
        let layout_hash = LayoutHash([3; 32]);
        let patches = FunctionPatchSet {
            functions: vec![
                FunctionPatch { fn_id: FnId(10) },
                FunctionPatch { fn_id: FnId(11) },
            ],
        };

        let result = CompileResult::success(request_id, layout_hash, patches.clone());
        assert_eq!(result.contract_version, CONTRACT_VERSION);
        assert_eq!(result.request_id, request_id);
        assert_eq!(result.status, CompileStatus::Success);
        assert!(result.diagnostics.is_empty());
        assert_eq!(result.layout_hash, Some(layout_hash));
        assert_eq!(result.fn_patch_set, Some(patches));
        assert_eq!(result.hook_symbol, None);
    }

    #[test]
    fn compile_success_can_include_hook_symbol() {
        let result = CompileResult::success_with_hook_symbol(
            RequestId(8),
            LayoutHash([5; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(12) }],
            },
            Some("on_code_swap".to_string()),
        );
        assert_eq!(result.status, CompileStatus::Success);
        assert_eq!(result.hook_symbol.as_deref(), Some("on_code_swap"));
    }

    #[test]
    fn compile_failure_has_diagnostics_and_no_patch_payload() {
        let request_id = RequestId(9);
        let diagnostic = Diagnostic {
            severity: DiagnosticSeverity::Error,
            message: "unexpected token".to_string(),
            path: Some(PathBuf::from("samples/game.stasis")),
            line: Some(12),
            column: Some(5),
        };

        let result = CompileResult::failed(request_id, vec![diagnostic.clone()]);
        assert_eq!(result.contract_version, CONTRACT_VERSION);
        assert_eq!(result.status, CompileStatus::Failed);
        assert_eq!(result.diagnostics, vec![diagnostic]);
        assert!(result.layout_hash.is_none());
        assert!(result.fn_patch_set.is_none());
        assert!(result.hook_symbol.is_none());
    }

    #[test]
    fn commit_success_contains_swapped_ids_and_generation() {
        let result =
            SwapCommitResult::success(RequestId(42), vec![FnId(1), FnId(2)], CodeGeneration(4));
        assert_eq!(result.contract_version, CONTRACT_VERSION);
        assert_eq!(result.status, SwapCommitStatus::Success);
        assert_eq!(result.swapped_fn_ids, vec![FnId(1), FnId(2)]);
        assert_eq!(result.new_generation, Some(CodeGeneration(4)));
        assert!(result.error.is_none());
    }

    #[test]
    fn commit_failure_contains_error_and_no_generation() {
        let result = SwapCommitResult::failed(RequestId(99), "on_code_swap failed");
        assert_eq!(result.contract_version, CONTRACT_VERSION);
        assert_eq!(result.status, SwapCommitStatus::Failed);
        assert!(result.swapped_fn_ids.is_empty());
        assert!(result.new_generation.is_none());
        assert_eq!(result.error.as_deref(), Some("on_code_swap failed"));
    }
}
