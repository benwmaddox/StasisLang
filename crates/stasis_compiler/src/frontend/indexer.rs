use std::ops::Range;

use crate::frontend::lexer::{lex, TokenKind};
use crate::frontend::parser::parse_top_level_functions;
use crate::frontend::types::{TypeId, TypeTable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFunction {
    pub name: String,
    pub name_hash: u64,
    pub source_range: Range<u32>,
    pub signature_range: Range<u32>,
    pub signature_hash: u64,
    pub body_hash: u64,
    pub param_names: Vec<String>,
    pub params: Vec<TypeId>,
    pub param_type_names: Vec<String>,
    pub return_type: TypeId,
    pub inline: bool,
    pub dependencies: Vec<IndexedCallDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedCallDependency {
    pub qualifier: Option<String>,
    pub qualifier_span: Option<Range<u32>>,
    pub name: String,
    pub name_span: Range<u32>,
}

/// Lightweight parser-owned declaration record for editor and CLI lookup.
/// This deliberately avoids type interning and semantic indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedSourceFunctionItem {
    pub name: String,
    pub source_range: Range<u32>,
    pub signature_range: Range<u32>,
}

pub fn source_function_items(source: &str) -> Result<Vec<IndexedSourceFunctionItem>, String> {
    parse_top_level_functions(source).map(|functions| {
        functions
            .into_iter()
            .map(|function| IndexedSourceFunctionItem {
                name: function.name,
                source_range: function.body_range.start as u32..function.body_range.end as u32,
                signature_range: function.signature_range.start as u32
                    ..function.signature_range.end as u32,
            })
            .collect()
    })
}

pub fn index_file(source: &str, types: &mut TypeTable) -> Result<Vec<IndexedFunction>, String> {
    let parsed = parse_top_level_functions(source)?;
    let mut out = Vec::with_capacity(parsed.len());
    for function in parsed {
        let mut params = Vec::with_capacity(function.params.len());
        let mut param_names = Vec::with_capacity(function.params.len());
        let mut param_type_names = Vec::with_capacity(function.params.len());
        for param in &function.params {
            let type_id = types.resolve_or_intern(&param.type_name)?;
            param_names.push(param.name.clone());
            param_type_names.push(param.type_name.clone());
            params.push(type_id);
        }
        let return_type = types.resolve_or_intern(&function.return_type_name)?;
        let name_hash = hash_text(&function.name);
        let inline = function
            .annotations
            .iter()
            .any(|annotation| annotation.name == "inline");
        let signature_hash = hash_signature(name_hash, &params, return_type, inline);
        let body_text = source
            .get(function.body_range.clone())
            .ok_or_else(|| "invalid function body range".to_string())?;
        let body_hash = hash_text(body_text);
        let dependencies = collect_dependencies(body_text)?;
        out.push(IndexedFunction {
            name: function.name,
            name_hash,
            source_range: function.body_range.start as u32..function.body_range.end as u32,
            signature_range: function.signature_range.start as u32
                ..function.signature_range.end as u32,
            signature_hash,
            body_hash,
            param_names,
            params,
            param_type_names,
            return_type,
            inline,
            dependencies,
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

fn hash_signature(name_hash: u64, params: &[TypeId], return_type: TypeId, inline: bool) -> u64 {
    let mut hash = name_hash;
    hash = hash
        .wrapping_mul(1099511628211)
        .wrapping_add(u64::from(return_type));
    for param in params {
        hash = hash
            .wrapping_mul(1099511628211)
            .wrapping_add(u64::from(*param));
    }
    hash = hash
        .wrapping_mul(1099511628211)
        .wrapping_add(u64::from(inline));
    hash
}

fn collect_dependencies(body_text: &str) -> Result<Vec<IndexedCallDependency>, String> {
    let tokens = lex(body_text)?;
    let mut dependencies = Vec::new();
    for index in 0..tokens.len() {
        let token = tokens[index];
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let identifier = &body_text[token.start..token.end];
        if is_call_keyword(identifier) {
            continue;
        }
        if tokens.get(index + 1).is_some_and(|next| {
            next.kind == TokenKind::Other && &body_text[next.start..next.end] == "."
        }) && tokens
            .get(index + 2)
            .is_some_and(|method| method.kind == TokenKind::Identifier)
            && tokens
                .get(index + 3)
                .is_some_and(|paren| paren.kind == TokenKind::LParen)
        {
            let method = tokens[index + 2];
            dependencies.push(IndexedCallDependency {
                qualifier: Some(identifier.to_string()),
                qualifier_span: Some(token.start as u32..token.end as u32),
                name: body_text[method.start..method.end].to_string(),
                name_span: method.start as u32..method.end as u32,
            });
            continue;
        }
        if tokens.get(index.wrapping_sub(1)).is_some_and(|previous| {
            previous.kind == TokenKind::Other && &body_text[previous.start..previous.end] == "."
        }) {
            let receiver_method_already_collected = tokens
                .get(index.wrapping_sub(2))
                .is_some_and(|receiver| receiver.kind == TokenKind::Identifier);
            if !receiver_method_already_collected
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.kind == TokenKind::LParen)
            {
                dependencies.push(IndexedCallDependency {
                    qualifier: None,
                    qualifier_span: None,
                    name: identifier.to_string(),
                    name_span: token.start as u32..token.end as u32,
                });
            }
            continue;
        }
        if tokens
            .get(index + 1)
            .is_some_and(|next| next.kind == TokenKind::LParen)
        {
            dependencies.push(IndexedCallDependency {
                qualifier: None,
                qualifier_span: None,
                name: identifier.to_string(),
                name_span: token.start as u32..token.end as u32,
            });
        }
    }
    Ok(dependencies)
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
        assert_eq!(
            indexed[0].return_type,
            types.resolve("i32").unwrap_or_default()
        );
        assert!(!indexed[0].inline);
    }

    #[test]
    fn indexes_inline_as_part_of_the_lowering_contract() {
        let mut types = TypeTable::new();
        let plain = index_file(
            "function helper(value: i32): i32 { return value; }\n",
            &mut types,
        )
        .expect("plain index");
        let annotated = index_file(
            "function @inline helper(value: i32): i32 { return value; }\n",
            &mut types,
        )
        .expect("inline index");
        assert!(!plain[0].inline);
        assert!(annotated[0].inline);
        assert_ne!(plain[0].signature_hash, annotated[0].signature_hash);
    }

    #[test]
    fn collects_qualified_and_bare_dependencies_for_call_sites() {
        let mut types = TypeTable::new();
        let source = "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper() + math.value(); }\n";
        let indexed = index_file(source, &mut types).expect("index");
        let main = indexed
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        assert_eq!(
            main.dependencies,
            vec![
                IndexedCallDependency {
                    qualifier: None,
                    qualifier_span: None,
                    name: "helper".to_string(),
                    name_span: 9..15,
                },
                IndexedCallDependency {
                    qualifier: Some("math".to_string()),
                    qualifier_span: Some(20..24),
                    name: "value".to_string(),
                    name_span: 25..30,
                },
            ]
        );
    }

    #[test]
    fn collects_method_dependencies_from_indexed_and_chained_receivers() {
        let mut types = TypeTable::new();
        let source = concat!(
            "function main(index: i32): i32 { ",
            "return state.assets[index].poll() + state.current.release(); ",
            "}\n"
        );
        let indexed = index_file(source, &mut types).expect("index");
        let main = indexed
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        assert_eq!(
            main.dependencies
                .iter()
                .map(|dependency| dependency.name.as_str())
                .collect::<Vec<_>>(),
            vec!["poll", "release"]
        );
        assert_eq!(main.dependencies[0].qualifier, None);
        assert_eq!(main.dependencies[1].qualifier.as_deref(), Some("current"));
    }
}
