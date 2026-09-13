//! Frontend elaboration for compile-time generic parameters.
//!
//! This first slice lowers generic declarations to ordinary concrete
//! declarations before the existing indexer, body parser, and backends run.
//! Keeping this pass in the frontend gives every backend the same concrete
//! fixed-layout input.

use std::collections::{BTreeMap, VecDeque};

use crate::compiler::SourceFile;
use crate::frontend::indexer::hash_text;
use crate::frontend::lexer::{lex, TokenKind};
use crate::frontend::parser::{
    parse_top_level_functions, parse_top_level_struct_definitions, parse_top_level_type_layout,
    ParsedFunctionSignature, ParsedGenericParameter, ParsedGenericParameterKind,
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
    file_index: usize,
    path: String,
    name: String,
    parameters: Vec<ParsedGenericParameter>,
    fields: Vec<crate::frontend::parser::ParsedField>,
    definition_range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct GenericFunctionDefinition {
    file_index: usize,
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
    name: String,
    type_name: String,
    value_text: String,
}

#[derive(Debug, Clone, Default)]
struct GenericEnvironment {
    values: BTreeMap<String, i32>,
    types: BTreeMap<String, String>,
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
    generic_structs: BTreeMap<String, GenericStructDefinition>,
    generic_functions: Vec<GenericFunctionDefinition>,
    generic_functions_by_name: BTreeMap<String, Vec<usize>>,
    constants: BTreeMap<String, i32>,
    struct_specializations: BTreeMap<StructSpecializationKey, StructSpecialization>,
    struct_work: VecDeque<(StructSpecializationKey, usize)>,
    function_specializations: BTreeMap<FunctionSpecializationKey, FunctionSpecialization>,
    function_work: VecDeque<(FunctionSpecializationKey, usize)>,
    struct_fields: BTreeMap<String, Vec<crate::frontend::parser::ParsedField>>,
    concrete_paths: BTreeMap<String, String>,
    active_depth: Option<usize>,
}

pub(crate) fn expand_sources(files: &mut [SourceFile]) -> Result<(), String> {
    let raw_files = files
        .iter()
        .map(|file| RawFile {
            path: file.path.clone(),
            source: file.original_content.clone(),
        })
        .collect::<Vec<_>>();
    let mut expansion = Expansion::new(raw_files)?;
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
    fn new(files: Vec<RawFile>) -> Result<Self, String> {
        let mut generic_structs = BTreeMap::new();
        let mut generic_functions = Vec::new();
        let mut generic_functions_by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut constant_definitions = Vec::new();
        let mut struct_fields = BTreeMap::new();
        let mut concrete_paths = BTreeMap::new();

        for (file_index, file) in files.iter().enumerate() {
            let layout = parse_top_level_type_layout(&file.source)
                .map_err(|error| format!("{}: {error}", file.path))?;
            for global in &layout.globals {
                concrete_paths.insert(global.name.clone(), global.type_name.clone());
            }
            for block in &layout.global_blocks {
                for field in &block.fields {
                    concrete_paths.insert(
                        format!("{}.{}", block.name, field.name),
                        field.type_name.clone(),
                    );
                }
            }
            for structure in &layout.structs {
                if let Some(existing) = struct_fields.get(&structure.name) {
                    if existing != &structure.fields {
                        return Err(format!(
                            "conflicting struct definition for '{}'",
                            structure.name
                        ));
                    }
                } else {
                    struct_fields.insert(structure.name.clone(), structure.fields.clone());
                }
            }
            for constant in layout.constants {
                constant_definitions.push(ConstantDefinition {
                    name: constant.name,
                    type_name: constant.type_name,
                    value_text: constant.value_text,
                });
            }
            for structure in layout.structs {
                if structure.generic_parameters.is_empty() {
                    continue;
                }
                if generic_structs.contains_key(&structure.name) {
                    return Err(format!(
                        "duplicate generic struct declaration '{}'",
                        structure.name
                    ));
                }
                let definition_range = find_struct_definition_range(
                    &file.source,
                    &structure.name,
                    &structure.generic_parameters,
                )?;
                generic_structs.insert(
                    structure.name.clone(),
                    GenericStructDefinition {
                        file_index,
                        path: file.path.clone(),
                        name: structure.name,
                        parameters: structure.generic_parameters,
                        fields: structure.fields,
                        definition_range,
                    },
                );
            }
            for function in parse_top_level_functions(&file.source)
                .map_err(|error| format!("{}: {error}", file.path))?
            {
                if function.generic_parameters.is_empty() {
                    continue;
                }
                let definition = generic_functions.len();
                generic_functions_by_name
                    .entry(function.name.clone())
                    .or_default()
                    .push(definition);
                generic_functions.push(GenericFunctionDefinition {
                    file_index,
                    name: function.name.clone(),
                    parameters: function.generic_parameters.clone(),
                    signature: function,
                });
            }
        }

        let mut constants = BTreeMap::new();
        for definition in &constant_definitions {
            if definition.type_name.trim() == "i32" {
                let value = evaluate_constant_name(
                    &definition.name,
                    &constant_definitions,
                    &mut constants,
                    &mut Vec::new(),
                    &mut 0,
                )?;
                constants.insert(definition.name.clone(), value);
            }
        }

        let expansion = Self {
            files,
            generic_structs,
            generic_functions,
            generic_functions_by_name,
            constants,
            struct_specializations: BTreeMap::new(),
            struct_work: VecDeque::new(),
            function_specializations: BTreeMap::new(),
            function_work: VecDeque::new(),
            struct_fields,
            concrete_paths,
            active_depth: None,
        };
        Ok(expansion)
    }

    fn populate_concrete_paths(&mut self) -> Result<(), String> {
        let roots = self
            .concrete_paths
            .iter()
            .filter(|(path, _)| !path.contains('.'))
            .map(|(path, type_name)| (path.clone(), type_name.clone()))
            .collect::<Vec<_>>();
        let block_fields = self
            .concrete_paths
            .iter()
            .filter(|(path, _)| path.contains('.'))
            .map(|(path, type_name)| (path.clone(), type_name.clone()))
            .collect::<Vec<_>>();
        for (path, type_name) in roots.into_iter().chain(block_fields) {
            self.populate_concrete_type(
                &path,
                &type_name,
                &GenericEnvironment::default(),
                &mut Vec::new(),
            )?;
        }
        Ok(())
    }

    fn populate_concrete_type(
        &mut self,
        path: &str,
        type_name: &str,
        environment: &GenericEnvironment,
        visiting: &mut Vec<String>,
    ) -> Result<(), String> {
        let resolved_type = rewrite_generic_identifiers(type_name, environment);
        self.materialize_type(&resolved_type, &GenericEnvironment::default())?;
        self.concrete_paths
            .insert(path.to_string(), resolved_type.clone());

        let (struct_name, nested_environment, visit_key) = if let Some((base, arguments)) =
            parse_type_application(&resolved_type)?
        {
            let Some(definition) = self.lookup_generic_struct(base).cloned() else {
                return Ok(());
            };
            let resolved_arguments = self.resolve_argument_list(
                &definition.parameters,
                &arguments,
                &GenericEnvironment::default(),
            )?;
            let nested_environment =
                GenericEnvironment::from_parameters(&definition.parameters, &resolved_arguments)?;
            (
                base.to_string(),
                nested_environment,
                format!("{}<{}>", base, arguments.join(",")),
            )
        } else if self.struct_fields.contains_key(&resolved_type) {
            (
                resolved_type.clone(),
                environment.clone(),
                resolved_type.clone(),
            )
        } else {
            return Ok(());
        };
        let Some(fields) = self.struct_fields.get(&struct_name).cloned() else {
            return Ok(());
        };
        if visiting.iter().any(|existing| existing == &visit_key) {
            return Ok(());
        }
        visiting.push(visit_key);
        for field in fields {
            self.populate_concrete_type(
                &format!("{path}.{}", field.name),
                &field.type_name,
                &nested_environment,
                visiting,
            )?;
        }
        visiting.pop();
        Ok(())
    }

    fn seed_direct_uses(&mut self) -> Result<(), String> {
        for file_index in 0..self.files.len() {
            let source = self.files[file_index].source.clone();
            let source_without_templates = self.source_without_templates(file_index, &source)?;
            self.collect_type_applications(
                &source_without_templates,
                &GenericEnvironment::default(),
            )?;
            for call in collect_explicit_generic_calls(&source_without_templates)? {
                let definitions = self
                    .generic_functions_by_name
                    .get(&call.name)
                    .cloned()
                    .unwrap_or_default();
                for definition in definitions {
                    let parameters = self.generic_functions[definition].parameters.clone();
                    let arguments = self.resolve_argument_list(
                        &parameters,
                        &call.arguments,
                        &GenericEnvironment::default(),
                    )?;
                    self.schedule_function(definition, arguments)?;
                }
            }
            self.seed_inferred_receiver_calls(&source_without_templates)?;
            self.seed_inferred_argument_calls(&source_without_templates)?;
        }
        Ok(())
    }

    fn seed_inferred_receiver_calls(&mut self, source: &str) -> Result<(), String> {
        for call in collect_inferred_receiver_calls(source) {
            let Some(actual_type) = self.concrete_paths.get(&call.receiver).cloned() else {
                continue;
            };
            let definitions = self
                .generic_functions_by_name
                .get(&call.name)
                .cloned()
                .unwrap_or_default();
            for definition in definitions {
                let generic = self.generic_functions[definition].clone();
                let Some(receiver) = generic.signature.params.first() else {
                    continue;
                };
                let mut environment = GenericEnvironment::default();
                if !self.infer_type_pattern(
                    &receiver.type_name,
                    &actual_type,
                    &generic.parameters,
                    &mut environment,
                )? {
                    continue;
                }
                let Some(arguments) = inferred_arguments(&generic.parameters, &environment) else {
                    continue;
                };
                self.schedule_function(definition, arguments)?;
            }
        }
        Ok(())
    }

    fn seed_inferred_argument_calls(&mut self, source: &str) -> Result<(), String> {
        for call in collect_inferred_argument_calls(source)? {
            let definitions = self
                .generic_functions_by_name
                .get(&call.name)
                .cloned()
                .unwrap_or_default();
            for definition in definitions {
                let generic = self.generic_functions[definition].clone();
                if generic.signature.params.len() != call.arguments.len() {
                    continue;
                }
                let mut environment = GenericEnvironment::default();
                let mut viable = true;
                for (parameter, argument) in generic.signature.params.iter().zip(&call.arguments) {
                    let Some(actual_type) = self.concrete_paths.get(argument.trim()).cloned()
                    else {
                        viable = false;
                        break;
                    };
                    if !self.infer_type_pattern(
                        &parameter.type_name,
                        &actual_type,
                        &generic.parameters,
                        &mut environment,
                    )? {
                        viable = false;
                        break;
                    }
                }
                if viable {
                    if let Some(arguments) = inferred_arguments(&generic.parameters, &environment) {
                        self.schedule_function(definition, arguments)?;
                    }
                }
            }
        }
        Ok(())
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
            if let Some(parameter) = parameters.iter().find(|parameter| {
                parameter.kind == ParsedGenericParameterKind::I32
                    && parameter.name == pattern_extent
            }) {
                let value = evaluate_i32_expression(
                    actual_extent,
                    &GenericEnvironment::default(),
                    &self.constants,
                )?;
                return bind_inferred_value(environment, &parameter.name, value);
            }
            let Some(expected) =
                evaluate_i32_expression(pattern_extent, environment, &self.constants).ok()
            else {
                return Ok(false);
            };
            let Some(observed) = evaluate_i32_expression(
                actual_extent,
                &GenericEnvironment::default(),
                &self.constants,
            )
            .ok() else {
                return Ok(false);
            };
            return Ok(expected == observed);
        }

        if let Some((pattern_base, pattern_arguments)) = parse_type_application(pattern)? {
            let Some((actual_base, actual_arguments)) = parse_type_application(actual)? else {
                return Ok(false);
            };
            if !same_type_name(pattern_base, actual_base)
                || pattern_arguments.len() != actual_arguments.len()
            {
                return Ok(false);
            }
            let Some(definition) = self.lookup_generic_struct(pattern_base).cloned() else {
                return Ok(false);
            };
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
                            let expected = evaluate_i32_expression(
                                pattern_argument,
                                environment,
                                &self.constants,
                            )
                            .ok();
                            let observed = evaluate_i32_expression(
                                actual_argument,
                                &GenericEnvironment::default(),
                                &self.constants,
                            )
                            .ok();
                            if expected != observed {
                                return Ok(false);
                            }
                            continue;
                        };
                        if nested.kind != ParsedGenericParameterKind::I32 {
                            return Ok(false);
                        }
                        let value = evaluate_i32_expression(
                            actual_argument,
                            &GenericEnvironment::default(),
                            &self.constants,
                        )?;
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

    fn process_worklist(&mut self) -> Result<(), String> {
        while !self.struct_work.is_empty() || !self.function_work.is_empty() {
            if let Some((key, depth)) = self.struct_work.pop_front() {
                let previous_depth = self.active_depth.replace(depth);
                let result = self.materialize_struct(&key);
                self.active_depth = previous_depth;
                result?;
            }
            if let Some((key, depth)) = self.function_work.pop_front() {
                let previous_depth = self.active_depth.replace(depth);
                let result = self.materialize_function(&key);
                self.active_depth = previous_depth;
                result?;
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
            let name = &source[start..cursor];
            let after_name = skip_ascii_whitespace(source, cursor);
            if self.lookup_generic_struct(name).is_none()
                || source.as_bytes().get(after_name) != Some(&b'<')
            {
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
            let value = evaluate_i32_expression(extent, environment, &self.constants)?;
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
                .lookup_generic_struct(base)
                .ok_or_else(|| format!("unknown generic type '{base}'"))?
                .clone();
            let arguments =
                self.resolve_argument_list(&definition.parameters, &arguments, environment)?;
            let key = StructSpecializationKey {
                definition: definition.name.clone(),
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
            let value = evaluate_i32_expression(extent, environment, &self.constants)?;
            if value < 0 {
                return Err(format!("negative array extent {} is invalid", value));
            }
            return Ok(format!("{element}[{value}]"));
        }
        if let Some((base, arguments)) = parse_type_application(trimmed)? {
            let definition = self
                .lookup_generic_struct(base)
                .ok_or_else(|| format!("unknown generic type '{base}'"))?;
            let resolved = self.resolve_argument_list_readonly(
                &definition.parameters,
                &arguments,
                environment,
            )?;
            let key = StructSpecializationKey {
                definition: definition.name.clone(),
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
        Ok(trimmed.to_string())
    }

    fn lookup_generic_struct(&self, name: &str) -> Option<&GenericStructDefinition> {
        self.generic_structs.get(name).or_else(|| {
            name.rsplit('.')
                .next()
                .and_then(|short| self.generic_structs.get(short))
        })
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
                    let value = evaluate_i32_expression(argument, environment, &self.constants)?;
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
                    evaluate_i32_expression(argument, environment, &self.constants)?,
                )),
                ParsedGenericParameterKind::Type => {
                    Ok(ConcreteArgument::Type(if is_type_argument_text(argument) {
                        self.materialize_type_readonly(argument, environment)?
                    } else {
                        return Err(format!(
                            "generic parameter '{}' expects a type argument, got '{}'",
                            parameter.name, argument
                        ));
                    }))
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
        let environment =
            GenericEnvironment::from_parameters(&definition.parameters, &key.arguments)?;
        for field in &definition.fields {
            self.materialize_type(&field.type_name, &environment)?;
        }
        Ok(())
    }

    fn materialize_function(&mut self, key: &FunctionSpecializationKey) -> Result<(), String> {
        let generic = self
            .generic_functions
            .get(key.definition)
            .cloned()
            .ok_or_else(|| "internal error: missing generic function definition".to_string())?;
        let environment = GenericEnvironment::from_parameters(&generic.parameters, &key.arguments)?;
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

        for call in collect_explicit_generic_calls(&stripped)? {
            let definitions = self
                .generic_functions_by_name
                .get(&call.name)
                .cloned()
                .unwrap_or_default();
            for definition in definitions {
                let parameters = self.generic_functions[definition].parameters.clone();
                let arguments =
                    self.resolve_argument_list(&parameters, &call.arguments, &environment)?;
                self.schedule_function(definition, arguments)?;
            }
        }

        let substituted = rewrite_generic_identifiers(&stripped, &environment);
        let substituted = self.rewrite_type_applications(&substituted, &environment)?;

        for call in collect_plain_calls(&substituted)? {
            let definitions = self
                .generic_functions_by_name
                .get(&call)
                .cloned()
                .unwrap_or_default();
            for definition in definitions {
                let parameters = self.generic_functions[definition].parameters.clone();
                if parameters.iter().all(|parameter| match parameter.kind {
                    ParsedGenericParameterKind::I32 => {
                        environment.values.contains_key(&parameter.name)
                    }
                    ParsedGenericParameterKind::Type => {
                        environment.types.contains_key(&parameter.name)
                    }
                }) {
                    let arguments = parameters
                        .iter()
                        .map(|parameter| match parameter.kind {
                            ParsedGenericParameterKind::I32 => {
                                ConcreteArgument::I32(environment.values[&parameter.name])
                            }
                            ParsedGenericParameterKind::Type => {
                                ConcreteArgument::Type(environment.types[&parameter.name].clone())
                            }
                        })
                        .collect();
                    self.schedule_function(definition, arguments)?;
                }
            }
        }

        let record = self
            .function_specializations
            .get_mut(key)
            .ok_or_else(|| "internal error: function specialization disappeared".to_string())?;
        let parsed = parse_top_level_functions(&substituted)?;
        let signature = parsed
            .first()
            .ok_or_else(|| "specialized generic function has no parsed signature".to_string())?;
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
            let name = &source[start..cursor];
            let after = skip_ascii_whitespace(source, cursor);
            if self.lookup_generic_struct(name).is_some()
                && source.as_bytes().get(after) == Some(&b'<')
            {
                let close = matching_angle(source, after)?;
                let replacement = self.materialize_type(&source[start..=close], environment)?;
                output.push_str(&replacement);
                cursor = close + 1;
            } else {
                output.push_str(name);
            }
        }
        Ok(output)
    }

    fn write_sources(&self, files: &mut [SourceFile]) -> Result<(), String> {
        let function_names = self.function_names()?;
        for (file_index, file) in files.iter_mut().enumerate() {
            let raw = &self.files[file_index].source;
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
            let mut generated = rewrite_kept_source(self, raw, &removals, &function_names)?;

            for (key, specialization) in &self.struct_specializations {
                let Some(definition) = self.lookup_generic_struct(&key.definition) else {
                    continue;
                };
                if definition.file_index != file_index {
                    continue;
                }
                let environment =
                    GenericEnvironment::from_parameters(&definition.parameters, &key.arguments)?;
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

            for (key, specialization) in &self.function_specializations {
                let definition = &self.generic_functions[specialization.definition];
                if definition.file_index != file_index || specialization.source.is_empty() {
                    continue;
                }
                let source = rewrite_explicit_generic_calls_with_map(
                    self,
                    &specialization.source,
                    &GenericEnvironment::default(),
                    &function_names,
                )?;
                let source = rename_function_declaration(
                    &source,
                    &definition.name,
                    function_names.get(key).ok_or_else(|| {
                        "missing generic function specialization name".to_string()
                    })?,
                )?;
                generated.push('\n');
                generated.push_str(&source);
                generated.push('\n');
            }
            file.content = generated;
            file.hash = hash_text(&file.content);
        }
        Ok(())
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
            let needs_mangled_names = keys.len() > 1;
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
                    let identity_path = format!(
                        "{}#{}",
                        self.files[definition.file_index].path,
                        definition.signature.signature_range.start
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
}

fn rewrite_kept_source(
    expansion: &Expansion,
    source: &str,
    removals: &[std::ops::Range<usize>],
    function_names: &BTreeMap<FunctionSpecializationKey, String>,
) -> Result<String, String> {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for range in removals {
        if range.start < cursor || range.end > source.len() {
            return Err("generic declaration range is invalid".to_string());
        }
        output.push_str(&ordinary_source_piece(
            expansion,
            &source[cursor..range.start],
            function_names,
        )?);
        cursor = range.end;
    }
    output.push_str(&ordinary_source_piece(
        expansion,
        &source[cursor..],
        function_names,
    )?);
    Ok(output)
}

fn ordinary_source_piece(
    expansion: &Expansion,
    source: &str,
    function_names: &BTreeMap<FunctionSpecializationKey, String>,
) -> Result<String, String> {
    let source = rewrite_i32_constants(source, &expansion.constants)?;
    let source = expansion.rewrite_type_applications_readonly(&source)?;
    rewrite_explicit_generic_calls_with_map(
        expansion,
        &source,
        &GenericEnvironment::default(),
        function_names,
    )
}

impl Expansion {
    fn rewrite_type_applications_readonly(&self, source: &str) -> Result<String, String> {
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
            let name = &source[start..cursor];
            let after = skip_ascii_whitespace(source, cursor);
            if self.lookup_generic_struct(name).is_some()
                && source.as_bytes().get(after) == Some(&b'<')
            {
                let close = matching_angle(source, after)?;
                let replacement = self.materialize_type_readonly(
                    &source[start..=close],
                    &GenericEnvironment::default(),
                )?;
                output.push_str(&replacement);
                cursor = close + 1;
            } else {
                output.push_str(name);
            }
        }
        Ok(output)
    }
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

fn rewrite_explicit_generic_calls_with_map(
    expansion: &Expansion,
    source: &str,
    environment: &GenericEnvironment,
    function_names: &BTreeMap<FunctionSpecializationKey, String>,
) -> Result<String, String> {
    let calls = collect_explicit_generic_calls(source)?;
    let mut replacements = Vec::with_capacity(calls.len());
    for call in calls {
        let definitions = expansion
            .generic_functions_by_name
            .get(&call.name)
            .cloned()
            .unwrap_or_default();
        if definitions.is_empty() {
            return Err(format!(
                "unknown generic function '{}' in explicit call",
                call.name
            ));
        }
        let mut targets = Vec::new();
        for definition in definitions {
            let parameters = &expansion.generic_functions[definition].parameters;
            let arguments = expansion.resolve_argument_list_readonly(
                parameters,
                &call.arguments,
                environment,
            )?;
            let key = FunctionSpecializationKey {
                definition,
                arguments,
            };
            if let Some(target) = function_names.get(&key) {
                if !targets.iter().any(|existing| existing == target) {
                    targets.push(target.clone());
                }
            }
        }
        let target = match targets.as_slice() {
            [target] => target.clone(),
            [] => {
                return Err(format!(
                    "missing specialization for explicit generic call '{}::<...>'",
                    call.name
                ));
            }
            _ => {
                return Err(format!("ambiguous explicit generic call '{}'", call.name));
            }
        };
        replacements.push((call.name_start, call.end, target));
    }
    apply_replacements(source, &replacements)
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

#[derive(Debug, Clone)]
struct ExplicitCall {
    name: String,
    arguments: Vec<String>,
    name_start: usize,
    end: usize,
}

fn collect_explicit_generic_calls(source: &str) -> Result<Vec<ExplicitCall>, String> {
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
        let name = source[start..cursor].to_string();
        let after_name = skip_ascii_whitespace(source, cursor);
        if source.as_bytes().get(after_name) != Some(&b':')
            || source.as_bytes().get(after_name + 1) != Some(&b':')
        {
            continue;
        }
        let open = skip_ascii_whitespace(source, after_name + 2);
        if source.as_bytes().get(open) != Some(&b'<') {
            continue;
        }
        let close = matching_angle(source, open)?;
        let after = skip_ascii_whitespace(source, close + 1);
        if source.as_bytes().get(after) != Some(&b'(') {
            continue;
        }
        calls.push(ExplicitCall {
            name: name.rsplit('.').next().unwrap_or(&name).to_string(),
            arguments: split_top_level_arguments(&source[open + 1..close])?,
            name_start: start,
            end: close + 1,
        });
        cursor = close + 1;
    }
    Ok(calls)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InferredReceiverCall {
    receiver: String,
    name: String,
}

fn collect_inferred_receiver_calls(source: &str) -> Vec<InferredReceiverCall> {
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
                calls.push(InferredReceiverCall {
                    receiver: receiver.clone(),
                    name: source[segment_start..segment_end].to_string(),
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
    calls
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct InferredArgumentCall {
    name: String,
    arguments: Vec<String>,
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
        let close = find_matching_byte(source, after_name, b'(', b')')
            .ok_or_else(|| "unterminated call while inferring generic arguments".to_string())?;
        calls.push(InferredArgumentCall {
            name: source[start..cursor].to_string(),
            arguments: split_top_level_arguments(&source[after_name + 1..close])?,
        });
        cursor = after_name + 1;
    }
    Ok(calls)
}

fn collect_plain_calls(source: &str) -> Result<Vec<String>, String> {
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
        let name = source[start..cursor].to_string();
        let after = skip_ascii_whitespace(source, cursor);
        if source.as_bytes().get(after) == Some(&b'(') {
            calls.push(name);
        }
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

fn evaluate_constant_name(
    name: &str,
    definitions: &[ConstantDefinition],
    resolved: &mut BTreeMap<String, i32>,
    stack: &mut Vec<String>,
    steps: &mut usize,
) -> Result<i32, String> {
    if let Some(value) = resolved.get(name) {
        return Ok(*value);
    }
    if stack.iter().any(|entry| entry == name) {
        let mut chain = stack.clone();
        chain.push(name.to_string());
        return Err(format!("constant reference cycle: {}", chain.join(" -> ")));
    }
    let definition = definitions
        .iter()
        .find(|definition| definition.name == name)
        .ok_or_else(|| format!("unknown compile-time constant '{name}'"))?;
    stack.push(name.to_string());
    let mut environment = GenericEnvironment::default();
    let tokens = tokenize_constant_expression(&definition.value_text)?;
    for token in tokens {
        let ConstantToken::Identifier(identifier) = token else {
            continue;
        };
        if identifier == name || environment.values.contains_key(&identifier) {
            continue;
        }
        if let Some(candidate) = definitions
            .iter()
            .find(|candidate| candidate.name == identifier && candidate.type_name.trim() == "i32")
        {
            let value =
                evaluate_constant_name(&candidate.name, definitions, resolved, stack, steps)?;
            environment.values.insert(identifier, value);
        }
    }
    let value =
        evaluate_i32_expression_with_steps(&definition.value_text, &environment, resolved, steps)?;
    stack.pop();
    resolved.insert(name.to_string(), value);
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
) -> Result<String, String> {
    let tokens = lex(source)?;
    let mut replacements = Vec::<(usize, usize, String)>::new();
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
        replacements.push((
            tokens[equals_index].end,
            tokens[semicolon_index].start,
            format!(" {value} "),
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_value_generic_structs_and_functions_before_indexing() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { count: i32; values: i32[N]; }\nglobal samples: Buffer<4>;\nfunction clear<N: i32>(self: Buffer<N>): void { self.count = 0; }\nfunction capacity<N: i32>(): i32 { return N; }\nfunction reset(): void { samples.clear(); }\nfunction main(): i32 { reset(); return capacity::<4>(); }\n",
        );
        compiler.check().expect("generic program compiles");
        let file = &compiler.files()[0];
        assert!(!file.content.contains("Buffer<N"));
        assert!(file.content.contains("values: i32[4]"));
        assert!(compiler
            .functions()
            .iter()
            .any(|function| function.name == "clear"));
        assert!(compiler
            .functions()
            .iter()
            .any(|function| function.name == "capacity"));
    }

    #[test]
    fn concrete_generic_program_executes_through_jit() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { count: i32; values: i32[N]; }\nglobal samples: Buffer<4>;\nfunction clear<N: i32>(self: Buffer<N>): void { self.count = 0; }\nfunction capacity<N: i32>(): i32 { return N; }\nfunction main(): i32 { samples.clear(); return capacity::<4>(); }\n",
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
    fn keeps_multiple_explicit_value_function_specializations_distinct() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "function capacity<N: i32>(): i32 { return N; }\nfunction main(): i32 { return capacity::<4>() + capacity::<8>(); }\n",
        );
        process
            .compile()
            .expect("multiple explicit generic calls compile");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("multiple explicit generic calls execute"),
            12
        );
    }

    #[test]
    fn infers_value_arguments_from_distinct_receiver_applications() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct Buffer<N: i32> { value: i32; }\nglobal first: Buffer<4>;\nglobal second: Buffer<8>;\nfunction capacity<N: i32>(self: Buffer<N>): i32 { return N; }\nfunction main(): i32 { return first.capacity() + second.capacity(); }\n",
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
    fn infers_value_arguments_from_exact_fixed_array_arguments() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "global values: i32[6];\nfunction extent<N: i32>(items: i32[N]): i32 { return N; }\nfunction main(): i32 { return extent(values); }\n",
        );
        process.compile().expect("fixed-array generic inference");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("fixed-array inferred generic call executes"),
            6
        );
    }

    #[test]
    fn infers_value_arguments_from_nested_fixed_array_arguments() {
        let mut process = crate::backend::jit::JitProcess::new();
        process.upsert_file(
            "main.stasis",
            "struct State { values: i32[5]; }\nglobal state: State;\nfunction extent<N: i32>(items: i32[N]): i32 { return N; }\nfunction main(): i32 { return extent(state.values); }\n",
        );
        process
            .compile()
            .expect("nested fixed-array generic inference");
        assert_eq!(
            process
                .execute_i32_noarg_by_name("main")
                .expect("nested fixed-array inferred generic call executes"),
            5
        );
    }

    #[test]
    fn lowers_value_generic_storage_through_aot_and_wasm() {
        let source = "struct Buffer<N: i32> { values: i32[N]; }\nglobal samples: Buffer<4>;\nfunction main(): i32 { samples.values[0] = 7; return samples.values[0]; }\n";

        let mut aot = crate::backend::aot::AotProcess::new();
        aot.upsert_file("main.stasis", source);
        let report = aot.compile().expect("generic AOT compile");
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(aot.artifacts().len(), 1);

        let mut wasm = crate::backend::wasm::WasmProcess::new();
        wasm.set_required_emit_roots(&["main".to_string()]);
        wasm.upsert_file("main.stasis", source);
        wasm.compile().expect("generic Wasm compile");
        assert!(wasm.module_bytes().starts_with(b"\0asm\x01\0\0\0"));
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
            "function offset<N: i32>(): i32 { return N; }\nfunction main(): i32 { return offset::<-2147483648>(); }\n",
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
    fn rejects_expanding_generic_recursion_at_the_deterministic_depth_limit() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "recursive.stasis",
            "function loop<N: i32>(): i32 { return loop::<N + 1>(); }\nfunction main(): i32 { return loop::<0>(); }\n",
        );
        let error = compiler
            .check()
            .expect_err("expanding generic recursion must be bounded");
        assert!(format!("{error:?}").contains("generic instantiation depth exceeded"));
    }

    #[test]
    fn rejects_writes_to_specialized_value_parameters() {
        let mut compiler = crate::compiler::Compiler::new();
        compiler.upsert_file(
            "write.stasis",
            "function mutate<N: i32>(): void { N = 3; return; }\nfunction main(): i32 { mutate::<4>(); return 0; }\n",
        );
        let error = compiler
            .check()
            .expect_err("generic value parameters are immutable");
        assert!(format!("{error:?}").contains("cannot assign to compile-time generic parameter"));
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
}
