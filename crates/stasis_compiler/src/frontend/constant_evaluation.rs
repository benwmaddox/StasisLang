//! Source-aware evaluation of scalar compile-time expressions.
//!
//! Constant lookup is intentionally scoped by the module graph.  This module
//! does not build a flattened name-to-value table: the same leaf name in two
//! modules remains two distinct declarations until a caller resolves it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::compiler::SourceFile;
use crate::frontend::body_parser::parse_value_expression_at;
use crate::frontend::module_graph::ModuleGraph;
use crate::frontend::parser::{
    parse_top_level_type_layout, ParsedConstDefinition, ParsedEnumDefinition,
};
use crate::frontend::types::{
    SourceTypeOrigin, TypeCategory, TypeId, TypeTable, TYPE_ID_BOOL, TYPE_ID_F32, TYPE_ID_F64,
    TYPE_ID_I32, TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8,
};
use crate::ir::hir::{ExprBinaryOp, ExprUnaryOp, SimpleExpr};

const MAX_CONSTANT_RECURSION_DEPTH: usize = 128;

/// Exact scalar result of a source-level constant evaluation.
///
/// `type_id` is local to the current compile and is not a stable identity.
/// Enum users must retain `nominal_origin` when persisting the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvaluatedScalar {
    pub(crate) type_id: TypeId,
    pub(crate) bits: u64,
    pub(crate) nominal_origin: Option<SourceTypeOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct DeclarationKey {
    path: String,
    name: String,
}

#[derive(Debug, Clone)]
struct ConstantDeclaration {
    key: DeclarationKey,
    definition: ParsedConstDefinition,
}

#[derive(Debug, Clone)]
struct EnumDeclaration {
    key: DeclarationKey,
    definition: ParsedEnumDefinition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    Bool,
    I32,
    U8,
    U16,
    U32,
    F32,
    F64,
}

impl ScalarKind {
    fn integer_width(self) -> Option<u32> {
        match self {
            Self::I32 | Self::U32 => Some(32),
            Self::U16 => Some(16),
            Self::U8 => Some(8),
            Self::Bool | Self::F32 | Self::F64 => None,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    fn is_unsigned(self) -> bool {
        matches!(self, Self::U8 | Self::U16 | Self::U32)
    }
}

/// A single source-indexed evaluator is reused across all defaults in a
/// compile.  Declarations are parsed once; only referenced constants are
/// recursively evaluated and memoized.
pub(crate) struct ScalarConstantEvaluator<'a> {
    files: &'a [SourceFile],
    graph: &'a ModuleGraph,
    types: &'a TypeTable,
    constants: BTreeMap<DeclarationKey, Vec<ConstantDeclaration>>,
    enums: BTreeMap<DeclarationKey, Vec<EnumDeclaration>>,
    value_names_by_path: BTreeMap<String, BTreeSet<String>>,
    constant_cache: HashMap<DeclarationKey, EvaluatedScalar>,
    constant_stack: Vec<DeclarationKey>,
}

impl<'a> ScalarConstantEvaluator<'a> {
    pub(crate) fn new(
        files: &'a [SourceFile],
        graph: &'a ModuleGraph,
        types: &'a TypeTable,
    ) -> Result<Self, String> {
        let mut constants = BTreeMap::<DeclarationKey, Vec<ConstantDeclaration>>::new();
        let mut enums = BTreeMap::<DeclarationKey, Vec<EnumDeclaration>>::new();
        let mut value_names_by_path = BTreeMap::<String, BTreeSet<String>>::new();

        for file in files {
            // `original_content` is the declaration source of truth. Generic
            // expansion can append specialized structs to `content`, which
            // must not duplicate module constants or enum declarations.
            let layout = parse_top_level_type_layout(&file.original_content).map_err(|error| {
                format!(
                    "failed parsing scalar declarations in {}: {error}",
                    file.path
                )
            })?;
            let value_names = value_names_by_path.entry(file.path.clone()).or_default();
            value_names.extend(
                layout
                    .constants
                    .iter()
                    .map(|constant| constant.name.clone()),
            );
            value_names.extend(layout.globals.iter().map(|global| global.name.clone()));
            value_names.extend(layout.global_blocks.iter().map(|block| block.name.clone()));
            for definition in layout.constants {
                let key = DeclarationKey {
                    path: file.path.clone(),
                    name: definition.name.clone(),
                };
                constants
                    .entry(key.clone())
                    .or_default()
                    .push(ConstantDeclaration { key, definition });
            }
            for definition in layout.enums {
                let key = DeclarationKey {
                    path: file.path.clone(),
                    name: definition.name.clone(),
                };
                enums
                    .entry(key.clone())
                    .or_default()
                    .push(EnumDeclaration { key, definition });
            }
        }

        Ok(Self {
            files,
            graph,
            types,
            constants,
            enums,
            value_names_by_path,
            constant_cache: HashMap::new(),
            constant_stack: Vec::new(),
        })
    }

    pub(crate) fn evaluate(
        &mut self,
        scope_path: &str,
        expression: &str,
        expected_type: TypeId,
    ) -> Result<EvaluatedScalar, String> {
        if !self.files.iter().any(|file| file.path == scope_path) {
            return Err(format!("unknown scalar initializer scope '{scope_path}'"));
        }
        self.validate_expected_type(scope_path, expected_type)?;
        let parsed = parse_value_expression_at(expression, 0)
            .map_err(|error| format!("invalid scalar constant expression: {error}"))?;
        let result = self.evaluate_expression(scope_path, &parsed, Some(expected_type))?;
        self.coerce_to_expected(scope_path, result, expected_type)
    }

    fn validate_expected_type(&self, scope_path: &str, expected: TypeId) -> Result<(), String> {
        if self.scalar_kind(expected).is_some() {
            return Ok(());
        }
        let info = self
            .types
            .type_info(expected)
            .ok_or_else(|| format!("unknown scalar initializer type {expected}"))?;
        if info.category == TypeCategory::Named
            && !self.expected_enum_origins(scope_path, expected)?.is_empty()
        {
            return Ok(());
        }
        Err(format!(
            "type '{}' is not a supported scalar initializer type",
            info.name
        ))
    }

    fn evaluate_expression(
        &mut self,
        scope_path: &str,
        expression: &SimpleExpr,
        expected: Option<TypeId>,
    ) -> Result<EvaluatedScalar, String> {
        match expression {
            SimpleExpr::Int(value) => self.evaluate_integer_literal(*value, expected),
            SimpleExpr::Float(value) => self.evaluate_float_literal(*value, expected),
            SimpleExpr::Bool(value) => {
                if expected.is_some_and(|type_id| type_id != TYPE_ID_BOOL) {
                    return Err("boolean literal cannot initialize a non-bool scalar".into());
                }
                Ok(EvaluatedScalar {
                    type_id: TYPE_ID_BOOL,
                    bits: if *value { 1 } else { 0 },
                    nominal_origin: None,
                })
            }
            SimpleExpr::Identifier(name) => self.resolve_identifier(scope_path, name),
            SimpleExpr::Unary { op, operand } => {
                self.evaluate_unary(scope_path, *op, operand, expected)
            }
            SimpleExpr::Binary { lhs, op, rhs } => {
                self.evaluate_binary(scope_path, lhs, *op, rhs, expected)
            }
            SimpleExpr::DefaultValue(_) => {
                Err("default-value expressions are not compile-time scalar constants".into())
            }
            SimpleExpr::StringLiteral(_) => {
                Err("string literals are not scalar compile-time defaults".into())
            }
            SimpleExpr::Condition(_) => {
                Err("conditions are not scalar compile-time defaults".into())
            }
            SimpleExpr::IndexedPath { .. } => {
                Err("runtime state paths are not compile-time scalar constants".into())
            }
            SimpleExpr::Call { target, .. } => Err(format!(
                "runtime call '{target}' is not allowed in a scalar compile-time expression"
            )),
        }
    }

    fn evaluate_integer_literal(
        &self,
        value: i64,
        expected: Option<TypeId>,
    ) -> Result<EvaluatedScalar, String> {
        let type_id = match expected {
            Some(type_id)
                if self
                    .scalar_kind(type_id)
                    .is_some_and(|kind| kind.integer_width().is_some()) =>
            {
                type_id
            }
            Some(type_id) => {
                return Err(format!(
                    "integer literal cannot initialize scalar type {}",
                    self.type_name(type_id)
                ));
            }
            None => TYPE_ID_I32,
        };
        if !self.types.integer_literal_fits(value, type_id) {
            return Err(format!(
                "integer literal {value} is not representable in scalar type {}",
                self.type_name(type_id)
            ));
        }
        Ok(EvaluatedScalar {
            type_id,
            bits: value as u64,
            nominal_origin: None,
        })
    }

    fn evaluate_float_literal(
        &self,
        value: f64,
        expected: Option<TypeId>,
    ) -> Result<EvaluatedScalar, String> {
        let type_id = match expected {
            Some(type_id @ (TYPE_ID_F32 | TYPE_ID_F64)) => type_id,
            Some(type_id) => {
                return Err(format!(
                    "floating-point literal cannot initialize scalar type {}",
                    self.type_name(type_id)
                ));
            }
            None => TYPE_ID_F32,
        };
        let bits = if type_id == TYPE_ID_F64 {
            value.to_bits()
        } else {
            (value as f32).to_bits() as u64
        };
        Ok(EvaluatedScalar {
            type_id,
            bits,
            nominal_origin: None,
        })
    }

    fn evaluate_unary(
        &mut self,
        scope_path: &str,
        op: ExprUnaryOp,
        operand: &SimpleExpr,
        expected: Option<TypeId>,
    ) -> Result<EvaluatedScalar, String> {
        let contextual_integer = SimpleExpr::Unary {
            op,
            operand: Box::new(operand.clone()),
        }
        .contextual_integer_literal_value();
        if op == ExprUnaryOp::Negate {
            if let Some(value) = contextual_integer {
                return self.evaluate_integer_literal(value, expected);
            }
        }

        let contextual_type = expected.filter(|type_id| self.scalar_kind(*type_id).is_some());
        let value = self.evaluate_expression(scope_path, operand, contextual_type)?;
        let Some(kind) = self.scalar_kind(value.type_id) else {
            return Err(format!(
                "unary '{}' requires a numeric scalar operand",
                op.spelling()
            ));
        };
        let mut result = value;
        match op {
            ExprUnaryOp::Negate if kind.is_float() => {
                result.bits ^= if kind == ScalarKind::F32 {
                    1_u64 << 31
                } else {
                    1_u64 << 63
                };
            }
            ExprUnaryOp::Negate if kind.integer_width().is_some() => {
                result.bits = match kind {
                    ScalarKind::I32 | ScalarKind::U32 => {
                        (0_u32.wrapping_sub(result.bits as u32)) as u64
                    }
                    ScalarKind::U16 => (0_u16.wrapping_sub(result.bits as u16)) as u64,
                    ScalarKind::U8 => (0_u8.wrapping_sub(result.bits as u8)) as u64,
                    _ => unreachable!("integer width classified scalar kind"),
                };
            }
            ExprUnaryOp::Negate => {
                return Err("unary '-' requires a numeric scalar operand".into());
            }
            ExprUnaryOp::BitwiseNot if kind.integer_width().is_some() => {
                result.bits = normalize_integer_bits(!result.bits, kind);
            }
            ExprUnaryOp::BitwiseNot => {
                return Err("unary '~' requires an exact builtin integer operand".into());
            }
        }
        if let Some(expected) = expected {
            self.coerce_to_expected(scope_path, result, expected)
        } else {
            Ok(result)
        }
    }

    fn evaluate_binary(
        &mut self,
        scope_path: &str,
        lhs: &SimpleExpr,
        op: ExprBinaryOp,
        rhs: &SimpleExpr,
        expected: Option<TypeId>,
    ) -> Result<EvaluatedScalar, String> {
        let expected_kind = expected.and_then(|type_id| self.scalar_kind(type_id));
        if expected.is_some() && expected_kind.is_none() {
            return Err("binary scalar expression has a non-scalar contextual type".into());
        }

        let contextual_type = expected.filter(|type_id| self.scalar_kind(*type_id).is_some());
        let lhs_value = self.evaluate_expression(scope_path, lhs, contextual_type)?;
        let rhs_expected = if op.is_shift() {
            None
        } else {
            contextual_type.or_else(|| {
                self.scalar_kind(lhs_value.type_id)
                    .filter(|kind| kind.is_unsigned())
                    .map(|_| lhs_value.type_id)
            })
        };
        let rhs_value = self.evaluate_expression(scope_path, rhs, rhs_expected)?;

        let lhs_kind = self
            .scalar_kind(lhs_value.type_id)
            .ok_or_else(|| format!("binary '{}' requires scalar operands", op.spelling()))?;
        let rhs_kind = self
            .scalar_kind(rhs_value.type_id)
            .ok_or_else(|| format!("binary '{}' requires scalar operands", op.spelling()))?;
        let result_type = if op.requires_integer_operands() {
            if lhs_kind.integer_width().is_none() || rhs_kind.integer_width().is_none() {
                return Err(format!(
                    "binary '{}' requires exact builtin integer operands",
                    op.spelling()
                ));
            }
            if !op.is_shift() && lhs_value.type_id != rhs_value.type_id {
                return Err(format!(
                    "binary '{}' requires matching integer lanes, found {} and {}",
                    op.spelling(),
                    self.type_name(lhs_value.type_id),
                    self.type_name(rhs_value.type_id)
                ));
            }
            lhs_value.type_id
        } else {
            self.binary_result_type(expected, lhs_value.type_id, rhs_value.type_id, op)?
        };
        let result_kind = self
            .scalar_kind(result_type)
            .ok_or_else(|| format!("binary '{}' produced a non-scalar result", op.spelling()))?;

        if op.requires_integer_operands() {
            if lhs_kind.integer_width().is_none() || rhs_kind.integer_width().is_none() {
                return Err(format!(
                    "binary '{}' requires exact builtin integer operands",
                    op.spelling()
                ));
            }
            let lhs_bits = lhs_value.bits as u32 as u64;
            let rhs_bits = rhs_value.bits as u32 as u64;
            let width = result_kind.integer_width().expect("integer result lane");
            let shift = (rhs_bits as u32) & (width - 1);
            let bits = match op {
                ExprBinaryOp::ShiftLeft => (lhs_bits as u32).wrapping_shl(shift) as u64,
                ExprBinaryOp::ShiftRight if result_kind == ScalarKind::I32 => {
                    ((lhs_bits as i32) >> shift) as u32 as u64
                }
                ExprBinaryOp::ShiftRight => ((lhs_bits as u32) >> shift) as u64,
                ExprBinaryOp::BitAnd => lhs_bits & rhs_bits,
                ExprBinaryOp::BitXor => lhs_bits ^ rhs_bits,
                ExprBinaryOp::BitOr => lhs_bits | rhs_bits,
                ExprBinaryOp::Add
                | ExprBinaryOp::Subtract
                | ExprBinaryOp::Multiply
                | ExprBinaryOp::Divide
                | ExprBinaryOp::Remainder => unreachable!("operator requires integer operands"),
            };
            return Ok(EvaluatedScalar {
                type_id: result_type,
                bits: normalize_integer_bits(bits, result_kind),
                nominal_origin: None,
            });
        }

        if lhs_kind.is_float() || rhs_kind.is_float() {
            if op == ExprBinaryOp::Remainder {
                return Err(format!(
                    "floating-point '%' is not supported in scalar constant expressions"
                ));
            }
            let (lhs_float, rhs_float) = (
                self.convert_to_float(lhs_value, result_kind)?,
                self.convert_to_float(rhs_value, result_kind)?,
            );
            let bits = if result_kind == ScalarKind::F64 {
                let lhs = f64::from_bits(lhs_float);
                let rhs = f64::from_bits(rhs_float);
                let result = match op {
                    ExprBinaryOp::Add => lhs + rhs,
                    ExprBinaryOp::Subtract => lhs - rhs,
                    ExprBinaryOp::Multiply => lhs * rhs,
                    ExprBinaryOp::Divide => lhs / rhs,
                    _ => unreachable!("integer-only operators handled above"),
                };
                result.to_bits()
            } else {
                let lhs = f32::from_bits(lhs_float as u32);
                let rhs = f32::from_bits(rhs_float as u32);
                let result = match op {
                    ExprBinaryOp::Add => lhs + rhs,
                    ExprBinaryOp::Subtract => lhs - rhs,
                    ExprBinaryOp::Multiply => lhs * rhs,
                    ExprBinaryOp::Divide => lhs / rhs,
                    _ => unreachable!("integer-only operators handled above"),
                };
                result.to_bits() as u64
            };
            return Ok(EvaluatedScalar {
                type_id: result_type,
                bits,
                nominal_origin: None,
            });
        }

        if lhs_kind.integer_width().is_none() || rhs_kind.integer_width().is_none() {
            return Err(format!(
                "binary '{}' requires numeric operands",
                op.spelling()
            ));
        }
        let lhs_bits = lhs_value.bits as u32;
        let rhs_bits = rhs_value.bits as u32;
        let bits = match op {
            ExprBinaryOp::Add => lhs_bits.wrapping_add(rhs_bits) as u64,
            ExprBinaryOp::Subtract => lhs_bits.wrapping_sub(rhs_bits) as u64,
            ExprBinaryOp::Multiply => lhs_bits.wrapping_mul(rhs_bits) as u64,
            ExprBinaryOp::Divide if rhs_bits == 0 => {
                return Err("division by zero in scalar constant expression".into())
            }
            ExprBinaryOp::Remainder if rhs_bits == 0 => {
                return Err("remainder by zero in scalar constant expression".into())
            }
            ExprBinaryOp::Divide if result_kind == ScalarKind::I32 => {
                ((lhs_bits as i32).wrapping_div(rhs_bits as i32)) as u32 as u64
            }
            ExprBinaryOp::Remainder if result_kind == ScalarKind::I32 => {
                ((lhs_bits as i32).wrapping_rem(rhs_bits as i32)) as u32 as u64
            }
            ExprBinaryOp::Divide => (lhs_bits / rhs_bits) as u64,
            ExprBinaryOp::Remainder => (lhs_bits % rhs_bits) as u64,
            ExprBinaryOp::ShiftLeft
            | ExprBinaryOp::ShiftRight
            | ExprBinaryOp::BitAnd
            | ExprBinaryOp::BitXor
            | ExprBinaryOp::BitOr => unreachable!("integer-only operators handled above"),
        };
        Ok(EvaluatedScalar {
            type_id: result_type,
            bits: normalize_integer_bits(bits, result_kind),
            nominal_origin: None,
        })
    }

    fn binary_result_type(
        &self,
        expected: Option<TypeId>,
        lhs: TypeId,
        rhs: TypeId,
        op: ExprBinaryOp,
    ) -> Result<TypeId, String> {
        let lhs_kind = self
            .scalar_kind(lhs)
            .ok_or_else(|| "unsupported scalar operand".to_string())?;
        let rhs_kind = self
            .scalar_kind(rhs)
            .ok_or_else(|| "unsupported scalar operand".to_string())?;
        if op.requires_integer_operands() {
            if let Some(expected) = expected.filter(|type_id| {
                self.scalar_kind(*type_id)
                    .is_some_and(|kind| kind.integer_width().is_some())
            }) {
                return Ok(expected);
            }
            if lhs_kind.integer_width().is_none() || rhs_kind.integer_width().is_none() {
                return Err(format!(
                    "binary '{}' requires exact builtin integer operands",
                    op.spelling()
                ));
            }
            return Ok(if lhs == rhs { lhs } else { TYPE_ID_I32 });
        }

        if let Some(expected) = expected {
            if self
                .scalar_kind(expected)
                .is_some_and(|kind| kind.is_float())
            {
                return Ok(expected);
            }
            if self
                .scalar_kind(expected)
                .is_some_and(|kind| kind.integer_width().is_some())
                && lhs_kind.integer_width().is_some()
                && rhs_kind.integer_width().is_some()
            {
                return Ok(expected);
            }
        }
        if lhs_kind.is_float() || rhs_kind.is_float() {
            if !lhs_kind.is_float() && lhs_kind.integer_width().is_none()
                || !rhs_kind.is_float() && rhs_kind.integer_width().is_none()
            {
                return Err(format!(
                    "binary '{}' requires numeric operands",
                    op.spelling()
                ));
            }
            return Ok(if lhs == TYPE_ID_F64 || rhs == TYPE_ID_F64 {
                TYPE_ID_F64
            } else {
                TYPE_ID_F32
            });
        }
        if lhs_kind.integer_width().is_none() || rhs_kind.integer_width().is_none() {
            return Err(format!(
                "binary '{}' requires numeric operands",
                op.spelling()
            ));
        }
        Ok(expected
            .filter(|type_id| self.scalar_kind(*type_id).is_some())
            .unwrap_or_else(|| if lhs == rhs { lhs } else { TYPE_ID_I32 }))
    }

    fn convert_to_float(&self, value: EvaluatedScalar, target: ScalarKind) -> Result<u64, String> {
        let source = self
            .scalar_kind(value.type_id)
            .ok_or_else(|| "unsupported float operand".to_string())?;
        if source == ScalarKind::F64 && target == ScalarKind::F32 {
            return Err("f64 operand cannot be narrowed to f32 in a constant expression".into());
        }
        let value = match source {
            ScalarKind::F32 => f64::from(f32::from_bits(value.bits as u32)),
            ScalarKind::F64 => f64::from_bits(value.bits),
            ScalarKind::I32 => f64::from(value.bits as u32 as i32),
            ScalarKind::U8 => f64::from(value.bits as u8),
            ScalarKind::U16 => f64::from(value.bits as u16),
            ScalarKind::U32 => f64::from(value.bits as u32),
            ScalarKind::Bool => return Err("bool is not a numeric operand".into()),
        };
        Ok(if target == ScalarKind::F64 {
            value.to_bits()
        } else {
            (value as f32).to_bits() as u64
        })
    }

    fn resolve_identifier(
        &mut self,
        scope_path: &str,
        identifier: &str,
    ) -> Result<EvaluatedScalar, String> {
        let parts = identifier.split('.').collect::<Vec<_>>();
        match parts.as_slice() {
            [name] => {
                let constant = self.resolve_unqualified_constant(scope_path, name)?;
                self.evaluate_constant(constant)
            }
            [module_alias, name] if self.resolve_alias(scope_path, module_alias).is_some() => {
                self.reject_shadowed_module_alias(scope_path, module_alias, identifier)?;
                let target = self
                    .resolve_alias(scope_path, module_alias)
                    .unwrap()
                    .to_string();
                let constant = self.resolve_exact_constant(&target, name)?;
                self.evaluate_constant(constant)
            }
            [type_name, variant] => {
                let enumeration = self.resolve_unqualified_enum(scope_path, type_name)?;
                self.evaluate_variant(enumeration, variant)
            }
            [module_alias, type_name, variant] => {
                self.reject_shadowed_module_alias(scope_path, module_alias, identifier)?;
                let target = self
                    .resolve_alias(scope_path, module_alias)
                    .ok_or_else(|| {
                        format!("module alias '{module_alias}' is not imported in '{scope_path}'")
                    })?;
                let enumeration = self.resolve_exact_enum(target, type_name)?;
                self.evaluate_variant(enumeration, variant)
            }
            _ => Err(format!("unsupported scalar constant path '{identifier}'")),
        }
    }

    fn reject_shadowed_module_alias(
        &self,
        scope_path: &str,
        alias: &str,
        identifier: &str,
    ) -> Result<(), String> {
        if self.has_visible_value_name(scope_path, alias) {
            return Err(format!(
                "value binding '{alias}' shadows the imported module alias in '{identifier}'"
            ));
        }
        Ok(())
    }

    fn has_visible_value_name(&self, scope_path: &str, name: &str) -> bool {
        if self
            .value_names_by_path
            .get(scope_path)
            .is_some_and(|names| names.contains(name))
        {
            return true;
        }
        self.graph
            .dependency_closure(scope_path)
            .into_iter()
            .filter(|path| path != scope_path)
            .any(|path| {
                self.value_names_by_path
                    .get(&path)
                    .is_some_and(|names| names.contains(name))
            })
    }

    fn resolve_alias<'b>(&'b self, scope_path: &str, alias: &str) -> Option<&'b str> {
        if let Some(module) = self.graph.module(scope_path) {
            if module.alias == alias {
                return Some(&module.path);
            }
        }
        self.graph.imported_alias_target(scope_path, alias)
    }

    fn resolve_unqualified_constant(
        &self,
        scope_path: &str,
        name: &str,
    ) -> Result<ConstantDeclaration, String> {
        let local = self.constants_for_path_name(scope_path, name);
        if !local.is_empty() {
            return unique_declaration(local, "constant", name, scope_path);
        }
        let visible = self
            .graph
            .dependency_closure(scope_path)
            .into_iter()
            .filter(|path| path != scope_path)
            .flat_map(|path| self.constants_for_path_name(&path, name))
            .collect::<Vec<_>>();
        unique_declaration(visible, "constant", name, scope_path)
    }

    fn resolve_exact_constant(
        &self,
        source_path: &str,
        name: &str,
    ) -> Result<ConstantDeclaration, String> {
        unique_declaration(
            self.constants_for_path_name(source_path, name),
            "constant",
            name,
            source_path,
        )
    }

    fn constants_for_path_name(&self, path: &str, name: &str) -> Vec<ConstantDeclaration> {
        self.constants
            .get(&DeclarationKey {
                path: path.to_string(),
                name: name.to_string(),
            })
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    fn resolve_unqualified_enum(
        &self,
        scope_path: &str,
        name: &str,
    ) -> Result<EnumDeclaration, String> {
        let local = self.enums_for_path_name(scope_path, name);
        if !local.is_empty() {
            return unique_declaration(local, "enum", name, scope_path);
        }
        let visible = self
            .graph
            .dependency_closure(scope_path)
            .into_iter()
            .filter(|path| path != scope_path)
            .flat_map(|path| self.enums_for_path_name(&path, name))
            .collect::<Vec<_>>();
        unique_declaration(visible, "enum", name, scope_path)
    }

    fn resolve_exact_enum(&self, source_path: &str, name: &str) -> Result<EnumDeclaration, String> {
        unique_declaration(
            self.enums_for_path_name(source_path, name),
            "enum",
            name,
            source_path,
        )
    }

    fn enums_for_path_name(&self, path: &str, name: &str) -> Vec<EnumDeclaration> {
        self.enums
            .get(&DeclarationKey {
                path: path.to_string(),
                name: name.to_string(),
            })
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    fn evaluate_variant(
        &self,
        declaration: EnumDeclaration,
        variant_name: &str,
    ) -> Result<EvaluatedScalar, String> {
        let mut next_value = 0_i32;
        for variant in &declaration.definition.variants {
            let value = variant.value.unwrap_or(next_value);
            if variant.name == variant_name {
                let type_id = self.enum_type_id(&declaration)?;
                return Ok(EvaluatedScalar {
                    type_id,
                    bits: value as u32 as u64,
                    nominal_origin: Some(SourceTypeOrigin {
                        path: declaration.key.path,
                        name: declaration.key.name,
                    }),
                });
            }
            next_value = value.checked_add(1).ok_or_else(|| {
                format!(
                    "enum '{}' variant discriminant overflow",
                    declaration.definition.name
                )
            })?;
        }
        Err(format!(
            "unknown enum variant '{}.{}'",
            declaration.definition.name, variant_name
        ))
    }

    fn enum_type_id(&self, declaration: &EnumDeclaration) -> Result<TypeId, String> {
        let ids = self.enum_type_ids(declaration, None);
        self.types
            .resolve(&declaration.key.name)
            .filter(|type_id| ids.contains(type_id))
            .or_else(|| ids.iter().next().copied())
            .ok_or_else(|| {
                format!(
                    "enum type '{}' from '{}' has no resolved TypeId",
                    declaration.key.name, declaration.key.path
                )
            })
    }

    fn enum_type_ids(
        &self,
        declaration: &EnumDeclaration,
        visible_scope: Option<&str>,
    ) -> BTreeSet<TypeId> {
        let source_alias = self
            .graph
            .module(&declaration.key.path)
            .map(|module| module.alias.as_str());
        let qualified = source_alias.map(|alias| format!("{alias}.{}", declaration.key.name));
        let imported_aliases = visible_scope
            .and_then(|path| self.graph.module(path))
            .into_iter()
            .flat_map(|module| module.imports.iter())
            .filter(|import| import.target == declaration.key.path)
            .map(|import| format!("{}.{}", import.alias, declaration.key.name));
        qualified
            .into_iter()
            .chain(imported_aliases)
            .chain(std::iter::once(declaration.key.name.clone()))
            .filter_map(|name| self.types.resolve(&name))
            .collect()
    }

    fn evaluate_constant(
        &mut self,
        declaration: ConstantDeclaration,
    ) -> Result<EvaluatedScalar, String> {
        if let Some(value) = self.constant_cache.get(&declaration.key) {
            return Ok(value.clone());
        }
        if let Some(cycle_start) = self
            .constant_stack
            .iter()
            .position(|key| key == &declaration.key)
        {
            let mut cycle = self.constant_stack[cycle_start..]
                .iter()
                .map(display_declaration_key)
                .collect::<Vec<_>>();
            cycle.push(display_declaration_key(&declaration.key));
            return Err(format!(
                "compile-time constant cycle: {}",
                cycle.join(" -> ")
            ));
        }
        if self.constant_stack.len() >= MAX_CONSTANT_RECURSION_DEPTH {
            return Err(format!(
                "compile-time constant evaluation exceeded {MAX_CONSTANT_RECURSION_DEPTH} nested declarations"
            ));
        }

        self.constant_stack.push(declaration.key.clone());
        let result = (|| {
            let declared_type = self
                .resolve_declared_type(&declaration.key.path, &declaration.definition.type_name)?;
            let expression = parse_value_expression_at(&declaration.definition.value_text, 0)
                .map_err(|error| {
                    format!(
                        "invalid initializer for constant '{}': {error}",
                        declaration.definition.name
                    )
                })?;
            let value =
                self.evaluate_expression(&declaration.key.path, &expression, Some(declared_type))?;
            self.coerce_to_expected(&declaration.key.path, value, declared_type)
        })();
        self.constant_stack.pop();
        if let Ok(value) = &result {
            self.constant_cache
                .insert(declaration.key.clone(), value.clone());
        }
        result
    }

    fn resolve_declared_type(&self, scope_path: &str, type_name: &str) -> Result<TypeId, String> {
        let type_name = type_name.trim();
        if let Some(type_id) = self.types.resolve(type_name) {
            if self.scalar_kind(type_id).is_some() {
                return Ok(type_id);
            }
        }
        let parts = type_name.split('.').collect::<Vec<_>>();
        let declaration = match parts.as_slice() {
            [name] => self.resolve_unqualified_enum(scope_path, name)?,
            [alias, name] => {
                let target = self.resolve_alias(scope_path, alias).ok_or_else(|| {
                    format!("module alias '{alias}' is not imported in '{scope_path}'")
                })?;
                self.resolve_exact_enum(target, name)?
            }
            _ => return Err(format!("unsupported scalar type name '{type_name}'")),
        };
        self.types
            .resolve(type_name)
            .map(Ok)
            .unwrap_or_else(|| self.enum_type_id(&declaration))
    }

    fn coerce_to_expected(
        &self,
        scope_path: &str,
        value: EvaluatedScalar,
        expected: TypeId,
    ) -> Result<EvaluatedScalar, String> {
        let Some(source_kind) = self.scalar_kind(value.type_id) else {
            return self.coerce_enum_to_expected(scope_path, value, expected);
        };
        let Some(target_kind) = self.scalar_kind(expected) else {
            return Err(format!(
                "scalar type {} cannot initialize {}",
                self.type_name(value.type_id),
                self.type_name(expected)
            ));
        };
        let (bits, type_id) = match (source_kind, target_kind) {
            (ScalarKind::Bool, ScalarKind::Bool) => (value.bits, expected),
            (source, target)
                if source.integer_width().is_some() && target.integer_width().is_some() =>
            {
                let integer = integer_value(value, source);
                let bits = encode_integer_value(integer, target).ok_or_else(|| {
                    format!(
                        "constant value is not representable in scalar type {}",
                        self.type_name(expected)
                    )
                })?;
                (bits, expected)
            }
            (source, target) if source.is_float() && target.is_float() && source == target => {
                (value.bits, expected)
            }
            _ => {
                return Err(format!(
                    "scalar type {} is incompatible with {}",
                    self.type_name(value.type_id),
                    self.type_name(expected)
                ));
            }
        };
        Ok(EvaluatedScalar {
            type_id,
            bits,
            nominal_origin: None,
        })
    }

    fn coerce_enum_to_expected(
        &self,
        scope_path: &str,
        value: EvaluatedScalar,
        expected: TypeId,
    ) -> Result<EvaluatedScalar, String> {
        let Some(origin) = value.nominal_origin.as_ref() else {
            return Err(format!(
                "scalar type {} is incompatible with {}",
                self.type_name(value.type_id),
                self.type_name(expected)
            ));
        };
        let expected_origins = self.expected_enum_origins(scope_path, expected)?;
        if expected_origins.is_empty() || !expected_origins.contains(origin) {
            return Err(format!(
                "enum value from '{}.{}' does not match expected enum type {}",
                origin.path,
                origin.name,
                self.type_name(expected)
            ));
        }
        Ok(EvaluatedScalar {
            type_id: expected,
            ..value
        })
    }

    fn expected_enum_origins(
        &self,
        scope_path: &str,
        expected: TypeId,
    ) -> Result<BTreeSet<SourceTypeOrigin>, String> {
        if let Some(origin) = self.types.source_type_origin(expected) {
            return Ok(self
                .enums_for_path_name(&origin.path, &origin.name)
                .into_iter()
                .map(|declaration| SourceTypeOrigin {
                    path: declaration.key.path,
                    name: declaration.key.name,
                })
                .collect());
        }
        let info = self
            .types
            .type_info(expected)
            .ok_or_else(|| format!("unknown scalar initializer type {expected}"))?;
        if let Some((qualifier, expected_name)) = info.name.rsplit_once('.') {
            let visible = self.graph.dependency_closure(scope_path);
            let source_path = self
                .resolve_alias(scope_path, qualifier)
                .map(str::to_string)
                .or_else(|| {
                    self.graph
                        .modules()
                        .values()
                        .find(|module| module.alias == qualifier && visible.contains(&module.path))
                        .map(|module| module.path.clone())
                });
            let Some(source_path) = source_path else {
                return Ok(BTreeSet::new());
            };
            return Ok(self
                .enums_for_path_name(&source_path, expected_name)
                .into_iter()
                .filter(|declaration| {
                    self.enum_type_ids(declaration, Some(scope_path))
                        .contains(&expected)
                })
                .map(|declaration| SourceTypeOrigin {
                    path: declaration.key.path,
                    name: declaration.key.name,
                })
                .collect());
        }
        let expected_name = info.name.as_str();
        let local = self.enums_for_path_name(scope_path, expected_name);
        let candidates = if local.is_empty() {
            self.graph
                .dependency_closure(scope_path)
                .into_iter()
                .flat_map(|path| self.enums_for_path_name(&path, expected_name))
                .collect::<Vec<_>>()
        } else {
            local
        };
        let mut origins = BTreeSet::new();
        for declaration in candidates {
            if self
                .enum_type_ids(&declaration, Some(scope_path))
                .contains(&expected)
            {
                origins.insert(SourceTypeOrigin {
                    path: declaration.key.path,
                    name: declaration.key.name,
                });
            }
        }
        Ok(origins)
    }

    fn scalar_kind(&self, type_id: TypeId) -> Option<ScalarKind> {
        match type_id {
            TYPE_ID_BOOL => Some(ScalarKind::Bool),
            TYPE_ID_I32 => Some(ScalarKind::I32),
            TYPE_ID_U8 => Some(ScalarKind::U8),
            TYPE_ID_U16 => Some(ScalarKind::U16),
            TYPE_ID_U32 => Some(ScalarKind::U32),
            TYPE_ID_F32 => Some(ScalarKind::F32),
            TYPE_ID_F64 => Some(ScalarKind::F64),
            _ => None,
        }
    }

    fn type_name(&self, type_id: TypeId) -> String {
        self.types
            .type_info(type_id)
            .map_or_else(|| format!("type {type_id}"), |info| info.name.clone())
    }
}

fn unique_declaration<T: Clone>(
    mut declarations: Vec<T>,
    kind: &str,
    name: &str,
    scope_path: &str,
) -> Result<T, String>
where
    T: DeclarationPath,
{
    declarations.sort_by(|lhs, rhs| lhs.path().cmp(rhs.path()));
    match declarations.as_slice() {
        [declaration] => Ok(declaration.clone()),
        [] => Err(format!(
            "unknown compile-time {kind} '{name}' visible from '{scope_path}'"
        )),
        _ => Err(format!(
            "ambiguous compile-time {kind} '{name}' visible from '{scope_path}': {}",
            declarations
                .iter()
                .map(|declaration| declaration.path())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

trait DeclarationPath {
    fn path(&self) -> &str;
}

impl DeclarationPath for ConstantDeclaration {
    fn path(&self) -> &str {
        &self.key.path
    }
}

impl DeclarationPath for EnumDeclaration {
    fn path(&self) -> &str {
        &self.key.path
    }
}

fn display_declaration_key(key: &DeclarationKey) -> String {
    format!("{}::{}", key.path, key.name)
}

fn integer_value(value: EvaluatedScalar, kind: ScalarKind) -> i128 {
    match kind {
        ScalarKind::I32 => i128::from(value.bits as u32 as i32),
        ScalarKind::U8 => i128::from(value.bits as u8),
        ScalarKind::U16 => i128::from(value.bits as u16),
        ScalarKind::U32 => i128::from(value.bits as u32),
        _ => unreachable!("integer scalar kind required"),
    }
}

fn encode_integer_value(value: i128, kind: ScalarKind) -> Option<u64> {
    match kind {
        ScalarKind::I32 => i32::try_from(value).ok().map(|value| value as u32 as u64),
        ScalarKind::U8 => u8::try_from(value).ok().map(u64::from),
        ScalarKind::U16 => u16::try_from(value).ok().map(u64::from),
        ScalarKind::U32 => u32::try_from(value).ok().map(u64::from),
        _ => None,
    }
}

fn normalize_integer_bits(bits: u64, kind: ScalarKind) -> u64 {
    match kind {
        ScalarKind::U8 => bits & u64::from(u8::MAX),
        ScalarKind::U16 => bits & u64::from(u16::MAX),
        ScalarKind::I32 | ScalarKind::U32 => bits & u64::from(u32::MAX),
        _ => bits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::module_graph::ModuleGraph;
    use crate::frontend::parser::parse_top_level_type_layout;

    fn setup(
        sources: &[(&str, &str)],
        roots: &[&str],
    ) -> (Vec<SourceFile>, ModuleGraph, TypeTable) {
        let source_map = sources
            .iter()
            .map(|(path, source)| (path.to_string(), source.to_string()))
            .collect::<BTreeMap<_, _>>();
        let (graph, _) = ModuleGraph::load(roots.iter().map(|root| root.to_string()), |path| {
            source_map
                .get(path)
                .cloned()
                .ok_or_else(|| format!("missing test module '{path}'"))
        })
        .expect("load test module graph");
        let files = sources
            .iter()
            .map(|(path, source)| SourceFile {
                path: path.to_string(),
                content: source.to_string(),
                original_content: source.to_string(),
                hash: 0,
                functions: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut types = TypeTable::new();
        for file in &files {
            let layout = parse_top_level_type_layout(&file.original_content)
                .expect("parse test type declarations");
            for enumeration in layout.enums {
                types
                    .resolve_or_intern(&enumeration.name)
                    .expect("intern test enum type");
            }
        }
        (files, graph, types)
    }

    fn evaluate(
        files: &[SourceFile],
        graph: &ModuleGraph,
        types: &TypeTable,
        scope: &str,
        expression: &str,
        expected_type: TypeId,
    ) -> Result<EvaluatedScalar, String> {
        ScalarConstantEvaluator::new(files, graph, types)?.evaluate(
            scope,
            expression,
            expected_type,
        )
    }

    #[test]
    fn evaluates_contextual_integer_widths_and_exact_float_bits() {
        let (files, graph, types) = setup(
            &[(
                "main.stasis",
                concat!(
                    "const BYTE_LIMIT: u8 = 255; ",
                    "const WORD_LIMIT: u32 = 4294967295; ",
                    "const ZERO_F32: f32 = 0.0; ",
                    "const ZERO_F64: f64 = 0.0; ",
                    "const MINIMUM: i32 = -2147483648;",
                ),
            )],
            &["main.stasis"],
        );

        let mut evaluator = ScalarConstantEvaluator::new(&files, &graph, &types).unwrap();
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "BYTE_LIMIT", TYPE_ID_U8)
                .unwrap()
                .bits,
            255
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "WORD_LIMIT", TYPE_ID_U32)
                .unwrap()
                .bits,
            u64::from(u32::MAX)
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "-ZERO_F32", TYPE_ID_F32)
                .unwrap()
                .bits,
            0x8000_0000
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "-ZERO_F64", TYPE_ID_F64)
                .unwrap()
                .bits,
            0x8000_0000_0000_0000
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "0.0 - ZERO_F32", TYPE_ID_F32)
                .unwrap()
                .bits,
            0,
            "ordinary subtraction of positive zeros must stay positive zero"
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "MINIMUM", TYPE_ID_I32)
                .unwrap()
                .bits,
            0x8000_0000
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "5 + 2 * 3", TYPE_ID_I32)
                .unwrap()
                .bits,
            11
        );
        assert!(evaluator
            .evaluate("main.stasis", "256", TYPE_ID_U8)
            .unwrap_err()
            .contains("not representable"));
        assert!(evaluator
            .evaluate("main.stasis", "-1", TYPE_ID_U8)
            .unwrap_err()
            .contains("not representable"));
        assert!(evaluator
            .evaluate("main.stasis", "1", TYPE_ID_BOOL)
            .unwrap_err()
            .contains("integer literal"));
        assert!(evaluator
            .evaluate("main.stasis", "1 / 0", TYPE_ID_I32)
            .unwrap_err()
            .contains("division by zero"));
    }

    #[test]
    fn resolves_constants_and_enum_variants_by_module_visibility_and_origin() {
        let sources = [
            ("main.stasis", "import \"a.stasis\"; import \"b.stasis\";"),
            (
                "a.stasis",
                "const VALUE: i32 = 11; enum Phase { Active = 7, }",
            ),
            (
                "b.stasis",
                "const VALUE: i32 = 22; enum Phase { Active = 9, }",
            ),
        ];
        let (files, graph, types) = setup(&sources, &["main.stasis"]);
        let phase_type = types.resolve("Phase").expect("shared nominal enum lane");
        let mut evaluator = ScalarConstantEvaluator::new(&files, &graph, &types).unwrap();

        assert_eq!(
            evaluator
                .evaluate("main.stasis", "a.VALUE", TYPE_ID_I32)
                .unwrap()
                .bits,
            11
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "b.VALUE", TYPE_ID_I32)
                .unwrap()
                .bits,
            22
        );
        assert!(evaluator
            .evaluate("main.stasis", "VALUE", TYPE_ID_I32)
            .unwrap_err()
            .contains("ambiguous compile-time constant"));

        let from_a = evaluator
            .evaluate("main.stasis", "a.Phase.Active", phase_type)
            .unwrap();
        let from_b = evaluator
            .evaluate("main.stasis", "b.Phase.Active", phase_type)
            .unwrap();
        assert_eq!(from_a.bits, 7);
        assert_eq!(from_b.bits, 9);
        assert_eq!(
            from_a.nominal_origin,
            Some(SourceTypeOrigin {
                path: "a.stasis".to_string(),
                name: "Phase".to_string(),
            })
        );
        assert_eq!(
            from_b.nominal_origin,
            Some(SourceTypeOrigin {
                path: "b.stasis".to_string(),
                name: "Phase".to_string(),
            })
        );
    }

    #[test]
    fn rejects_hidden_constants_cycles_and_runtime_expressions() {
        let sources = [
            ("main.stasis", "import \"visible.stasis\";"),
            ("visible.stasis", "const VISIBLE: i32 = 3;"),
            (
                "hidden.stasis",
                "const SECRET: i32 = 9; const FIRST: i32 = SECOND; const SECOND: i32 = FIRST; function compute(): i32 { return 1; }",
            ),
        ];
        let (files, graph, types) = setup(&sources, &["main.stasis"]);
        let mut evaluator = ScalarConstantEvaluator::new(&files, &graph, &types).unwrap();
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "VISIBLE", TYPE_ID_I32)
                .unwrap()
                .bits,
            3
        );
        assert!(evaluator
            .evaluate("main.stasis", "SECRET", TYPE_ID_I32)
            .unwrap_err()
            .contains("unknown compile-time constant"));
        let hidden_constant_error = evaluator
            .evaluate("main.stasis", "hidden.SECRET", TYPE_ID_I32)
            .unwrap_err();
        // `hidden.stasis` is present in the test file list but absent from the
        // module graph. Do not guess that an unimported qualifier names it.
        assert!(
            hidden_constant_error
                .contains("unknown compile-time enum 'hidden' visible from 'main.stasis'"),
            "unexpected hidden-constant diagnostic: {hidden_constant_error}"
        );
        let cycle = evaluator
            .evaluate("hidden.stasis", "FIRST", TYPE_ID_I32)
            .unwrap_err();
        assert!(cycle.contains("compile-time constant cycle"), "{cycle}");
        assert!(evaluator
            .evaluate("main.stasis", "compute()", TYPE_ID_I32)
            .unwrap_err()
            .contains("runtime call 'compute'"));
    }

    #[test]
    fn local_constant_shadows_imported_unqualified_name() {
        let sources = [
            (
                "main.stasis",
                "import \"values.stasis\"; const LIMIT: i32 = 5;",
            ),
            ("values.stasis", "const LIMIT: i32 = 9;"),
        ];
        let (files, graph, types) = setup(&sources, &["main.stasis"]);
        let mut evaluator = ScalarConstantEvaluator::new(&files, &graph, &types).unwrap();
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "LIMIT", TYPE_ID_I32)
                .unwrap()
                .bits,
            5
        );
        assert_eq!(
            evaluator
                .evaluate("main.stasis", "values.LIMIT", TYPE_ID_I32)
                .unwrap()
                .bits,
            9
        );
    }

    #[test]
    fn source_value_roots_shadow_imported_module_aliases() {
        let module_source = "const VALUE: i32 = 9; enum Phase { Active = 7, }";
        for main_source in [
            "import \"limits.stasis\"; const limits: i32 = 3;",
            "import \"limits.stasis\"; global limits: i32;",
            "import \"limits.stasis\"; global limits { value: i32; }",
        ] {
            let sources = [
                ("main.stasis", main_source),
                ("limits.stasis", module_source),
            ];
            let (files, graph, types) = setup(&sources, &["main.stasis"]);
            let mut evaluator = ScalarConstantEvaluator::new(&files, &graph, &types).unwrap();
            for expression in ["limits.VALUE", "limits.Phase.Active"] {
                let error = evaluator
                    .evaluate("main.stasis", expression, TYPE_ID_I32)
                    .expect_err("a source value root must shadow the imported alias");
                assert!(
                    error.contains("value binding 'limits' shadows the imported module alias"),
                    "unexpected error for {expression}: {error}"
                );
            }
        }

        let sources = [
            (
                "main.stasis",
                "import \"limits.stasis\"; import \"bindings.stasis\";",
            ),
            ("limits.stasis", module_source),
            ("bindings.stasis", "const limits: i32 = 3;"),
        ];
        let (files, graph, types) = setup(&sources, &["main.stasis"]);
        let error = evaluate(
            &files,
            &graph,
            &types,
            "main.stasis",
            "limits.VALUE",
            TYPE_ID_I32,
        )
        .expect_err("a visible imported value name must shadow the module alias");
        assert!(
            error.contains("value binding 'limits' shadows the imported module alias"),
            "unexpected error: {error}"
        );
    }
}
