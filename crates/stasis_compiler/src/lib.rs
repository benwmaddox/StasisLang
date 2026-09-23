#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]

pub mod backend;
pub mod compiler;
pub mod data_flow;
pub mod frontend;
pub mod host_exports;
pub mod identity;
pub mod ir;
pub mod performance;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDiagnosticCode {
    Generic,
    ExplicitGenericCall,
    Parse,
    UnresolvedExtern,
    MissingModule,
    DuplicateImportAlias,
    UnsupportedStructReturn,
    StoredViewField,
}

impl SourceDiagnosticCode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Generic => "stasis.generic",
            Self::ExplicitGenericCall => "stasis.explicitGenericCall",
            Self::Parse => "stasis.parse",
            Self::UnresolvedExtern => "stasis.unresolvedExtern",
            Self::MissingModule => "stasis.missingModule",
            Self::DuplicateImportAlias => "stasis.duplicateImportAlias",
            Self::UnsupportedStructReturn => "stasis.unsupportedStructReturn",
            Self::StoredViewField => "stasis.storedViewField",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnosticEdit {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnosticFix {
    pub title: String,
    pub edits: Vec<SourceDiagnosticEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnosticRelated {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub symbol: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnostic {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub symbol: String,
    pub message: String,
    pub code: SourceDiagnosticCode,
    pub fixes: Vec<SourceDiagnosticFix>,
    pub related: Vec<SourceDiagnosticRelated>,
}

impl SourceDiagnostic {
    pub fn new(
        path: impl Into<String>,
        start: usize,
        end: usize,
        symbol: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            start,
            end,
            symbol: symbol.into(),
            message: message.into(),
            code: SourceDiagnosticCode::Generic,
            fixes: Vec::new(),
            related: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: SourceDiagnosticCode) -> Self {
        self.code = code;
        self
    }

    pub fn with_fix(mut self, fix: SourceDiagnosticFix) -> Self {
        self.fixes.push(fix);
        self
    }

    pub fn with_related(mut self, related: SourceDiagnosticRelated) -> Self {
        self.related.push(related);
        self
    }
}
