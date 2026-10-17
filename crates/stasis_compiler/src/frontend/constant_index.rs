//! Type-independent source index for compile-time declarations.
//!
//! The generic pass runs before nominal TypeIds exist, so this index retains
//! source declaration identity and spelling only. Semantic evaluation later
//! materializes typed values from the same records.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::rc::Rc;

use crate::compiler::SourceFile;
use crate::frontend::module_graph::ModuleGraph;
use crate::frontend::parser::{
    parse_top_level_type_layout, ParsedConstDefinition, ParsedConstGroupDefinition,
    ParsedConstGroupMemberDefinition, ParsedEnumDefinition,
};
use crate::ir::hir::{CompileTimeValueId, CompileTimeValueOwner, ExprBinaryOp, ExprUnaryOp};

pub(crate) const MAX_CONSTANT_EVALUATION_STEPS: usize = 10_000;
const MAX_CONSTANT_DEPENDENCY_DEPTH: usize = 128;

#[derive(Default)]
pub(crate) struct GenericI32EvaluationState {
    cache: HashMap<CompileTimeValueId, i32>,
    stack: Vec<CompileTimeValueId>,
    steps: Rc<Cell<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConstantToken {
    Integer(String),
    Identifier(String),
    Operator(ExprBinaryOp),
    Unary(ExprUnaryOp),
    Open,
    Close,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexedConstantDeclaration {
    pub(crate) id: CompileTimeValueId,
    pub(crate) name_range: Option<Range<usize>>,
    pub(crate) value_range: Range<usize>,
    pub(crate) type_name: String,
    pub(crate) value_text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexedConstantGroup {
    pub(crate) source_path: String,
    pub(crate) name: String,
    pub(crate) members: Vec<IndexedConstantDeclaration>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CompileTimeDeclarationIndex {
    flat_constants: BTreeMap<(String, String), Vec<IndexedConstantDeclaration>>,
    groups: BTreeMap<(String, String), Vec<IndexedConstantGroup>>,
    enums: BTreeMap<(String, String), Vec<ParsedEnumDefinition>>,
    value_names_by_path: BTreeMap<String, BTreeSet<String>>,
}

impl CompileTimeDeclarationIndex {
    pub(crate) fn build(files: &[SourceFile]) -> Result<Self, (String, String)> {
        let mut index = Self::default();
        for file in files {
            // Original source is stable across generic materialization. Indexing
            // generated content here would duplicate declarations on rebuild.
            let layout = parse_top_level_type_layout(&file.original_content)
                .map_err(|error| (file.path.clone(), error))?;
            let names = index
                .value_names_by_path
                .entry(file.path.clone())
                .or_default();
            names.extend(layout.constants.iter().map(|item| item.name.clone()));
            names.extend(layout.globals.iter().map(|item| item.name.clone()));
            names.extend(layout.global_blocks.iter().map(|item| item.name.clone()));

            for constant in layout.constants {
                let declaration = indexed_flat_constant(&file.path, constant);
                index
                    .flat_constants
                    .entry((file.path.clone(), declaration.id.member.clone()))
                    .or_default()
                    .push(declaration);
            }
            for group in layout.const_groups {
                let indexed = indexed_group(&file.path, group);
                index
                    .groups
                    .entry((file.path.clone(), indexed.name.clone()))
                    .or_default()
                    .push(indexed);
            }
            for enumeration in layout.enums {
                index
                    .enums
                    .entry((file.path.clone(), enumeration.name.clone()))
                    .or_default()
                    .push(enumeration);
            }
        }
        Ok(index)
    }

    pub(crate) fn flat_constants(
        &self,
        path: &str,
        name: &str,
    ) -> Vec<&IndexedConstantDeclaration> {
        self.flat_constants
            .get(&(path.to_string(), name.to_string()))
            .into_iter()
            .flatten()
            .collect()
    }

    pub(crate) fn groups(&self, path: &str, name: &str) -> Vec<&IndexedConstantGroup> {
        self.groups
            .get(&(path.to_string(), name.to_string()))
            .into_iter()
            .flatten()
            .collect()
    }

    pub(crate) fn enums(&self, path: &str, name: &str) -> Vec<&ParsedEnumDefinition> {
        self.enums
            .get(&(path.to_string(), name.to_string()))
            .into_iter()
            .flatten()
            .collect()
    }

    pub(crate) fn all_flat_constants(&self) -> impl Iterator<Item = &IndexedConstantDeclaration> {
        self.flat_constants.values().flatten()
    }

    pub(crate) fn all_groups(&self) -> impl Iterator<Item = &IndexedConstantGroup> {
        self.groups.values().flatten()
    }

    pub(crate) fn enum_declarations(&self) -> impl Iterator<Item = (&str, &ParsedEnumDefinition)> {
        self.enums.iter().flat_map(|((path, _), declarations)| {
            declarations
                .iter()
                .map(move |declaration| (path.as_str(), declaration))
        })
    }

    pub(crate) fn has_value_name(&self, path: &str, name: &str) -> bool {
        self.value_names_by_path
            .get(path)
            .is_some_and(|names| names.contains(name))
    }

    pub(crate) fn has_visible_flat_constant(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        name: &str,
    ) -> bool {
        !self.flat_constants(source_path, name).is_empty()
            || graph
                .dependency_closure(source_path)
                .into_iter()
                .filter(|path| path != source_path)
                .any(|path| !self.flat_constants(&path, name).is_empty())
    }

    pub(crate) fn has_visible_group_namespace(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        name: &str,
    ) -> bool {
        !self.groups(source_path, name).is_empty()
            || graph
                .dependency_closure(source_path)
                .into_iter()
                .filter(|path| path != source_path)
                .any(|path| !self.groups(&path, name).is_empty())
    }

    pub(crate) fn has_visible_group_member_name(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        name: &str,
    ) -> bool {
        self.all_groups().any(|group| {
            (group.source_path == source_path
                || graph
                    .dependency_closure(source_path)
                    .contains(&group.source_path))
                && group.members.iter().any(|member| member.id.member == name)
        })
    }

    pub(crate) fn imported_alias_shadow_error(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        identifier: &str,
    ) -> Option<String> {
        let alias = identifier.split('.').next()?;
        graph.imported_alias_target(source_path, alias)?;
        self.has_visible_value_name(graph, source_path, alias)
            .then(|| format!("value binding '{alias}' shadows the imported module alias"))
    }

    fn constant_declaration(&self, id: &CompileTimeValueId) -> Option<&IndexedConstantDeclaration> {
        match &id.owner {
            CompileTimeValueOwner::Flat => self
                .flat_constants(&id.source_path, &id.member)
                .into_iter()
                .find(|declaration| declaration.id == *id),
            CompileTimeValueOwner::Group(group) => self
                .groups(&id.source_path, group)
                .into_iter()
                .flat_map(|owner| owner.members.iter())
                .find(|declaration| declaration.id == *id),
            CompileTimeValueOwner::Enum(_) => None,
        }
    }

    pub(crate) fn evaluate_generic_i32(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        expression: &str,
        generic_values: &BTreeMap<String, i32>,
    ) -> Result<i32, String> {
        self.evaluate_generic_i32_with_state(
            graph,
            source_path,
            expression,
            generic_values,
            &mut GenericI32EvaluationState::default(),
        )
    }

    pub(crate) fn evaluate_generic_i32_with_state(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        expression: &str,
        generic_values: &BTreeMap<String, i32>,
        state: &mut GenericI32EvaluationState,
    ) -> Result<i32, String> {
        // Each source expression has its own work ceiling. The reusable state
        // keeps declaration memoization and cycle tracking, while only the
        // declaration batch shares one cumulative prefix-step budget.
        state.steps.set(0);
        GenericI32Evaluator {
            index: self,
            graph,
            state,
        }
        .evaluate_expression(source_path, expression, generic_values)
    }

    pub(crate) fn evaluate_i32_declaration_with_state(
        &self,
        graph: &ModuleGraph,
        declaration_id: &CompileTimeValueId,
        state: &mut GenericI32EvaluationState,
    ) -> Result<i32, String> {
        if matches!(declaration_id.owner, CompileTimeValueOwner::Enum(_)) {
            return Err(format!(
                "compile-time value '{}' is not i32",
                display_value_id(declaration_id)
            ));
        }
        GenericI32Evaluator {
            index: self,
            graph,
            state,
        }
        .evaluate_id(declaration_id.clone())
    }

    /// Resolve a constant-like path without flattening its source/module
    /// identity. Runtime lexical roots are supplied by the caller because
    /// function scopes are not part of this source-level index.
    pub(crate) fn resolve_compile_time_value(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        identifier: &str,
        lexical_root_shadowed: bool,
    ) -> Result<Option<CompileTimeValueId>, String> {
        let parts = identifier.split('.').collect::<Vec<_>>();
        match parts.as_slice() {
            [name] => {
                let local = self.flat_constants(source_path, name);
                if !local.is_empty() {
                    if local.len() > 1 {
                        return Err(ambiguous_flat_constant_error(
                            identifier,
                            source_path,
                            local.iter().map(|item| item.id.source_path.as_str()),
                        ));
                    }
                    return Ok(local.first().map(|item| item.id.clone()));
                }
                let visible = graph
                    .dependency_closure(source_path)
                    .into_iter()
                    .filter(|path| path != source_path)
                    .flat_map(|path| self.flat_constants(&path, name))
                    .collect::<Vec<_>>();
                if visible.len() > 1 {
                    return Err(ambiguous_flat_constant_error(
                        identifier,
                        source_path,
                        visible.iter().map(|item| item.id.source_path.as_str()),
                    ));
                }
                Ok(visible.first().map(|item| item.id.clone()))
            }
            [root, member] => {
                if lexical_root_shadowed || self.has_visible_value_name(graph, source_path, root) {
                    return Ok(None);
                }
                self.resolve_two_part(graph, source_path, root, member, identifier)
            }
            [alias, owner, member] => {
                if lexical_root_shadowed || self.has_visible_value_name(graph, source_path, alias) {
                    return Ok(None);
                }
                let Some(target) = graph.imported_alias_target(source_path, alias) else {
                    return Ok(None);
                };
                self.resolve_exact_qualified(graph, target, owner, member, alias, identifier)
                    .map(Some)
            }
            _ => Ok(None),
        }
    }

    fn resolve_two_part(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        root: &str,
        member: &str,
        spelling: &str,
    ) -> Result<Option<CompileTimeValueId>, String> {
        let mut candidates = Vec::new();
        let mut missing_owner_error = None;

        if let Some(target) = self.visible_alias_target(graph, source_path, root) {
            let flat = self.flat_constants(target, member);
            if flat.len() > 1 {
                return Err(format!(
                    "ambiguous compile-time value '{spelling}' in module '{root}'"
                ));
            }
            if flat.is_empty() {
                missing_owner_error = Some(format!(
                    "unknown constant '{member}' in imported module '{root}'"
                ));
            }
            candidates.extend(flat.into_iter().map(|item| item.id.clone()));
        }

        if let Some(group) = self.visible_group_owner(graph, source_path, root)? {
            let members = group
                .members
                .iter()
                .filter(|item| item.id.member == member)
                .collect::<Vec<_>>();
            match members.as_slice() {
                [member] => candidates.push(member.id.clone()),
                [] => {
                    missing_owner_error = Some(format!(
                        "unknown constant group member '{root}.{member}' in '{}'",
                        group.source_path
                    ));
                }
                _ => return Err(format!("ambiguous constant group member '{root}.{member}'")),
            }
        }

        if let Some(enumeration) = self.visible_enum_owner(graph, source_path, root)? {
            let variants = enumeration
                .variants
                .iter()
                .filter(|variant| variant.name == member)
                .collect::<Vec<_>>();
            match variants.as_slice() {
                [_] => candidates.push(CompileTimeValueId {
                    source_path: self
                        .enum_source_path(graph, source_path, root)?
                        .ok_or_else(|| format!("missing enum source for '{root}'"))?,
                    owner: CompileTimeValueOwner::Enum(root.to_string()),
                    member: member.to_string(),
                }),
                [] if missing_owner_error.is_none() => {
                    missing_owner_error = Some(format!("unknown enum variant '{root}.{member}'"));
                }
                [] => {}
                _ => return Err(format!("ambiguous enum variant '{root}.{member}'")),
            }
        }

        if candidates.is_empty() {
            if let Some(error) = missing_owner_error {
                return Err(error);
            }
        }
        unique_identities(candidates.iter(), spelling)
    }

    fn resolve_exact_qualified(
        &self,
        _graph: &ModuleGraph,
        target: &str,
        owner: &str,
        member: &str,
        alias: &str,
        spelling: &str,
    ) -> Result<CompileTimeValueId, String> {
        if let Some(group) = unique_group(self.groups(target, owner), owner, target)? {
            let matches = group
                .members
                .iter()
                .filter(|item| item.id.member == member)
                .collect::<Vec<_>>();
            return match matches.as_slice() {
                [declaration] => Ok(declaration.id.clone()),
                [] => Err(format!(
                    "unknown constant group member '{owner}.{member}' in module '{alias}'"
                )),
                _ => Err(format!("ambiguous constant group member '{spelling}'")),
            };
        }
        if let Some(enumeration) = unique_enum(self.enums(target, owner), owner, target)? {
            if enumeration
                .variants
                .iter()
                .any(|variant| variant.name == member)
            {
                return Ok(CompileTimeValueId {
                    source_path: target.to_string(),
                    owner: CompileTimeValueOwner::Enum(owner.to_string()),
                    member: member.to_string(),
                });
            }
            return Err(format!(
                "unknown enum variant '{owner}.{member}' in module '{alias}'"
            ));
        }
        Err(format!(
            "unknown compile-time owner '{owner}' in module '{alias}'"
        ))
    }

    fn visible_alias_target<'a>(
        &self,
        graph: &'a ModuleGraph,
        source_path: &str,
        alias: &str,
    ) -> Option<&'a str> {
        graph
            .module(source_path)
            .filter(|module| module.alias == alias)
            .map(|module| module.path.as_str())
            .or_else(|| graph.imported_alias_target(source_path, alias))
    }

    fn visible_group_owner<'a>(
        &'a self,
        graph: &ModuleGraph,
        source_path: &str,
        name: &str,
    ) -> Result<Option<&'a IndexedConstantGroup>, String> {
        unique_group_owner(
            self.groups(source_path, name),
            graph
                .dependency_closure(source_path)
                .into_iter()
                .filter(|path| path != source_path)
                .flat_map(|path| self.groups(&path, name)),
            name,
            source_path,
        )
    }

    fn visible_enum_owner<'a>(
        &'a self,
        graph: &ModuleGraph,
        source_path: &str,
        name: &str,
    ) -> Result<Option<&'a ParsedEnumDefinition>, String> {
        let local = self.enums(source_path, name);
        if !local.is_empty() {
            return unique_enum(local, name, source_path);
        }
        let visible = graph
            .dependency_closure(source_path)
            .into_iter()
            .filter(|path| path != source_path)
            .flat_map(|path| self.enums(&path, name))
            .collect::<Vec<_>>();
        unique_enum(visible, name, source_path)
    }

    fn enum_source_path(
        &self,
        graph: &ModuleGraph,
        source_path: &str,
        name: &str,
    ) -> Result<Option<String>, String> {
        if !self.enums(source_path, name).is_empty() {
            return Ok(Some(source_path.to_string()));
        }
        let paths = graph
            .dependency_closure(source_path)
            .into_iter()
            .filter(|path| path != source_path && !self.enums(&path, name).is_empty())
            .collect::<Vec<_>>();
        match paths.as_slice() {
            [] => Ok(None),
            [path] => Ok(Some(path.clone())),
            _ => Err(format!(
                "ambiguous compile-time enum '{name}' visible from '{source_path}'"
            )),
        }
    }

    fn has_visible_value_name(&self, graph: &ModuleGraph, source_path: &str, name: &str) -> bool {
        self.has_value_name(source_path, name)
            || graph
                .dependency_closure(source_path)
                .into_iter()
                .filter(|path| path != source_path)
                .any(|path| self.has_value_name(&path, name))
    }
}

pub(crate) fn validate_group_declaration_names(
    files: &[SourceFile],
) -> Result<(), (String, Range<usize>, String, String)> {
    let mut ordered_files = files.iter().collect::<Vec<_>>();
    ordered_files.sort_by(|left, right| left.path.cmp(&right.path));
    for file in ordered_files {
        let layout = parse_top_level_type_layout(&file.original_content).map_err(|message| {
            (
                file.path.clone(),
                0..file.original_content.len().max(1),
                String::new(),
                message,
            )
        })?;
        let other_top_level_names = layout
            .constants
            .iter()
            .map(|declaration| declaration.name.as_str())
            .chain(
                layout
                    .enums
                    .iter()
                    .map(|declaration| declaration.name.as_str()),
            )
            .chain(
                layout
                    .structs
                    .iter()
                    .map(|declaration| declaration.name.as_str()),
            )
            .collect::<BTreeSet<_>>();
        let mut group_names = BTreeSet::new();
        for group in &layout.const_groups {
            if other_top_level_names.contains(group.name.as_str())
                || !group_names.insert(group.name.as_str())
            {
                return Err((
                    file.path.clone(),
                    group.name_range.clone(),
                    group.name.clone(),
                    format!("duplicate top-level declaration '{}'", group.name),
                ));
            }
            let mut member_names = BTreeSet::new();
            for member in &group.members {
                if !member_names.insert(member.name.as_str()) {
                    return Err((
                        file.path.clone(),
                        member.name_range.clone(),
                        member.name.clone(),
                        format!(
                            "duplicate constant group member '{}.{}'",
                            group.name, member.name
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn unique_identities<'a>(
    ids: impl IntoIterator<Item = &'a CompileTimeValueId>,
    spelling: &str,
) -> Result<Option<CompileTimeValueId>, String> {
    let mut unique = ids.into_iter().cloned().collect::<BTreeSet<_>>();
    match unique.len() {
        0 => Ok(None),
        1 => Ok(unique.pop_first()),
        _ => Err(format!("ambiguous compile-time value '{spelling}'")),
    }
}

fn ambiguous_flat_constant_error<'a>(
    name: &str,
    source_path: &str,
    paths: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    format!(
        "ambiguous compile-time constant '{name}' from '{source_path}': {}",
        paths.join(", ")
    )
}

fn unique_group<'a>(
    groups: Vec<&'a IndexedConstantGroup>,
    name: &str,
    source_path: &str,
) -> Result<Option<&'a IndexedConstantGroup>, String> {
    match groups.as_slice() {
        [] => Ok(None),
        [group] => Ok(Some(*group)),
        _ => Err(format!(
            "ambiguous constant group '{name}' in '{source_path}'"
        )),
    }
}

fn unique_group_owner<'a>(
    local: Vec<&'a IndexedConstantGroup>,
    visible: impl IntoIterator<Item = &'a IndexedConstantGroup>,
    name: &str,
    source_path: &str,
) -> Result<Option<&'a IndexedConstantGroup>, String> {
    if !local.is_empty() {
        return unique_group(local, name, source_path);
    }
    let visible = visible.into_iter().collect::<Vec<_>>();
    match visible.as_slice() {
        [] => Ok(None),
        [group] => Ok(Some(*group)),
        _ => Err(format!(
            "ambiguous constant group '{name}' visible from '{source_path}'"
        )),
    }
}

fn unique_enum<'a>(
    enums: Vec<&'a ParsedEnumDefinition>,
    name: &str,
    source_path: &str,
) -> Result<Option<&'a ParsedEnumDefinition>, String> {
    match enums.as_slice() {
        [] => Ok(None),
        [enumeration] => Ok(Some(*enumeration)),
        _ => Err(format!("ambiguous enum '{name}' in '{source_path}'")),
    }
}

fn indexed_flat_constant(
    path: &str,
    constant: ParsedConstDefinition,
) -> IndexedConstantDeclaration {
    IndexedConstantDeclaration {
        id: CompileTimeValueId {
            source_path: path.to_string(),
            owner: CompileTimeValueOwner::Flat,
            member: constant.name,
        },
        name_range: None,
        value_range: constant.value_range,
        type_name: constant.type_name,
        value_text: constant.value_text,
    }
}

fn indexed_group(path: &str, group: ParsedConstGroupDefinition) -> IndexedConstantGroup {
    IndexedConstantGroup {
        source_path: path.to_string(),
        name: group.name.clone(),
        members: group
            .members
            .into_iter()
            .map(|member| indexed_group_member(path, &group.name, member))
            .collect(),
    }
}

fn indexed_group_member(
    path: &str,
    group_name: &str,
    member: ParsedConstGroupMemberDefinition,
) -> IndexedConstantDeclaration {
    IndexedConstantDeclaration {
        id: CompileTimeValueId {
            source_path: path.to_string(),
            owner: CompileTimeValueOwner::Group(group_name.to_string()),
            member: member.name,
        },
        name_range: Some(member.name_range),
        value_range: member.value_range,
        type_name: member.type_name,
        value_text: member.value_text,
    }
}

struct GenericI32Evaluator<'index, 'graph, 'state> {
    index: &'index CompileTimeDeclarationIndex,
    graph: &'graph ModuleGraph,
    state: &'state mut GenericI32EvaluationState,
}

impl GenericI32Evaluator<'_, '_, '_> {
    fn evaluate_expression(
        &mut self,
        source_path: &str,
        expression: &str,
        generic_values: &BTreeMap<String, i32>,
    ) -> Result<i32, String> {
        let steps = Rc::clone(&self.state.steps);
        let result = evaluate_i32_expression_with_resolver(
            expression,
            generic_values,
            &steps,
            &mut |identifier| self.resolve_identifier(source_path, identifier, generic_values),
        );
        result
    }

    fn resolve_identifier(
        &mut self,
        source_path: &str,
        identifier: &str,
        generic_values: &BTreeMap<String, i32>,
    ) -> Result<i32, String> {
        if !identifier.contains('.')
            && !self
                .index
                .has_visible_flat_constant(self.graph, source_path, identifier)
            && self
                .index
                .has_visible_group_member_name(self.graph, source_path, identifier)
        {
            return Err(format!(
                "constant group member '{identifier}' must be qualified by its group name"
            ));
        }
        let root = identifier.split('.').next().unwrap_or(identifier);
        let id = self
            .index
            .resolve_compile_time_value(
                self.graph,
                source_path,
                identifier,
                generic_values.contains_key(root),
            )?
            .ok_or_else(|| format!("unknown compile-time i32 value '{identifier}'"))?;
        self.evaluate_id(id)
    }

    fn evaluate_id(&mut self, id: CompileTimeValueId) -> Result<i32, String> {
        if let Some(value) = self.state.cache.get(&id) {
            return Ok(*value);
        }
        if let Some(cycle_start) = self.state.stack.iter().position(|entry| entry == &id) {
            let mut cycle = self.state.stack[cycle_start..]
                .iter()
                .map(display_value_id)
                .collect::<Vec<_>>();
            cycle.push(display_value_id(&id));
            return Err(format!("constant reference cycle: {}", cycle.join(" -> ")));
        }
        if self.state.stack.len() >= MAX_CONSTANT_DEPENDENCY_DEPTH {
            return Err(format!(
                "constant dependency depth exceeded (maximum {MAX_CONSTANT_DEPENDENCY_DEPTH})"
            ));
        }
        if matches!(id.owner, CompileTimeValueOwner::Enum(_)) {
            return Err(format!(
                "compile-time value '{}' is not i32",
                display_value_id(&id)
            ));
        }
        let declaration = self
            .index
            .constant_declaration(&id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "missing indexed constant '{}'.{}",
                    display_value_id(&id),
                    id.member
                )
            })?;
        if declaration.type_name.trim() != "i32" {
            return Err(format!(
                "compile-time value '{}' is not i32",
                display_value_id(&id)
            ));
        }
        let path = declaration.id.source_path.clone();
        let value_text = declaration.value_text;
        self.state.stack.push(id.clone());
        // Declaration initializers are evaluated in their defining source
        // scope, without the caller's generic parameter environment.
        let value = self.evaluate_expression(&path, &value_text, &BTreeMap::new());
        self.state.stack.pop();
        let value = value?;
        self.state.cache.insert(id, value);
        Ok(value)
    }
}

pub(crate) fn evaluate_i32_expression_with_resolver(
    source: &str,
    generic_values: &BTreeMap<String, i32>,
    steps: &Cell<usize>,
    resolver: &mut dyn FnMut(&str) -> Result<i32, String>,
) -> Result<i32, String> {
    let mut parser = ConstantExpressionParser {
        tokens: tokenize_constant_expression(source)?,
        cursor: 0,
        generic_values,
        steps,
        resolver,
    };
    let value = parser.parse_expression(0)?;
    if parser.cursor != parser.tokens.len() {
        return Err(format!(
            "unexpected token in compile-time expression '{source}'"
        ));
    }
    Ok(value)
}

struct ConstantExpressionParser<'a> {
    tokens: Vec<ConstantToken>,
    cursor: usize,
    generic_values: &'a BTreeMap<String, i32>,
    steps: &'a Cell<usize>,
    resolver: &'a mut dyn FnMut(&str) -> Result<i32, String>,
}

impl ConstantExpressionParser<'_> {
    fn peek(&self) -> Option<&ConstantToken> {
        self.tokens.get(self.cursor)
    }

    fn parse_expression(&mut self, min_precedence: u8) -> Result<i32, String> {
        let mut lhs = self.parse_prefix()?;
        loop {
            let Some(ConstantToken::Operator(operator)) = self.peek() else {
                break;
            };
            let precedence = operator.precedence();
            if precedence < min_precedence {
                break;
            }
            let operator = *operator;
            self.cursor += 1;
            let rhs = self.parse_expression(precedence + 1)?;
            lhs = checked_i32_binary(lhs, operator, rhs)?;
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<i32, String> {
        let steps = self.steps.get().saturating_add(1);
        self.steps.set(steps);
        ensure_constant_evaluation_steps(steps)?;
        let token = self
            .tokens
            .get(self.cursor)
            .cloned()
            .ok_or_else(|| "compile-time expression ended unexpectedly".to_string())?;
        self.cursor += 1;
        match token {
            ConstantToken::Integer(text) => {
                let value = text
                    .parse::<i64>()
                    .map_err(|error| format!("invalid compile-time integer '{text}': {error}"))?;
                i32::try_from(value)
                    .map_err(|_| format!("compile-time integer '{text}' is outside i32 range"))
            }
            ConstantToken::Identifier(name) => self
                .generic_values
                .get(&name)
                .copied()
                .map(Ok)
                .unwrap_or_else(|| (self.resolver)(&name)),
            ConstantToken::Operator(ExprBinaryOp::Add) => self.parse_prefix(),
            ConstantToken::Operator(ExprBinaryOp::Subtract) => {
                if let Some(ConstantToken::Integer(text)) = self.tokens.get(self.cursor).cloned() {
                    self.cursor += 1;
                    let value = text.parse::<i64>().map_err(|error| {
                        format!("invalid compile-time integer '{text}': {error}")
                    })?;
                    return i32::try_from(
                        value
                            .checked_neg()
                            .ok_or_else(|| "compile-time negation overflowed i64".to_string())?,
                    )
                    .map_err(|_| format!("compile-time integer '-{text}' is outside i32 range"));
                }
                self.parse_prefix()?
                    .checked_neg()
                    .ok_or_else(|| "compile-time negation overflowed i32".to_string())
            }
            ConstantToken::Unary(ExprUnaryOp::BitwiseNot) => Ok(!self.parse_prefix()?),
            ConstantToken::Unary(ExprUnaryOp::Negate) => self
                .parse_prefix()?
                .checked_neg()
                .ok_or_else(|| "compile-time negation overflowed i32".to_string()),
            ConstantToken::Open => {
                let value = self.parse_expression(0)?;
                match self.tokens.get(self.cursor) {
                    Some(ConstantToken::Close) => {
                        self.cursor += 1;
                        Ok(value)
                    }
                    _ => Err("missing ')' in compile-time expression".to_string()),
                }
            }
            other => Err(format!(
                "unexpected token {other:?} in compile-time expression"
            )),
        }
    }
}

pub(crate) fn tokenize_constant_expression(source: &str) -> Result<Vec<ConstantToken>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if bytes[cursor].is_ascii_digit() {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            tokens.push(ConstantToken::Integer(source[start..cursor].to_string()));
            continue;
        }
        if is_constant_identifier_start(bytes[cursor]) {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_constant_identifier_char(bytes[cursor]) {
                cursor += 1;
            }
            while cursor + 1 < bytes.len()
                && bytes[cursor] == b'.'
                && is_constant_identifier_start(bytes[cursor + 1])
            {
                cursor += 2;
                while cursor < bytes.len() && is_constant_identifier_char(bytes[cursor]) {
                    cursor += 1;
                }
            }
            tokens.push(ConstantToken::Identifier(source[start..cursor].to_string()));
            continue;
        }
        let token = match bytes[cursor] {
            b'<' if cursor + 1 < bytes.len() && bytes[cursor + 1] == b'<' => {
                cursor += 2;
                tokens.push(ConstantToken::Operator(ExprBinaryOp::ShiftLeft));
                continue;
            }
            b'>' if cursor + 1 < bytes.len() && bytes[cursor + 1] == b'>' => {
                cursor += 2;
                tokens.push(ConstantToken::Operator(ExprBinaryOp::ShiftRight));
                continue;
            }
            b'+' => ConstantToken::Operator(ExprBinaryOp::Add),
            b'-' => ConstantToken::Operator(ExprBinaryOp::Subtract),
            b'*' => ConstantToken::Operator(ExprBinaryOp::Multiply),
            b'/' => ConstantToken::Operator(ExprBinaryOp::Divide),
            b'%' => ConstantToken::Operator(ExprBinaryOp::Remainder),
            b'&' => ConstantToken::Operator(ExprBinaryOp::BitAnd),
            b'^' => ConstantToken::Operator(ExprBinaryOp::BitXor),
            b'|' => ConstantToken::Operator(ExprBinaryOp::BitOr),
            b'~' => ConstantToken::Unary(ExprUnaryOp::BitwiseNot),
            b'(' => ConstantToken::Open,
            b')' => ConstantToken::Close,
            other => {
                return Err(format!(
                    "unsupported token '{}' in compile-time expression",
                    other as char
                ))
            }
        };
        tokens.push(token);
        cursor += 1;
    }
    Ok(tokens)
}

fn is_constant_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_constant_identifier_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) fn ensure_constant_evaluation_steps(steps: usize) -> Result<(), String> {
    if steps > MAX_CONSTANT_EVALUATION_STEPS {
        return Err(format!(
            "compile-time expression evaluation exceeded {MAX_CONSTANT_EVALUATION_STEPS} steps"
        ));
    }
    Ok(())
}

pub(crate) fn checked_i32_binary(
    lhs: i32,
    operator: ExprBinaryOp,
    rhs: i32,
) -> Result<i32, String> {
    let checked = match operator {
        ExprBinaryOp::Add => lhs.checked_add(rhs),
        ExprBinaryOp::Subtract => lhs.checked_sub(rhs),
        ExprBinaryOp::Multiply => lhs.checked_mul(rhs),
        ExprBinaryOp::Divide => lhs.checked_div(rhs),
        ExprBinaryOp::Remainder => lhs.checked_rem(rhs),
        ExprBinaryOp::ShiftLeft => Some(lhs.wrapping_shl((rhs as u32) & 31)),
        ExprBinaryOp::ShiftRight => Some(lhs.wrapping_shr((rhs as u32) & 31)),
        ExprBinaryOp::BitAnd => Some(lhs & rhs),
        ExprBinaryOp::BitXor => Some(lhs ^ rhs),
        ExprBinaryOp::BitOr => Some(lhs | rhs),
    };
    checked.ok_or_else(|| {
        format!(
            "checked compile-time operation '{}' overflowed or divided by zero",
            operator.spelling()
        )
    })
}

fn display_value_id(id: &CompileTimeValueId) -> String {
    let owner = match &id.owner {
        CompileTimeValueOwner::Flat => String::new(),
        CompileTimeValueOwner::Group(group) | CompileTimeValueOwner::Enum(group) => {
            format!("{group}.")
        }
    };
    format!("{}::{}{}", id.source_path, owner, id.member)
}
