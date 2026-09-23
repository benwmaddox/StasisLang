//! Versioned guest exports. Boolean values use the existing 32-bit integer ABI.
use crate::frontend::parser::{ParsedFunctionAnnotationArgumentKind, ParsedFunctionSignature};
use serde::{Deserialize, Serialize};

pub const HOST_EXPORT_ABI_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExport {
    pub name: String,
    pub parameters: Vec<String>,
    pub return_type: String,
}

impl HostExport {
    pub fn symbol(&self) -> String {
        format!("stasis_host_v1_{}", self.name)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || !self.name.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            })
            || self.name.starts_with("stasis_")
            || matches!(
                self.name.as_str(),
                "main" | "tick" | "render" | "on_code_swap"
            )
        {
            return Err("@host_export requires a non-reserved ASCII identifier".to_string());
        }
        if self.parameters.len() > 3
            || self
                .parameters
                .iter()
                .any(|ty| !matches!(ty.as_str(), "i32" | "bool"))
            || !matches!(self.return_type.as_str(), "void" | "i32")
        {
            return Err(
                "host export ABI v1 supports at most three i32/bool parameters and void/i32 return"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn c_declaration(&self) -> String {
        format!(
            "{} {}({});",
            self.c_return_type(),
            self.symbol(),
            self.c_parameters()
        )
    }
    fn c_return_type(&self) -> &str {
        if self.return_type == "void" {
            "void"
        } else {
            "int32_t"
        }
    }
    fn c_parameters(&self) -> String {
        if self.parameters.is_empty() {
            "void".to_string()
        } else {
            (0..self.parameters.len())
                .map(|i| format!("int32_t arg{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
    pub fn c_wrapper(&self, target: &str) -> String {
        let args = (0..self.parameters.len())
            .map(|i| format!("arg{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let prefix = if self.return_type == "void" {
            ""
        } else {
            "return "
        };
        format!(
            "extern {} {target}({});\nSTASIS_EXPORT {} {}({}) {{ {prefix}{target}({args}); }}\n",
            self.c_return_type(),
            self.c_parameters(),
            self.c_return_type(),
            self.symbol(),
            self.c_parameters()
        )
    }
}

pub(crate) fn parse(function: &ParsedFunctionSignature) -> Result<Option<HostExport>, String> {
    let mut annotations = function
        .annotations
        .iter()
        .filter(|annotation| annotation.name == "host_export");
    let Some(annotation) = annotations.next() else {
        return Ok(None);
    };
    if annotations.next().is_some()
        || !annotation.has_parentheses
        || annotation.arguments.len() != 1
        || annotation.arguments[0].kind != ParsedFunctionAnnotationArgumentKind::Identifier
    {
        return Err("declare @host_export(name) exactly once with one identifier".to_string());
    }
    if matches!(
        function.name.as_str(),
        "main"
            | "tick"
            | "render"
            | "on_code_swap"
            | "gfx_cmd_construction_reset"
            | "gfx_cmd_construction_finish"
    ) {
        return Err("@host_export cannot replace a fixed lifecycle entry".to_string());
    }
    if !function.generic_parameters.is_empty()
        || function
            .annotations
            .iter()
            .any(|a| matches!(a.name.as_str(), "requires" | "extern"))
    {
        return Err(
            "@host_export requires a concrete guest function without caller preconditions"
                .to_string(),
        );
    }
    let export = HostExport {
        name: annotation.arguments[0].text.clone(),
        parameters: function
            .params
            .iter()
            .map(|param| param.type_name.clone())
            .collect(),
        return_type: function.return_type_name.clone(),
    };
    export.validate()?;
    Ok(Some(export))
}

pub(crate) fn symbol(function: &crate::compiler::FunctionMeta) -> String {
    function
        .host_export
        .as_ref()
        .map_or_else(|| function.name.clone(), HostExport::symbol)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExportRecord {
    pub signature: HostExport,
    pub symbol: String,
    pub target_symbol: String,
    pub source_symbol_id: String,
    pub source_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostExports {
    pub abi_version: u32,
    pub functions: Vec<HostExportRecord>,
}

impl HostExports {
    pub fn from_compiler(compiler: &crate::compiler::Compiler) -> Self {
        let mut functions: Vec<_> = compiler
            .functions()
            .iter()
            .filter_map(|function| {
                let signature = function.host_export.clone()?;
                Some(HostExportRecord {
                    symbol: signature.symbol(),
                    signature,
                    target_symbol: format!("aot_fn_{}", function.id),
                    source_symbol_id: function.symbol_id.to_string(),
                    source_path: compiler.files()[function.file_id as usize].path.clone(),
                })
            })
            .collect();
        functions.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        Self {
            abi_version: HOST_EXPORT_ABI_VERSION,
            functions,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.abi_version != HOST_EXPORT_ABI_VERSION {
            return Err(format!(
                "unsupported host export ABI version {}",
                self.abi_version
            ));
        }
        let mut symbols = std::collections::BTreeSet::new();
        for record in &self.functions {
            record.signature.validate()?;
            if record.symbol != record.signature.symbol() || !symbols.insert(&record.symbol) {
                return Err("invalid or duplicate host export symbol".to_string());
            }
            if !record
                .target_symbol
                .strip_prefix("aot_fn_")
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err("invalid host export target symbol".to_string());
            }
        }
        Ok(())
    }

    pub fn from_manifest(manifest: &serde_json::Value) -> Result<Self, String> {
        let Some(value) = manifest.get("host_exports") else {
            return Ok(Self {
                abi_version: HOST_EXPORT_ABI_VERSION,
                functions: Vec::new(),
            });
        };
        let exports: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid host exports: {error}"))?;
        exports.validate()?;
        let functions = manifest["functions"]
            .as_array()
            .ok_or("host exports require manifest functions")?;
        for record in &exports.functions {
            let matches: Vec<_> = functions
                .iter()
                .filter(|row| row["symbol"].as_str() == Some(&record.target_symbol))
                .collect();
            if matches.len() != 1
                || matches[0]["symbol_id"].as_str() != Some(&record.source_symbol_id)
                || matches[0]["parameter_count"].as_u64()
                    != Some(record.signature.parameters.len() as u64)
                || matches[0]["return_type"].as_u64()
                    != Some(u64::from(record.signature.return_type == "i32"))
            {
                return Err(format!(
                    "host export '{}' does not match its manifest function",
                    record.signature.name
                ));
            }
        }
        Ok(exports)
    }

    pub fn header(&self) -> Result<String, String> {
        self.validate()?;
        let mut out = String::from("#ifndef STASIS_HOST_EXPORTS_H\n#define STASIS_HOST_EXPORTS_H\n#include <stdint.h>\n#define STASIS_HOST_EXPORT_ABI_VERSION 1\n#ifdef __cplusplus\nextern \"C\" {\n#endif\n/* bool arguments use int32_t: exactly 0 or 1. Call after main, between ticks. */\n");
        for record in &self.functions {
            out.push_str(&record.signature.c_declaration());
            out.push('\n');
        }
        out.push_str("#ifdef __cplusplus\n}\n#endif\n#endif\n");
        Ok(out)
    }

    pub fn c_wrappers(&self) -> Result<String, String> {
        self.validate()?;
        Ok(self
            .functions
            .iter()
            .map(|record| record.signature.c_wrapper(&record.target_symbol))
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostValue {
    I32(i32),
    Bool(bool),
}
