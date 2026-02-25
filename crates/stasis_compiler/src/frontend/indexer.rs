use std::collections::BTreeSet;
use std::ops::Range;

use crate::frontend::lexer::{lex, TokenKind};
use crate::frontend::parser::parse_top_level_functions;
use crate::frontend::types::{TypeId, TypeTable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFunction {
    pub name: String,
    pub name_hash: u64,
    pub source_range: Range<u32>,
    pub signature_hash: u64,
    pub body_hash: u64,
    pub param_names: Vec<String>,
    pub params: Vec<TypeId>,
    pub return_type: TypeId,
    pub dependency_name_hashes: Vec<u64>,
}

pub fn index_file(source: &str, types: &mut TypeTable) -> Result<Vec<IndexedFunction>, String> {
    let parsed = parse_top_level_functions(source)?;
    let mut out = Vec::with_capacity(parsed.len());
    for function in parsed {
        let mut params = Vec::with_capacity(function.params.len());
        let mut param_names = Vec::with_capacity(function.params.len());
        for param in &function.params {
            let type_id = types.resolve_or_intern(&param.type_name)?;
            param_names.push(param.name.clone());
            params.push(type_id);
        }
        let return_type = types.resolve_or_intern(&function.return_type_name)?;
        let name_hash = hash_text(&function.name);
        let signature_hash = hash_signature(name_hash, &params, return_type);
        let body_text = source
            .get(function.body_range.clone())
            .ok_or_else(|| "invalid function body range".to_string())?;
        let body_hash = hash_text(body_text);
        let dependency_name_hashes = collect_dependency_hashes(body_text)?;
        out.push(IndexedFunction {
            name: function.name,
            name_hash,
            source_range: function.body_range.start as u32..function.body_range.end as u32,
            signature_hash,
            body_hash,
            param_names,
            params,
            return_type,
            dependency_name_hashes,
        });
    }
    Ok(out)
}

pub fn hash_text(text: &str) -> u64 {
    let mut hash: u64 = 1469598103934665603;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn hash_signature(name_hash: u64, params: &[TypeId], return_type: TypeId) -> u64 {
    let mut hash = name_hash;
    hash = hash
        .wrapping_mul(1099511628211)
        .wrapping_add(u64::from(return_type));
    for param in params {
        hash = hash
            .wrapping_mul(1099511628211)
            .wrapping_add(u64::from(*param));
    }
    hash
}

fn collect_dependency_hashes(body_text: &str) -> Result<Vec<u64>, String> {
    let tokens = lex(body_text)?;
    let mut hashes = BTreeSet::new();
    for window in tokens.windows(2) {
        let lhs = window[0];
        let rhs = window[1];
        if lhs.kind != TokenKind::Identifier || rhs.kind != TokenKind::LParen {
            continue;
        }
        let identifier = &body_text[lhs.start..lhs.end];
        if is_call_keyword(identifier) {
            continue;
        }
        hashes.insert(hash_text(identifier));
    }
    Ok(hashes.into_iter().collect())
}

fn is_call_keyword(identifier: &str) -> bool {
    matches!(identifier, "if" | "for" | "foreach" | "return" | "function")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_basic_function_metadata() {
        let mut types = TypeTable::new();
        let source = "function main(value: i32): i32 { return value; }\n";
        let indexed = index_file(source, &mut types).expect("index");
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].name, "main");
        assert_eq!(indexed[0].param_names, vec!["value".to_string()]);
        assert_eq!(indexed[0].params.len(), 1);
        assert_eq!(indexed[0].return_type, types.resolve("i32").unwrap_or_default());
    }

    #[test]
    fn collects_dependency_hashes_for_call_sites() {
        let mut types = TypeTable::new();
        let source =
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n";
        let indexed = index_file(source, &mut types).expect("index");
        let main = indexed
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        assert!(
            main.dependency_name_hashes.contains(&hash_text("helper")),
            "expected helper dependency hash"
        );
    }
}
