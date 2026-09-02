use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::frontend::lexer::{lex, lex_with_diagnostic, Token, TokenKind};
use crate::frontend::parser::{lexer_error_context, parse_string_literal_text};
use crate::{SourceDiagnostic, SourceDiagnosticCode, SourceDiagnosticEdit, SourceDiagnosticFix};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleImport {
    pub path: String,
    pub target: String,
    pub alias: String,
    pub span: Range<usize>,
    pub declaration_span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRecord {
    pub path: String,
    pub alias: String,
    pub imports: Vec<ModuleImport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleGraph {
    roots: BTreeSet<String>,
    modules: BTreeMap<String, ModuleRecord>,
    reverse_edges: BTreeMap<String, BTreeSet<String>>,
    traversal: Vec<String>,
}

impl ModuleGraph {
    pub fn load(
        roots: impl IntoIterator<Item = String>,
        mut load_source: impl FnMut(&str) -> Result<String, String>,
    ) -> Result<(Self, BTreeMap<String, String>), SourceDiagnostic> {
        let roots: BTreeSet<String> = roots.into_iter().collect();
        let mut pending: Vec<(String, Option<(String, Range<usize>, Range<usize>, String)>)> =
            roots
                .iter()
                .rev()
                .cloned()
                .map(|root| (root, None))
                .collect();
        let mut sources = BTreeMap::new();
        let mut modules = BTreeMap::new();

        while let Some((path, imported_from)) = pending.pop() {
            if modules.contains_key(&path) {
                continue;
            }
            let source = load_source(&path).map_err(|message| match imported_from {
                None => SourceDiagnostic::new(path.clone(), 0, 0, module_alias(&path), message),
                Some((importer, span, declaration_span, alias)) => {
                    let fix = SourceDiagnosticFix {
                        title: format!("Remove unresolved import '{alias}'"),
                        edits: vec![SourceDiagnosticEdit {
                            path: importer.clone(),
                            start: declaration_span.start,
                            end: declaration_span.end,
                            new_text: String::new(),
                        }],
                    };
                    SourceDiagnostic::new(importer, span.start, span.end, alias, message)
                        .with_code(SourceDiagnosticCode::MissingModule)
                        .with_fix(fix)
                }
            })?;
            let imports = parse_imports(&path, &source)?;
            validate_graphics_internal_source_policy(&path, &source, &imports)?;
            for import in imports.iter().rev() {
                if !modules.contains_key(&import.target) {
                    pending.push((
                        import.target.clone(),
                        Some((
                            path.clone(),
                            import.span.clone(),
                            import.declaration_span.clone(),
                            import.alias.clone(),
                        )),
                    ));
                }
            }
            let alias = module_alias(&path);
            sources.insert(path.clone(), source);
            modules.insert(
                path.clone(),
                ModuleRecord {
                    path,
                    alias,
                    imports,
                },
            );
        }

        validate_unique_aliases(&modules)?;
        let traversal = deterministic_traversal(&roots, &modules)?;
        let mut reverse_edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (path, module) in &modules {
            reverse_edges.entry(path.clone()).or_default();
            for import in &module.imports {
                reverse_edges
                    .entry(import.target.clone())
                    .or_default()
                    .insert(path.clone());
            }
        }
        Ok((
            Self {
                roots,
                modules,
                reverse_edges,
                traversal,
            },
            sources,
        ))
    }

    pub fn roots(&self) -> &BTreeSet<String> {
        &self.roots
    }

    pub(crate) fn set_roots(&mut self, roots: BTreeSet<String>) {
        self.roots = roots;
    }

    pub fn modules(&self) -> &BTreeMap<String, ModuleRecord> {
        &self.modules
    }

    pub fn traversal(&self) -> &[String] {
        &self.traversal
    }

    pub fn module(&self, path: &str) -> Option<&ModuleRecord> {
        self.modules.get(path)
    }

    pub fn direct_dependencies(&self, path: &str) -> Vec<&str> {
        self.modules.get(path).map_or_else(Vec::new, |module| {
            module
                .imports
                .iter()
                .map(|import| import.target.as_str())
                .collect()
        })
    }

    pub fn dependency_closure(&self, path: &str) -> BTreeSet<String> {
        closure_from(path, |item| {
            self.modules
                .get(item)
                .into_iter()
                .flat_map(|module| module.imports.iter().map(|import| import.target.clone()))
                .collect()
        })
    }

    pub fn invalidation_closure(&self, path: &str) -> BTreeSet<String> {
        closure_from(path, |item| {
            self.reverse_edges
                .get(item)
                .into_iter()
                .flat_map(|paths| paths.iter().cloned())
                .collect()
        })
    }

    pub fn imported_alias_target(&self, path: &str, alias: &str) -> Option<&str> {
        self.modules
            .get(path)?
            .imports
            .iter()
            .find_map(|import| (import.alias == alias).then_some(import.target.as_str()))
    }
}

fn validate_graphics_internal_source_policy(
    path: &str,
    source: &str,
    imports: &[ModuleImport],
) -> Result<(), SourceDiagnostic> {
    let normalized = path.replace('\\', "/");
    let is_graphics_implementation = normalized.ends_with("stdlib/graphics.stasis")
        || normalized.ends_with("stdlib/internal/gfx_cmd.stasis");
    let is_explicit_seam =
        normalized.starts_with("tests/stasis/") || normalized.contains("/tests/stasis/");
    if is_graphics_implementation || is_explicit_seam {
        return Ok(());
    }

    if let Some(import) = imports
        .iter()
        .find(|import| import.target.ends_with("stdlib/internal/gfx_cmd.stasis"))
    {
        return Err(diagnostic(
            path,
            import.span.clone(),
            &import.path,
            "graphics command storage is internal; import stdlib/graphics.stasis and use its public API",
        ));
    }

    let tokens = lex_with_diagnostic(source).map_err(|error| {
        diagnostic(
            path,
            error.offset..error.offset,
            "graphics internal",
            error.message,
        )
    })?;
    for token in tokens {
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let identifier = token_text(source, token);
        if identifier.starts_with("gfx_cmd_")
            || identifier.starts_with("gfx_sprite_writer_")
            || identifier.starts_with("GFX_")
            || identifier.starts_with("graphics_line_batch_")
        {
            return Err(diagnostic(
                path,
                token.start..token.end,
                identifier,
                format!(
                    "graphics internal identifier '{identifier}' is unavailable here; use stdlib/graphics.stasis"
                ),
            ));
        }
    }
    Ok(())
}

pub fn load_project_module_graph(
    project_root: &Path,
    entry_file: &Path,
) -> Result<(ModuleGraph, BTreeMap<String, String>), SourceDiagnostic> {
    let root = ConfinedProjectRoot::new(project_root)
        .map_err(|message| diagnostic(&project_root.to_string_lossy(), 0..0, "", message))?;
    let entry = root
        .entry_key(entry_file)
        .map_err(|message| diagnostic(&entry_file.to_string_lossy(), 0..0, "", message))?;
    ModuleGraph::load([entry], |path| root.read_source(path))
}

#[derive(Debug, Clone)]
pub(crate) struct ConfinedProjectRoot {
    canonical: PathBuf,
}

impl ConfinedProjectRoot {
    pub(crate) fn new(root: &Path) -> Result<Self, String> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|error| format!("project root cannot be resolved: {error}"))?;
        if !canonical.is_dir() {
            return Err(format!(
                "project root is not a directory: '{}'",
                display_path(&canonical)
            ));
        }
        Ok(Self { canonical })
    }

    pub(crate) fn entry_key(&self, entry: &Path) -> Result<String, String> {
        let candidate = if entry.is_absolute() {
            entry.to_path_buf()
        } else {
            self.canonical.join(entry)
        };
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|error| format!("entry module cannot be resolved: {error}"))?;
        self.project_key(&canonical)
    }

    pub(crate) fn read_source(&self, path: &str) -> Result<String, String> {
        let candidate = self.canonical.join(path);
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|error| format!("missing imported module '{path}': {error}"))?;
        self.project_key(&canonical)
            .map_err(|_| format!("imported module escapes project root: '{path}'"))?;
        std::fs::read_to_string(canonical)
            .map_err(|error| format!("missing imported module '{path}': {error}"))
    }

    fn project_key(&self, path: &Path) -> Result<String, String> {
        confined_project_path(&self.canonical, path)
    }
}

fn confined_project_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "source path is outside project root: '{}'",
            display_path(path)
        )
    })?;
    crate::identity::canonical_source_path(None, &display_path(relative))
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    text.strip_prefix("//?/").unwrap_or(&text).to_string()
}

pub fn parse_imports(path: &str, source: &str) -> Result<Vec<ModuleImport>, SourceDiagnostic> {
    let tokens = lex_with_diagnostic(source).map_err(|error| {
        let context = lexer_error_context(source, error.message, error.offset);
        diagnostic(
            path,
            context.start..context.end,
            context.symbol,
            context.message,
        )
        .with_code(SourceDiagnosticCode::Parse)
    })?;
    let mut imports = Vec::new();
    let mut cursor = 0usize;
    while cursor < tokens.len() {
        let token = tokens[cursor];
        if token.kind != TokenKind::Identifier || token_text(source, token) != "import" {
            cursor += 1;
            continue;
        }
        let Some(literal) = tokens.get(cursor + 1).copied() else {
            return Err(diagnostic(
                path,
                token.start..token.end,
                "import",
                "import must be followed by a string literal path",
            )
            .with_code(SourceDiagnosticCode::Parse));
        };
        if literal.kind != TokenKind::StringLiteral {
            return Err(diagnostic(
                path,
                token.start..literal.end,
                "import",
                "import must be followed by a string literal path",
            )
            .with_code(SourceDiagnosticCode::Parse));
        }
        let import_path =
            parse_string_literal_text(token_text(source, literal)).map_err(|message| {
                diagnostic(path, literal.start..literal.end, "import", message)
                    .with_code(SourceDiagnosticCode::Parse)
            })?;
        let target = resolve_import_path(path, &import_path).map_err(|message| {
            diagnostic(path, literal.start..literal.end, &import_path, message)
                .with_code(SourceDiagnosticCode::Parse)
        })?;
        let alias = module_alias(&target);
        let declaration_end = tokens
            .get(cursor + 2)
            .filter(|token| token.kind == TokenKind::Semicolon)
            .map_or(literal.end, |token| token.end);
        imports.push(ModuleImport {
            path: import_path,
            target,
            alias,
            span: literal.start..literal.end,
            declaration_span: import_removal_span(source, token.start, declaration_end),
        });
        cursor += 2;
    }
    imports.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then(left.span.start.cmp(&right.span.start))
    });
    for pair in imports.windows(2) {
        if pair[0].alias == pair[1].alias {
            let duplicate = &pair[1];
            let fix = SourceDiagnosticFix {
                title: format!("Remove duplicate import '{}'", duplicate.path),
                edits: vec![SourceDiagnosticEdit {
                    path: path.to_string(),
                    start: duplicate.declaration_span.start,
                    end: duplicate.declaration_span.end,
                    new_text: String::new(),
                }],
            };
            return Err(diagnostic(
                path,
                duplicate.span.clone(),
                &duplicate.alias,
                format!("duplicate imported module alias '{}'", duplicate.alias),
            )
            .with_code(SourceDiagnosticCode::DuplicateImportAlias)
            .with_fix(fix));
        }
    }
    Ok(imports)
}

fn import_removal_span(source: &str, start: usize, end: usize) -> Range<usize> {
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    let line_end = source[end..]
        .find('\n')
        .map_or(source.len(), |index| end + index + 1);
    let suffix_end = line_end.saturating_sub(usize::from(
        line_end > 0 && source.as_bytes().get(line_end - 1) == Some(&b'\n'),
    ));
    if source[line_start..start].trim().is_empty() && source[end..suffix_end].trim().is_empty() {
        line_start..line_end
    } else {
        start..end
    }
}

pub(crate) fn resolve_import_path(importer: &str, import: &str) -> Result<String, String> {
    let normalized = import.replace('\\', "/");
    if !normalized.ends_with(".stasis") {
        return Err(format!("import target must end in .stasis: '{import}'"));
    }
    if normalized.starts_with("//") || import.as_bytes().get(1) == Some(&b':') {
        return Err(format!(
            "import target must be project-relative: '{import}'"
        ));
    }
    if let Some(project_path) = normalized.strip_prefix('/') {
        if project_path.is_empty()
            || project_path
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err(format!(
                "project-root import must stay within the project: '{import}'"
            ));
        }
        return crate::identity::canonical_source_path(None, project_path);
    }
    let parent = importer.rsplit_once('/').map_or("", |(parent, _)| parent);
    let joined = if parent.is_empty() {
        normalized
    } else {
        format!("{parent}/{normalized}")
    };
    crate::identity::canonical_source_path(None, &joined)
}

fn module_alias(path: &str) -> String {
    root_module_alias(path)
}

fn root_module_alias(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let raw = name.strip_suffix(".stasis").unwrap_or(name);
    let mut alias: String = raw
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect();
    if alias.is_empty() || alias.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        alias.insert(0, '_');
    }
    if lex(&alias)
        .ok()
        .and_then(|tokens| tokens.first().copied())
        .is_none_or(|token| token.kind != TokenKind::Identifier)
    {
        alias.insert(0, '_');
    }
    alias
}

fn validate_unique_aliases(
    modules: &BTreeMap<String, ModuleRecord>,
) -> Result<(), SourceDiagnostic> {
    let mut aliases: BTreeMap<&str, &str> = BTreeMap::new();
    for module in modules.values() {
        if let Some(previous) = aliases.insert(&module.alias, &module.path) {
            if previous != module.path {
                let import_site = modules.values().find_map(|importer| {
                    importer
                        .imports
                        .iter()
                        .find(|import| import.target == module.path)
                        .map(|import| {
                            (
                                importer.path.as_str(),
                                import.span.clone(),
                                import.declaration_span.clone(),
                                import.path.as_str(),
                            )
                        })
                });
                let (path, span, declaration_span, import_path) = import_site.unwrap_or((
                    &module.path,
                    0..module.path.len(),
                    0..0,
                    module.path.as_str(),
                ));
                let mut result = diagnostic(
                    path,
                    span,
                    &module.alias,
                    format!(
                        "duplicate module alias '{}' for '{}' and '{}'",
                        module.alias, previous, module.path
                    ),
                )
                .with_code(SourceDiagnosticCode::DuplicateImportAlias);
                if !declaration_span.is_empty() {
                    result = result.with_fix(SourceDiagnosticFix {
                        title: format!("Remove conflicting import '{import_path}'"),
                        edits: vec![SourceDiagnosticEdit {
                            path: path.to_string(),
                            start: declaration_span.start,
                            end: declaration_span.end,
                            new_text: String::new(),
                        }],
                    });
                }
                return Err(result);
            }
        }
    }
    Ok(())
}

fn deterministic_traversal(
    roots: &BTreeSet<String>,
    modules: &BTreeMap<String, ModuleRecord>,
) -> Result<Vec<String>, SourceDiagnostic> {
    fn visit(
        path: &str,
        modules: &BTreeMap<String, ModuleRecord>,
        visiting: &mut Vec<String>,
        visited: &mut BTreeSet<String>,
        out: &mut Vec<String>,
    ) -> Result<(), SourceDiagnostic> {
        if visited.contains(path) {
            return Ok(());
        }
        if let Some(index) = visiting.iter().position(|item| item == path) {
            let mut cycle = visiting[index..].to_vec();
            cycle.push(path.to_string());
            let importer = visiting.last().map_or(path, String::as_str);
            let import = modules
                .get(importer)
                .and_then(|module| module.imports.iter().find(|item| item.target == path));
            return Err(diagnostic(
                importer,
                import.map_or(0..0, |item| item.span.clone()),
                import.map_or("", |item| item.alias.as_str()),
                format!("import cycle: {}", cycle.join(" -> ")),
            ));
        }
        visiting.push(path.to_string());
        if let Some(module) = modules.get(path) {
            for import in &module.imports {
                visit(&import.target, modules, visiting, visited, out)?;
            }
        }
        visiting.pop();
        visited.insert(path.to_string());
        out.push(path.to_string());
        Ok(())
    }

    let mut out = Vec::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        visit(root, modules, &mut Vec::new(), &mut visited, &mut out)?;
    }
    Ok(out)
}

fn closure_from(path: &str, mut adjacent: impl FnMut(&str) -> Vec<String>) -> BTreeSet<String> {
    let mut closure = BTreeSet::new();
    let mut pending = vec![path.to_string()];
    while let Some(item) = pending.pop() {
        if closure.insert(item.clone()) {
            pending.extend(adjacent(&item));
        }
    }
    closure
}

fn token_text(source: &str, token: Token) -> &str {
    &source[token.start..token.end]
}

fn diagnostic(
    path: &str,
    span: Range<usize>,
    symbol: impl Into<String>,
    message: impl Into<String>,
) -> SourceDiagnostic {
    SourceDiagnostic::new(path, span.start, span.end, symbol, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(roots: &[&str], files: &[(&str, &str)]) -> Result<ModuleGraph, SourceDiagnostic> {
        let files: BTreeMap<_, _> = files
            .iter()
            .map(|(path, source)| (path.to_string(), source.to_string()))
            .collect();
        ModuleGraph::load(roots.iter().map(|root| root.to_string()), |path| {
            files
                .get(path)
                .cloned()
                .ok_or_else(|| format!("missing module '{path}'"))
        })
        .map(|(graph, _)| graph)
    }

    #[test]
    fn lexer_owned_graph_loads_nested_same_line_imports_deterministically() {
        let graph = graph(
            &["src/main.stasis"],
            &[
                (
                    "src/main.stasis",
                    "import \"lib/a.stasis\"; function main(): i32 { return value(); }",
                ),
                (
                    "src/lib/a.stasis",
                    "import \"../shared.stasis\"; function value(): i32 { return 7; }",
                ),
                ("src/shared.stasis", "function shared(): i32 { return 1; }"),
            ],
        )
        .unwrap();
        assert_eq!(
            graph.traversal(),
            &["src/shared.stasis", "src/lib/a.stasis", "src/main.stasis"]
        );
        assert_eq!(
            graph.invalidation_closure("src/shared.stasis"),
            BTreeSet::from_iter([
                "src/shared.stasis".to_string(),
                "src/lib/a.stasis".to_string(),
                "src/main.stasis".to_string(),
            ])
        );
        assert_eq!(
            graph.dependency_closure("src/lib/a.stasis"),
            BTreeSet::from_iter([
                "src/lib/a.stasis".to_string(),
                "src/shared.stasis".to_string(),
            ])
        );
        assert!(!graph
            .dependency_closure("src/shared.stasis")
            .contains("src/lib/a.stasis"));
    }

    #[test]
    fn graph_reports_cycle_at_import_span() {
        let error = graph(
            &["a.stasis"],
            &[
                ("a.stasis", "import \"b.stasis\";"),
                ("b.stasis", "import \"a.stasis\";"),
            ],
        )
        .unwrap_err();
        assert_eq!(error.path, "b.stasis");
        assert_eq!(
            &"import \"a.stasis\";"[error.start..error.end],
            "\"a.stasis\""
        );
        assert_eq!(
            error.message,
            "import cycle: a.stasis -> b.stasis -> a.stasis"
        );
    }

    #[test]
    fn graph_reports_missing_module_at_the_import_literal() {
        let source = "function main(): i32 { return 0; } import \"missing.stasis\";";
        let error = graph(&["main.stasis"], &[("main.stasis", source)]).unwrap_err();
        assert_eq!(error.path, "main.stasis");
        assert_eq!(&source[error.start..error.end], "\"missing.stasis\"");
        assert_eq!(error.symbol, "missing");
        assert!(error.message.contains("missing module 'missing.stasis'"));
        assert_eq!(error.code, SourceDiagnosticCode::MissingModule);
        assert_eq!(error.fixes.len(), 1);
        assert_eq!(error.fixes[0].edits.len(), 1);
        let edit = &error.fixes[0].edits[0];
        assert_eq!(&source[edit.start..edit.end], "import \"missing.stasis\";");
        assert!(edit.new_text.is_empty());
    }

    #[test]
    fn imported_lexer_failure_preserves_active_function_context_and_span() {
        let imported = concat!(
            "function helper(): void {}\n",
            "function active(): void { \"unterminated\n",
        );
        let error = graph(
            &["main.stasis"],
            &[
                (
                    "main.stasis",
                    "import \"broken.stasis\";\nfunction main(): void {}\n",
                ),
                ("broken.stasis", imported),
            ],
        )
        .expect_err("imported lexer failure must be reported");
        assert_eq!(error.path, "broken.stasis");
        assert_eq!(error.symbol, "active");
        assert_eq!(error.start, imported.find("\"unterminated").unwrap());
        assert_eq!(error.end, imported.len());
        assert_eq!(error.code, SourceDiagnosticCode::Parse);
    }

    #[test]
    fn duplicate_import_diagnostic_owns_the_removal_fix() {
        let source = "import \"helper.stasis\"; import \"helper.stasis\";";
        let error = parse_imports("main.stasis", source).expect_err("duplicate import");
        assert_eq!(error.code, SourceDiagnosticCode::DuplicateImportAlias);
        let edit = &error.fixes[0].edits[0];
        assert_eq!(&source[edit.start..edit.end], "import \"helper.stasis\";");
        assert_eq!(edit.start, source.rfind("import").expect("second import"));
    }

    #[test]
    fn graph_rejects_escape_non_stasis_and_duplicate_basename() {
        for (source, expected) in [
            ("import \"../outside.stasis\";", "escapes project root"),
            ("import \"helper.txt\";", "must end in .stasis"),
        ] {
            let error = graph(&["main.stasis"], &[("main.stasis", source)]).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
            assert_eq!(error.code, SourceDiagnosticCode::Parse);
        }
        let error = graph(
            &["main.stasis"],
            &[
                (
                    "main.stasis",
                    "import \"one/util.stasis\"; import \"two/util.stasis\";",
                ),
                ("one/util.stasis", ""),
                ("two/util.stasis", ""),
            ],
        )
        .unwrap_err();
        assert!(error
            .message
            .contains("duplicate imported module alias 'util'"));
    }

    #[test]
    fn project_root_imports_resolve_from_any_source_directory() {
        for importer in ["src/main.stasis", "src/game/player.stasis"] {
            assert_eq!(
                resolve_import_path(importer, "/vendor/stasis/stdlib/storage.stasis")
                    .expect("project-root import"),
                "vendor/stasis/stdlib/storage.stasis"
            );
        }
        assert_eq!(
            resolve_import_path(
                "vendor/stasis/stdlib/graphics.stasis",
                "internal/host_frame.stasis"
            )
            .expect("package-internal relative import"),
            "vendor/stasis/stdlib/internal/host_frame.stasis"
        );
    }

    #[test]
    fn project_root_imports_cannot_escape_the_project() {
        let error = resolve_import_path("src/main.stasis", "/vendor/../main.stasis")
            .expect_err("project-root escape must fail");
        assert!(error.contains("must stay within the project"));
    }

    #[test]
    fn non_identifier_module_filenames_get_internal_identifier_aliases() {
        let graph = graph(
            &["main.stasis"],
            &[
                (
                    "main.stasis",
                    "import \"generated/balance.generated.stasis\"; import \"helpers/bad-name.stasis\";",
                ),
                ("generated/balance.generated.stasis", ""),
                ("helpers/bad-name.stasis", ""),
            ],
        )
        .expect("sanitized module aliases");
        assert_eq!(
            graph
                .module("generated/balance.generated.stasis")
                .unwrap()
                .alias,
            "balance_generated"
        );
        assert_eq!(
            graph.module("helpers/bad-name.stasis").unwrap().alias,
            "bad_name"
        );
    }

    #[test]
    fn non_identifier_root_filename_gets_internal_identifier_alias() {
        let graph = graph(
            &["main.test.stasis", "binding-shape.stasis"],
            &[
                ("main.test.stasis", "function main(): i32 { return 0; }"),
                (
                    "binding-shape.stasis",
                    "function binding(): i32 { return 0; }",
                ),
            ],
        )
        .expect("root aliases");
        assert_eq!(graph.module("main.test.stasis").unwrap().alias, "main_test");
        assert_eq!(
            graph.module("binding-shape.stasis").unwrap().alias,
            "binding_shape"
        );
    }

    #[test]
    fn project_loader_resolves_relative_root_and_entry_once_and_confines_entry() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let cwd = std::env::current_dir().expect("current directory");
        let base = cwd
            .join("target")
            .join(format!("module_graph_loader_{stamp}"));
        let root = base.join("project");
        std::fs::create_dir_all(root.join("src")).expect("project directories");
        std::fs::write(
            root.join("src/main.stasis"),
            "import \"helper.stasis\"; function main(): i32 { return helper(); }",
        )
        .expect("main source");
        std::fs::write(
            root.join("src/helper.stasis"),
            "function helper(): i32 { return 7; }",
        )
        .expect("helper source");
        std::fs::write(
            base.join("outside.stasis"),
            "function outside(): i32 { return 0; }",
        )
        .expect("outside source");

        let relative_root = root.strip_prefix(&cwd).expect("relative project root");
        let (graph, _) = load_project_module_graph(relative_root, Path::new("src/main.stasis"))
            .expect("relative project load");
        assert_eq!(
            graph.modules().keys().cloned().collect::<Vec<_>>(),
            vec!["src/helper.stasis", "src/main.stasis"]
        );

        let error = load_project_module_graph(&root, Path::new("../outside.stasis"))
            .expect_err("outside entry must be rejected");
        assert!(error.message.contains("outside project root"), "{error:?}");
        std::fs::remove_dir_all(base).expect("temporary project cleanup");
    }

    #[test]
    fn graphics_internal_policy_rejects_imports_and_transitive_abi_names() {
        let import_source = "import \"vendor/stasis/stdlib/internal/gfx_cmd.stasis\";";
        let import_error = graph(
            &["main.stasis"],
            &[
                ("main.stasis", import_source),
                ("vendor/stasis/stdlib/internal/gfx_cmd.stasis", ""),
            ],
        )
        .expect_err("application import must be rejected");
        assert!(import_error
            .message
            .contains("graphics command storage is internal"));

        for source in [
            "function main(): i32 { return gfx_cmd_line_count(); }",
            "global gfx_cmd_i32: i32[4];",
            "const GFX_I_FLAGS: i32 = 2;",
            "function gfx_sprite_writer_redeclare(): void {}",
        ] {
            let error = graph(&["main.stasis"], &[("main.stasis", source)])
                .expect_err("internal name must be rejected");
            assert!(error.message.contains("graphics internal identifier"));
        }

        let transitive_error = graph(
            &["main.stasis"],
            &[
                ("main.stasis", "import \"helper.stasis\";"),
                (
                    "helper.stasis",
                    "extern function gfx_cmd_submit(): void; function helper(): void { gfx_cmd_submit(); }",
                ),
            ],
        )
        .expect_err("transitively imported internals must be rejected");
        assert_eq!(transitive_error.path, "helper.stasis");
        assert!(transitive_error
            .message
            .contains("graphics internal identifier"));
    }

    #[test]
    fn graphics_internal_policy_allows_implementation_and_explicit_seams() {
        graph(
            &["vendor/stasis/stdlib/graphics.stasis"],
            &[(
                "vendor/stasis/stdlib/graphics.stasis",
                "function draw_line(): i32 { return gfx_cmd_line_count(); }",
            )],
        )
        .expect("graphics implementation owns the private vocabulary");
        graph(
            &["tests/stasis/seams/gfx_probe.stasis"],
            &[(
                "tests/stasis/seams/gfx_probe.stasis",
                "global gfx_cmd_i32: i32[4]; function main(): i32 { return GFX_I_FLAGS; }",
            )],
        )
        .expect("explicit ABI seams may inspect storage");
    }

    #[cfg(unix)]
    #[test]
    fn project_loader_rejects_import_symlink_escape() {
        use std::os::unix::fs::symlink;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("module_graph_symlink_{stamp}"));
        let root = base.join("project");
        std::fs::create_dir_all(&root).expect("project directory");
        std::fs::write(
            root.join("main.stasis"),
            "import \"escape.stasis\"; function main(): i32 { return outside(); }",
        )
        .expect("main source");
        std::fs::write(
            base.join("outside.stasis"),
            "function outside(): i32 { return 0; }",
        )
        .expect("outside source");
        symlink(base.join("outside.stasis"), root.join("escape.stasis")).expect("source symlink");

        let error = load_project_module_graph(&root, Path::new("main.stasis"))
            .expect_err("symlink escape must be rejected");
        assert!(error.message.contains("escapes project root"), "{error:?}");
        std::fs::remove_dir_all(base).expect("temporary project cleanup");
    }
}
