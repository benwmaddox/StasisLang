use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;

use crate::frontend::lexer::{lex, Token, TokenKind};
use crate::frontend::parser::{parse_top_level_functions, parse_top_level_type_layout};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkshopSymbolKind {
    Struct,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkshopSymbolGroupKind {
    Main,
    Struct,
    System,
    Root,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSourceFile {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSymbol {
    pub kind: WorkshopSymbolKind,
    pub name: String,
    pub owner: Option<String>,
    pub file: String,
    pub signature: String,
    pub source_span: WorkshopSourceSpan,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkshopSymbolGroup {
    pub kind: WorkshopSymbolGroupKind,
    pub name: String,
    pub symbols: Vec<WorkshopSymbol>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkshopSymbolTree {
    pub groups: Vec<WorkshopSymbolGroup>,
}

#[derive(Debug, Clone)]
struct PendingSymbol {
    group_kind: WorkshopSymbolGroupKind,
    group_name: String,
    symbol: WorkshopSymbol,
}

pub fn build_workshop_symbol_tree(
    files: &[WorkshopSourceFile],
) -> Result<WorkshopSymbolTree, String> {
    let mut struct_names = BTreeSet::new();
    let mut structs_by_file: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for file in files {
        let layout = parse_top_level_type_layout(&file.source)?;
        for parsed in layout.structs {
            struct_names.insert(parsed.name.clone());
            structs_by_file
                .entry(file.path.as_str())
                .or_default()
                .push(parsed.name);
        }
    }

    let mut pending = Vec::new();
    for file in files {
        pending.extend(index_file_symbols(
            file,
            &struct_names,
            structs_by_file
                .get(file.path.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?);
    }

    let mut by_group: BTreeMap<(WorkshopSymbolGroupKind, String), Vec<WorkshopSymbol>> =
        BTreeMap::new();
    for pending_symbol in pending {
        by_group
            .entry((pending_symbol.group_kind, pending_symbol.group_name))
            .or_default()
            .push(pending_symbol.symbol);
    }

    let mut groups = Vec::new();
    for group_kind in [
        WorkshopSymbolGroupKind::Main,
        WorkshopSymbolGroupKind::Struct,
        WorkshopSymbolGroupKind::System,
        WorkshopSymbolGroupKind::Root,
    ] {
        let keys = by_group
            .keys()
            .filter(|(kind, _)| *kind == group_kind)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let mut symbols = by_group.remove(&key).unwrap_or_default();
            symbols.sort_by_key(|symbol| (symbol.file.clone(), symbol.source_span.start));
            groups.push(WorkshopSymbolGroup {
                kind: key.0,
                name: key.1,
                symbols,
            });
        }
    }

    Ok(WorkshopSymbolTree { groups })
}

fn index_file_symbols(
    file: &WorkshopSourceFile,
    struct_names: &BTreeSet<String>,
    file_structs: &[String],
) -> Result<Vec<PendingSymbol>, String> {
    let mut out = Vec::new();
    for parsed_struct in parse_struct_spans(&file.source)? {
        let source = source_for_range(&file.source, parsed_struct.range.clone())?;
        out.push(PendingSymbol {
            group_kind: WorkshopSymbolGroupKind::Struct,
            group_name: parsed_struct.name.clone(),
            symbol: WorkshopSymbol {
                kind: WorkshopSymbolKind::Struct,
                name: parsed_struct.name.clone(),
                owner: Some(parsed_struct.name.clone()),
                file: file.path.clone(),
                signature: format!("struct {}", parsed_struct.name),
                source_span: span_from_range(parsed_struct.range.clone())?,
                source,
            },
        });
    }

    for function in parse_top_level_functions(&file.source)? {
        let full_range = function.signature_range.start..function.body_range.end;
        let source = source_for_range(&file.source, full_range.clone())?;
        let signature =
            format_function_signature(&function.name, &function.params, &function.return_type_name);
        let owner = function_owner(
            &file.path,
            &function.name,
            function
                .params
                .first()
                .map(|param| param.type_name.as_str()),
            &function.return_type_name,
            file_structs,
            struct_names,
        );
        let (group_kind, group_name) = function_group(&file.path, &function.name, owner.as_deref());
        out.push(PendingSymbol {
            group_kind,
            group_name,
            symbol: WorkshopSymbol {
                kind: WorkshopSymbolKind::Function,
                name: function.name,
                owner,
                file: file.path.clone(),
                signature,
                source_span: span_from_range(full_range)?,
                source,
            },
        });
    }

    Ok(out)
}

fn function_owner(
    path: &str,
    function_name: &str,
    first_param_type: Option<&str>,
    return_type: &str,
    file_structs: &[String],
    struct_names: &BTreeSet<String>,
) -> Option<String> {
    if is_lifecycle_function(function_name) || is_system_path(path) || is_root_path(path) {
        return None;
    }

    if let Some(param_type) = first_param_type {
        if struct_names.contains(param_type) {
            return Some(param_type.to_string());
        }
    }

    if file_structs.iter().any(|name| name == return_type) {
        return Some(return_type.to_string());
    }

    if file_structs.len() == 1 && stem_matches_struct(path, &file_structs[0]) {
        return Some(file_structs[0].clone());
    }

    None
}

fn function_group(
    path: &str,
    function_name: &str,
    owner: Option<&str>,
) -> (WorkshopSymbolGroupKind, String) {
    if is_lifecycle_function(function_name) || is_main_path(path) {
        return (WorkshopSymbolGroupKind::Main, "Main".to_string());
    }
    if let Some(owner) = owner {
        return (WorkshopSymbolGroupKind::Struct, owner.to_string());
    }
    if is_system_path(path) {
        return (
            WorkshopSymbolGroupKind::System,
            title_case_stem(path).unwrap_or_else(|| "System".to_string()),
        );
    }
    (WorkshopSymbolGroupKind::Root, "Root".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedStructSpan {
    name: String,
    range: Range<usize>,
}

fn parse_struct_spans(source: &str) -> Result<Vec<ParsedStructSpan>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut depth = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        match token.kind {
            TokenKind::LBrace => {
                depth = depth.saturating_add(1);
                cursor += 1;
                continue;
            }
            TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                continue;
            }
            TokenKind::Identifier if depth == 0 && token_text(source, token) == "struct" => {
                let name_token = expect_token(&tokens, cursor + 1, TokenKind::Identifier)?;
                let open_index = cursor + 2;
                expect_token(&tokens, open_index, TokenKind::LBrace)?;
                let close_index = find_matching_rbrace(&tokens, open_index + 1, 1)?;
                let end = tokens[close_index].end;
                out.push(ParsedStructSpan {
                    name: token_text(source, name_token).to_string(),
                    range: token.start..end,
                });
                cursor = close_index + 1;
                continue;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(out)
}

fn format_function_signature(
    name: &str,
    params: &[crate::frontend::parser::ParsedParam],
    return_type_name: &str,
) -> String {
    let params = params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.type_name))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({params}): {return_type_name}")
}

fn is_lifecycle_function(name: &str) -> bool {
    matches!(name, "main" | "init" | "tick" | "render" | "on_code_swap")
}

fn is_main_path(path: &str) -> bool {
    file_stem(path).is_some_and(|stem| stem == "main")
}

fn is_root_path(path: &str) -> bool {
    file_stem(path).is_some_and(|stem| stem == "root")
}

fn is_system_path(path: &str) -> bool {
    path.replace('\\', "/").contains("/systems/")
}

fn stem_matches_struct(path: &str, struct_name: &str) -> bool {
    file_stem(path).is_some_and(|stem| stem == snake_case(struct_name))
}

fn title_case_stem(path: &str) -> Option<String> {
    file_stem(path).map(|stem| {
        stem.split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<String>()
    })
}

fn file_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn source_for_range(source: &str, range: Range<usize>) -> Result<String, String> {
    source
        .get(range)
        .map(str::to_string)
        .ok_or_else(|| "invalid workshop symbol source span".to_string())
}

fn span_from_range(range: Range<usize>) -> Result<WorkshopSourceSpan, String> {
    Ok(WorkshopSourceSpan {
        start: u32::try_from(range.start)
            .map_err(|_| "symbol span start exceeds u32".to_string())?,
        end: u32::try_from(range.end).map_err(|_| "symbol span end exceeds u32".to_string())?,
    })
}

fn expect_token(tokens: &[Token], cursor: usize, kind: TokenKind) -> Result<Token, String> {
    let token = tokens
        .get(cursor)
        .copied()
        .ok_or_else(|| format!("unexpected end of token stream, expected {kind:?}"))?;
    if token.kind != kind {
        return Err(format!(
            "expected token {kind:?} but found {:?}",
            token.kind
        ));
    }
    Ok(token)
}

fn find_matching_rbrace(tokens: &[Token], start: usize, mut depth: usize) -> Result<usize, String> {
    let mut cursor = start;
    while cursor < tokens.len() {
        match tokens[cursor].kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return Ok(cursor);
                }
            }
            TokenKind::Eof => break,
            _ => {}
        }
        cursor += 1;
    }
    Err("missing closing '}' for struct body".to_string())
}

fn token_text<'a>(source: &'a str, token: Token) -> &'a str {
    &source[token.start..token.end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_android_workshop_example_symbols() {
        let files = vec![
            WorkshopSourceFile {
                path: "src/main.stasis".to_string(),
                source: r#"import "game_state.stasis";
function main(): void { init(); }
function init(): void { GameState.score = 0; }
function tick(): void { GameState.player.update(read_input()); }
function on_code_swap(): void { }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/player.stasis".to_string(),
                source: r#"struct Player {
    x: f32;
    y: f32;
    velocity_y: f32;
    jump_cooldown_ticks: i32;
}

function update(self: Player, input: InputState): void { self.y += self.velocity_y; }
function jump(self: Player): void { self.velocity_y = -8.5; }
function create_default_player(): Player { return GameState.player; }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/enemy.stasis".to_string(),
                source: r#"struct Enemy { x: f32; y: f32; hp: i32; active: bool; }
function update(self: Enemy): void { self.x -= 1.0; }
function damage(self: Enemy, amount: i32): void { self.hp -= amount; }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/systems/collision.stasis".to_string(),
                source: r#"function collision_update(): void { }
function player_overlaps_enemy(player: Player, enemy: Enemy): bool { return true; }
"#
                .to_string(),
            },
            WorkshopSourceFile {
                path: "src/root.stasis".to_string(),
                source: "function get_starting_level_index(): i32 { return 0; }\n".to_string(),
            },
        ];

        let tree = build_workshop_symbol_tree(&files).expect("symbol tree");
        let group_names = tree
            .groups
            .iter()
            .map(|group| (group.kind, group.name.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            group_names,
            vec![
                (WorkshopSymbolGroupKind::Main, "Main"),
                (WorkshopSymbolGroupKind::Struct, "Enemy"),
                (WorkshopSymbolGroupKind::Struct, "Player"),
                (WorkshopSymbolGroupKind::System, "Collision"),
                (WorkshopSymbolGroupKind::Root, "Root"),
            ]
        );

        let player = tree
            .groups
            .iter()
            .find(|group| group.name == "Player")
            .expect("player group");
        assert_eq!(
            player
                .symbols
                .iter()
                .map(|symbol| (symbol.kind, symbol.signature.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (WorkshopSymbolKind::Struct, "struct Player"),
                (
                    WorkshopSymbolKind::Function,
                    "update(self: Player, input: InputState): void"
                ),
                (WorkshopSymbolKind::Function, "jump(self: Player): void"),
                (
                    WorkshopSymbolKind::Function,
                    "create_default_player(): Player"
                ),
            ]
        );
        assert!(player.symbols[0].source.starts_with("struct Player"));
        assert!(player.symbols[1].source.starts_with("function update"));
        assert_eq!(player.symbols[1].owner.as_deref(), Some("Player"));

        let collision = tree
            .groups
            .iter()
            .find(|group| group.name == "Collision")
            .expect("collision group");
        assert_eq!(collision.symbols.len(), 2);
        assert!(collision
            .symbols
            .iter()
            .all(|symbol| symbol.owner.is_none()));
    }
}
