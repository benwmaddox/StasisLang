//! Frontend elaboration for compile-time generic parameters.
//!
//! This first slice lowers generic declarations to ordinary concrete
//! declarations before the existing indexer, body parser, and backends run.
//! Keeping this pass in the frontend gives every backend the same concrete
//! fixed-layout input.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::compiler::SourceFile;
use crate::frontend::indexer::hash_text;
use crate::frontend::lexer::{lex, TokenKind};
use crate::frontend::module_graph::ModuleGraph;
use crate::frontend::parser::{
    parse_top_level_extern_functions, parse_top_level_functions,
    parse_top_level_struct_definitions, parse_top_level_type_layout, ParsedFunctionSignature,
    ParsedGenericParameter, ParsedGenericParameterKind,
};

const MAX_SPECIALIZATIONS: usize = 4096;
const MAX_INSTANTIATION_DEPTH: usize = 128;
const MAX_CONSTANT_EVALUATION_STEPS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ConcreteArgument {
    Type(String),
    I32(i32),
}

#[derive(Debug, Clone)]
struct GenericStructDefinition {
    identity: String,
    file_index: usize,
    path: String,
    module_alias: String,
    name: String,
    parameters: Vec<ParsedGenericParameter>,
    fields: Vec<crate::frontend::parser::ParsedField>,
    definition_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct GenericFunctionDefinition {
    file_index: usize,
    path: String,
    module_alias: String,
    name: String,
    parameters: Vec<ParsedGenericParameter>,
    signature: ParsedFunctionSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct StructSpecializationKey {
    definition: String,
    arguments: Vec<ConcreteArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct FunctionSpecializationKey {
    definition: usize,
    arguments: Vec<ConcreteArgument>,
}

#[derive(Debug, Clone)]
struct StructSpecialization {
    generated_name: String,
}

#[derive(Debug, Clone)]
struct FunctionSpecialization {
    definition: usize,
    source: String,
    param_type_names: Vec<String>,
}

#[derive(Debug, Clone)]
struct RawFile {
    path: String,
    source: String,
}

#[derive(Debug, Clone)]
struct ConstantDefinition {
    path: String,
    name: String,
    type_name: String,
    value_text: String,
}

#[derive(Debug, Clone)]
struct ConcretePathDefinition {
    file_index: usize,
    path: String,
    type_name: String,
}

#[derive(Debug, Clone)]
struct OrdinaryStructDefinition {
    file_index: usize,
    path: String,
    name: String,
    fields: Vec<crate::frontend::parser::ParsedField>,
}

#[derive(Debug, Clone, Default)]
struct GenericEnvironment {
    values: BTreeMap<String, i32>,
    types: BTreeMap<String, String>,
    module_alias: Option<String>,
    source_path: Option<String>,
}

impl GenericEnvironment {
    fn from_parameters(
        parameters: &[ParsedGenericParameter],
        arguments: &[ConcreteArgument],
    ) -> Result<Self, String> {
        if parameters.len() != arguments.len() {
            return Err(format!(
                "generic argument arity mismatch: expected {}, got {}",
                parameters.len(),
                arguments.len()
            ));
        }
        let mut environment = Self::default();
        for (parameter, argument) in parameters.iter().zip(arguments) {
            match (&parameter.kind, argument) {
                (ParsedGenericParameterKind::I32, ConcreteArgument::I32(value)) => {
                    environment.values.insert(parameter.name.clone(), *value);
                }
                (ParsedGenericParameterKind::Type, ConcreteArgument::Type(value)) => {
                    environment
                        .types
                        .insert(parameter.name.clone(), value.clone());
                }
                (ParsedGenericParameterKind::I32, ConcreteArgument::Type(value)) => {
                    return Err(format!(
                        "generic parameter '{}' expects an i32 argument, got type '{}'",
                        parameter.name, value
                    ));
                }
                (ParsedGenericParameterKind::Type, ConcreteArgument::I32(value)) => {
                    return Err(format!(
                        "generic parameter '{}' expects a type argument, got value {}",
                        parameter.name, value
                    ));
                }
            }
        }
        Ok(environment)
    }
}

struct Expansion {
    files: Vec<RawFile>,
    module_graph: ModuleGraph,
    generic_structs: BTreeMap<String, GenericStructDefinition>,
    generic_structs_by_name: BTreeMap<String, Vec<String>>,
    generic_functions: Vec<GenericFunctionDefinition>,
    generic_functions_by_name: BTreeMap<String, Vec<usize>>,
    ordinary_function_names: BTreeMap<String, Vec<(usize, String, Vec<String>)>>,
    constant_definitions: Vec<ConstantDefinition>,
    constant_values: BTreeMap<String, i32>,
    struct_specializations: BTreeMap<StructSpecializationKey, StructSpecialization>,
    struct_work: VecDeque<(StructSpecializationKey, usize)>,
    function_specializations: BTreeMap<FunctionSpecializationKey, FunctionSpecialization>,
    function_work: VecDeque<(FunctionSpecializationKey, usize)>,
    ordinary_structs_by_name: BTreeMap<String, Vec<OrdinaryStructDefinition>>,
    known_type_files_by_name: BTreeMap<String, BTreeSet<usize>>,
    concrete_path_definitions: Vec<ConcretePathDefinition>,
    concrete_paths_by_file: BTreeMap<usize, BTreeMap<String, String>>,
    active_depth: Option<usize>,
}

pub(crate) struct ExpansionError {
    pub(crate) path: Option<String>,
    pub(crate) message: String,
}

impl ExpansionError {
    fn without_path(message: String) -> Self {
        Self {
            path: None,
            message,
        }
    }

    fn for_file(path: String, message: String) -> Self {
        Self {
            path: Some(path),
            message,
        }
    }
}

impl From<String> for ExpansionError {
    fn from(message: String) -> Self {
        Self::without_path(message)
    }
}

pub(crate) fn expand_sources(
    files: &mut [SourceFile],
    module_graph: &ModuleGraph,
) -> Result<(), ExpansionError> {
    let raw_files = files
        .iter()
        .map(|file| RawFile {
            path: file.path.clone(),
            source: file.original_content.clone(),
        })
        .collect::<Vec<_>>();
    let mut expansion = Expansion::new(raw_files, module_graph.clone())?;
    expansion.reject_explicit_generic_calls()?;
    if expansion.generic_structs.is_empty() && expansion.generic_functions.is_empty() {
        for file in files {
            file.content = file.original_content.clone();
            file.hash = hash_text(&file.content);
        }
        return Ok(());
    }
    expansion.populate_concrete_paths()?;
    expansion.seed_direct_uses()?;
    expansion.process_worklist()?;
    expansion.write_sources(files)
}

impl Expansion {
    fn environment_for_file(&self, file_index: usize) -> GenericEnvironment {
        let path = self.files[file_index].path.clone();
        GenericEnvironment {
            module_alias: self
                .module_graph
                .module(&path)
                .map(|module| module.alias.clone())
                .or_else(|| Some(module_alias_for_path(&path))),
            source_path: Some(path),
            ..GenericEnvironment::default()
        }
    }

    fn imported_alias_target(&self, file_index: usize, alias: &str) -> Option<&str> {
        self.module_graph
            .imported_alias_target(&self.files[file_index].path, alias)
    }

    fn visible_paths(&self, file_index: usize) -> BTreeSet<String> {
        self.module_graph
            .dependency_closure(&self.files[file_index].path)
    }

    fn visible_constant_definition(
        &self,
        name: &str,
        source_path: &str,
    ) -> Result<Option<&ConstantDefinition>, String> {
        let local = self
            .constant_definitions
            .iter()
            .filter(|definition| {
                definition.path == source_path
                    && definition.name == name
                    && definition.type_name.trim() == "i32"
            })
            .collect::<Vec<_>>();
        let matches = if local.is_empty() {
            let visible = self.module_graph.dependency_closure(source_path);
            self.constant_definitions
                .iter()
                .filter(|definition| {
                    visible.contains(&definition.path)
                        && definition.name == name
                        && definition.type_name.trim() == "i32"
                })
                .collect::<Vec<_>>()
        } else {
            local
        };
        match matches.as_slice() {
            [] => Ok(None),
            [definition] => Ok(Some(*definition)),
            _ => Err(format!(
                "ambiguous compile-time constant '{}' from '{}': {}",
                name,
                source_path,
                sorted_paths(matches.iter().map(|definition| definition.path.as_str()))
            )),
        }
    }

    fn visible_constant_value(
        &self,
        name: &str,
        environment: &GenericEnvironment,
    ) -> Result<Option<i32>, String> {
        let Some(source_path) = environment.source_path.as_deref() else {
            return Ok(None);
        };
        let Some(definition) = self.visible_constant_definition(name, source_path)? else {
            return Ok(None);
        };
        Ok(self
            .constant_values
            .get(&constant_definition_identity(definition))
            .copied())
    }

    fn evaluate_i32_expression(
        &self,
        source: &str,
        environment: &GenericEnvironment,
    ) -> Result<i32, String> {
        let mut constants = BTreeMap::new();
        for token in tokenize_constant_expression(source)? {
            let ConstantToken::Identifier(identifier) = token else {
                continue;
            };
            if environment.values.contains_key(&identifier) {
                continue;
            }
            if let Some(value) = self.visible_constant_value(&identifier, environment)? {
                constants.insert(identifier, value);
            }
        }
        evaluate_i32_expression(source, environment, &constants)
    }

    fn local_constant_values(&self, file_index: usize) -> BTreeMap<String, i32> {
        let path = &self.files[file_index].path;
        self.constant_definitions
            .iter()
            .filter(|definition| &definition.path == path)
            .filter_map(|definition| {
                self.constant_values
                    .get(&constant_definition_identity(definition))
                    .copied()
                    .map(|value| (definition.name.clone(), value))
            })
            .collect()
    }

    fn constant_rewrites_for_source(
        &self,
        file_index: usize,
        source: &str,
    ) -> Result<BTreeMap<String, (String, i32)>, String> {
        let duplicate_names = self
            .constant_definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .filter(|name| {
                self.constant_definitions
                    .iter()
                    .filter(|definition| definition.name == **name)
                    .map(|definition| definition.path.as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
                    > 1
            })
            .collect::<BTreeSet<_>>();
        let names = constant_identifier_ranges(source, &duplicate_names)?
            .into_iter()
            .map(|(start, end)| &source[start..end])
            .collect::<BTreeSet<_>>();
        let mut rewrites = BTreeMap::new();
        for name in names {
            let Some(definition) =
                self.visible_constant_definition(name, &self.files[file_index].path)?
            else {
                continue;
            };
            let identity = constant_definition_identity(definition);
            let Some(value) = self.constant_values.get(&identity).copied() else {
                continue;
            };
            let generated_name = format!("__stasis_const_{:016x}", hash_text(&identity));
            rewrites.insert(name.to_string(), (generated_name, value));
        }
        Ok(rewrites)
    }

    fn lookup_ordinary_struct_for_environment(
        &self,
        name: &str,
        environment: &GenericEnvironment,
    ) -> Result<Option<&OrdinaryStructDefinition>, String> {
        let name = name.trim();
        if name.starts_with("__module_type_") {
            let generated_matches = self
                .ordinary_structs_by_name
                .values()
                .flatten()
                .filter(|definition| ordinary_struct_generated_name(definition) == name)
                .collect::<Vec<_>>();
            match generated_matches.as_slice() {
                [definition] => return Ok(Some(*definition)),
                [] => {}
                _ => return Err(format!("ambiguous generated struct type '{name}'")),
            }
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        let Some(candidates) = self.ordinary_structs_by_name.get(short) else {
            return Ok(None);
        };
        let Some(caller_path) = environment.source_path.as_deref() else {
            return Ok(None);
        };
        let matches = if let Some((alias, _)) = name.rsplit_once('.') {
            let Some(target) = self.module_graph.imported_alias_target(caller_path, alias) else {
                return Ok(None);
            };
            candidates
                .iter()
                .filter(|definition| definition.path == target)
                .collect::<Vec<_>>()
        } else {
            let local = candidates
                .iter()
                .filter(|definition| definition.path == caller_path)
                .collect::<Vec<_>>();
            if !local.is_empty() {
                local
            } else {
                let visible = self.module_graph.dependency_closure(caller_path);
                candidates
                    .iter()
                    .filter(|definition| visible.contains(&definition.path))
                    .collect::<Vec<_>>()
            }
        };
        match matches.as_slice() {
            [] => Ok(None),
            [definition] => Ok(Some(*definition)),
            _ => Err(format!(
                "ambiguous struct type '{}' from '{}': {}",
                name,
                caller_path,
                sorted_paths(matches.iter().map(|definition| definition.path.as_str()))
            )),
        }
    }

    fn is_known_concrete_type(&self, name: &str, file_index: usize) -> Result<bool, String> {
        if matches!(
            name,
            "void"
                | "i32"
                | "f32"
                | "bool"
                | "f64"
                | "u8"
                | "u16"
                | "u32"
                | "ascii"
                | "utf8"
                | "string"
        ) {
            return Ok(true);
        }
        let environment = self.environment_for_file(file_index);
        if self
            .lookup_ordinary_struct_for_environment(name, &environment)?
            .is_some()
            || self
                .lookup_generic_struct_for_environment(name, &environment)?
                .is_some()
        {
            return Ok(true);
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        let Some(candidate_files) = self.known_type_files_by_name.get(short) else {
            return Ok(false);
        };
        let caller_path = &self.files[file_index].path;
        let matches = if let Some((alias, _)) = name.rsplit_once('.') {
            let Some(target) = self.module_graph.imported_alias_target(caller_path, alias) else {
                return Ok(false);
            };
            candidate_files
                .iter()
                .filter(|candidate| self.files[**candidate].path == target)
                .count()
        } else if candidate_files.contains(&file_index) {
            1
        } else {
            let visible = self.visible_paths(file_index);
            candidate_files
                .iter()
                .filter(|candidate| visible.contains(&self.files[**candidate].path))
                .count()
        };
        if matches > 1 {
            return Err(format!("ambiguous type '{}' from '{}'", name, caller_path));
        }
        Ok(matches == 1)
    }

    fn ordinary_type_replacements_for_source(
        &self,
        file_index: usize,
        source: &str,
    ) -> Result<Vec<(usize, usize, String)>, String> {
        let environment = self.environment_for_file(file_index);
        let mut replacements = Vec::new();
        for (start, end) in type_identifier_ranges(source)? {
            let name = &source[start..end];
            let Some(definition) =
                self.lookup_ordinary_struct_for_environment(name, &environment)?
            else {
                continue;
            };
            replacements.push((start, end, ordinary_struct_generated_name(definition)));
        }
        Ok(replacements)
    }

    fn reject_explicit_generic_calls(&self) -> Result<(), ExpansionError> {
        for (file_index, file) in self.files.iter().enumerate() {
            reject_explicit_generic_calls(&file.source, |name, qualifier| {
                !self
                    .generic_function_candidates(name, qualifier, file_index)
                    .is_empty()
            })
            .map_err(|message| ExpansionError::for_file(file.path.clone(), message))?;
        }
        Ok(())
    }

    fn new(files: Vec<RawFile>, module_graph: ModuleGraph) -> Result<Self, ExpansionError> {
        let mut generic_structs: BTreeMap<String, GenericStructDefinition> = BTreeMap::new();
        let mut generic_structs_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let generic_functions = Vec::new();
        let generic_functions_by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let ordinary_function_names: BTreeMap<String, Vec<(usize, String, Vec<String>)>> =
            BTreeMap::new();
        let mut constant_definitions = Vec::new();
        let mut ordinary_structs_by_name: BTreeMap<String, Vec<OrdinaryStructDefinition>> =
            BTreeMap::new();
        let mut known_type_files_by_name: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        let mut concrete_path_definitions = Vec::new();
        let mut parsed_functions = Vec::new();

        for (file_index, file) in files.iter().enumerate() {
            // This discovery pass must not take ownership of malformed-source
            // diagnostics. The canonical parser/indexer below has the source
            // span and function context needed to report those errors.
            let Ok(layout) = parse_top_level_type_layout(&file.source) else {
                continue;
            };
            for global in &layout.globals {
                concrete_path_definitions.push(ConcretePathDefinition {
                    file_index,
                    path: global.name.clone(),
                    type_name: global.type_name.clone(),
                });
            }
            for block in &layout.global_blocks {
                for field in &block.fields {
                    concrete_path_definitions.push(ConcretePathDefinition {
                        file_index,
                        path: format!("{}.{}", block.name, field.name),
                        type_name: field.type_name.clone(),
                    });
                }
            }
            for structure in &layout.structs {
                known_type_files_by_name
                    .entry(structure.name.clone())
                    .or_default()
                    .insert(file_index);
                if !structure.generic_parameters.is_empty() {
                    continue;
                }
                ordinary_structs_by_name
                    .entry(structure.name.clone())
                    .or_default()
                    .push(OrdinaryStructDefinition {
                        file_index,
                        path: file.path.clone(),
                        name: structure.name.clone(),
                        fields: structure.fields.clone(),
                    });
            }
            for constant in layout.constants {
                constant_definitions.push(ConstantDefinition {
                    path: file.path.clone(),
                    name: constant.name,
                    type_name: constant.type_name,
                    value_text: constant.value_text,
                });
            }
            for structure in layout.structs {
                if structure.generic_parameters.is_empty() {
                    continue;
                }
                let definition_range = find_struct_definition_range(
                    &file.source,
                    &structure.name,
                    &structure.generic_parameters,
                )
                .map_err(|message| ExpansionError::for_file(file.path.clone(), message))?;
                if generic_structs_by_name
                    .get(&structure.name)
                    .into_iter()
                    .flatten()
                    .any(|identity| generic_structs[identity].file_index == file_index)
                {
                    return Err(ExpansionError::for_file(
                        file.path.clone(),
                        format!("duplicate generic struct declaration '{}'", structure.name),
                    ));
                }
                let identity = generic_definition_identity(&file.path, &structure.name);
                generic_structs.insert(
                    identity.clone(),
                    GenericStructDefinition {
                        identity: identity.clone(),
                        file_index,
                        path: file.path.clone(),
                        module_alias: module_graph
                            .module(&file.path)
                            .map(|module| module.alias.clone())
                            .unwrap_or_else(|| module_alias_for_path(&file.path)),
                        name: structure.name.clone(),
                        parameters: structure.generic_parameters,
                        fields: structure.fields,
                        definition_range,
                    },
                );
                generic_structs_by_name
                    .entry(structure.name)
                    .or_default()
                    .push(identity);
            }
            if let Ok(externs) = parse_top_level_extern_functions(&file.source) {
                if let Some(extern_decl) = externs
                    .iter()
                    .find(|declaration| !declaration.generic_parameters.is_empty())
                {
                    return Err(ExpansionError::for_file(
                        file.path.clone(),
                        format!(
                            "generic extern function '{}' cannot be a host declaration; use a concrete wrapper",
                            extern_decl.name
                        ),
                    ));
                }
            }
            let Ok(functions) = parse_top_level_functions(&file.source) else {
                continue;
            };
            for function in functions {
                if !function.generic_parameters.is_empty() {
                    return Err(ExpansionError::for_file(
                        file.path.clone(),
                        format!(
                            "generic function declaration '{}<...>' is no longer supported; remove the function generic parameter list and bind parameters through the first parameter's generic struct",
                            function.name,
                        ),
                    ));
                }
                parsed_functions.push((
                    file_index,
                    module_graph
                        .module(&file.path)
                        .map(|module| module.alias.clone())
                        .unwrap_or_else(|| module_alias_for_path(&file.path)),
                    function,
                ));
            }
            for enumeration in &layout.enums {
                known_type_files_by_name
                    .entry(enumeration.name.clone())
                    .or_default()
                    .insert(file_index);
            }
        }

        let mut constant_values = BTreeMap::new();
        for definition in &constant_definitions {
            if definition.type_name.trim() == "i32" {
                evaluate_constant_definition(
                    definition,
                    &constant_definitions,
                    &module_graph,
                    &mut constant_values,
                    &mut Vec::new(),
                    &mut 0,
                )
                .map_err(|message| ExpansionError::for_file(definition.path.clone(), message))?;
            }
        }

        let mut expansion = Self {
            files,
            module_graph,
            generic_structs,
            generic_structs_by_name,
            generic_functions,
            generic_functions_by_name,
            ordinary_function_names,
            constant_definitions,
            constant_values,
            struct_specializations: BTreeMap::new(),
            struct_work: VecDeque::new(),
            function_specializations: BTreeMap::new(),
            function_work: VecDeque::new(),
            ordinary_structs_by_name,
            known_type_files_by_name,
            concrete_path_definitions,
            concrete_paths_by_file: BTreeMap::new(),
            active_depth: None,
        };
        for (file_index, module_alias, function) in parsed_functions {
            let parameters = expansion
                .receiver_generic_parameters(file_index, &function)
                .map_err(|message| {
                    ExpansionError::for_file(expansion.files[file_index].path.clone(), message)
                })?;
            if let Some(parameters) = parameters {
                if is_concrete_only_function_name(&function.name) {
                    return Err(ExpansionError::for_file(
                        expansion.files[file_index].path.clone(),
                        format!(
                            "generic function '{}' cannot be a lifecycle or host entry; use a concrete wrapper",
                            function.name
                        ),
                    ));
                }
                let definition = expansion.generic_functions.len();
                expansion
                    .generic_functions_by_name
                    .entry(function.name.clone())
                    .or_default()
                    .push(definition);
                expansion.generic_functions.push(GenericFunctionDefinition {
                    file_index,
                    path: expansion.files[file_index].path.clone(),
                    module_alias,
                    name: function.name.clone(),
                    parameters,
                    signature: function,
                });
            } else {
                expansion
                    .validate_unbound_function_signature(&function, file_index)
                    .map_err(|message| {
                        ExpansionError::for_file(expansion.files[file_index].path.clone(), message)
                    })?;
                expansion
                    .ordinary_function_names
                    .entry(function.name.clone())
                    .or_default()
                    .push((
                        file_index,
                        module_alias,
                        function
                            .params
                            .iter()
                            .map(|parameter| parameter.type_name.clone())
                            .collect(),
                    ));
            }
        }
        Ok(expansion)
    }

    fn receiver_generic_parameters(
        &self,
        file_index: usize,
        function: &ParsedFunctionSignature,
    ) -> Result<Option<Vec<ParsedGenericParameter>>, String> {
        let Some(first) = function.params.first() else {
            return Ok(None);
        };
        let Some((base, arguments)) = parse_type_application(&first.type_name)? else {
            return Ok(None);
        };
        let lookup_environment = self.environment_for_file(file_index);
        let Some(definition) =
            self.lookup_generic_struct_for_environment(base, &lookup_environment)?
        else {
            return Ok(None);
        };
        if definition.parameters.len() != arguments.len() {
            return Err(format!(
                "first parameter of generic function '{}' must apply '{}' with {} arguments; got {}",
                function.name,
                base,
                definition.parameters.len(),
                arguments.len()
            ));
        }
        let mut parameters = Vec::new();
        for (argument, parameter) in arguments.iter().zip(&definition.parameters) {
            self.collect_receiver_generic_parameters(
                argument,
                parameter.kind,
                file_index,
                &mut parameters,
            )?;
        }
        if parameters.is_empty() {
            return Ok(None);
        }
        if let Some(conflict) = function.params.iter().find(|parameter| {
            parameters
                .iter()
                .any(|generic| generic.name == parameter.name)
        }) {
            return Err(format!(
                "receiver generic name '{}' conflicts with an ordinary function parameter",
                conflict.name
            ));
        }
        self.validate_receiver_signature(function, &parameters, file_index)?;
        self.validate_receiver_body(function, &parameters, file_index)?;
        Ok(Some(parameters))
    }

    fn collect_receiver_generic_parameters(
        &self,
        argument: &str,
        kind: ParsedGenericParameterKind,
        file_index: usize,
        parameters: &mut Vec<ParsedGenericParameter>,
    ) -> Result<(), String> {
        let argument = argument.trim();
        match kind {
            ParsedGenericParameterKind::Type => {
                if let Some((base, arguments)) = parse_type_application(argument)? {
                    let lookup_environment = self.environment_for_file(file_index);
                    let Some(definition) =
                        self.lookup_generic_struct_for_environment(base, &lookup_environment)?
                    else {
                        return Ok(());
                    };
                    if definition.parameters.len() != arguments.len() {
                        return Err(format!(
                            "nested receiver generic type '{}' has {} arguments; expected {}",
                            base,
                            arguments.len(),
                            definition.parameters.len()
                        ));
                    }
                    for (nested, parameter) in arguments.iter().zip(&definition.parameters) {
                        self.collect_receiver_generic_parameters(
                            nested,
                            parameter.kind,
                            file_index,
                            parameters,
                        )?;
                    }
                    return Ok(());
                }
                if let Some((element, extent)) = split_array_suffix(argument) {
                    self.collect_receiver_generic_parameters(
                        element,
                        ParsedGenericParameterKind::Type,
                        file_index,
                        parameters,
                    )?;
                    if !extent.trim().is_empty() {
                        self.collect_receiver_generic_parameters(
                            extent,
                            ParsedGenericParameterKind::I32,
                            file_index,
                            parameters,
                        )?;
                    }
                    return Ok(());
                }
                if is_identifier_text(argument)
                    && !self.is_known_concrete_type(argument, file_index)?
                {
                    add_receiver_parameter(parameters, argument, ParsedGenericParameterKind::Type)?;
                } else if !is_type_argument_text(argument) {
                    return Err(format!(
                        "receiver generic type argument '{}' is not a type expression",
                        argument
                    ));
                }
            }
            ParsedGenericParameterKind::I32 => {
                let environment = self.environment_for_file(file_index);
                if is_identifier_text(argument)
                    && self
                        .visible_constant_value(argument, &environment)?
                        .is_none()
                {
                    add_receiver_parameter(parameters, argument, ParsedGenericParameterKind::I32)?;
                } else if self
                    .evaluate_i32_expression(argument, &environment)
                    .is_err()
                {
                    return Err(format!(
                        "receiver generic i32 argument '{}' must be a checked constant or identifier",
                        argument
                    ));
                }
            }
        }
        Ok(())
    }

    fn is_generic_placeholder_name(&self, name: &str) -> bool {
        let short_name = name.rsplit('.').next().unwrap_or(name);
        (short_name.len() == 1
            && short_name
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_uppercase))
            || self.generic_structs.values().any(|definition| {
                definition
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == short_name)
            })
    }

    fn validate_receiver_signature(
        &self,
        function: &ParsedFunctionSignature,
        parameters: &[ParsedGenericParameter],
        file_index: usize,
    ) -> Result<(), String> {
        for parameter in function.params.iter().skip(1) {
            self.validate_receiver_type_expression(
                &parameter.type_name,
                parameters,
                &function.name,
                file_index,
            )?;
        }
        self.validate_receiver_type_expression(
            &function.return_type_name,
            parameters,
            &function.name,
            file_index,
        )?;
        Ok(())
    }

    fn validate_receiver_body(
        &self,
        function: &ParsedFunctionSignature,
        parameters: &[ParsedGenericParameter],
        file_index: usize,
    ) -> Result<(), String> {
        let Ok(bindings) =
            crate::frontend::parser::parse_typed_local_bindings(&self.files[file_index].source)
        else {
            // Leave malformed-body diagnostics to the canonical parser.
            return Ok(());
        };
        for binding in bindings.into_iter().filter(|binding| {
            binding.function_name == function.name
                && binding.name_range.start >= function.body_range.start
                && binding.name_range.end <= function.body_range.end
        }) {
            self.validate_receiver_type_expression(
                &binding.type_name,
                parameters,
                &function.name,
                file_index,
            )?;
        }
        Ok(())
    }

    fn validate_unbound_function_signature(
        &self,
        function: &ParsedFunctionSignature,
        file_index: usize,
    ) -> Result<(), String> {
        let no_parameters = [];
        for parameter in &function.params {
            self.validate_receiver_type_expression(
                &parameter.type_name,
                &no_parameters,
                &function.name,
                file_index,
            )?;
        }
        self.validate_receiver_type_expression(
            &function.return_type_name,
            &no_parameters,
            &function.name,
            file_index,
        )
    }

    fn validate_receiver_type_expression(
        &self,
        type_name: &str,
        parameters: &[ParsedGenericParameter],
        function_name: &str,
        file_index: usize,
    ) -> Result<(), String> {
        let type_name = type_name.trim();
        if let Some((element, extent)) = split_array_suffix(type_name) {
            self.validate_receiver_type_expression(element, parameters, function_name, file_index)?;
            if !extent.trim().is_empty() {
                self.validate_receiver_i32_expression(
                    extent,
                    parameters,
                    function_name,
                    file_index,
                )?;
            }
            return Ok(());
        }
        if let Some((base, arguments)) = parse_type_application(type_name)? {
            let environment = self.environment_for_file(file_index);
            let Some(definition) =
                self.lookup_generic_struct_for_environment(base, &environment)?
            else {
                return Err(format!(
                    "generic name in function '{}' uses unknown type '{}'; bind it through the first parameter's generic struct",
                    function_name, base
                ));
            };
            if definition.parameters.len() != arguments.len() {
                return Err(format!(
                    "generic receiver function '{}' applies '{}' with {} arguments; expected {}",
                    function_name,
                    base,
                    arguments.len(),
                    definition.parameters.len()
                ));
            }
            for (argument, parameter) in arguments.iter().zip(&definition.parameters) {
                self.validate_receiver_argument(
                    argument,
                    parameter.kind,
                    parameters,
                    function_name,
                    file_index,
                )?;
            }
            return Ok(());
        }
        if is_identifier_text(type_name) {
            if let Some(parameter) = parameters
                .iter()
                .find(|parameter| parameter.name == type_name)
            {
                if parameter.kind != ParsedGenericParameterKind::Type {
                    return Err(format!(
                        "generic receiver function '{}' uses i32 binding '{}' as a type",
                        function_name, type_name
                    ));
                }
                return Ok(());
            }
            if !self.is_known_concrete_type(type_name, file_index)?
                && self.is_generic_placeholder_name(type_name)
            {
                return Err(format!(
                    "generic name '{}' in function '{}' is not bound by the first parameter's generic struct",
                    type_name, function_name
                ));
            }
        }
        Ok(())
    }

    fn validate_receiver_argument(
        &self,
        argument: &str,
        kind: ParsedGenericParameterKind,
        parameters: &[ParsedGenericParameter],
        function_name: &str,
        file_index: usize,
    ) -> Result<(), String> {
        match kind {
            ParsedGenericParameterKind::Type => self.validate_receiver_type_expression(
                argument,
                parameters,
                function_name,
                file_index,
            ),
            ParsedGenericParameterKind::I32 => self.validate_receiver_i32_expression(
                argument,
                parameters,
                function_name,
                file_index,
            ),
        }
    }

    fn validate_receiver_i32_expression(
        &self,
        expression: &str,
        parameters: &[ParsedGenericParameter],
        function_name: &str,
        file_index: usize,
    ) -> Result<(), String> {
        let Ok(tokens) = tokenize_constant_expression(expression) else {
            return Err(format!(
                "generic receiver function '{}' has an invalid i32 expression '{}'; bind it through the first parameter's generic struct",
                function_name, expression.trim()
            ));
        };
        for token in tokens {
            let ConstantToken::Identifier(identifier) = token else {
                continue;
            };
            if self
                .visible_constant_value(&identifier, &self.environment_for_file(file_index))?
                .is_some()
                || parameters.iter().any(|parameter| {
                    parameter.name == identifier
                        && parameter.kind == ParsedGenericParameterKind::I32
                })
            {
                continue;
            }
            return Err(format!(
                "generic name '{}' in function '{}' is not bound by the first parameter's generic struct",
                identifier, function_name
            ));
        }
        Ok(())
    }

    fn populate_concrete_paths(&mut self) -> Result<(), ExpansionError> {
        for file_index in 0..self.files.len() {
            let path = self.files[file_index].path.clone();
            let result = (|| -> Result<(), String> {
                let visible = self.visible_paths(file_index);
                let names = self
                    .concrete_path_definitions
                    .iter()
                    .filter(|definition| visible.contains(&self.files[definition.file_index].path))
                    .map(|definition| definition.path.clone())
                    .collect::<BTreeSet<_>>();
                let mut paths = BTreeMap::new();
                for name in names {
                    let local = self
                        .concrete_path_definitions
                        .iter()
                        .filter(|definition| {
                            definition.file_index == file_index && definition.path == name
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let matches = if local.is_empty() {
                        self.concrete_path_definitions
                            .iter()
                            .filter(|definition| {
                                visible.contains(&self.files[definition.file_index].path)
                                    && definition.path == name
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    } else {
                        local
                    };
                    let [definition] = matches.as_slice() else {
                        continue;
                    };
                    let environment = self.environment_for_file(definition.file_index);
                    self.populate_concrete_type(
                        &mut paths,
                        &definition.path,
                        &definition.type_name,
                        &environment,
                        &mut Vec::new(),
                        0,
                    )?;
                }
                self.concrete_paths_by_file.insert(file_index, paths);
                Ok(())
            })();
            result.map_err(|message| ExpansionError::for_file(path, message))?;
        }
        Ok(())
    }

    fn populate_concrete_type(
        &mut self,
        paths: &mut BTreeMap<String, String>,
        path: &str,
        type_name: &str,
        environment: &GenericEnvironment,
        visiting: &mut Vec<String>,
        generic_depth: usize,
    ) -> Result<(), String> {
        let resolved_type = rewrite_generic_identifiers(type_name, environment);
        let materialized_type = self.materialize_type(&resolved_type, environment)?;
        paths.insert(path.to_string(), materialized_type.clone());

        let subject = split_array_suffix(&materialized_type)
            .map_or(materialized_type.as_str(), |(element, _)| element);
        let Some((fields, nested_environment, visit_key)) =
            self.struct_fields_for_type(subject, environment)?
        else {
            return Ok(());
        };
        if visiting.iter().any(|existing| existing == &visit_key) {
            return Ok(());
        }
        let is_generic = visit_key.starts_with("generic:");
        if is_generic && generic_depth > MAX_INSTANTIATION_DEPTH {
            return Err(format!(
                "generic instantiation depth exceeded (maximum {})",
                MAX_INSTANTIATION_DEPTH
            ));
        }
        visiting.push(visit_key);
        for field in fields {
            self.populate_concrete_type(
                paths,
                &format!("{path}.{}", field.name),
                &field.type_name,
                &nested_environment,
                visiting,
                generic_depth + usize::from(is_generic),
            )?;
        }
        visiting.pop();
        Ok(())
    }

    fn local_paths_for_function(
        &mut self,
        file_index: usize,
        function: &ParsedFunctionSignature,
        source: &str,
        environment: &GenericEnvironment,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut paths = self
            .concrete_paths_by_file
            .get(&file_index)
            .cloned()
            .unwrap_or_default();
        for parameter in &function.params {
            let type_name = rewrite_generic_identifiers(&parameter.type_name, environment);
            self.populate_local_type_paths(
                &mut paths,
                &parameter.name,
                &type_name,
                environment,
                &mut Vec::new(),
                0,
            )?;
        }
        if let Ok(bindings) = crate::frontend::parser::parse_typed_local_bindings(source) {
            for binding in bindings
                .into_iter()
                .filter(|binding| binding.function_name == function.name)
            {
                let type_name = rewrite_generic_identifiers(&binding.type_name, environment);
                self.populate_local_type_paths(
                    &mut paths,
                    &binding.name,
                    &type_name,
                    environment,
                    &mut Vec::new(),
                    0,
                )?;
            }
        }
        let _ = file_index;
        Ok(paths)
    }

    fn populate_local_type_paths(
        &mut self,
        paths: &mut BTreeMap<String, String>,
        path: &str,
        type_name: &str,
        environment: &GenericEnvironment,
        visiting: &mut Vec<String>,
        generic_depth: usize,
    ) -> Result<(), String> {
        let resolved_type = rewrite_generic_identifiers(type_name, environment);
        let materialized_type = self.materialize_type(&resolved_type, environment)?;
        paths.insert(path.to_string(), materialized_type.clone());
        let subject = split_array_suffix(&materialized_type)
            .map_or(materialized_type.as_str(), |(element, _)| element);
        if let Some((fields, nested_environment, visit_key)) =
            self.struct_fields_for_type(subject, environment)?
        {
            if visiting.iter().any(|existing| existing == &visit_key) {
                return Ok(());
            }
            let is_generic = visit_key.starts_with("generic:");
            if is_generic && generic_depth > MAX_INSTANTIATION_DEPTH {
                return Err(format!(
                    "generic instantiation depth exceeded (maximum {})",
                    MAX_INSTANTIATION_DEPTH
                ));
            }
            visiting.push(visit_key);
            for field in fields {
                self.populate_local_type_paths(
                    paths,
                    &format!("{path}.{}", field.name),
                    &field.type_name,
                    &nested_environment,
                    visiting,
                    generic_depth + usize::from(is_generic),
                )?;
            }
            visiting.pop();
        }
        Ok(())
    }

    fn seed_direct_uses(&mut self) -> Result<(), ExpansionError> {
        for file_index in 0..self.files.len() {
            let path = self.files[file_index].path.clone();
            let result = (|| -> Result<(), String> {
                let source = self.files[file_index].source.clone();
                let source_without_templates =
                    self.source_without_templates(file_index, &source)?;
                let source_environment = self.environment_for_file(file_index);
                self.collect_type_applications(&source_without_templates, &source_environment)?;
                let Ok(functions) = parse_top_level_functions(&source_without_templates) else {
                    return Ok(());
                };
                for function in functions {
                    let Some(body) = source_without_templates.get(function.body_range.clone())
                    else {
                        continue;
                    };
                    let local_paths = self.local_paths_for_function(
                        file_index,
                        &function,
                        &source_without_templates,
                        &source_environment,
                    )?;
                    self.seed_inferred_receiver_calls(body, file_index, &local_paths)?;
                    self.seed_inferred_argument_calls(body, file_index, &local_paths)?;
                }
                Ok(())
            })();
            result.map_err(|message| ExpansionError::for_file(path, message))?;
        }
        Ok(())
    }

    fn seed_inferred_receiver_calls(
        &mut self,
        source: &str,
        file_index: usize,
        local_paths: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        for call in collect_inferred_receiver_calls(source)? {
            let is_module_call = !local_paths.contains_key(&call.receiver)
                && self
                    .imported_alias_target(file_index, &call.receiver)
                    .is_some();
            if is_module_call {
                let Some(actual_types) = call
                    .arguments
                    .iter()
                    .map(|argument| {
                        self.infer_expression_type_text(argument, local_paths, file_index)
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                let definitions =
                    self.generic_function_candidates(&call.name, Some(&call.receiver), file_index);
                if self.has_ordinary_function_candidate(
                    &call.name,
                    Some(&call.receiver),
                    file_index,
                    &actual_types,
                ) {
                    continue;
                }
                let had_viable_definition = !definitions.is_empty();
                let mut scheduled = false;
                for definition in definitions.iter().copied() {
                    let generic = self.generic_functions[definition].clone();
                    let Some(arguments) =
                        self.infer_generic_call_arguments(&generic, &actual_types)?
                    else {
                        continue;
                    };
                    self.schedule_function(definition, arguments)?;
                    scheduled = true;
                }
                if had_viable_definition && !scheduled {
                    return Err(format!(
                        "could not infer a complete generic argument list for '{}'; the first parameter must provide every generic binding",
                        call.name
                    ));
                }
                continue;
            }
            let Some(actual_type) = local_paths.get(&call.receiver).cloned() else {
                continue;
            };
            let mut actual_types = vec![actual_type];
            let Some(argument_types) = call
                .arguments
                .iter()
                .map(|argument| self.infer_expression_type_text(argument, local_paths, file_index))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            actual_types.extend(argument_types);
            let definitions = self.generic_function_candidates(&call.name, None, file_index);
            if self.has_ordinary_function_candidate(&call.name, None, file_index, &actual_types) {
                continue;
            }
            let had_viable_definition = !definitions.is_empty();
            let mut scheduled = false;
            for definition in definitions.iter().copied() {
                let generic = self.generic_functions[definition].clone();
                let Some(arguments) = self.infer_generic_call_arguments(&generic, &actual_types)?
                else {
                    continue;
                };
                self.schedule_function(definition, arguments)?;
                scheduled = true;
            }
            if had_viable_definition && !scheduled {
                return Err(format!(
                    "could not infer a complete generic argument list for '{}'; the first parameter must provide every generic binding",
                    call.name
                ));
            }
        }
        Ok(())
    }

    fn seed_inferred_argument_calls(
        &mut self,
        source: &str,
        file_index: usize,
        local_paths: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        for call in collect_inferred_argument_calls(source)? {
            let definitions = self.generic_function_candidates(&call.name, None, file_index);
            if definitions.is_empty() {
                continue;
            }
            let mut actual_types = Vec::new();
            let Some(inferred_types) = call
                .arguments
                .iter()
                .map(|argument| self.infer_expression_type_text(argument, local_paths, file_index))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            actual_types.extend(inferred_types);
            if self.has_ordinary_function_candidate(&call.name, None, file_index, &actual_types) {
                continue;
            }
            let mut scheduled = false;
            for definition in definitions.iter().copied() {
                let generic = self.generic_functions[definition].clone();
                if generic.signature.params.len() != call.arguments.len() {
                    continue;
                }
                if let Some(arguments) =
                    self.infer_generic_call_arguments(&generic, &actual_types)?
                {
                    self.schedule_function(definition, arguments)?;
                    scheduled = true;
                }
            }
            if !scheduled {
                return Err(format!(
                    "could not infer a complete generic argument list for '{}'; the first parameter must provide every generic binding",
                    call.name
                ));
            }
        }
        Ok(())
    }

    fn infer_generic_call_arguments(
        &mut self,
        generic: &GenericFunctionDefinition,
        actual_types: &[String],
    ) -> Result<Option<Vec<ConcreteArgument>>, String> {
        if generic.signature.params.len() != actual_types.len() {
            return Ok(None);
        }
        let mut environment = GenericEnvironment::default();
        environment.module_alias = Some(generic.module_alias.clone());
        environment.source_path = Some(generic.path.clone());
        // Generic names are definitionally owned by the first parameter.  It
        // is the only parameter allowed to introduce bindings; all remaining
        // parameters merely validate the already-substituted signature.
        let Some((first_parameter, first_actual)) =
            generic.signature.params.first().zip(actual_types.first())
        else {
            return Ok(None);
        };
        if !self.infer_type_pattern(
            &first_parameter.type_name,
            first_actual,
            &generic.parameters,
            &mut environment,
        )? {
            return Ok(None);
        }
        for (parameter, actual) in generic.signature.params.iter().zip(actual_types).skip(1) {
            let before_validation = environment.clone();
            let mut validation_environment = environment.clone();
            if !self.infer_type_pattern(
                &parameter.type_name,
                actual,
                &generic.parameters,
                &mut validation_environment,
            )? {
                return Ok(None);
            }
            if validation_environment.types != before_validation.types
                || validation_environment.values != before_validation.values
            {
                return Ok(None);
            }
        }
        Ok(inferred_arguments(&generic.parameters, &environment))
    }

    fn infer_expression_type_text(
        &self,
        expression: &str,
        local_paths: &BTreeMap<String, String>,
        file_index: usize,
    ) -> Option<String> {
        let mut expression = expression.trim();
        while has_outer_parentheses(expression) {
            expression = expression[1..expression.len() - 1].trim();
        }
        if let Some(type_name) = local_paths.get(expression) {
            return Some(type_name.clone());
        }
        if self
            .visible_constant_value(expression, &self.environment_for_file(file_index))
            .ok()
            .flatten()
            .is_some()
        {
            return Some("i32".to_string());
        }
        if expression == "true" || expression == "false" {
            return Some("bool".to_string());
        }
        if expression.starts_with('"') && expression.ends_with('"') {
            return Some("string".to_string());
        }
        if expression.parse::<i32>().is_ok()
            || expression
                .strip_prefix('-')
                .is_some_and(|value| value.parse::<i32>().is_ok())
        {
            return Some("i32".to_string());
        }
        if expression.parse::<f32>().is_ok() && expression.contains('.') {
            return Some("f32".to_string());
        }
        if let Some((lhs, operator, rhs)) = split_binary_expression(expression) {
            let lhs = self.infer_expression_type_text(lhs, local_paths, file_index)?;
            let rhs = self.infer_expression_type_text(rhs, local_paths, file_index)?;
            if operator == '<' || operator == '>' || operator == '=' {
                return Some("bool".to_string());
            }
            if lhs == "f64" || rhs == "f64" {
                return Some("f64".to_string());
            }
            if lhs == "f32" || rhs == "f32" {
                return Some("f32".to_string());
            }
            return Some(lhs);
        }
        if let Some((collection, suffix)) = split_indexed_expression(expression) {
            let collection_type =
                self.infer_expression_type_text(collection, local_paths, file_index)?;
            let element_type = split_array_suffix(&collection_type)?.0.to_string();
            if suffix.is_empty() {
                return Some(element_type);
            }
            return local_paths
                .get(&format!("{collection}[0]{suffix}"))
                .cloned()
                .or_else(|| self.field_type_from_text(&element_type, suffix, file_index));
        }
        None
    }

    fn field_type_from_text(
        &self,
        type_name: &str,
        suffix: &str,
        file_index: usize,
    ) -> Option<String> {
        let mut current = type_name.trim().to_string();
        let mut current_environment = self.environment_for_file(file_index);
        for field_name in suffix.trim_start_matches('.').split('.') {
            let fields = if let Some((base, arguments)) = parse_type_application(&current).ok()? {
                let definition = self
                    .lookup_generic_struct_for_environment(base, &current_environment)
                    .ok()??
                    .clone();
                let mut lookup_environment = current_environment.clone();
                lookup_environment.module_alias = Some(definition.module_alias.clone());
                lookup_environment.source_path = Some(definition.path.clone());
                let resolved_arguments = definition
                    .parameters
                    .iter()
                    .zip(arguments.iter())
                    .map(|(parameter, argument)| match parameter.kind {
                        ParsedGenericParameterKind::Type => Some(ConcreteArgument::Type(
                            self.materialize_type_readonly(argument, &lookup_environment)
                                .ok()?,
                        )),
                        ParsedGenericParameterKind::I32 => Some(ConcreteArgument::I32(
                            self.evaluate_i32_expression(argument, &lookup_environment)
                                .ok()?,
                        )),
                    })
                    .collect::<Option<Vec<_>>>()?;
                let mut environment = GenericEnvironment::from_parameters(
                    &definition.parameters,
                    &resolved_arguments,
                )
                .ok()?;
                environment.module_alias = Some(definition.module_alias.clone());
                environment.source_path = Some(definition.path.clone());
                current_environment = environment.clone();
                definition
                    .fields
                    .iter()
                    .find(|field| field.name == field_name)
                    .map(|field| {
                        let substituted =
                            rewrite_generic_identifiers(&field.type_name, &environment);
                        self.materialize_type_readonly(&substituted, &environment)
                            .unwrap_or(substituted)
                    })
            } else if let Some((definition, arguments)) =
                self.generated_struct_application(&current)
            {
                let mut environment =
                    GenericEnvironment::from_parameters(&definition.parameters, &arguments).ok()?;
                environment.module_alias = Some(definition.module_alias.clone());
                environment.source_path = Some(definition.path.clone());
                current_environment = environment.clone();
                definition
                    .fields
                    .iter()
                    .find(|field| field.name == field_name)
                    .map(|field| {
                        let substituted =
                            rewrite_generic_identifiers(&field.type_name, &environment);
                        self.materialize_type_readonly(&substituted, &environment)
                            .unwrap_or(substituted)
                    })
            } else {
                let definition = self
                    .lookup_ordinary_struct_for_environment(&current, &current_environment)
                    .ok()??;
                current_environment = self.environment_for_file(definition.file_index);
                definition
                    .fields
                    .iter()
                    .find(|field| field.name == field_name)
                    .map(|field| field.type_name.clone())
            }?;
            current = fields;
        }
        Some(current)
    }

    fn infer_type_pattern(
        &mut self,
        pattern: &str,
        actual: &str,
        parameters: &[ParsedGenericParameter],
        environment: &mut GenericEnvironment,
    ) -> Result<bool, String> {
        let pattern = pattern.trim();
        let actual = actual.trim();
        if let Some(parameter) = parameters
            .iter()
            .find(|parameter| parameter.name == pattern)
        {
            return match parameter.kind {
                ParsedGenericParameterKind::Type => {
                    let actual = self.materialize_type(actual, &GenericEnvironment::default())?;
                    bind_inferred_type(environment, &parameter.name, actual)
                }
                ParsedGenericParameterKind::I32 => Err(format!(
                    "compile-time value parameter '{}' cannot be used as a type",
                    parameter.name
                )),
            };
        }

        if let Some((pattern_element, pattern_extent)) = split_array_suffix(pattern) {
            let Some((actual_element, actual_extent)) = split_array_suffix(actual) else {
                return Ok(false);
            };
            if !self.infer_type_pattern(pattern_element, actual_element, parameters, environment)? {
                return Ok(false);
            }
            let pattern_extent = pattern_extent.trim();
            let actual_extent = actual_extent.trim();
            // A view captures only the element type.  A fixed array is
            // compatible with a view, but its capacity is deliberately not
            // allowed to bind a compile-time value parameter.
            if pattern_extent.is_empty() {
                return Ok(true);
            }
            // The reverse conversion is not valid for inference: a runtime
            // view has no statically known capacity for T[N].
            if actual_extent.is_empty() {
                return Ok(false);
            }
            if let Some(parameter) = parameters.iter().find(|parameter| {
                parameter.kind == ParsedGenericParameterKind::I32
                    && parameter.name == pattern_extent
            }) {
                let value = self.evaluate_i32_expression(actual_extent, environment)?;
                return bind_inferred_value(environment, &parameter.name, value);
            }
            let expected = match self.evaluate_i32_expression(pattern_extent, environment) {
                Ok(value) => value,
                Err(_)
                    if expression_mentions_unbound_value(
                        pattern_extent,
                        parameters,
                        environment,
                    ) =>
                {
                    // Do not solve N+1=capacity.  Another exact occurrence
                    // may bind N later, after which the expression is checked.
                    return Ok(true);
                }
                Err(_) => return Ok(false),
            };
            let Some(observed) = self
                .evaluate_i32_expression(actual_extent, environment)
                .ok()
            else {
                return Ok(false);
            };
            return Ok(expected == observed);
        }

        if let Some((pattern_base, pattern_arguments)) = parse_type_application(pattern)? {
            let Some((actual_identity, actual_arguments)) =
                self.generic_application_identity(actual, environment)
            else {
                return Ok(false);
            };
            if pattern_arguments.len() != actual_arguments.len() {
                return Ok(false);
            }
            let Some(definition) = self
                .lookup_generic_struct_for_environment(pattern_base, environment)?
                .cloned()
            else {
                return Ok(false);
            };
            if definition.identity != actual_identity {
                return Ok(false);
            }
            if definition.parameters.len() != pattern_arguments.len() {
                return Ok(false);
            }
            for ((pattern_argument, actual_argument), parameter) in pattern_arguments
                .iter()
                .zip(actual_arguments.iter())
                .zip(definition.parameters.iter())
            {
                match parameter.kind {
                    ParsedGenericParameterKind::Type => {
                        if let Some(nested) = parameters
                            .iter()
                            .find(|candidate| candidate.name == pattern_argument.trim())
                        {
                            if nested.kind != ParsedGenericParameterKind::Type {
                                return Ok(false);
                            }
                            let concrete = self.materialize_type(
                                actual_argument,
                                &GenericEnvironment::default(),
                            )?;
                            if !bind_inferred_type(environment, &nested.name, concrete)? {
                                return Ok(false);
                            }
                        } else if !self.infer_type_pattern(
                            pattern_argument,
                            actual_argument,
                            parameters,
                            environment,
                        )? {
                            return Ok(false);
                        }
                    }
                    ParsedGenericParameterKind::I32 => {
                        let Some(nested) = parameters
                            .iter()
                            .find(|candidate| candidate.name == pattern_argument.trim())
                        else {
                            let expected = self
                                .evaluate_i32_expression(pattern_argument, environment)
                                .ok();
                            let observed = self
                                .evaluate_i32_expression(actual_argument, environment)
                                .ok();
                            match (expected, observed) {
                                (Some(expected), Some(observed)) => {
                                    if expected != observed {
                                        return Ok(false);
                                    }
                                }
                                (None, _)
                                    if expression_mentions_unbound_value(
                                        pattern_argument,
                                        parameters,
                                        environment,
                                    ) =>
                                {
                                    return Ok(true)
                                }
                                _ => return Ok(false),
                            }
                            continue;
                        };
                        if nested.kind != ParsedGenericParameterKind::I32 {
                            return Ok(false);
                        }
                        let value = self.evaluate_i32_expression(actual_argument, environment)?;
                        if !bind_inferred_value(environment, &nested.name, value)? {
                            return Ok(false);
                        }
                    }
                }
            }
            return Ok(true);
        }

        Ok(same_type_name(pattern, actual))
    }

    fn source_without_templates(&self, file_index: usize, source: &str) -> Result<String, String> {
        let mut removals = Vec::new();
        for definition in self.generic_structs.values() {
            if definition.file_index == file_index {
                removals.push(definition.definition_range.clone());
            }
        }
        for definition in &self.generic_functions {
            if definition.file_index == file_index {
                removals.push(
                    definition.signature.signature_range.start..definition.signature.body_range.end,
                );
            }
        }
        removals.sort_by_key(|range| (range.start, range.end));
        let mut output = String::with_capacity(source.len());
        let mut cursor = 0usize;
        for range in removals {
            output.push_str(&source[cursor..range.start]);
            cursor = range.end;
        }
        output.push_str(&source[cursor..]);
        Ok(output)
    }

    fn process_worklist(&mut self) -> Result<(), ExpansionError> {
        while !self.struct_work.is_empty() || !self.function_work.is_empty() {
            if let Some((key, depth)) = self.struct_work.pop_front() {
                let path = self
                    .lookup_generic_struct(&key.definition)
                    .map(|definition| definition.path.clone());
                let previous_depth = self.active_depth.replace(depth);
                let result = self.materialize_struct(&key);
                self.active_depth = previous_depth;
                result.map_err(|message| match path {
                    Some(path) => ExpansionError::for_file(path, message),
                    None => ExpansionError::without_path(message),
                })?;
            }
            if let Some((key, depth)) = self.function_work.pop_front() {
                let path = self
                    .generic_functions
                    .get(key.definition)
                    .map(|definition| definition.path.clone());
                let previous_depth = self.active_depth.replace(depth);
                let result = self.materialize_function(&key);
                self.active_depth = previous_depth;
                result.map_err(|message| match path {
                    Some(path) => ExpansionError::for_file(path, message),
                    None => ExpansionError::without_path(message),
                })?;
            }
        }
        Ok(())
    }

    fn collect_type_applications(
        &mut self,
        source: &str,
        environment: &GenericEnvironment,
    ) -> Result<(), String> {
        let bytes = source.as_bytes();
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            if bytes[cursor] == b'"' {
                cursor = skip_string(source, cursor)?;
                continue;
            }
            if starts_comment(source, cursor) {
                cursor = skip_comment(source, cursor)?;
                continue;
            }
            if !is_identifier_start(bytes[cursor]) {
                cursor += 1;
                continue;
            }
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
                cursor += 1;
            }
            let qualified_end = qualified_identifier_end(source, start, cursor);
            let name = &source[start..qualified_end];
            let after_name = skip_ascii_whitespace(source, qualified_end);
            if self
                .lookup_generic_struct_for_environment(name, environment)?
                .is_none()
                || source.as_bytes().get(after_name) != Some(&b'<')
            {
                cursor = qualified_end;
                continue;
            }
            let close = matching_angle(source, after_name)?;
            self.materialize_type(&source[start..=close], environment)?;
            cursor = close + 1;
        }
        Ok(())
    }

    fn materialize_type(
        &mut self,
        type_text: &str,
        environment: &GenericEnvironment,
    ) -> Result<String, String> {
        let trimmed = type_text.trim();
        if trimmed.is_empty() {
            return Err("generic type argument cannot be empty".to_string());
        }
        if let Some((element, extent)) = split_array_suffix(trimmed) {
            let element = self.materialize_type(element, environment)?;
            if extent.is_empty() {
                return Ok(format!("{element}[]"));
            }
            let value = self.evaluate_i32_expression(extent, environment)?;
            if value < 0 {
                return Err(format!(
                    "negative array extent {} is invalid after generic substitution",
                    value
                ));
            }
            return Ok(format!("{element}[{value}]"));
        }
        if let Some((base, arguments)) = parse_type_application(trimmed)? {
            let definition = self
                .lookup_generic_struct_for_environment(base, environment)?
                .ok_or_else(|| format!("unknown generic type '{base}'"))?
                .clone();
            let arguments =
                self.resolve_argument_list(&definition.parameters, &arguments, environment)?;
            let key = StructSpecializationKey {
                definition: definition.identity.clone(),
                arguments: arguments.clone(),
            };
            return self.schedule_struct(key, &definition);
        }
        if let Some(value) = environment.types.get(trimmed) {
            return Ok(value.clone());
        }
        if environment.values.contains_key(trimmed) {
            return Err(format!(
                "compile-time value parameter '{}' cannot be used as a type",
                trimmed
            ));
        }
        if let Some(definition) =
            self.lookup_ordinary_struct_for_environment(trimmed, environment)?
        {
            return Ok(ordinary_struct_generated_name(definition));
        }
        Ok(trimmed.to_string())
    }

    fn materialize_type_readonly(
        &self,
        type_text: &str,
        environment: &GenericEnvironment,
    ) -> Result<String, String> {
        let trimmed = type_text.trim();
        if let Some((element, extent)) = split_array_suffix(trimmed) {
            let element = self.materialize_type_readonly(element, environment)?;
            if extent.is_empty() {
                return Ok(format!("{element}[]"));
            }
            let value = self.evaluate_i32_expression(extent, environment)?;
            if value < 0 {
                return Err(format!("negative array extent {} is invalid", value));
            }
            return Ok(format!("{element}[{value}]"));
        }
        if let Some((base, arguments)) = parse_type_application(trimmed)? {
            let definition = self
                .lookup_generic_struct_for_environment(base, environment)?
                .ok_or_else(|| format!("unknown generic type '{base}'"))?;
            let resolved = self.resolve_argument_list_readonly(
                &definition.parameters,
                &arguments,
                environment,
            )?;
            let key = StructSpecializationKey {
                definition: definition.identity.clone(),
                arguments: resolved,
            };
            return self
                .struct_specializations
                .get(&key)
                .map(|specialization| specialization.generated_name.clone())
                .ok_or_else(|| format!("missing generic type specialization '{trimmed}'"));
        }
        if let Some(value) = environment.types.get(trimmed) {
            return Ok(value.clone());
        }
        if let Some(definition) =
            self.lookup_ordinary_struct_for_environment(trimmed, environment)?
        {
            return Ok(ordinary_struct_generated_name(definition));
        }
        Ok(trimmed.to_string())
    }

    fn lookup_generic_struct(&self, name: &str) -> Option<&GenericStructDefinition> {
        self.generic_structs.get(name)
    }

    fn lookup_generic_struct_for_environment(
        &self,
        name: &str,
        environment: &GenericEnvironment,
    ) -> Result<Option<&GenericStructDefinition>, String> {
        if let Some(definition) = self.generic_structs.get(name) {
            return Ok(Some(definition));
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        let Some(candidates) = self.generic_structs_by_name.get(short) else {
            return Ok(None);
        };
        let Some(caller_path) = environment.source_path.as_deref() else {
            return Ok(None);
        };
        let candidate_definitions = candidates
            .iter()
            .filter_map(|identity| self.generic_structs.get(identity));
        let matches = if let Some((alias, _)) = name.rsplit_once('.') {
            let Some(target_path) = self.module_graph.imported_alias_target(caller_path, alias)
            else {
                return Ok(None);
            };
            candidate_definitions
                .filter(|definition| definition.path == target_path)
                .collect::<Vec<_>>()
        } else {
            let local = candidate_definitions
                .clone()
                .filter(|definition| definition.path == caller_path)
                .collect::<Vec<_>>();
            if !local.is_empty() {
                local
            } else {
                let visible = self.module_graph.dependency_closure(caller_path);
                candidate_definitions
                    .filter(|definition| visible.contains(&definition.path))
                    .collect::<Vec<_>>()
            }
        };
        match matches.as_slice() {
            [] => Ok(None),
            [definition] => Ok(Some(*definition)),
            _ => Err(format!(
                "ambiguous generic type '{}' from '{}'; qualify the declaration with a directly imported module",
                name, caller_path
            )),
        }
    }

    fn generated_struct_application(
        &self,
        type_name: &str,
    ) -> Option<(GenericStructDefinition, Vec<ConcreteArgument>)> {
        self.struct_specializations
            .iter()
            .find(|(_, specialization)| specialization.generated_name == type_name.trim())
            .and_then(|(key, _)| {
                self.generic_structs
                    .get(&key.definition)
                    .cloned()
                    .map(|definition| (definition, key.arguments.clone()))
            })
    }

    fn generic_application_identity(
        &self,
        type_name: &str,
        environment: &GenericEnvironment,
    ) -> Option<(String, Vec<String>)> {
        if let Some((base, arguments)) = parse_type_application(type_name).ok().flatten() {
            let definition = self
                .lookup_generic_struct_for_environment(base, environment)
                .ok()??;
            return Some((definition.identity.clone(), arguments));
        }
        let (definition, arguments) = self.generated_struct_application(type_name)?;
        Some((
            definition.identity,
            arguments
                .into_iter()
                .map(|argument| match argument {
                    ConcreteArgument::Type(value) => value,
                    ConcreteArgument::I32(value) => value.to_string(),
                })
                .collect(),
        ))
    }

    fn struct_fields_for_type(
        &mut self,
        type_name: &str,
        environment: &GenericEnvironment,
    ) -> Result<
        Option<(
            Vec<crate::frontend::parser::ParsedField>,
            GenericEnvironment,
            String,
        )>,
        String,
    > {
        if let Some((base, arguments)) = parse_type_application(type_name)? {
            let Some(definition) = self
                .lookup_generic_struct_for_environment(base, environment)?
                .cloned()
            else {
                return Ok(None);
            };
            let resolved_arguments =
                self.resolve_argument_list(&definition.parameters, &arguments, environment)?;
            let mut nested_environment =
                GenericEnvironment::from_parameters(&definition.parameters, &resolved_arguments)?;
            nested_environment.module_alias = Some(definition.module_alias.clone());
            nested_environment.source_path = Some(definition.path.clone());
            return Ok(Some((
                definition.fields,
                nested_environment,
                format!("generic:{}<{resolved_arguments:?}>", definition.identity),
            )));
        }
        if let Some((definition, arguments)) = self.generated_struct_application(type_name) {
            let mut nested_environment =
                GenericEnvironment::from_parameters(&definition.parameters, &arguments)?;
            nested_environment.module_alias = Some(definition.module_alias.clone());
            nested_environment.source_path = Some(definition.path.clone());
            return Ok(Some((
                definition.fields,
                nested_environment,
                format!("generic:{}<{arguments:?}>", definition.identity),
            )));
        }
        let Some(definition) = self
            .lookup_ordinary_struct_for_environment(type_name.trim(), environment)?
            .cloned()
        else {
            return Ok(None);
        };
        Ok(Some((
            definition.fields,
            self.environment_for_file(definition.file_index),
            format!("struct:{}::{}", definition.path, definition.name),
        )))
    }

    fn resolve_argument_list(
        &mut self,
        parameters: &[ParsedGenericParameter],
        arguments: &[String],
        environment: &GenericEnvironment,
    ) -> Result<Vec<ConcreteArgument>, String> {
        if parameters.len() != arguments.len() {
            return Err(format!(
                "generic argument arity mismatch: expected {}, got {}",
                parameters.len(),
                arguments.len()
            ));
        }
        let mut resolved = Vec::with_capacity(arguments.len());
        for (parameter, argument) in parameters.iter().zip(arguments) {
            match parameter.kind {
                ParsedGenericParameterKind::I32 => {
                    let value = self.evaluate_i32_expression(argument, environment)?;
                    resolved.push(ConcreteArgument::I32(value));
                }
                ParsedGenericParameterKind::Type => {
                    if !is_type_argument_text(argument) {
                        return Err(format!(
                            "generic parameter '{}' expects a type argument, got '{}'",
                            parameter.name, argument
                        ));
                    }
                    let value = self.materialize_type(argument, environment)?;
                    if contains_void_type(&value) {
                        return Err(format!(
                            "generic parameter '{}' cannot be instantiated with void",
                            parameter.name
                        ));
                    }
                    resolved.push(ConcreteArgument::Type(value));
                }
            }
        }
        Ok(resolved)
    }

    fn resolve_argument_list_readonly(
        &self,
        parameters: &[ParsedGenericParameter],
        arguments: &[String],
        environment: &GenericEnvironment,
    ) -> Result<Vec<ConcreteArgument>, String> {
        if parameters.len() != arguments.len() {
            return Err(format!(
                "generic argument arity mismatch: expected {}, got {}",
                parameters.len(),
                arguments.len()
            ));
        }
        arguments
            .iter()
            .zip(parameters)
            .map(|(argument, parameter)| match parameter.kind {
                ParsedGenericParameterKind::I32 => Ok(ConcreteArgument::I32(
                    self.evaluate_i32_expression(argument, environment)?,
                )),
                ParsedGenericParameterKind::Type => {
                    let value = if is_type_argument_text(argument) {
                        self.materialize_type_readonly(argument, environment)?
                    } else {
                        return Err(format!(
                            "generic parameter '{}' expects a type argument, got '{}'",
                            parameter.name, argument
                        ));
                    };
                    if contains_void_type(&value) {
                        return Err(format!(
                            "generic parameter '{}' cannot be instantiated with void",
                            parameter.name
                        ));
                    }
                    Ok(ConcreteArgument::Type(value))
                }
            })
            .collect()
    }

    fn schedule_struct(
        &mut self,
        key: StructSpecializationKey,
        definition: &GenericStructDefinition,
    ) -> Result<String, String> {
        if let Some(existing) = self.struct_specializations.get(&key) {
            return Ok(existing.generated_name.clone());
        }
        self.check_specialization_limit()?;
        let generated_name =
            mangle_specialization("type", &definition.path, &definition.name, &key.arguments);
        self.struct_specializations.insert(
            key.clone(),
            StructSpecialization {
                generated_name: generated_name.clone(),
            },
        );
        self.enqueue_depth()?;
        self.struct_work.push_back((key, self.next_depth()));
        Ok(generated_name)
    }

    fn schedule_function(
        &mut self,
        definition: usize,
        arguments: Vec<ConcreteArgument>,
    ) -> Result<(), String> {
        let generic = &self.generic_functions[definition];
        if generic.parameters.len() != arguments.len() {
            return Err(format!(
                "generic function '{}' expects {} arguments, got {}",
                generic.name,
                generic.parameters.len(),
                arguments.len()
            ));
        }
        let key = FunctionSpecializationKey {
            definition,
            arguments,
        };
        if self.function_specializations.contains_key(&key) {
            return Ok(());
        }
        self.check_specialization_limit()?;
        self.function_specializations.insert(
            key.clone(),
            FunctionSpecialization {
                definition,
                source: String::new(),
                param_type_names: Vec::new(),
            },
        );
        self.enqueue_depth()?;
        self.function_work.push_back((key, self.next_depth()));
        Ok(())
    }

    fn next_depth(&self) -> usize {
        self.active_depth.map_or(0, |depth| depth.saturating_add(1))
    }

    fn enqueue_depth(&self) -> Result<(), String> {
        let depth = self.next_depth();
        if depth > MAX_INSTANTIATION_DEPTH {
            return Err(format!(
                "generic instantiation depth exceeded (maximum {})",
                MAX_INSTANTIATION_DEPTH
            ));
        }
        Ok(())
    }

    fn materialize_struct(&mut self, key: &StructSpecializationKey) -> Result<(), String> {
        let definition = self
            .lookup_generic_struct(&key.definition)
            .ok_or_else(|| format!("unknown generic struct '{}'", key.definition))?
            .clone();
        let mut environment =
            GenericEnvironment::from_parameters(&definition.parameters, &key.arguments)?;
        environment.module_alias = Some(definition.module_alias.clone());
        environment.source_path = Some(definition.path.clone());
        let generated_name = self
            .struct_specializations
            .get(key)
            .map(|specialization| specialization.generated_name.clone())
            .ok_or_else(|| "internal error: missing struct specialization".to_string())?;
        for field in &definition.fields {
            let materialized = self.materialize_type(&field.type_name, &environment)?;
            if contains_void_type(&materialized) {
                return Err(format!(
                    "generic struct '{}<...>' field '{}' cannot contain void after substitution",
                    definition.name, field.name
                ));
            }
            if contains_view_type(&materialized) {
                return Err(format!(
                    "generic struct '{}<...>' field '{}' cannot store view type '{}'",
                    definition.name, field.name, materialized
                ));
            }
            if contains_type_name(&materialized, &generated_name) {
                return Err(format!(
                    "recursive generic struct field '{}.{}' contains '{}' by value",
                    definition.name, field.name, materialized
                ));
            }
        }
        Ok(())
    }

    fn materialize_function(&mut self, key: &FunctionSpecializationKey) -> Result<(), String> {
        let generic = self
            .generic_functions
            .get(key.definition)
            .cloned()
            .ok_or_else(|| "internal error: missing generic function definition".to_string())?;
        let mut environment =
            GenericEnvironment::from_parameters(&generic.parameters, &key.arguments)?;
        environment.module_alias = Some(generic.module_alias.clone());
        environment.source_path = Some(generic.path.clone());
        let source = self.files[generic.file_index].source.clone();
        let start = generic.signature.signature_range.start;
        let end = generic.signature.body_range.end;
        let original = source.get(start..end).ok_or_else(|| {
            format!(
                "generic function '{}' has invalid source range",
                generic.name
            )
        })?;
        let body = source
            .get(generic.signature.body_range.clone())
            .ok_or_else(|| format!("generic function '{}' has invalid body range", generic.name))?;
        reject_value_parameter_writes(body, &generic.parameters)?;
        let stripped = strip_generic_declaration(original, "function")?;

        let substituted = rewrite_generic_identifiers(&stripped, &environment);
        let substituted = self.rewrite_type_applications(&substituted, &environment)?;
        let substituted = rewrite_i32_constants(
            &substituted,
            &BTreeMap::new(),
            &self.constant_rewrites_for_source(generic.file_index, &substituted)?,
        )?;
        let substituted = apply_replacements(
            &substituted,
            &self.ordinary_type_replacements_for_source(generic.file_index, &substituted)?,
        )?;

        let parsed_substituted = parse_top_level_functions(&substituted)?;
        let specialized_signature = parsed_substituted
            .first()
            .ok_or_else(|| "specialized generic function has no parsed signature".to_string())?
            .clone();
        let local_paths = self.local_paths_for_function(
            generic.file_index,
            &specialized_signature,
            &substituted,
            &environment,
        )?;

        for call in collect_inferred_receiver_calls(&substituted)? {
            let is_module_call = !local_paths.contains_key(&call.receiver)
                && self
                    .imported_alias_target(generic.file_index, &call.receiver)
                    .is_some();
            if is_module_call {
                let Some(actual_types) = call
                    .arguments
                    .iter()
                    .map(|argument| {
                        self.infer_expression_type_text(argument, &local_paths, generic.file_index)
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                let definitions = self.generic_function_candidates(
                    &call.name,
                    Some(&call.receiver),
                    generic.file_index,
                );
                if self.has_ordinary_function_candidate(
                    &call.name,
                    Some(&call.receiver),
                    generic.file_index,
                    &actual_types,
                ) {
                    continue;
                }
                let mut scheduled = false;
                for definition in definitions.iter().copied() {
                    let target = self.generic_functions[definition].clone();
                    if let Some(arguments) =
                        self.infer_generic_call_arguments(&target, &actual_types)?
                    {
                        self.schedule_function(definition, arguments)?;
                        scheduled = true;
                    }
                }
                if !definitions.is_empty() && !scheduled {
                    return Err(format!(
                        "could not infer a complete generic argument list for '{}'; the first parameter must provide every generic binding",
                        call.name
                    ));
                }
                continue;
            }
            let Some(actual_type) = local_paths.get(&call.receiver).cloned() else {
                continue;
            };
            let Some(argument_types) = call
                .arguments
                .iter()
                .map(|argument| {
                    self.infer_expression_type_text(argument, &local_paths, generic.file_index)
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let mut actual_types = vec![actual_type];
            actual_types.extend(argument_types);
            let definitions =
                self.generic_function_candidates(&call.name, None, generic.file_index);
            if self.has_ordinary_function_candidate(
                &call.name,
                None,
                generic.file_index,
                &actual_types,
            ) {
                continue;
            }
            let mut scheduled = false;
            for definition in definitions.iter().copied() {
                let target = self.generic_functions[definition].clone();
                if let Some(arguments) =
                    self.infer_generic_call_arguments(&target, &actual_types)?
                {
                    self.schedule_function(definition, arguments)?;
                    scheduled = true;
                }
            }
            if !definitions.is_empty() && !scheduled {
                return Err(format!(
                    "could not infer a complete generic argument list for '{}'; the first parameter must provide every generic binding",
                    call.name
                ));
            }
        }

        for call in collect_inferred_argument_calls(&substituted)? {
            let definitions =
                self.generic_function_candidates(&call.name, None, generic.file_index);
            let Some(actual_types) = call
                .arguments
                .iter()
                .map(|argument| {
                    self.infer_expression_type_text(argument, &local_paths, generic.file_index)
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if self.has_ordinary_function_candidate(
                &call.name,
                None,
                generic.file_index,
                &actual_types,
            ) {
                continue;
            }
            let mut scheduled = false;
            for definition in definitions.iter().copied() {
                let target = self.generic_functions[definition].clone();
                if let Some(arguments) =
                    self.infer_generic_call_arguments(&target, &actual_types)?
                {
                    self.schedule_function(definition, arguments)?;
                    scheduled = true;
                }
            }
            if !definitions.is_empty() && !scheduled {
                return Err(format!(
                    "could not infer a complete generic argument list for '{}'; the first parameter must provide every generic binding",
                    call.name
                ));
            }
        }

        let record = self
            .function_specializations
            .get_mut(key)
            .ok_or_else(|| "internal error: function specialization disappeared".to_string())?;
        let signature = &specialized_signature;
        record.param_type_names = signature
            .params
            .iter()
            .map(|parameter| parameter.type_name.clone())
            .collect();
        record.source = substituted;
        Ok(())
    }

    fn rewrite_type_applications(
        &mut self,
        source: &str,
        environment: &GenericEnvironment,
    ) -> Result<String, String> {
        let bytes = source.as_bytes();
        let mut output = String::with_capacity(source.len());
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            if bytes[cursor] == b'"' {
                let end = skip_string(source, cursor)?;
                output.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
            if starts_comment(source, cursor) {
                let end = skip_comment(source, cursor)?;
                output.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
            if !is_identifier_start(bytes[cursor]) {
                output.push(bytes[cursor] as char);
                cursor += 1;
                continue;
            }
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
                cursor += 1;
            }
            let qualified_end = qualified_identifier_end(source, start, cursor);
            let name = &source[start..qualified_end];
            let after = skip_ascii_whitespace(source, qualified_end);
            if self
                .lookup_generic_struct_for_environment(name, environment)?
                .is_some()
                && source.as_bytes().get(after) == Some(&b'<')
            {
                let close = matching_angle(source, after)?;
                let replacement = self.materialize_type(&source[start..=close], environment)?;
                output.push_str(&replacement);
                cursor = close + 1;
            } else {
                output.push_str(&source[start..qualified_end]);
                cursor = qualified_end;
            }
        }
        Ok(output)
    }

    fn write_sources(&mut self, files: &mut [SourceFile]) -> Result<(), ExpansionError> {
        let function_names = self
            .function_names()
            .map_err(ExpansionError::without_path)?;
        for file_index in 0..files.len() {
            let path = self.files[file_index].path.clone();
            let result = (|| -> Result<String, String> {
                let raw = self.files[file_index].source.clone();
                let mut removals = Vec::new();
                for definition in self.generic_structs.values() {
                    if definition.file_index == file_index {
                        removals.push(definition.definition_range.clone());
                    }
                }
                for definition in &self.generic_functions {
                    if definition.file_index == file_index {
                        removals.push(
                            definition.signature.signature_range.start
                                ..definition.signature.body_range.end,
                        );
                    }
                }
                removals.sort_by_key(|range| (range.start, range.end));
                let mut generated = rewrite_kept_source(self, file_index, &raw, &removals)?;
                generated = self.rewrite_inferred_generic_calls(
                    file_index,
                    &generated,
                    &GenericEnvironment {
                        module_alias: Some(module_alias_for_path(&self.files[file_index].path)),
                        source_path: Some(self.files[file_index].path.clone()),
                        ..GenericEnvironment::default()
                    },
                    &function_names,
                )?;

                for (key, specialization) in &self.struct_specializations {
                    let Some(definition) = self.lookup_generic_struct(&key.definition) else {
                        continue;
                    };
                    if definition.file_index != file_index {
                        continue;
                    }
                    let mut environment = GenericEnvironment::from_parameters(
                        &definition.parameters,
                        &key.arguments,
                    )?;
                    environment.module_alias = Some(definition.module_alias.clone());
                    environment.source_path = Some(definition.path.clone());
                    let mut fields = String::new();
                    for field in &definition.fields {
                        let field_type =
                            self.materialize_type_readonly(&field.type_name, &environment)?;
                        fields.push_str("    ");
                        fields.push_str(&field.name);
                        fields.push_str(": ");
                        fields.push_str(&field_type);
                        fields.push_str(";\n");
                    }
                    generated.push_str(&format!(
                        "\nstruct {} {{\n{fields}}}\n",
                        specialization.generated_name
                    ));
                }

                let function_specializations = self
                    .function_specializations
                    .iter()
                    .map(|(key, specialization)| (key.clone(), specialization.clone()))
                    .collect::<Vec<_>>();
                for (key, specialization) in function_specializations {
                    let definition = self.generic_functions[specialization.definition].clone();
                    if definition.file_index != file_index || specialization.source.is_empty() {
                        continue;
                    }
                    let mut environment = GenericEnvironment::from_parameters(
                        &definition.parameters,
                        &key.arguments,
                    )?;
                    environment.module_alias = Some(definition.module_alias.clone());
                    environment.source_path = Some(definition.path.clone());
                    let source = self.rewrite_inferred_generic_calls(
                        file_index,
                        &specialization.source,
                        &environment,
                        &function_names,
                    )?;
                    let source = rename_function_declaration(
                        &source,
                        &definition.name,
                        function_names.get(&key).ok_or_else(|| {
                            "missing generic function specialization name".to_string()
                        })?,
                    )?;
                    generated.push('\n');
                    generated.push_str(&source);
                    generated.push('\n');
                }
                Ok(generated)
            })();
            let generated = result.map_err(|message| ExpansionError::for_file(path, message))?;
            let file = &mut files[file_index];
            file.content = generated;
            file.hash = hash_text(&file.content);
        }
        Ok(())
    }

    fn rewrite_inferred_generic_calls(
        &mut self,
        file_index: usize,
        source: &str,
        environment: &GenericEnvironment,
        function_names: &BTreeMap<FunctionSpecializationKey, String>,
    ) -> Result<String, String> {
        let functions = parse_top_level_functions(source)?;
        let mut replacements = Vec::new();
        for function in functions {
            let Some(body) = source.get(function.body_range.clone()) else {
                return Err(format!(
                    "function '{}' has invalid body range while rewriting generic calls",
                    function.name
                ));
            };
            let local_paths =
                self.local_paths_for_function(file_index, &function, source, environment)?;
            let body_offset = function.body_range.start;

            for call in collect_inferred_receiver_calls(body)? {
                let is_module_call = !local_paths.contains_key(&call.receiver)
                    && self
                        .imported_alias_target(file_index, &call.receiver)
                        .is_some();
                let mut actual_types = Vec::new();
                if !is_module_call {
                    let Some(receiver_type) = local_paths.get(&call.receiver).cloned() else {
                        continue;
                    };
                    actual_types.push(receiver_type);
                }
                let Some(argument_types) = call
                    .arguments
                    .iter()
                    .map(|argument| {
                        self.infer_expression_type_text(argument, &local_paths, file_index)
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                actual_types.extend(argument_types);
                let qualifier = is_module_call.then_some(call.receiver.as_str());
                let definitions =
                    self.generic_function_candidates(&call.name, qualifier, file_index);
                if definitions.is_empty()
                    || self.has_ordinary_function_candidate(
                        &call.name,
                        qualifier,
                        file_index,
                        &actual_types,
                    )
                {
                    continue;
                }
                let targets = self.inferred_generic_target_names(
                    &call.name,
                    qualifier,
                    &actual_types,
                    &definitions,
                    function_names,
                )?;
                let target = match targets.as_slice() {
                    [target] => target.clone(),
                    [] => continue,
                    _ => {
                        return Err(format!(
                            "ambiguous inferred generic call '{}'; generic arguments must be inferred from the first parameter",
                            call.name
                        ));
                    }
                };
                replacements.push((
                    body_offset + call.name_start,
                    body_offset + call.name_end,
                    target,
                ));
            }

            for call in collect_inferred_argument_calls(body)? {
                let definitions = self.generic_function_candidates(&call.name, None, file_index);
                if definitions.is_empty() {
                    continue;
                }
                let Some(actual_types) = call
                    .arguments
                    .iter()
                    .map(|argument| {
                        self.infer_expression_type_text(argument, &local_paths, file_index)
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                if self.has_ordinary_function_candidate(&call.name, None, file_index, &actual_types)
                {
                    continue;
                }
                let targets = self.inferred_generic_target_names(
                    &call.name,
                    None,
                    &actual_types,
                    &definitions,
                    function_names,
                )?;
                let target = match targets.as_slice() {
                    [target] => target.clone(),
                    [] => continue,
                    _ => {
                        return Err(format!(
                            "ambiguous inferred generic call '{}'; generic arguments must be inferred from the first parameter",
                            call.name
                        ));
                    }
                };
                replacements.push((
                    body_offset + call.name_start,
                    body_offset + call.name_end,
                    target,
                ));
            }
        }
        replacements.sort_by_key(|(start, _, _)| *start);
        apply_replacements(source, &replacements)
    }

    fn inferred_generic_target_names(
        &mut self,
        name: &str,
        qualifier: Option<&str>,
        actual_types: &[String],
        definitions: &[usize],
        function_names: &BTreeMap<FunctionSpecializationKey, String>,
    ) -> Result<Vec<String>, String> {
        let mut targets = BTreeSet::new();
        for definition in definitions {
            let generic = self.generic_functions[*definition].clone();
            let Some(arguments) = self.infer_generic_call_arguments(&generic, actual_types)? else {
                continue;
            };
            let key = FunctionSpecializationKey {
                definition: *definition,
                arguments,
            };
            let Some(target) = function_names.get(&key) else {
                return Err(format!(
                    "missing specialization for inferred generic call '{}{}'",
                    qualifier.map_or(String::new(), |value| format!("{value}.")),
                    name
                ));
            };
            targets.insert(target.clone());
        }
        Ok(targets.into_iter().collect())
    }

    fn function_names(&self) -> Result<BTreeMap<FunctionSpecializationKey, String>, String> {
        let mut groups: BTreeMap<(usize, String, Vec<String>), Vec<FunctionSpecializationKey>> =
            BTreeMap::new();
        for (key, specialization) in &self.function_specializations {
            let definition = self
                .generic_functions
                .get(specialization.definition)
                .ok_or_else(|| "internal error: missing generic function definition".to_string())?;
            groups
                .entry((
                    definition.file_index,
                    definition.name.clone(),
                    specialization.param_type_names.clone(),
                ))
                .or_default()
                .push(key.clone());
        }

        let mut names = BTreeMap::new();
        for keys in groups.into_values() {
            // Every concrete generic function gets a canonical name that
            // carries its defining declaration and evaluated arguments.  A
            // specialization must not reuse the template's source name when
            // it is the only instance in this compilation: doing so makes
            // incremental snapshots treat `f<4>` and `f<8>` as one function.
            let needs_mangled_names = true;
            for key in keys {
                let specialization = self
                    .function_specializations
                    .get(&key)
                    .ok_or_else(|| "internal error: missing function specialization".to_string())?;
                let definition = self
                    .generic_functions
                    .get(specialization.definition)
                    .ok_or_else(|| {
                        "internal error: missing generic function definition".to_string()
                    })?;
                let name = if needs_mangled_names {
                    let identity_path = generic_function_identity(
                        &self.files[definition.file_index].path,
                        &definition.signature,
                    );
                    mangle_specialization(
                        "function",
                        &identity_path,
                        &definition.name,
                        &key.arguments,
                    )
                } else {
                    definition.name.clone()
                };
                names.insert(key, name);
            }
        }
        Ok(names)
    }

    fn check_specialization_limit(&self) -> Result<(), String> {
        let total = self.struct_specializations.len() + self.function_specializations.len();
        if total >= MAX_SPECIALIZATIONS {
            return Err(format!(
                "generic specialization limit exceeded (maximum {})",
                MAX_SPECIALIZATIONS
            ));
        }
        Ok(())
    }

    fn generic_function_candidates(
        &self,
        name: &str,
        qualifier: Option<&str>,
        file_index: usize,
    ) -> Vec<usize> {
        let Some(all) = self.generic_functions_by_name.get(name) else {
            return Vec::new();
        };
        if let Some(qualifier) = qualifier {
            let Some(target_path) = self.imported_alias_target(file_index, qualifier) else {
                return Vec::new();
            };
            return all
                .iter()
                .copied()
                .filter(|index| self.generic_functions[*index].path == target_path)
                .collect();
        }

        let local = all
            .iter()
            .copied()
            .filter(|index| self.generic_functions[*index].file_index == file_index)
            .collect::<Vec<_>>();
        if !local.is_empty() {
            return local;
        }
        let visible = self.visible_paths(file_index);
        let imported = all
            .iter()
            .copied()
            .filter(|index| visible.contains(&self.generic_functions[*index].path))
            .collect::<Vec<_>>();
        imported
    }

    fn has_ordinary_function_candidate(
        &self,
        name: &str,
        qualifier: Option<&str>,
        file_index: usize,
        actual_types: &[String],
    ) -> bool {
        let Some(all) = self.ordinary_function_names.get(name) else {
            return false;
        };
        let qualified_target =
            qualifier.and_then(|alias| self.imported_alias_target(file_index, alias));
        if qualifier.is_some() && qualified_target.is_none() {
            return false;
        }
        let has_local = qualifier.is_none()
            && all
                .iter()
                .any(|(candidate_file, _, _)| *candidate_file == file_index);
        let visible = self.visible_paths(file_index);
        all.iter()
            .filter(|(candidate_file, _, _)| {
                if let Some(target) = qualified_target {
                    self.files[*candidate_file].path == target
                } else if has_local {
                    *candidate_file == file_index
                } else {
                    visible.contains(&self.files[*candidate_file].path)
                }
            })
            .any(|(_, _, parameter_types)| {
                parameter_types.len() == actual_types.len()
                    && parameter_types
                        .iter()
                        .zip(actual_types)
                        .all(|(parameter, actual)| ordinary_types_compatible(parameter, actual))
            })
    }
}

fn rewrite_kept_source(
    expansion: &Expansion,
    file_index: usize,
    source: &str,
    removals: &[std::ops::Range<usize>],
) -> Result<String, String> {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for range in removals {
        if range.start < cursor || range.end > source.len() {
            return Err("generic declaration range is invalid".to_string());
        }
        output.push_str(&ordinary_source_piece(
            expansion,
            file_index,
            &source[cursor..range.start],
        )?);
        cursor = range.end;
    }
    output.push_str(&ordinary_source_piece(
        expansion,
        file_index,
        &source[cursor..],
    )?);
    Ok(output)
}

fn ordinary_source_piece(
    expansion: &Expansion,
    file_index: usize,
    source: &str,
) -> Result<String, String> {
    let source = apply_replacements(
        source,
        &expansion.ordinary_type_replacements_for_source(file_index, source)?,
    )?;
    let mut environment = GenericEnvironment::default();
    environment.module_alias = Some(module_alias_for_path(&expansion.files[file_index].path));
    environment.source_path = Some(expansion.files[file_index].path.clone());
    let source = expansion.rewrite_type_applications_readonly(&source, &environment)?;
    rewrite_i32_constants(
        &source,
        &expansion.local_constant_values(file_index),
        &expansion.constant_rewrites_for_source(file_index, &source)?,
    )
}

impl Expansion {
    fn rewrite_type_applications_readonly(
        &self,
        source: &str,
        environment: &GenericEnvironment,
    ) -> Result<String, String> {
        let bytes = source.as_bytes();
        let mut output = String::with_capacity(source.len());
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            if bytes[cursor] == b'"' {
                let end = skip_string(source, cursor)?;
                output.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
            if starts_comment(source, cursor) {
                let end = skip_comment(source, cursor)?;
                output.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
            if !is_identifier_start(bytes[cursor]) {
                output.push(bytes[cursor] as char);
                cursor += 1;
                continue;
            }
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
                cursor += 1;
            }
            let qualified_end = qualified_identifier_end(source, start, cursor);
            let name = &source[start..qualified_end];
            let after = skip_ascii_whitespace(source, qualified_end);
            if self
                .lookup_generic_struct_for_environment(name, environment)?
                .is_some()
                && source.as_bytes().get(after) == Some(&b'<')
            {
                let close = matching_angle(source, after)?;
                let replacement =
                    self.materialize_type_readonly(&source[start..=close], environment)?;
                output.push_str(&replacement);
                cursor = close + 1;
            } else {
                output.push_str(&source[start..qualified_end]);
                cursor = qualified_end;
            }
        }
        Ok(output)
    }
}

fn generic_definition_identity(path: &str, name: &str) -> String {
    format!("{path}::{name}")
}

fn add_receiver_parameter(
    parameters: &mut Vec<ParsedGenericParameter>,
    name: &str,
    kind: ParsedGenericParameterKind,
) -> Result<(), String> {
    if let Some(existing) = parameters.iter().find(|parameter| parameter.name == name) {
        if existing.kind != kind {
            return Err(format!(
                "receiver generic name '{}' is bound with conflicting type and i32 kinds",
                name
            ));
        }
        return Ok(());
    }
    parameters.push(ParsedGenericParameter {
        name: name.to_string(),
        kind,
    });
    Ok(())
}

fn is_identifier_text(source: &str) -> bool {
    let bytes = source.trim().as_bytes();
    let Some(first) = bytes.first().copied() else {
        return false;
    };
    is_identifier_start(first) && bytes.iter().copied().all(is_identifier_char)
}

fn reject_explicit_generic_calls(
    source: &str,
    mut is_generic: impl FnMut(&str, Option<&str>) -> bool,
) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"' {
            cursor = skip_string(source, cursor)?;
            continue;
        }
        if starts_comment(source, cursor) {
            cursor = skip_comment(source, cursor)?;
            continue;
        }
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
            cursor += 1;
        }
        if preceded_by_keyword(source, start, "function") {
            continue;
        }
        let after_name = skip_ascii_whitespace(source, cursor);
        let (open, legacy) = if source.as_bytes().get(cursor) == Some(&b'<') {
            // Keep ordinary a < b > (c) expressions intact. The direct
            // generic-call spelling is recognized only when the angle group
            // is attached to the callee; the unambiguous :: <...> form
            // remains whitespace-tolerant for migration diagnostics.
            (cursor, false)
        } else if source.as_bytes().get(after_name) == Some(&b':')
            && source.as_bytes().get(after_name + 1) == Some(&b':')
        {
            let open = skip_ascii_whitespace(source, after_name + 2);
            (open, true)
        } else {
            continue;
        };
        if source.as_bytes().get(open) != Some(&b'<') {
            continue;
        }
        let close = matching_angle(source, open)?;
        let after = skip_ascii_whitespace(source, close + 1);
        if source.as_bytes().get(after) != Some(&b'(') {
            continue;
        }
        let name = source[start..cursor].to_string();
        let qualifier = preceding_qualified_name(source, start);
        if !is_generic(&name, qualifier.as_deref()) {
            cursor = after + 1;
            continue;
        }
        let callee = preceding_qualified_name(source, start)
            .map(|qualifier| format!("{}{}", qualifier, &source[start..cursor]))
            .unwrap_or_else(|| source[start..cursor].to_string());
        let spelling = if legacy { ":: <...>" } else { "<...>" };
        return Err(format!(
            "explicit generic function call '{}' using {} is not supported; remove the explicit arguments and infer from the first parameter's generic struct",
            callee, spelling
        ));
    }
    Ok(())
}

fn generic_function_identity(path: &str, signature: &ParsedFunctionSignature) -> String {
    // Keep source offsets out of the identity: edits to constants or comments
    // before a declaration must not orphan an otherwise reusable specialization.
    let mut identity = format!("{path}::{}|", signature.name);
    for parameter in &signature.generic_parameters {
        identity.push_str(match parameter.kind {
            ParsedGenericParameterKind::Type => "type",
            ParsedGenericParameterKind::I32 => "i32",
        });
        identity.push('|');
    }
    for parameter in &signature.params {
        identity.push_str(&parameter.type_name);
        identity.push('|');
    }
    identity.push_str(&signature.return_type_name);
    identity
}

pub(super) fn module_alias_for_path(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let raw = name.strip_suffix(".stasis").unwrap_or(name);
    let mut alias = raw
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect::<String>();
    if alias.is_empty() || alias.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        alias.insert(0, '_');
    }
    alias
}

fn is_concrete_only_function_name(name: &str) -> bool {
    matches!(
        name,
        "main"
            | "tick"
            | "render"
            | "on_code_swap"
            | "gfx_cmd_construction_reset"
            | "gfx_cmd_construction_finish"
    )
}

fn find_struct_definition_range(
    source: &str,
    name: &str,
    parameters: &[ParsedGenericParameter],
) -> Result<std::ops::Range<usize>, String> {
    for structure in parse_top_level_struct_definitions(source)? {
        if structure.name == name && structure.generic_parameters == parameters {
            return Ok(structure.definition_range);
        }
    }
    Err(format!(
        "unable to locate generic struct definition '{name}'"
    ))
}

fn strip_generic_declaration(source: &str, keyword: &str) -> Result<String, String> {
    let Some(keyword_start) = source.find(keyword) else {
        return Err(format!("generic declaration is missing '{keyword}'"));
    };
    let mut cursor = keyword_start + keyword.len();
    while source
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    loop {
        if source.as_bytes().get(cursor) != Some(&b'@') {
            break;
        }
        cursor += 1;
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(|byte| is_identifier_char(*byte))
        {
            cursor += 1;
        }
        cursor = skip_ascii_whitespace(source, cursor);
        if source.as_bytes().get(cursor) == Some(&b'(') {
            let close = find_matching_byte(source, cursor, b'(', b')')
                .ok_or_else(|| "unterminated function annotation".to_string())?;
            cursor = skip_ascii_whitespace(source, close + 1);
        }
    }
    while source
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| is_identifier_char(*byte))
    {
        cursor += 1;
    }
    let after_name = skip_ascii_whitespace(source, cursor);
    if source.as_bytes().get(after_name) != Some(&b'<') {
        return Ok(source.to_string());
    }
    let close = matching_angle(source, after_name)?;
    let mut output = String::with_capacity(source.len());
    output.push_str(&source[..cursor]);
    output.push_str(&source[close + 1..]);
    Ok(output)
}

fn rewrite_generic_identifiers(source: &str, environment: &GenericEnvironment) -> String {
    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"' {
            if let Ok(end) = skip_string(source, cursor) {
                output.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
        }
        if starts_comment(source, cursor) {
            if let Ok(end) = skip_comment(source, cursor) {
                output.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
        }
        if !is_identifier_start(bytes[cursor]) {
            output.push(bytes[cursor] as char);
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
            cursor += 1;
        }
        let identifier = &source[start..cursor];
        let previous = source[..start]
            .chars()
            .rev()
            .find(|character| !character.is_ascii_whitespace());
        if previous == Some('.') {
            output.push_str(identifier);
        } else if let Some(value) = environment.values.get(identifier) {
            output.push_str(&value.to_string());
        } else if let Some(value) = environment.types.get(identifier) {
            output.push_str(value);
        } else {
            output.push_str(identifier);
        }
    }
    output
}

fn rename_function_declaration(
    source: &str,
    old_name: &str,
    new_name: &str,
) -> Result<String, String> {
    if old_name == new_name {
        return Ok(source.to_string());
    }
    let Some(keyword_start) = source.find("function") else {
        return Err("specialized function is missing its declaration keyword".to_string());
    };
    let mut cursor = skip_ascii_whitespace(source, keyword_start + "function".len());
    loop {
        if source.as_bytes().get(cursor) != Some(&b'@') {
            break;
        }
        cursor += 1;
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(|byte| is_identifier_char(*byte))
        {
            cursor += 1;
        }
        cursor = skip_ascii_whitespace(source, cursor);
        if source.as_bytes().get(cursor) == Some(&b'(') {
            let close = find_matching_byte(source, cursor, b'(', b')')
                .ok_or_else(|| "unterminated function annotation".to_string())?;
            cursor = skip_ascii_whitespace(source, close + 1);
        }
    }
    let start = cursor;
    while source
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| is_identifier_char(*byte))
    {
        cursor += 1;
    }
    if &source[start..cursor] != old_name {
        return Err(format!(
            "specialized function declaration name mismatch: expected '{}', got '{}'",
            old_name,
            &source[start..cursor]
        ));
    }
    apply_replacements(source, &[(start, cursor, new_name.to_string())])
}

fn reject_value_parameter_writes(
    body: &str,
    parameters: &[ParsedGenericParameter],
) -> Result<(), String> {
    let value_parameters = parameters
        .iter()
        .filter(|parameter| parameter.kind == ParsedGenericParameterKind::I32)
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>();
    if value_parameters.is_empty() {
        return Ok(());
    }
    let bytes = body.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"' {
            cursor = skip_string(body, cursor)?;
            continue;
        }
        if starts_comment(body, cursor) {
            cursor = skip_comment(body, cursor)?;
            continue;
        }
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
            cursor += 1;
        }
        let identifier = &body[start..cursor];
        if !value_parameters
            .iter()
            .any(|parameter| *parameter == identifier)
        {
            continue;
        }
        let previous = body[..start]
            .chars()
            .rev()
            .find(|character| !character.is_ascii_whitespace());
        if previous == Some('.') {
            continue;
        }
        let operator_start = skip_ascii_whitespace(body, cursor);
        let assignment = match body.as_bytes().get(operator_start) {
            Some(b'=') => body.as_bytes().get(operator_start + 1) != Some(&b'='),
            Some(b'+' | b'-' | b'*' | b'/') => {
                body.as_bytes().get(operator_start + 1) == Some(&b'=')
            }
            _ => false,
        };
        if assignment {
            return Err(format!(
                "cannot assign to compile-time generic parameter '{identifier}'"
            ));
        }
    }
    Ok(())
}

fn preceding_qualified_name(source: &str, start: usize) -> Option<String> {
    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut prefix_start = start;
    loop {
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b'.' {
            break;
        }
        cursor -= 1;
        while cursor > 0 && is_identifier_char(bytes[cursor - 1]) {
            cursor -= 1;
        }
        prefix_start = cursor;
    }
    (prefix_start < start).then(|| {
        source[prefix_start..start]
            .trim_end_matches('.')
            .to_string()
    })
}

fn preceded_by_keyword(source: &str, start: usize, keyword: &str) -> bool {
    let prefix = source[..start].trim_end();
    if !prefix.ends_with(keyword) {
        return false;
    }
    let keyword_start = prefix.len() - keyword.len();
    &prefix[keyword_start..] == keyword
        && (keyword_start == 0 || !is_identifier_char(prefix.as_bytes()[keyword_start - 1]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InferredReceiverCall {
    receiver: String,
    name: String,
    arguments: Vec<String>,
    name_start: usize,
    name_end: usize,
}

fn collect_inferred_receiver_calls(source: &str) -> Result<Vec<InferredReceiverCall>, String> {
    let bytes = source.as_bytes();
    let mut calls = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"' {
            cursor = skip_string(source, cursor).unwrap_or(source.len());
            continue;
        }
        if starts_comment(source, cursor) {
            cursor = skip_comment(source, cursor).unwrap_or(source.len());
            continue;
        }
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }

        let mut segment_start = cursor;
        let mut segment_end = cursor + 1;
        while segment_end < bytes.len() && is_identifier_char(bytes[segment_end]) {
            segment_end += 1;
        }
        let mut receiver = source[segment_start..segment_end].to_string();
        let mut probe = segment_end;
        let mut found_call = false;
        loop {
            let dot = skip_ascii_whitespace(source, probe);
            if source.as_bytes().get(dot) != Some(&b'.') {
                break;
            }
            segment_start = skip_ascii_whitespace(source, dot + 1);
            if !source
                .as_bytes()
                .get(segment_start)
                .copied()
                .is_some_and(is_identifier_start)
            {
                break;
            }
            segment_end = segment_start + 1;
            while segment_end < bytes.len() && is_identifier_char(bytes[segment_end]) {
                segment_end += 1;
            }
            let after_segment = skip_ascii_whitespace(source, segment_end);
            if source.as_bytes().get(after_segment) == Some(&b'(') {
                let close =
                    find_matching_byte(source, after_segment, b'(', b')').ok_or_else(|| {
                        "unterminated receiver call while inferring generic arguments".to_string()
                    })?;
                calls.push(InferredReceiverCall {
                    receiver: receiver.clone(),
                    name: source[segment_start..segment_end].to_string(),
                    arguments: split_top_level_arguments(&source[after_segment + 1..close])?,
                    name_start: segment_start,
                    name_end: segment_end,
                });
                found_call = true;
                probe = segment_end;
                break;
            }
            receiver.push('.');
            receiver.push_str(&source[segment_start..segment_end]);
            probe = segment_end;
        }
        cursor = if found_call { probe } else { segment_end };
    }
    Ok(calls)
}

fn inferred_arguments(
    parameters: &[ParsedGenericParameter],
    environment: &GenericEnvironment,
) -> Option<Vec<ConcreteArgument>> {
    parameters
        .iter()
        .map(|parameter| match parameter.kind {
            ParsedGenericParameterKind::Type => environment
                .types
                .get(&parameter.name)
                .cloned()
                .map(ConcreteArgument::Type),
            ParsedGenericParameterKind::I32 => environment
                .values
                .get(&parameter.name)
                .copied()
                .map(ConcreteArgument::I32),
        })
        .collect()
}

fn bind_inferred_type(
    environment: &mut GenericEnvironment,
    name: &str,
    value: String,
) -> Result<bool, String> {
    if let Some(existing) = environment.types.get(name) {
        return Ok(existing == &value);
    }
    if environment.values.contains_key(name) {
        return Err(format!(
            "generic parameter '{}' was inferred as both a type and an i32 value",
            name
        ));
    }
    environment.types.insert(name.to_string(), value);
    Ok(true)
}

fn bind_inferred_value(
    environment: &mut GenericEnvironment,
    name: &str,
    value: i32,
) -> Result<bool, String> {
    if let Some(existing) = environment.values.get(name) {
        return Ok(*existing == value);
    }
    if environment.types.contains_key(name) {
        return Err(format!(
            "generic parameter '{}' was inferred as both an i32 value and a type",
            name
        ));
    }
    environment.values.insert(name.to_string(), value);
    Ok(true)
}

fn same_type_name(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn ordinary_types_compatible(parameter: &str, actual: &str) -> bool {
    if type_names_equivalent(parameter, actual) {
        return true;
    }
    let Some((parameter_element, parameter_extent)) = split_array_suffix(parameter) else {
        return false;
    };
    let Some((actual_element, actual_extent)) = split_array_suffix(actual) else {
        return false;
    };
    type_names_equivalent(parameter_element, actual_element)
        && (parameter_extent.trim().is_empty()
            || (!actual_extent.trim().is_empty()
                && type_names_equivalent(parameter_extent, actual_extent)))
}

fn contains_view_type(type_name: &str) -> bool {
    let trimmed = type_name.trim();
    if trimmed == "string" {
        return true;
    }
    if let Some((element, extent)) = split_array_suffix(trimmed) {
        return extent.trim().is_empty() || contains_view_type(element);
    }
    parse_type_application(trimmed)
        .ok()
        .flatten()
        .is_some_and(|(_, arguments)| {
            arguments
                .iter()
                .any(|argument| contains_view_type(argument))
        })
}

fn contains_void_type(type_name: &str) -> bool {
    let trimmed = type_name.trim();
    if trimmed == "void" {
        return true;
    }
    if let Some((element, _)) = split_array_suffix(trimmed) {
        return contains_void_type(element);
    }
    parse_type_application(trimmed)
        .ok()
        .flatten()
        .is_some_and(|(_, arguments)| {
            arguments
                .iter()
                .any(|argument| contains_void_type(argument))
        })
}

fn contains_type_name(type_name: &str, needle: &str) -> bool {
    let trimmed = type_name.trim();
    if trimmed == needle {
        return true;
    }
    if let Some((element, _)) = split_array_suffix(trimmed) {
        return contains_type_name(element, needle);
    }
    parse_type_application(trimmed)
        .ok()
        .flatten()
        .is_some_and(|(_, arguments)| {
            arguments
                .iter()
                .any(|argument| contains_type_name(argument, needle))
        })
}

fn type_names_equivalent(left: &str, right: &str) -> bool {
    if same_type_name(left, right) {
        return true;
    }
    let Some((left_base, left_arguments)) = parse_type_application(left).ok().flatten() else {
        return false;
    };
    let Some((right_base, right_arguments)) = parse_type_application(right).ok().flatten() else {
        return false;
    };
    left_base.rsplit('.').next() == right_base.rsplit('.').next()
        && left_arguments.len() == right_arguments.len()
        && left_arguments
            .iter()
            .zip(right_arguments.iter())
            .all(|(left, right)| type_names_equivalent(left, right))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InferredArgumentCall {
    name: String,
    arguments: Vec<String>,
    name_start: usize,
    name_end: usize,
}

fn collect_inferred_argument_calls(source: &str) -> Result<Vec<InferredArgumentCall>, String> {
    let bytes = source.as_bytes();
    let mut calls = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"' {
            cursor = skip_string(source, cursor)?;
            continue;
        }
        if starts_comment(source, cursor) {
            cursor = skip_comment(source, cursor)?;
            continue;
        }
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
            cursor += 1;
        }
        let after_name = skip_ascii_whitespace(source, cursor);
        if source.as_bytes().get(after_name) != Some(&b'(') {
            continue;
        }
        let previous = source[..start]
            .chars()
            .rev()
            .find(|character| !character.is_ascii_whitespace());
        if previous == Some('.') {
            continue;
        }
        if preceded_by_keyword(source, start, "function") {
            continue;
        }
        let close = find_matching_byte(source, after_name, b'(', b')')
            .ok_or_else(|| "unterminated call while inferring generic arguments".to_string())?;
        calls.push(InferredArgumentCall {
            name: source[start..cursor].to_string(),
            arguments: split_top_level_arguments(&source[after_name + 1..close])?,
            name_start: start,
            name_end: cursor,
        });
        cursor = after_name + 1;
    }
    Ok(calls)
}

fn split_top_level_arguments(source: &str) -> Result<Vec<String>, String> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut bracket = 0i32;
    for (index, byte) in source.as_bytes().iter().copied().enumerate() {
        match byte {
            b'<' => angle += 1,
            b'>' => angle -= 1,
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'[' => bracket += 1,
            b']' => bracket -= 1,
            b',' if angle == 0 && paren == 0 && bracket == 0 => {
                let part = source[start..index].trim();
                if part.is_empty() {
                    return Err("generic argument list contains an empty argument".to_string());
                }
                out.push(part.to_string());
                start = index + 1;
            }
            _ => {}
        }
    }
    let part = source[start..].trim();
    if part.is_empty() {
        return Err("generic argument list contains an empty argument".to_string());
    }
    out.push(part.to_string());
    Ok(out)
}

fn mangle_specialization(
    kind: &str,
    path: &str,
    name: &str,
    arguments: &[ConcreteArgument],
) -> String {
    let mut identity = format!("{kind}|{path}|{name}|");
    for argument in arguments {
        match argument {
            ConcreteArgument::Type(value) => {
                identity.push_str("T:");
                identity.push_str(value);
            }
            ConcreteArgument::I32(value) => {
                identity.push_str("I:");
                identity.push_str(&value.to_string());
            }
        }
        identity.push('|');
    }
    format!("__stasis_{kind}_{}", fnv1a(identity.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> String {
    let mut hash = 1469598103934665603u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

fn parse_type_application(source: &str) -> Result<Option<(&str, Vec<String>)>, String> {
    let trimmed = source.trim();
    let Some(open) = trimmed.find('<') else {
        return Ok(None);
    };
    let close = matching_angle(trimmed, open)?;
    if close + 1 != trimmed.len() {
        return Ok(None);
    }
    let base = trimmed[..open].trim();
    if base.is_empty() {
        return Err("generic type application is missing its base name".to_string());
    }
    Ok(Some((
        base,
        split_top_level_arguments(&trimmed[open + 1..close])?,
    )))
}

fn split_array_suffix(source: &str) -> Option<(&str, &str)> {
    let trimmed = source.trim();
    if !trimmed.ends_with(']') {
        return None;
    }
    let mut depth = 0i32;
    let mut open = None;
    for (index, byte) in trimmed.as_bytes().iter().copied().enumerate().rev() {
        match byte {
            b']' => depth += 1,
            b'[' => {
                depth -= 1;
                if depth == 0 {
                    open = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let open = open?;
    if open == 0 {
        return None;
    }
    Some((
        trimmed[..open].trim(),
        trimmed[open + 1..trimmed.len() - 1].trim(),
    ))
}

fn has_outer_parentheses(source: &str) -> bool {
    let trimmed = source.trim();
    if trimmed.len() < 2
        || !trimmed.starts_with('(')
        || !trimmed.ends_with(')')
        || find_matching_byte(trimmed, 0, b'(', b')') != Some(trimmed.len() - 1)
    {
        return false;
    }
    true
}

fn split_indexed_expression(source: &str) -> Option<(&str, &str)> {
    let trimmed = source.trim();
    let bytes = trimmed.as_bytes();
    let mut depth = 0i32;
    let mut open = None;
    let mut close = None;
    for index in 0..bytes.len() {
        match bytes[index] {
            b'[' => {
                if depth == 0 {
                    open = Some(index);
                }
                depth += 1;
            }
            b']' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
                if depth == 0 {
                    close = Some(index);
                }
            }
            _ => {}
        }
    }
    let (open, close) = open.zip(close)?;
    let collection = trimmed[..open].trim();
    let suffix = trimmed[close + 1..].trim();
    (!collection.is_empty() && (suffix.is_empty() || suffix.starts_with('.')))
        .then_some((collection, suffix))
}

fn split_binary_expression(source: &str) -> Option<(&str, char, &str)> {
    let bytes = source.as_bytes();
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut angle = 0i32;
    let mut candidate: Option<(usize, char, usize)> = None;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'[' => bracket += 1,
            b']' => bracket -= 1,
            b'<' if paren == 0 && bracket == 0 => angle += 1,
            b'>' if paren == 0 && bracket == 0 && angle > 0 => angle -= 1,
            b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>' | b'='
                if paren == 0 && bracket == 0 && angle == 0 =>
            {
                let byte = bytes[index];
                let unary = index == 0
                    || matches!(
                        bytes[index.saturating_sub(1)],
                        b'(' | b'[' | b',' | b'+' | b'-' | b'*' | b'/' | b'%'
                    );
                if !unary {
                    let width: usize =
                        if byte == b'=' && bytes.get(index + 1).copied() == Some(b'=') {
                            2
                        } else {
                            1
                        };
                    let precedence = match byte {
                        b'<' | b'>' | b'=' => 1,
                        b'+' | b'-' => 2,
                        _ => 3,
                    };
                    if candidate.is_none_or(|(_, _, old_precedence)| precedence <= old_precedence) {
                        candidate = Some((index, byte as char, width));
                    }
                    index += width;
                    continue;
                }
            }
            _ => {}
        }
        index += 1;
    }
    let (index, operator, width) = candidate?;
    let lhs = source[..index].trim();
    let rhs = source[index + width..].trim();
    (!lhs.is_empty() && !rhs.is_empty()).then_some((lhs, operator, rhs))
}

fn expression_mentions_unbound_value(
    source: &str,
    parameters: &[ParsedGenericParameter],
    environment: &GenericEnvironment,
) -> bool {
    let Ok(tokens) = tokenize_constant_expression(source) else {
        return false;
    };
    tokens.into_iter().any(|token| {
        let ConstantToken::Identifier(name) = token else {
            return false;
        };
        parameters.iter().any(|parameter| {
            parameter.kind == ParsedGenericParameterKind::I32
                && parameter.name == name
                && !environment.values.contains_key(&name)
        })
    })
}

fn is_type_argument_text(source: &str) -> bool {
    let trimmed = source.trim();
    let Some(first) = trimmed.as_bytes().first().copied() else {
        return false;
    };
    if !is_identifier_start(first) {
        return false;
    }
    let mut angle = 0i32;
    let mut bracket = 0i32;
    for byte in trimmed.bytes() {
        match byte {
            b'<' => angle += 1,
            b'>' => angle -= 1,
            b'[' => bracket += 1,
            b']' => bracket -= 1,
            b'.' | b'_' if angle == 0 && bracket == 0 => {}
            byte if is_identifier_char(byte) || byte.is_ascii_whitespace() => {}
            b',' if angle > 0 => {}
            _ => return false,
        }
        if angle < 0 || bracket < 0 {
            return false;
        }
    }
    angle == 0 && bracket == 0
}

fn matching_angle(source: &str, open: usize) -> Result<usize, String> {
    if source.as_bytes().get(open) != Some(&b'<') {
        return Err("internal error: expected '<'".to_string());
    }
    let mut depth = 0i32;
    let mut cursor = open;
    while cursor < source.len() {
        match source.as_bytes()[cursor] {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(cursor);
                }
            }
            b'"' => cursor = skip_string(source, cursor)?.saturating_sub(1),
            _ => {}
        }
        cursor += 1;
    }
    Err("missing closing '>' in generic argument list".to_string())
}

fn constant_definition_identity(definition: &ConstantDefinition) -> String {
    format!("{}::{}", definition.path, definition.name)
}

fn ordinary_struct_generated_name(definition: &OrdinaryStructDefinition) -> String {
    format!(
        "__module_type_{:016x}",
        hash_text(&format!("{}::{}", definition.path, definition.name))
    )
}

fn type_identifier_ranges(source: &str) -> Result<Vec<(usize, usize)>, String> {
    let tokens = lex(source)?;
    let mut ranges = Vec::new();
    let mut in_type = false;
    let mut angle_depth = 0usize;
    let mut square_depth = 0usize;
    let mut index = 0usize;
    while index < tokens.len() {
        let token = tokens[index];
        let text = &source[token.start..token.end];
        if token.kind == TokenKind::Identifier && text == "struct" {
            if let Some(name) = tokens
                .get(index + 1)
                .filter(|candidate| candidate.kind == TokenKind::Identifier)
            {
                ranges.push((name.start, name.end));
            }
        }
        if token.kind == TokenKind::Colon {
            in_type = true;
            angle_depth = 0;
            square_depth = 0;
            index += 1;
            continue;
        }
        if !in_type {
            index += 1;
            continue;
        }
        if token.kind == TokenKind::Other {
            match text {
                "<" => angle_depth += 1,
                ">" => angle_depth = angle_depth.saturating_sub(1),
                "[" => square_depth += 1,
                "]" => square_depth = square_depth.saturating_sub(1),
                "=" if angle_depth == 0 && square_depth == 0 => in_type = false,
                _ => {}
            }
        }
        if angle_depth == 0
            && square_depth == 0
            && matches!(
                token.kind,
                TokenKind::Comma | TokenKind::RParen | TokenKind::Semicolon | TokenKind::LBrace
            )
        {
            in_type = false;
            index += 1;
            continue;
        }
        if token.kind == TokenKind::Identifier && angle_depth == 0 && square_depth == 0 {
            if tokens.get(index + 1).is_some_and(|candidate| {
                candidate.kind == TokenKind::Other && &source[candidate.start..candidate.end] == "."
            }) && tokens
                .get(index + 2)
                .is_some_and(|candidate| candidate.kind == TokenKind::Identifier)
            {
                let end = tokens[index + 2].end;
                ranges.push((token.start, end));
                index += 3;
                continue;
            }
            ranges.push((token.start, token.end));
        }
        index += 1;
    }
    ranges.sort_unstable();
    ranges.dedup();
    Ok(ranges)
}

fn constant_identifier_ranges(
    source: &str,
    candidate_names: &BTreeSet<&str>,
) -> Result<Vec<(usize, usize)>, String> {
    if candidate_names.is_empty() {
        return Ok(Vec::new());
    }
    let tokens = lex(source)?;
    let locals = crate::frontend::parser::parse_local_declarations(source).unwrap_or_default();
    let functions = parse_top_level_functions(source).unwrap_or_default();
    let mut enum_ranges = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        if token.kind != TokenKind::Identifier || &source[token.start..token.end] != "enum" {
            continue;
        }
        let Some(open_index) = (index + 1..tokens.len())
            .find(|candidate| tokens[*candidate].kind == TokenKind::LBrace)
        else {
            continue;
        };
        let mut depth = 0usize;
        for candidate in &tokens[open_index..] {
            match candidate.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        enum_ranges.push(token.start..candidate.end);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let mut ranges = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let name = &source[token.start..token.end];
        if !candidate_names.contains(name) {
            continue;
        }
        let previous = index.checked_sub(1).and_then(|value| tokens.get(value));
        let next = tokens.get(index + 1);
        let followed_by_assignment = next.is_some_and(|candidate| {
            candidate.kind == TokenKind::Other
                && &source[candidate.start..candidate.end] == "="
                && !tokens.get(index + 2).is_some_and(|following| {
                    following.kind == TokenKind::Other
                        && &source[following.start..following.end] == "="
                })
        });
        let is_constant_declaration = previous.is_some_and(|candidate| {
            candidate.kind == TokenKind::Identifier
                && &source[candidate.start..candidate.end] == "const"
        });
        if !is_constant_declaration {
            if enum_ranges
                .iter()
                .any(|range| range.start <= token.start && token.end <= range.end)
            {
                continue;
            }
            if previous.is_some_and(|candidate| {
                candidate.kind == TokenKind::Other && &source[candidate.start..candidate.end] == "."
            }) || next.is_some_and(|candidate| {
                matches!(
                    candidate.kind,
                    TokenKind::LParen | TokenKind::LBrace | TokenKind::Colon
                ) || (candidate.kind == TokenKind::Other
                    && &source[candidate.start..candidate.end] == ".")
            }) || previous.is_some_and(|candidate| {
                candidate.kind == TokenKind::Identifier
                    && matches!(
                        &source[candidate.start..candidate.end],
                        "struct" | "enum" | "global" | "let"
                    )
            }) || followed_by_assignment
            {
                continue;
            }
            if locals.iter().any(|local| {
                local.name == name
                    && local.visibility_range.start <= token.start
                    && token.end <= local.visibility_range.end
            }) {
                continue;
            }
            if functions.iter().any(|function| {
                function.body_range.start <= token.start
                    && token.end <= function.body_range.end
                    && function
                        .params
                        .iter()
                        .any(|parameter| parameter.name == name)
            }) {
                continue;
            }
        }
        ranges.push((token.start, token.end));
    }
    Ok(ranges)
}

fn sorted_paths<'a>(paths: impl Iterator<Item = &'a str>) -> String {
    let mut paths = paths.collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    paths.join(", ")
}

fn visible_constant_definition_in<'a>(
    name: &str,
    source_path: &str,
    definitions: &'a [ConstantDefinition],
    module_graph: &ModuleGraph,
) -> Result<Option<&'a ConstantDefinition>, String> {
    let local = definitions
        .iter()
        .filter(|definition| {
            definition.path == source_path
                && definition.name == name
                && definition.type_name.trim() == "i32"
        })
        .collect::<Vec<_>>();
    let matches = if local.is_empty() {
        let visible = module_graph.dependency_closure(source_path);
        definitions
            .iter()
            .filter(|definition| {
                visible.contains(&definition.path)
                    && definition.name == name
                    && definition.type_name.trim() == "i32"
            })
            .collect::<Vec<_>>()
    } else {
        local
    };
    match matches.as_slice() {
        [] => Ok(None),
        [definition] => Ok(Some(*definition)),
        _ => Err(format!(
            "ambiguous compile-time constant '{}' from '{}': {}",
            name,
            source_path,
            sorted_paths(matches.iter().map(|definition| definition.path.as_str()))
        )),
    }
}

fn evaluate_constant_definition(
    definition: &ConstantDefinition,
    definitions: &[ConstantDefinition],
    module_graph: &ModuleGraph,
    resolved: &mut BTreeMap<String, i32>,
    stack: &mut Vec<String>,
    steps: &mut usize,
) -> Result<i32, String> {
    let identity = constant_definition_identity(definition);
    if let Some(value) = resolved.get(&identity) {
        return Ok(*value);
    }
    if stack.iter().any(|entry| entry == &identity) {
        let mut chain = stack.clone();
        chain.push(identity.clone());
        return Err(format!("constant reference cycle: {}", chain.join(" -> ")));
    }
    stack.push(identity.clone());
    let mut environment = GenericEnvironment::default();
    environment.source_path = Some(definition.path.clone());
    let tokens = tokenize_constant_expression(&definition.value_text)?;
    for token in tokens {
        let ConstantToken::Identifier(identifier) = token else {
            continue;
        };
        if identifier == definition.name || environment.values.contains_key(&identifier) {
            continue;
        }
        if let Some(candidate) = visible_constant_definition_in(
            &identifier,
            &definition.path,
            definitions,
            module_graph,
        )? {
            let value = evaluate_constant_definition(
                candidate,
                definitions,
                module_graph,
                resolved,
                stack,
                steps,
            )?;
            environment.values.insert(identifier, value);
        }
    }
    let value = evaluate_i32_expression_with_steps(
        &definition.value_text,
        &environment,
        &BTreeMap::new(),
        steps,
    )?;
    stack.pop();
    resolved.insert(identity, value);
    Ok(value)
}

fn evaluate_i32_expression(
    source: &str,
    environment: &GenericEnvironment,
    constants: &BTreeMap<String, i32>,
) -> Result<i32, String> {
    let mut steps = 0;
    evaluate_i32_expression_with_steps(source, environment, constants, &mut steps)
}

fn evaluate_i32_expression_with_steps(
    source: &str,
    environment: &GenericEnvironment,
    constants: &BTreeMap<String, i32>,
    steps: &mut usize,
) -> Result<i32, String> {
    let mut parser = ConstantParser::new(source, environment, constants, steps)?;
    let value = parser.parse_expression(0)?;
    if parser.peek().is_some() {
        return Err(format!(
            "unexpected token in compile-time expression '{source}'"
        ));
    }
    Ok(value)
}

struct ConstantParser<'a> {
    tokens: Vec<ConstantToken>,
    cursor: usize,
    environment: &'a GenericEnvironment,
    constants: &'a BTreeMap<String, i32>,
    steps: &'a mut usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConstantToken {
    Integer(String),
    Identifier(String),
    Operator(char),
    Open,
    Close,
}

impl<'a> ConstantParser<'a> {
    fn new(
        source: &str,
        environment: &'a GenericEnvironment,
        constants: &'a BTreeMap<String, i32>,
        steps: &'a mut usize,
    ) -> Result<Self, String> {
        Ok(Self {
            tokens: tokenize_constant_expression(source)?,
            cursor: 0,
            environment,
            constants,
            steps,
        })
    }

    fn peek(&self) -> Option<&ConstantToken> {
        self.tokens.get(self.cursor)
    }

    fn parse_expression(&mut self, min_precedence: u8) -> Result<i32, String> {
        let mut lhs = self.parse_prefix()?;
        loop {
            let Some(ConstantToken::Operator(operator)) = self.peek() else {
                break;
            };
            let precedence = match operator {
                '+' | '-' => 10,
                '*' | '/' | '%' => 20,
                _ => break,
            };
            if precedence < min_precedence {
                break;
            }
            let operator = *operator;
            self.cursor += 1;
            let rhs = self.parse_expression(precedence + 1)?;
            lhs = checked_binary(lhs, operator, rhs)?;
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<i32, String> {
        *self.steps = self.steps.saturating_add(1);
        if *self.steps > MAX_CONSTANT_EVALUATION_STEPS {
            return Err(format!(
                "compile-time expression evaluation exceeded {} steps",
                MAX_CONSTANT_EVALUATION_STEPS
            ));
        }
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
                .environment
                .values
                .get(&name)
                .copied()
                .or_else(|| self.constants.get(&name).copied())
                .ok_or_else(|| format!("unknown compile-time value '{name}'")),
            ConstantToken::Operator('+') => self.parse_prefix(),
            ConstantToken::Operator('-') => {
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
                let value = self.parse_prefix()?;
                value
                    .checked_neg()
                    .ok_or_else(|| "compile-time negation overflowed i32".to_string())
            }
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

fn checked_binary(lhs: i32, operator: char, rhs: i32) -> Result<i32, String> {
    match operator {
        '+' => lhs.checked_add(rhs),
        '-' => lhs.checked_sub(rhs),
        '*' => lhs.checked_mul(rhs),
        '/' => lhs.checked_div(rhs),
        '%' => lhs.checked_rem(rhs),
        _ => None,
    }
    .ok_or_else(|| {
        format!(
            "checked compile-time operation '{}' overflowed or divided by zero",
            operator
        )
    })
}

fn tokenize_constant_expression(source: &str) -> Result<Vec<ConstantToken>, String> {
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
        if is_identifier_start(bytes[cursor]) {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
                cursor += 1;
            }
            tokens.push(ConstantToken::Identifier(source[start..cursor].to_string()));
            continue;
        }
        let token = match bytes[cursor] {
            b'+' | b'-' | b'*' | b'/' | b'%' => ConstantToken::Operator(bytes[cursor] as char),
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

fn rewrite_i32_constants(
    source: &str,
    constants: &BTreeMap<String, i32>,
    renames: &BTreeMap<String, (String, i32)>,
) -> Result<String, String> {
    let tokens = lex(source)?;
    let mut replacements = Vec::<(usize, usize, String)>::new();
    let mut initializer_ranges = Vec::<std::ops::Range<usize>>::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        if token.kind != TokenKind::Identifier || &source[token.start..token.end] != "const" {
            continue;
        }
        let Some(name) = tokens.get(index + 1).copied() else {
            continue;
        };
        if name.kind != TokenKind::Identifier {
            continue;
        }
        let Some(colon) = tokens
            .get(index + 2)
            .filter(|candidate| candidate.kind == TokenKind::Colon)
            .map(|_| index + 2)
        else {
            continue;
        };
        let type_start = tokens.get(colon + 1).map_or(0, |candidate| candidate.start);
        let Some(equals_index) = (colon + 1..tokens.len()).find(|candidate| {
            tokens[*candidate].kind == TokenKind::Other
                && &source[tokens[*candidate].start..tokens[*candidate].end] == "="
        }) else {
            continue;
        };
        let type_name = source[type_start..tokens[equals_index].start].trim();
        let Some(semicolon_index) = (equals_index + 1..tokens.len())
            .find(|candidate| tokens[*candidate].kind == TokenKind::Semicolon)
        else {
            continue;
        };
        if type_name != "i32" {
            continue;
        }
        let constant_name = &source[name.start..name.end];
        let Some(value) = constants.get(constant_name) else {
            continue;
        };
        initializer_ranges.push(tokens[equals_index].end..tokens[semicolon_index].start);
        replacements.push((
            tokens[equals_index].end,
            tokens[semicolon_index].start,
            format!(" {value} "),
        ));
    }
    let rename_names = renames.keys().map(String::as_str).collect::<BTreeSet<_>>();
    for (start, end) in constant_identifier_ranges(source, &rename_names)? {
        if initializer_ranges
            .iter()
            .any(|range| range.start <= start && end <= range.end)
        {
            continue;
        }
        let name = &source[start..end];
        let Some((generated_name, _)) = renames.get(name) else {
            continue;
        };
        replacements.push((start, end, generated_name.clone()));
    }
    replacements.sort_by_key(|(start, end, _)| (*start, *end));
    apply_replacements(source, &replacements)
}

fn apply_replacements(
    source: &str,
    replacements: &[(usize, usize, String)],
) -> Result<String, String> {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for (start, end, replacement) in replacements {
        if *start < cursor || *end > source.len() {
            return Err("overlapping source replacement".to_string());
        }
        output.push_str(&source[cursor..*start]);
        output.push_str(replacement);
        cursor = *end;
    }
    output.push_str(&source[cursor..]);
    Ok(output)
}

fn find_matching_byte(source: &str, open: usize, opening: u8, closing: u8) -> Option<usize> {
    if source.as_bytes().get(open) != Some(&opening) {
        return None;
    }
    let mut depth = 0i32;
    let mut cursor = open;
    while cursor < source.len() {
        match source.as_bytes()[cursor] {
            byte if byte == opening => depth += 1,
            byte if byte == closing => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            b'"' => cursor = skip_string(source, cursor).ok()?.saturating_sub(1),
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn starts_comment(source: &str, cursor: usize) -> bool {
    source
        .as_bytes()
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| bytes == b"//" || bytes == b"/*")
}

fn skip_comment(source: &str, cursor: usize) -> Result<usize, String> {
    if source.as_bytes().get(cursor..cursor + 2) == Some(b"//") {
        return Ok(source[cursor..]
            .find('\n')
            .map_or(source.len(), |offset| cursor + offset));
    }
    let Some(end) = source[cursor + 2..].find("*/") else {
        return Err("unterminated block comment".to_string());
    };
    Ok(cursor + 2 + end + 2)
}

fn skip_string(source: &str, cursor: usize) -> Result<usize, String> {
    let bytes = source.as_bytes();
    let mut index = cursor + 1;
    let mut escaped = false;
    while index < bytes.len() {
        if escaped {
            escaped = false;
        } else if bytes[index] == b'\\' {
            escaped = true;
        } else if bytes[index] == b'"' {
            return Ok(index + 1);
        }
        index += 1;
    }
    Err("unterminated string literal".to_string())
}

fn skip_ascii_whitespace(source: &str, mut cursor: usize) -> usize {
    while cursor < source.len() && source.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn qualified_identifier_end(source: &str, _start: usize, mut cursor: usize) -> usize {
    let bytes = source.as_bytes();
    loop {
        let dot = skip_ascii_whitespace(source, cursor);
        if bytes.get(dot) != Some(&b'.') {
            return cursor;
        }
        let segment_start = skip_ascii_whitespace(source, dot + 1);
        if !bytes
            .get(segment_start)
            .copied()
            .is_some_and(is_identifier_start)
        {
            return cursor;
        }
        cursor = segment_start + 1;
        while cursor < bytes.len() && is_identifier_char(bytes[cursor]) {
            cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_value_generic_structs_and_functions_before_indexing() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { count: i32; values: i32[N]; }\nglobal samples: Buffer<4>;\nfunction clear(self: Buffer<N>): void { self.count = 0; }\nfunction capacity(buffer: Buffer<N>): i32 { return N; }\nfunction reset(): void { samples.clear(); }\nfunction main(): i32 { reset(); return capacity(samples); }\n",
        );
        compiler.check().expect("generic program compiles");
        let file = &compiler.files()[0];
        assert!(!file.content.contains("Buffer<N"));
        assert!(file.content.contains("values: i32[4]"));
        assert!(compiler
            .functions()
            .iter()
            .any(|function| function.name.starts_with("__stasis_function_")));
        assert!(compiler
            .functions()
            .iter()
            .any(|function| function.name.starts_with("__stasis_function_")));
    }

    #[test]
    fn concrete_generic_program_executes_through_jit() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { count: i32; values: i32[N]; }\nglobal samples: Buffer<4>;\nfunction clear(self: Buffer<N>): void { self.count = 0; }\nfunction capacity(buffer: Buffer<N>): i32 { return N; }\nfunction main(): i32 { samples.clear(); return capacity(samples); }\n",
        );
        process.compile().expect("generic JIT program compiles");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("generic JIT program executes"),
            4
        );
    }

    #[test]
    fn keeps_multiple_receiver_value_specializations_distinct() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal first: Buffer<4>;\nglobal second: Buffer<8>;\nfunction capacity(buffer: Buffer<N>): i32 { return N; }\nfunction main(): i32 { return capacity(first) + second.capacity(); }\n",
        );
        process
            .compile()
            .expect("multiple receiver-inferred generic calls compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("multiple receiver-inferred generic calls execute"),
            12
        );
    }

    #[test]
    fn infers_value_arguments_from_distinct_receiver_applications() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal first: Buffer<4>;\nglobal second: Buffer<8>;\nfunction capacity(buffer: Buffer<N>): i32 { return N; }\nfunction main(): i32 { return first.capacity() + second.capacity(); }\n",
        );
        process
            .compile()
            .expect("receiver inference for generic values");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("receiver-inferred generic calls execute"),
            12
        );
    }

    #[test]
    fn rejects_generic_names_bound_only_by_a_fixed_array_view() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "global values: i32[6];\nfunction extent(items: i32[N]): i32 { return N; }\nfunction main(): i32 { return extent(values); }\n",
        );
        let error = compiler
            .check()
            .expect_err("fixed-array-only generic binding must fail");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn rejects_generic_names_bound_only_by_a_nested_fixed_array_view() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "struct State { values: i32[5]; }\nglobal state: State;\nfunction extent(items: i32[N]): i32 { return N; }\nfunction main(): i32 { return extent(state.values); }\n",
        );
        let error = compiler
            .check()
            .expect_err("nested fixed-array-only generic binding must fail");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn lowers_value_generic_storage_through_aot_and_wasm() {
        let source = "struct Buffer<N: i32> { values: i32[N]; }\nglobal samples: Buffer<4>;\nglobal other: Buffer<8>;\nfunction value(buffer: Buffer<N>): i32 { return buffer.values[0] + N; }\nfunction main(): i32 { samples.values[0] = 7; return value(samples) + value(other); }\n";

        let mut aot = crate::backend::aot::AotProcess::new();
        aot.set_required_emit_roots(&["main".to_string()]);
        aot.upsert_file("main.stasis", source);
        let report = aot.compile().expect("generic AOT compile");
        assert_eq!(report.emit.emitted_functions, 3);
        assert_eq!(aot.artifacts().len(), 3);
        assert!(aot
            .artifacts()
            .iter()
            .all(|artifact| artifact.object_bytes_len > 0));

        let mut wasm = crate::backend::wasm::WasmProcess::new();
        wasm.set_required_emit_roots(&["main".to_string()]);
        wasm.upsert_file("main.stasis", source);
        wasm.compile().expect("generic Wasm compile");
        assert!(wasm.module_bytes().starts_with(b"\0asm\x01\0\0\0"));

        let wasm_path = std::env::temp_dir().join(format!(
            "stasis_wasm_generic_parity_{}_{}.wasm",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&wasm_path, wasm.module_bytes()).expect("write generic parity wasm");
        let output = std::process::Command::new("node")
            .args([
                "-e",
                "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance}) => process.stdout.write(String(instance.exports.main()))).catch((error) => { console.error(error); process.exit(1); });",
            ])
            .arg(&wasm_path)
            .output()
            .expect("run Node for generic parity wasm");
        let _ = std::fs::remove_file(&wasm_path);
        assert!(
            output.status.success(),
            "Node failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "19");

        let mut jit = crate::backend::jit::JitProcess::new();
        jit.set_required_emit_roots(&["main".to_string()]);
        jit.upsert_file("main.stasis", source);
        jit.compile().expect("generic JIT compile");
        assert_eq!(
            jit.execute_i32_noarg_by_name("main")
                .expect("generic JIT execution"),
            19
        );
        assert_eq!(jit.artifacts().len(), 3);
    }

    #[test]
    fn rejects_generic_host_entries_and_externs() {
        let mut host_entry = crate::compiler::Compiler::new();
        host_entry.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal main_buffer: Buffer<4>;\nfunction main(buffer: Buffer<N>): i32 { return N; }\n",
        );
        let error = host_entry
            .check()
            .expect_err("generic host entry must require a concrete wrapper");
        assert!(format!("{error:?}").contains("cannot be a lifecycle or host entry"));

        let mut extern_function = crate::compiler::Compiler::new();
        extern_function.upsert_file(
            "main.stasis",
            "extern function host<N: i32>(): i32;\nfunction main(): i32 { return 0; }\n",
        );
        let error = extern_function
            .check()
            .expect_err("generic extern must require a concrete wrapper");
        assert!(format!("{error:?}").contains("cannot be a host declaration"));
    }

    #[test]
    fn canonicalizes_equal_value_arguments_and_rejects_runtime_or_wrong_kind_values() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "const CAPACITY: i32 = 12 + 12;\nstruct Rig<N: i32> { values: i32[N]; }\nglobal first: Rig<24>;\nglobal second: Rig<CAPACITY>;\nglobal third: Rig<12 + 12>;\nfunction main(): i32 { return first.values[0] + second.values[0] + third.values[0]; }\n",
        );
        compiler
            .check()
            .expect("equivalent generic arguments compile");
        let file = &compiler.files()[0].content;
        let generated_types = file
            .lines()
            .filter(|line| line.starts_with("struct __stasis_type_"))
            .collect::<Vec<_>>();
        assert_eq!(generated_types.len(), 1);

        let mut runtime = crate::compiler::Compiler::new();
        runtime.upsert_file(
            "runtime.stasis",
            "struct Rig<N: i32> { values: i32[N]; }\nglobal source: i32;\nglobal invalid: Rig<source>;\nfunction main(): i32 { return 0; }\n",
        );
        let error = runtime
            .check()
            .expect_err("runtime generic argument must fail");
        assert!(format!("{error:?}").contains("compile-time"));

        let mut wrong_kind = crate::compiler::Compiler::new();
        wrong_kind.upsert_file(
            "wrong_kind.stasis",
            "struct Box<T: type> { value: T; }\nglobal invalid: Box<24>;\nfunction main(): i32 { return 0; }\n",
        );
        let error = wrong_kind
            .check()
            .expect_err("value passed to a type parameter must fail");
        assert!(format!("{error:?}").contains("expects a type argument"));
    }

    #[test]
    fn accepts_i32_minimum_and_rejects_negative_array_extent_after_substitution() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "offset.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal minimum: Buffer<-2147483648>;\nfunction offset(buffer: Buffer<N>): i32 { return N; }\nfunction main(): i32 { return offset(minimum); }\n",
        );
        compiler.check().expect("minimum i32 value is valid");

        let mut negative_extent = crate::compiler::Compiler::new();
        negative_extent.upsert_file(
            "negative.stasis",
            "struct Buffer<N: i32> { values: i32[N]; }\nglobal invalid: Buffer<-1>;\nfunction main(): i32 { return 0; }\n",
        );
        let error = negative_extent
            .check()
            .expect_err("negative generic extent must fail");
        assert!(format!("{error:?}").contains("negative array extent"));
    }

    #[test]
    fn rejects_explicit_generic_calls_before_specialization() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "recursive.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal buffer: Buffer<4>;\nfunction loop(value: Buffer<N>): i32 { return loop::<N + 1>(value); }\nfunction main(): i32 { return loop(buffer); }\n",
        );
        let error = compiler
            .check()
            .expect_err("explicit generic call must be rejected");
        assert!(format!("{error:?}").contains("explicit generic function call"));
    }

    #[test]
    fn rejects_writes_to_specialized_value_parameters() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "write.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal buffer: Buffer<4>;\nfunction mutate(value: Buffer<N>): void { N = 3; return; }\nfunction main(): i32 { mutate(buffer); return 0; }\n",
        );
        let error = compiler
            .check()
            .expect_err("generic value parameters are immutable");
        assert!(format!("{error:?}").contains("cannot assign to compile-time generic parameter"));
    }

    #[test]
    fn defers_malformed_sources_to_canonical_parser_diagnostics() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file("broken.stasis", "function broken(: i32): void {}\n");
        compiler
            .check()
            .expect_err("malformed source must fail in the canonical parser");
        let diagnostic = compiler
            .last_source_diagnostic()
            .expect("canonical parser diagnostic");
        assert_eq!(diagnostic.path, "broken.stasis");
        assert_eq!(diagnostic.symbol, "broken");
        assert_eq!(diagnostic.code, crate::SourceDiagnosticCode::Parse);
    }

    #[test]
    fn evaluates_checked_i32_expressions() {
        let environment = GenericEnvironment::default();
        let constants = BTreeMap::new();
        assert_eq!(
            evaluate_i32_expression("12 + 12", &environment, &constants),
            Ok(24)
        );
        assert_eq!(
            evaluate_i32_expression("-(12 + 12)", &environment, &constants),
            Ok(-24)
        );
        assert!(evaluate_i32_expression("2147483647 + 1", &environment, &constants).is_err());
        assert!(evaluate_i32_expression("1 / 0", &environment, &constants).is_err());
        assert!(evaluate_i32_expression("-2147483648 / -1", &environment, &constants).is_err());
    }

    #[test]
    fn splits_nested_generic_arguments() {
        let parsed = parse_type_application("Outer<Inner<4>, 8>")
            .expect("parse application")
            .expect("application");
        assert_eq!(parsed.0, "Outer");
        assert_eq!(parsed.1, vec!["Inner<4>".to_string(), "8".to_string()]);
    }

    #[test]
    fn specializes_type_and_mixed_parameters_from_a_receiver() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             global integers: Buffer<i32, 4>;\n\
             global floats: Buffer<f32, 8>;\n\
             function capacity(self: Buffer<T, N>): i32 { return N; }\n\
             function main(): i32 { return integers.capacity() + floats.capacity(); }\n",
        );
        process
            .compile()
            .expect("mixed type/value receiver inference");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("mixed generic receiver calls execute"),
            12
        );
    }

    #[test]
    fn rejects_generic_names_bound_only_by_a_runtime_view() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "global values: i32[6];\n\
             function element(items: T[]): T { return items[0]; }\n\
             function main(): i32 { return element(values); }\n",
        );
        let error = compiler
            .check()
            .expect_err("runtime-view-only generic binding must fail");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn rejects_runtime_view_when_a_fixed_extent_is_required_for_inference() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "global values: i32[];\n\
             function extent(items: T[N]): i32 { return N; }\n\
             function main(): i32 { return extent(values); }\n",
        );
        let error = compiler
            .check()
            .expect_err("runtime view cannot infer a fixed extent");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn rejects_generic_helpers_not_bound_by_the_first_parameter() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "global values: i32[6];\n\
             function extent(items: T[N]): i32 { return N; }\n\
             function forward(items: T[N]): i32 { return extent(items); }\n\
             function main(): i32 { return forward(values); }\n",
        );
        let error = compiler
            .check()
            .expect_err("raw-view generic helper must fail");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn forwards_inferred_bindings_through_specialized_struct_parameters() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             global values: Buffer<i32, 6>;\n\
             function extent(items: Buffer<T, N>): i32 { return N; }\n\
             function forward(items: Buffer<U, M>): i32 { return extent(items); }\n\
             function main(): i32 { return forward(values); }\n",
        );
        process
            .compile()
            .expect("specialized struct parameter inference forwards");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("forwarded specialized struct call executes"),
            6
        );
    }

    #[test]
    fn preserves_nested_generic_struct_element_paths() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Item<T: type> { value: T; }\n\
             struct Box<T: type, N: i32> { items: Item<T>[N]; }\n\
             global values: Box<i32, 2>;\n\
             function first(self: Box<T, N>): T { return self.items[0].value; }\n\
             function main(): i32 { return values.first(); }\n",
        );
        process
            .compile()
            .expect("nested generic struct element paths compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("nested generic struct element path executes"),
            0
        );
    }

    #[test]
    fn rejects_conflicting_repeated_type_bindings() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "struct Box<T: type> { value: T; }\n\
             global integer: Box<i32>;\n\
             global float: f32;\n\
             function same(left: Box<T>, right: T): T { return left.value; }\n\
             function main(): i32 { return same(integer, float); }\n",
        );
        let error = compiler
            .check()
            .expect_err("later generic bindings must agree with the receiver");
        assert!(format!("{error:?}").contains("could not infer"));
    }

    #[test]
    fn keeps_same_named_generic_declarations_distinct_across_modules() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"left/left_box.stasis\";\n\
             import \"right/right_box.stasis\";\n\
             global left_value: left_box.Buffer<i32, 4>;\n\
             global right_value: right_box.Buffer<f32, 8>;\n\
             function main(): i32 { return left_box.capacity(left_value) + right_box.capacity(right_value); }\n",
        );
        process.upsert_file(
            "left/left_box.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             function capacity(self: Buffer<T, N>): i32 { return N; }\n",
        );
        process.upsert_file(
            "right/right_box.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             function capacity(self: Buffer<T, N>): i32 { return N; }\n",
        );
        process
            .compile()
            .expect("same-named generic declarations remain module-local");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("module-local generic receiver calls execute"),
            12
        );
    }

    #[test]
    fn rewrites_nested_receiver_calls_between_generic_methods() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"library.stasis\";\n\
             global value: library.Box<4>;\n\
             function main(): i32 { return value.forward(); }\n",
        );
        process.upsert_file(
            "library.stasis",
            "struct Item { score: i32; }\n\
             struct Box<N: i32> { values: Item[N]; }\n\
             function read(self: Box<N>, score: i32): i32 { return N + score; }\n\
             function forward(self: Box<N>): i32 { return self.read(self.values[0].score + 1); }\n",
        );
        process
            .compile()
            .expect("generic methods may call another method through their receiver");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("nested generic receiver call executes"),
            5
        );
    }

    #[test]
    fn rejects_view_and_void_substitutions_in_stored_generic_fields() {
        let mut view = crate::compiler::Compiler::new();
        view.upsert_file(
            "view.stasis",
            "struct Box<T: type> { value: T; }\n\
             global invalid: Box<i32[]>;\n\
             function main(): i32 { return 0; }\n",
        );
        let error = view
            .check()
            .expect_err("a stored generic field cannot contain an array view");
        assert!(format!("{error:?}").contains("cannot store view type"));

        let mut void = crate::compiler::Compiler::new();
        void.upsert_file(
            "void.stasis",
            "struct Box<T: type> { value: T; }\n\
             global invalid: Box<void>;\n\
             function main(): i32 { return 0; }\n",
        );
        let error = void
            .check()
            .expect_err("void is not a storable generic type argument");
        assert!(format!("{error:?}").contains("cannot be instantiated with void"));
    }

    #[test]
    fn rejects_recursive_generic_struct_storage() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "recursive.stasis",
            "struct Node<T: type> { next: Node<T>; }\n\
             global root: Node<i32>;\n\
             function main(): i32 { return 0; }\n",
        );
        let error = compiler
            .check()
            .expect_err("recursive generic structs cannot be stored by value");
        assert!(format!("{error:?}").contains("recursive generic struct field"));
    }

    #[test]
    fn accepts_finite_nested_specializations_of_the_same_generic_struct() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "finite_nested.stasis",
            "struct Box<T: type> { value: T; }\n\
             global nested: Box<Box<i32>>;\n\
             function main(): i32 { return nested.value.value; }\n",
        );
        compiler
            .check()
            .expect("finite nesting of one generic definition is valid");
    }

    #[test]
    fn rejects_expanding_recursive_generic_struct_storage_without_overflowing() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "expanding_recursive.stasis",
            "struct Node<N: i32> { next: Node<N + 1>; }\n\
             global root: Node<0>;\n\
             function main(): i32 { return 0; }\n",
        );
        let error = compiler
            .check()
            .expect_err("expanding recursive generic structs cannot be stored by value");
        assert!(format!("{error:?}").contains("generic instantiation depth exceeded"));
    }

    #[test]
    fn rejects_symbolic_extent_generic_names_not_bound_by_the_receiver() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "global values: i32[6];\n\
             function shifted(items: i32[N + 1]): i32 { return N; }\n\
             function main(): i32 { return shifted(values); }\n",
        );
        let error = compiler
            .check()
            .expect_err("N + 1 must not be solved from an array capacity");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn rejects_explicit_generic_calls_against_a_same_named_type_from_another_module() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "import \"left/left_box.stasis\";\n\
             import \"right/right_box.stasis\";\n\
             global left_value: left_box.Buffer<i32, 4>;\n\
             function main(): i32 { return right_box.capacity::<i32, 4>(left_value); }\n",
        );
        compiler.upsert_file(
            "left/left_box.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             function capacity(self: Buffer<T, N>): i32 { return N; }\n",
        );
        compiler.upsert_file(
            "right/right_box.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             function capacity(self: Buffer<T, N>): i32 { return N; }\n",
        );
        let error = compiler
            .check()
            .expect_err("generic receiver types remain nominal across modules");
        assert!(format!("{error:?}").contains("explicit generic function call"));
    }

    #[test]
    fn infers_generic_arguments_for_a_qualified_module_call() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "import \"left/left_box.stasis\";\n\
             global left_value: left_box.Buffer<i32, 4>;\n\
             function main(): i32 { return left_box.capacity(left_value); }\n",
        );
        process.upsert_file(
            "left/left_box.stasis",
            "struct Buffer<T: type, N: i32> { values: T[N]; }\n\
             function capacity(self: Buffer<T, N>): i32 { return N; }\n",
        );
        process
            .compile()
            .expect("qualified module calls infer generic arguments");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("qualified inferred generic call executes"),
            4
        );
    }

    #[test]
    fn rejects_legacy_function_declarations_with_a_migration_diagnostic() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "legacy.stasis",
            "function capacity<N: i32>(value: i32): i32 { return N + value; }\nfunction main(): i32 { return 0; }\n",
        );
        let error = compiler
            .check()
            .expect_err("legacy function generic declarations must fail");
        assert!(format!("{error:?}").contains("generic function declaration"));
        assert!(!format!("{error:?}").contains("specify explicit"));
    }

    #[test]
    fn rejects_body_only_generic_type_names() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "body_only.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal buffer: Buffer<4>;\nfunction body_only(value: Buffer<N>): i32 { let unknown: U = 0; return N; }\nfunction main(): i32 { return body_only(buffer); }\n",
        );
        let error = compiler
            .check()
            .expect_err("body-only generic names must fail");
        assert!(format!("{error:?}").contains("not bound by the first parameter"));
    }

    #[test]
    fn rejects_direct_and_turbofish_generic_calls_without_confusing_comparisons() {
        let direct = reject_explicit_generic_calls("capacity<i32>(value);", |name, qualifier| {
            name == "capacity" && qualifier.is_none()
        })
        .expect_err("direct generic call must be rejected");
        assert!(direct.contains("using <...>"));

        let turbofish =
            reject_explicit_generic_calls("module.capacity :: <i32>(value);", |name, qualifier| {
                name == "capacity" && qualifier == Some("module")
            })
            .expect_err("turbofish generic call must be rejected");
        assert!(turbofish.contains("using :: <...>"));

        reject_explicit_generic_calls("a < b > (c);", |_, _| true)
            .expect("comparison expression must remain ordinary syntax");
    }

    #[test]
    fn explicit_call_scanner_handles_utf8_source() {
        let error =
            reject_explicit_generic_calls("// 世\ncapacity<i32>(value);", |name, qualifier| {
                name == "capacity" && qualifier.is_none()
            })
            .expect_err("generic call after UTF-8 source must still be diagnosed");
        assert!(error.contains("explicit generic function call"));
    }
}
