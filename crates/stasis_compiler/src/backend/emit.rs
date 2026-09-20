use crate::backend::compile_analysis::{
    is_collection_handle_type, is_i32_abi_compatible_type, is_i32_numeric_type,
    is_i32_scalar_lane_type, is_struct_view_type, resolve_call_signature, CallSignature,
    CallSignatureMap, CollectionInfoMap, ConstantValue, ConstantValueMap, ExternImportKey,
    ForeachCollectionInfo, GlobalPathTypeMap, NamedStructFieldTypeMap,
};
use crate::compiler::{FunctionId, FunctionMeta};
use crate::data_flow::{FunctionDataFlowSummary, ParameterStorageKind};
use crate::frontend::types::{
    TypeCategory, TypeId, TypeTable, TypedCollectionDescriptor, TypedCollectionKind, TYPE_ID_BOOL,
    TYPE_ID_F32, TYPE_ID_F64, TYPE_ID_I32, TYPE_ID_U16, TYPE_ID_U32, TYPE_ID_U8, TYPE_ID_VOID,
};
use crate::ir::hir::{
    eval_const_i64, AssignOp, AssignTarget, ComparisonOp, ConversionKind, DebugStatement,
    FunctionHIR, SimpleCondition, SimpleExpr, SimpleStmt,
};
use cranelift_codegen::ir::{
    condcodes::{FloatCC, IntCC},
    immediates::{Ieee32, Ieee64},
    types, AbiParam, Block, FuncRef, InstBuilder, MemFlags, TrapCode, Value,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static RUNTIME_HELPER_TRAMPOLINES_DEFINED: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn reset_runtime_helper_trampoline_count_for_test() {
    RUNTIME_HELPER_TRAMPOLINES_DEFINED.store(0, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn runtime_helper_trampoline_count_for_test() -> usize {
    RUNTIME_HELPER_TRAMPOLINES_DEFINED.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeachBinding {
    pub(crate) collection_handle: ForeachCollectionHandle,
    pub(crate) index_var: Variable,
    pub(crate) len: i32,
    pub(crate) element_type: Option<TypeId>,
    pub(crate) struct_type_id: Option<TypeId>,
    pub(crate) field_types: BTreeMap<String, TypeId>,
    pub(crate) u8_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) u16_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) i32_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) f32_array_base_ptrs: BTreeMap<String, Value>,
    pub(crate) f64_array_base_ptrs: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForeachCollectionHandle {
    PathHash(i32),
    LocalVar(Variable),
}

pub(crate) type ForeachBindingMap = BTreeMap<String, ForeachBinding>;

fn normalize_unsigned_value(
    builder: &mut FunctionBuilder<'_>,
    value: Value,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Value {
    match type_table.unsigned_integer_bits(type_id) {
        Some(8) => builder.ins().band_imm(value, 0xff),
        Some(16) => builder.ins().band_imm(value, 0xffff),
        _ => value,
    }
}

fn integer_binary_result_type(
    expected_type: Option<TypeId>,
    lhs: TypeId,
    rhs: TypeId,
    type_table: &TypeTable,
) -> TypeId {
    if let Some(expected) = expected_type.filter(|id| type_table.is_integer(*id)) {
        return expected;
    }
    if lhs == rhs && type_table.is_integer(lhs) {
        lhs
    } else {
        TYPE_ID_I32
    }
}

fn unambiguous_call_params(
    target: &str,
    arg_count: usize,
    call_signatures: &CallSignatureMap,
) -> Option<Vec<TypeId>> {
    let mut candidates = call_signatures
        .get(target)?
        .iter()
        .filter(|signature| signature.params.len() == arg_count);
    let first = candidates.next()?.params.clone();
    candidates
        .all(|candidate| candidate.params == first)
        .then_some(first)
}

fn emit_integer_assignment_value(
    builder: &mut FunctionBuilder<'_>,
    lhs: Option<Value>,
    rhs: Value,
    op: AssignOp,
    type_table: &TypeTable,
    type_id: TypeId,
) -> Value {
    let unsigned = type_table.unsigned_integer_bits(type_id).is_some();
    let value = match op {
        AssignOp::Set => rhs,
        AssignOp::Add => builder
            .ins()
            .iadd(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Sub => builder
            .ins()
            .isub(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Mul => builder
            .ins()
            .imul(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Div if unsigned => builder
            .ins()
            .udiv(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Mod if unsigned => builder
            .ins()
            .urem(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Div => builder
            .ins()
            .sdiv(lhs.expect("compound assignment lhs"), rhs),
        AssignOp::Mod => builder
            .ins()
            .srem(lhs.expect("compound assignment lhs"), rhs),
    };
    normalize_unsigned_value(builder, value, type_id, type_table)
}

pub(crate) fn are_assignment_types_compatible(
    target_type: TypeId,
    expression_type: TypeId,
    type_table: &TypeTable,
) -> bool {
    type_table.assignment_types_are_compatible(target_type, expression_type)
}

pub(crate) struct RuntimeCallImportIds {
    pub(crate) print_i32: FuncId,
    pub(crate) print_string: FuncId,
    pub(crate) sin_fast: FuncId,
    pub(crate) cos_fast: FuncId,
    pub(crate) global_i32_load: FuncId,
    pub(crate) global_i32_store: FuncId,
    pub(crate) global_f32_load: FuncId,
    pub(crate) global_f32_store: FuncId,
    pub(crate) global_f64_load: FuncId,
    pub(crate) global_f64_store: FuncId,
    pub(crate) global_i32_array_load: FuncId,
    pub(crate) global_i32_array_store: FuncId,
    pub(crate) global_i32_array_ptr: FuncId,
    pub(crate) global_f32_array_load: FuncId,
    pub(crate) global_f32_array_store: FuncId,
    pub(crate) global_f32_array_ptr: FuncId,
    pub(crate) global_f64_array_load: FuncId,
    pub(crate) global_f64_array_store: FuncId,
    pub(crate) global_f64_array_ptr: FuncId,
    pub(crate) collection_i32_load: FuncId,
    pub(crate) collection_i32_store: FuncId,
    pub(crate) debug_frame_enter: Option<FuncId>,
    pub(crate) debug_frame_leave: Option<FuncId>,
    pub(crate) debug_statement: Option<FuncId>,
    pub(crate) debug_values_begin: Option<FuncId>,
    pub(crate) debug_value_i64: Option<FuncId>,
    pub(crate) debug_value_f64: Option<FuncId>,
    pub(crate) profile_frame_enter: Option<FuncId>,
    pub(crate) profile_frame_leave: Option<FuncId>,
    pub(crate) extern_calls: BTreeMap<ExternImportKey, FuncId>,
}

pub(crate) struct RuntimeCallRefs {
    pub(crate) print_i32: FuncRef,
    pub(crate) print_string: FuncRef,
    pub(crate) sin_fast: FuncRef,
    pub(crate) cos_fast: FuncRef,
    pub(crate) global_i32_load: FuncRef,
    pub(crate) global_i32_store: FuncRef,
    pub(crate) global_f32_load: FuncRef,
    pub(crate) global_f32_store: FuncRef,
    pub(crate) global_f64_load: FuncRef,
    pub(crate) global_f64_store: FuncRef,
    pub(crate) global_i32_array_load: FuncRef,
    pub(crate) global_i32_array_store: FuncRef,
    pub(crate) global_i32_array_ptr: FuncRef,
    pub(crate) global_f32_array_load: FuncRef,
    pub(crate) global_f32_array_store: FuncRef,
    pub(crate) global_f32_array_ptr: FuncRef,
    pub(crate) global_f64_array_load: FuncRef,
    pub(crate) global_f64_array_store: FuncRef,
    pub(crate) global_f64_array_ptr: FuncRef,
    pub(crate) collection_i32_load: FuncRef,
    pub(crate) collection_i32_store: FuncRef,
    pub(crate) debug: Option<DebugRuntimeRefs>,
    pub(crate) profile: Option<ProfileRuntimeRefs>,
    pub(crate) extern_calls: BTreeMap<ExternImportKey, FuncRef>,
    pub(crate) direct_storage: Option<DirectStorageRefs>,
}

#[derive(Debug, Clone)]
pub(crate) enum DirectStorageBinding {
    Absolute(usize),
    Symbol(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectStorageBindings {
    pub(crate) scalars: BTreeMap<String, DirectStorageBinding>,
    pub(crate) arrays: BTreeMap<(String, String), DirectArrayStorageBinding>,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectArrayStorageBinding {
    pub(crate) slot: DirectStorageBinding,
    pub(crate) storage_bytes: u8,
    pub(crate) static_len: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum DirectStorageRef {
    Absolute(usize),
    Symbol(cranelift_codegen::ir::GlobalValue),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectStorageRefs {
    pub(crate) scalars: BTreeMap<String, DirectStorageRef>,
    pub(crate) arrays: BTreeMap<(String, String), DirectArrayStorageRef>,
    pub(crate) arrays_by_hash: BTreeMap<(i32, i32), DirectArrayStorageRef>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectArrayStorageRef {
    pub(crate) slot: DirectStorageRef,
    pub(crate) storage_bytes: u8,
    pub(crate) static_len: Option<usize>,
}

pub(crate) struct DirectCallMode<'a> {
    pub(crate) module: &'a mut dyn Module,
    pub(crate) self_function_id: FunctionId,
    pub(crate) self_clif_func_id: FuncId,
    pub(crate) imported_function_ids: HashMap<FunctionId, FuncId>,
    pub(crate) symbol_prefix: &'static str,
    pub(crate) force_far_nonself_calls: bool,
    /// A direct `if (receiver.can_*())` preflight is emitted before its then
    /// arm.  The proof lives only in the compiler while lowering that arm;
    /// it is never materialized as an ABI value or a runtime helper object.
    typed_collection_guard_capture: Option<TypedCollectionGuardCapture>,
    typed_collection_guard_proofs: Vec<TypedCollectionGuardProof>,
}

pub(crate) enum InternalCallMode<'a> {
    Direct(DirectCallMode<'a>),
}

#[derive(Clone)]
struct TypedCollectionGuardCapture {
    target: String,
    args: Vec<SimpleExpr>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypedCollectionGuardAction {
    PoolPush,
    PoolRemove,
    StablePoolInsert,
    StablePoolRemove,
    MapPut,
    MapGet,
    MapRemove,
    SetAdd,
    SetRemove,
    QueuePush,
    QueuePop,
    QueuePeek,
    RingBufferPush,
    RingBufferPop,
    RingBufferPeek,
    PriorityQueuePush,
    PriorityQueuePop,
    PriorityQueuePeek,
    GridAccess,
    BitsetAccess,
}

#[derive(Clone, Copy)]
enum TypedCollectionGuardState {
    Pool {
        count: Value,
        index: Option<Value>,
    },
    StablePool {
        count: Value,
        index_or_first_free: Value,
    },
    Keyed {
        count: Value,
        existing: Value,
        first_free: Value,
    },
    Circular {
        count: Value,
        head: Value,
        physical: Option<Value>,
    },
    PriorityQueue {
        count: Value,
        next_order: Option<Value>,
    },
    Indexed {
        index: Value,
    },
}

#[derive(Clone)]
struct TypedCollectionGuardProof {
    action: TypedCollectionGuardAction,
    receiver: SimpleExpr,
    key_args: Vec<SimpleExpr>,
    state: TypedCollectionGuardState,
}

impl InternalCallMode<'_> {
    fn begin_typed_collection_guard_capture(&mut self, target: &str, args: &[SimpleExpr]) {
        let InternalCallMode::Direct(mode) = self;
        mode.typed_collection_guard_capture = Some(TypedCollectionGuardCapture {
            target: target.to_string(),
            args: args.to_vec(),
        });
    }

    fn clear_typed_collection_guard(&mut self) {
        let InternalCallMode::Direct(mode) = self;
        mode.typed_collection_guard_capture = None;
        mode.typed_collection_guard_proofs.clear();
    }

    fn capture_typed_collection_guard_proof(
        &mut self,
        can_target: &str,
        action: TypedCollectionGuardAction,
        args: &[SimpleExpr],
        state: TypedCollectionGuardState,
    ) {
        let InternalCallMode::Direct(mode) = self;
        let Some(capture) = mode.typed_collection_guard_capture.as_ref() else {
            return;
        };
        if capture.target != can_target || capture.args != args {
            return;
        }
        let Some(receiver) = args.first().cloned() else {
            return;
        };
        let key_args = args.iter().skip(1).cloned().collect();
        mode.typed_collection_guard_proofs
            .push(TypedCollectionGuardProof {
                action,
                receiver,
                key_args,
                state,
            });
        mode.typed_collection_guard_capture = None;
    }

    fn take_typed_collection_guard_proof(
        &mut self,
        action: TypedCollectionGuardAction,
        args: &[SimpleExpr],
    ) -> Option<TypedCollectionGuardProof> {
        let InternalCallMode::Direct(mode) = self;
        let key_indices: &[usize] = match action {
            TypedCollectionGuardAction::PoolRemove
            | TypedCollectionGuardAction::StablePoolRemove
            | TypedCollectionGuardAction::MapPut
            | TypedCollectionGuardAction::MapGet
            | TypedCollectionGuardAction::MapRemove
            | TypedCollectionGuardAction::SetAdd
            | TypedCollectionGuardAction::SetRemove
            | TypedCollectionGuardAction::QueuePeek
            | TypedCollectionGuardAction::RingBufferPeek
            | TypedCollectionGuardAction::BitsetAccess => &[1],
            TypedCollectionGuardAction::GridAccess => &[1, 2],
            TypedCollectionGuardAction::PoolPush
            | TypedCollectionGuardAction::StablePoolInsert
            | TypedCollectionGuardAction::QueuePush
            | TypedCollectionGuardAction::QueuePop
            | TypedCollectionGuardAction::RingBufferPush
            | TypedCollectionGuardAction::RingBufferPop
            | TypedCollectionGuardAction::PriorityQueuePush
            | TypedCollectionGuardAction::PriorityQueuePop
            | TypedCollectionGuardAction::PriorityQueuePeek => &[],
        };
        let action_key_args = key_indices
            .iter()
            .map(|index| args.get(*index).cloned())
            .collect::<Option<Vec<_>>>()?;
        let receiver = args.first().cloned()?;
        let index = mode
            .typed_collection_guard_proofs
            .iter()
            .rposition(|proof| {
                proof.action == action
                    && proof.receiver == receiver
                    && proof.key_args == action_key_args
            })?;
        Some(mode.typed_collection_guard_proofs.remove(index))
    }

    fn take_typed_collection_guard_state(
        &mut self,
        action: TypedCollectionGuardAction,
        args: &[SimpleExpr],
    ) -> Option<TypedCollectionGuardState> {
        self.take_typed_collection_guard_proof(action, args)
            .map(|proof| proof.state)
    }

    fn typed_collection_guard_proofs(&self) -> Vec<TypedCollectionGuardProof> {
        let InternalCallMode::Direct(mode) = self;
        mode.typed_collection_guard_proofs.clone()
    }

    fn restore_typed_collection_guard_proofs(&mut self, proofs: Vec<TypedCollectionGuardProof>) {
        let InternalCallMode::Direct(mode) = self;
        mode.typed_collection_guard_capture = None;
        mode.typed_collection_guard_proofs = proofs;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SharedCompileBackendMode {
    JitDirect,
    AotDirect,
}

#[derive(Clone, Copy)]
pub(crate) enum RuntimeHelperLinkage<'a> {
    Imported,
    LocalTrampolines(&'a BTreeMap<String, usize>),
}

fn emit_direct_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    mode: &mut DirectCallMode<'_>,
    signature: &CallSignature,
    arg_values: &[Value],
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<Option<Value>, String> {
    if signature.extern_symbol.is_some() {
        return Err("direct call emission requested for extern signature".to_string());
    }
    let function_id = signature
        .function_id
        .ok_or_else(|| "direct call emission requested for missing function id".to_string())?;

    let callee_func_id = if function_id == mode.self_function_id {
        mode.self_clif_func_id
    } else if let Some(existing) = mode.imported_function_ids.get(&function_id).copied() {
        existing
    } else {
        let symbol = format!("{}{function_id}", mode.symbol_prefix);
        let mut import_signature = mode.module.make_signature();
        for param_type in &signature.params {
            append_abi_params_for_type_id(
                &mut import_signature.params,
                *param_type,
                type_table,
                named_struct_field_types,
            )?;
        }
        if signature.return_type != TYPE_ID_VOID {
            import_signature
                .returns
                .push(AbiParam::new(clif_type_for_type_id(
                    signature.return_type,
                    type_table,
                )?));
        }
        let func_id = mode
            .module
            .declare_function(&symbol, Linkage::Import, &import_signature)
            .map_err(|error| format!("failed to declare AOT import {symbol}: {error}"))?;
        mode.imported_function_ids.insert(function_id, func_id);
        func_id
    };

    let func_ref = mode
        .module
        .declare_func_in_func(callee_func_id, builder.func);
    if mode.force_far_nonself_calls && function_id != mode.self_function_id {
        builder.func.dfg.ext_funcs[func_ref].colocated = false;
    }
    let call = builder.ins().call(func_ref, arg_values);
    if signature.return_type == TYPE_ID_VOID {
        Ok(None)
    } else {
        let value = builder
            .inst_results(call)
            .first()
            .copied()
            .ok_or_else(|| "direct call expected value result but produced none".to_string())?;
        Ok(Some(value))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ValueBinding {
    pub(crate) value: Value,
    pub(crate) type_id: TypeId,
}

#[derive(Clone, Copy)]
pub(crate) struct StructViewValue {
    pub(crate) type_id: TypeId,
    pub(crate) base: Value,
    pub(crate) index: Value,
    pub(crate) len: Value,
    pub(crate) storage_kind: StructViewStorageKind,
    pub(crate) known_collection_hash: Option<i32>,
    pub(crate) bounds_proven: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StructViewStorageKind {
    Dynamic,
    Aos,
    Soa,
}

#[derive(Clone, Copy)]
pub(crate) struct LocalBinding {
    pub(crate) var: Variable,
    pub(crate) type_id: TypeId,
    pub(crate) struct_view: Option<StructViewBinding>,
    pub(crate) proven_index_upper: Option<usize>,
}

#[derive(Clone, Copy)]
pub(crate) struct StructViewBinding {
    pub(crate) index_var: Variable,
    pub(crate) len_var: Variable,
    pub(crate) storage_kind: StructViewStorageKind,
    pub(crate) known_collection_hash: Option<i32>,
    pub(crate) bounds_proven: bool,
}

pub(crate) fn compile_function_with_module<M, T, BeforeStatement, OnFunctionBuilt, Finalize>(
    mut module: M,
    meta: &FunctionMeta,
    hir: &FunctionHIR,
    symbol: &str,
    runtime_helper_linkage: RuntimeHelperLinkage<'_>,
    backend_mode: SharedCompileBackendMode,
    call_signatures: &CallSignatureMap,
    type_table: &mut TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    data_flow_summary: Option<&FunctionDataFlowSummary>,
    direct_storage: Option<&DirectStorageBindings>,
    defined_runtime_helper_trampolines: Option<&mut BTreeSet<String>>,
    debug_instrumentation: bool,
    profile_instrumentation: bool,
    mut before_statement: BeforeStatement,
    on_function_built: OnFunctionBuilt,
    finalize: Finalize,
) -> Result<T, String>
where
    M: Module,
    BeforeStatement: FnMut(&SimpleStmt) -> Result<(), String>,
    OnFunctionBuilt: FnOnce(&FunctionMeta, &cranelift_codegen::ir::Function),
    Finalize: FnOnce(M, FuncId, cranelift_codegen::Context) -> Result<T, String>,
{
    let mut context = module.make_context();
    context.func.signature = module.make_signature();
    for param_type in &meta.params {
        append_abi_params_for_type_id(
            &mut context.func.signature.params,
            *param_type,
            type_table,
            named_struct_field_types,
        )?;
    }
    if meta.return_type != TYPE_ID_VOID {
        let clif_return_type =
            clif_type_for_type_id(meta.return_type, type_table).map_err(|_| {
                format!(
                    "unsupported return type id {} for function {}",
                    meta.return_type, meta.name
                )
            })?;
        context
            .func
            .signature
            .returns
            .push(AbiParam::new(clif_return_type));
    }

    let function_id = module
        .declare_function(symbol, Linkage::Export, &context.func.signature)
        .map_err(|error| format!("failed to declare function {symbol}: {error}"))?;
    let referenced_call_targets = collect_call_targets_from_hir(hir);
    let has_struct_view_param = meta
        .params
        .iter()
        .any(|type_id| is_struct_view_type(*type_id, named_struct_field_types));
    let uses_runtime_storage = backend_mode == SharedCompileBackendMode::JitDirect
        || !global_path_types.is_empty()
        || has_struct_view_param;
    let uses_collection_runtime = backend_mode == SharedCompileBackendMode::JitDirect
        || !collection_infos.is_empty()
        || has_struct_view_param;
    let runtime_call_imports = match backend_mode {
        SharedCompileBackendMode::JitDirect | SharedCompileBackendMode::AotDirect => {
            build_direct_runtime_call_import_ids(
                &mut module,
                function_id,
                runtime_helper_linkage,
                uses_runtime_storage,
                uses_collection_runtime,
                &referenced_call_targets,
                call_signatures,
                type_table,
                named_struct_field_types,
                debug_instrumentation,
                profile_instrumentation,
            )?
        }
    };

    let mut function_builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut function_builder_context);
        let runtime_call_refs = build_runtime_call_refs(
            &mut module,
            &runtime_call_imports,
            builder.func,
            direct_storage,
        )?;
        let entry = builder.create_block();
        for param_type in &meta.params {
            if is_struct_view_type(*param_type, named_struct_field_types) {
                for _ in 0..STRUCT_VIEW_ABI_WORDS {
                    builder.append_block_param(entry, types::I32);
                }
            } else {
                builder.append_block_param(entry, clif_type_for_type_id(*param_type, type_table)?);
            }
        }
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        if meta.param_names.len() != meta.params.len() {
            return Err(format!(
                "parameter metadata mismatch for function '{}' ({} names, {} types)",
                meta.name,
                meta.param_names.len(),
                meta.params.len()
            ));
        }
        let mut values_by_name: BTreeMap<String, LocalBinding> = BTreeMap::new();
        let mut next_variable = 0u32;
        let block_params: Vec<Value> = builder.block_params(entry).to_vec();
        let mut block_param_cursor = 0usize;
        for (index, name) in meta.param_names.iter().enumerate() {
            let param_type = meta.params[index];
            let (variable, struct_view) =
                if is_struct_view_type(param_type, named_struct_field_types) {
                    let base_value =
                        block_params
                            .get(block_param_cursor)
                            .copied()
                            .ok_or_else(|| {
                                format!(
                                    "missing struct view base parameter {} for function '{}'",
                                    block_param_cursor, meta.name
                                )
                            })?;
                    let index_value = block_params
                        .get(block_param_cursor + 1)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "missing struct view index parameter {} for function '{}'",
                                block_param_cursor + 1,
                                meta.name
                            )
                        })?;
                    let len_value = block_params
                        .get(block_param_cursor + 2)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "missing struct view len parameter {} for function '{}'",
                                block_param_cursor + 2,
                                meta.name
                            )
                        })?;
                    block_param_cursor += STRUCT_VIEW_ABI_WORDS;

                    let base_var = declare_new_variable(
                        &mut builder,
                        &mut next_variable,
                        base_value,
                        param_type,
                        type_table,
                    )?;
                    let index_var = declare_new_variable(
                        &mut builder,
                        &mut next_variable,
                        index_value,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    let len_var = declare_new_variable(
                        &mut builder,
                        &mut next_variable,
                        len_value,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    (
                        base_var,
                        Some(StructViewBinding {
                            index_var,
                            len_var,
                            storage_kind: data_flow_summary
                                .and_then(|summary| summary.parameter_storage_kinds.get(index))
                                .map_or(StructViewStorageKind::Dynamic, |kind| match kind {
                                    ParameterStorageKind::Dynamic => StructViewStorageKind::Dynamic,
                                    ParameterStorageKind::Aos => StructViewStorageKind::Aos,
                                    ParameterStorageKind::Soa => StructViewStorageKind::Soa,
                                }),
                            known_collection_hash: None,
                            bounds_proven: false,
                        }),
                    )
                } else {
                    let value = block_params
                        .get(block_param_cursor)
                        .copied()
                        .ok_or_else(|| {
                            format!(
                                "missing block parameter {} for function '{}'",
                                block_param_cursor, meta.name
                            )
                        })?;
                    block_param_cursor = block_param_cursor.saturating_add(1);
                    (
                        declare_new_variable(
                            &mut builder,
                            &mut next_variable,
                            value,
                            param_type,
                            type_table,
                        )?,
                        None,
                    )
                };
            if values_by_name.contains_key(name) {
                return Err(format!("parameter '{}' shadows existing variable", name));
            }
            values_by_name.insert(
                name.clone(),
                LocalBinding {
                    var: variable,
                    type_id: param_type,
                    struct_view,
                    proven_index_upper: None,
                },
            );
        }
        if block_param_cursor != block_params.len() {
            return Err(format!(
                "block parameter count mismatch for function '{}' (consumed {}, found {})",
                meta.name,
                block_param_cursor,
                block_params.len()
            ));
        }

        let empty_foreach_bindings = ForeachBindingMap::new();
        let symbol_prefix = match backend_mode {
            SharedCompileBackendMode::JitDirect => "jit_fn_",
            SharedCompileBackendMode::AotDirect => "aot_fn_",
        };
        // Cranelift's JIT allocator maps functions independently, so no architecture's
        // range-limited direct-call relocation can assume that functions are colocated.
        let force_far_nonself_calls = backend_mode == SharedCompileBackendMode::JitDirect;
        let mut internal_calls = InternalCallMode::Direct(DirectCallMode {
            module: &mut module,
            self_function_id: meta.id,
            self_clif_func_id: function_id,
            imported_function_ids: HashMap::new(),
            symbol_prefix,
            force_far_nonself_calls,
            typed_collection_guard_capture: None,
            typed_collection_guard_proofs: Vec::new(),
        });
        if let Some(debug) = runtime_call_refs.debug.as_ref() {
            if hir.debug_statements.len() != hir.statements.len() {
                return Err(format!(
                    "debug statement metadata mismatch for function '{}'",
                    meta.name
                ));
            }
            emit_debug_frame_boundary(&mut builder, debug.frame_enter, meta.id);
        }
        if let Some(profile) = runtime_call_refs.profile.as_ref() {
            emit_function_frame_boundary(&mut builder, profile.frame_enter, meta.id);
        }
        let mut terminated = false;
        for (index, statement) in hir.statements.iter().enumerate() {
            if terminated {
                break;
            }
            before_statement(statement)?;
            terminated = emit_simple_statements(
                &mut builder,
                std::slice::from_ref(statement),
                runtime_call_refs
                    .debug
                    .as_ref()
                    .map(|_| std::slice::from_ref(&hir.debug_statements[index])),
                runtime_call_refs.debug.as_ref(),
                meta.id,
                &mut values_by_name,
                &runtime_call_refs,
                &mut internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                &empty_foreach_bindings,
                None,
                meta.return_type,
                &mut next_variable,
            )?;
        }
        if !terminated {
            if meta.return_type == TYPE_ID_VOID {
                if let Some(debug) = runtime_call_refs.debug.as_ref() {
                    emit_debug_frame_boundary(&mut builder, debug.frame_leave, meta.id);
                }
                if let Some(profile) = runtime_call_refs.profile.as_ref() {
                    emit_function_frame_boundary(&mut builder, profile.frame_leave, meta.id);
                }
                builder.ins().return_(&[]);
            } else {
                return Err(format!(
                    "non-void function '{}' must end with a return statement",
                    meta.name
                ));
            }
        }
        builder.finalize();
    }

    define_referenced_runtime_helper_trampolines(
        &mut module,
        runtime_helper_linkage,
        &context.func,
        defined_runtime_helper_trampolines,
    )?;

    on_function_built(meta, &context.func);
    finalize(module, function_id, context)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionMetaKind {
    Length = 1,
    MaxLength = 2,
    CharLength = 3,
}

pub(crate) fn collection_meta_kind_from_suffix(suffix: &str) -> Option<CollectionMetaKind> {
    match suffix {
        "length" => Some(CollectionMetaKind::Length),
        "max_length" => Some(CollectionMetaKind::MaxLength),
        "char_length" => Some(CollectionMetaKind::CharLength),
        // Alias: treat byte_length as length (read-only in source-level semantics for now).
        "byte_length" => Some(CollectionMetaKind::Length),
        _ => None,
    }
}

pub(crate) fn declare_runtime_helper(
    module: &mut impl Module,
    symbol: &str,
    signature: cranelift_codegen::ir::Signature,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    match linkage {
        RuntimeHelperLinkage::Imported => module
            .declare_function(symbol, Linkage::Import, &signature)
            .map_err(|error| format!("failed to declare JIT import {symbol}: {error}")),
        RuntimeHelperLinkage::LocalTrampolines(addresses) => {
            if !addresses.contains_key(symbol) {
                return Err(format!("missing runtime helper address for {symbol}"));
            }
            let local_symbol = format!("__stasis_runtime_helper_{symbol}");
            // Preemptible deliberately marks the helper as non-colocated. On AArch64,
            // Cranelift then emits an address load plus an indirect call instead of a
            // range-limited BL relocation. The helper is still defined in this private
            // JIT module; the linkage only controls the generated call sequence.
            module
                .declare_function(&local_symbol, Linkage::Preemptible, &signature)
                .map_err(|error| {
                    format!("failed to declare runtime helper trampoline {symbol}: {error}")
                })
        }
    }
}

fn define_referenced_runtime_helper_trampolines(
    module: &mut impl Module,
    linkage: RuntimeHelperLinkage<'_>,
    function: &cranelift_codegen::ir::Function,
    mut defined: Option<&mut BTreeSet<String>>,
) -> Result<(), String> {
    let RuntimeHelperLinkage::LocalTrampolines(addresses) = linkage else {
        return Ok(());
    };
    let mut referenced = BTreeSet::new();
    for block in function.layout.blocks() {
        for instruction in function.layout.block_insts(block) {
            let cranelift_codegen::ir::InstructionData::Call { func_ref, .. } =
                &function.dfg.insts[instruction]
            else {
                continue;
            };
            let cranelift_codegen::ir::ExternalName::User(name_ref) =
                &function.dfg.ext_funcs[*func_ref].name
            else {
                continue;
            };
            let name = &function.params.user_named_funcs()[*name_ref];
            if name.namespace == 0 {
                referenced.insert(FuncId::from_u32(name.index));
            }
        }
    }

    let mut trampolines = Vec::new();
    for func_id in referenced {
        let declaration = module.declarations().get_function_decl(func_id);
        let Some(symbol) = declaration
            .name
            .as_deref()
            .and_then(|name| name.strip_prefix("__stasis_runtime_helper_"))
        else {
            continue;
        };
        let address = addresses
            .get(symbol)
            .copied()
            .ok_or_else(|| format!("missing runtime helper address for {symbol}"))?;
        trampolines.push((
            func_id,
            symbol.to_string(),
            declaration.signature.clone(),
            address,
        ));
    }
    for (func_id, symbol, signature, address) in trampolines {
        if defined
            .as_deref_mut()
            .is_some_and(|defined| !defined.insert(symbol.clone()))
        {
            continue;
        }
        define_runtime_helper_trampoline(module, func_id, &symbol, signature, address)?;
    }
    Ok(())
}

fn define_runtime_helper_trampoline(
    module: &mut impl Module,
    func_id: FuncId,
    symbol: &str,
    signature: cranelift_codegen::ir::Signature,
    address: usize,
) -> Result<(), String> {
    let pointer_type = module.target_config().pointer_type();
    let pointer_bits = pointer_type.bits();
    if pointer_bits < usize::BITS && address > u32::MAX as usize {
        return Err(format!(
            "runtime helper address for {symbol} does not fit target pointer type"
        ));
    }
    let mut context = module.make_context();
    context.func.signature = signature.clone();
    let mut builder_context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        for param in &signature.params {
            builder.append_block_param(entry, param.value_type);
        }
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let args = builder.block_params(entry).to_vec();
        let address_value = builder.ins().iconst(pointer_type, address as i64);
        let signature_ref = builder.func.import_signature(signature);
        let call = builder
            .ins()
            .call_indirect(signature_ref, address_value, &args);
        let results = builder.inst_results(call).to_vec();
        builder.ins().return_(&results);
        builder.finalize();
    }
    module
        .define_function(func_id, &mut context)
        .map_err(|error| format!("failed to define runtime helper trampoline {symbol}: {error}"))?;
    module.clear_context(&mut context);
    #[cfg(test)]
    RUNTIME_HELPER_TRAMPOLINES_DEFINED.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
pub(crate) fn declare_i32_call_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    for _ in 0..param_count {
        signature.params.push(AbiParam::new(types::I32));
    }
    signature.returns.push(AbiParam::new(types::I32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_direct_f32_unary_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::F32));
    signature.returns.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_void_call_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
    param_count: usize,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    for _ in 0..param_count {
        signature.params.push(AbiParam::new(types::I32));
    }
    declare_runtime_helper(module, symbol, signature, linkage)
}

fn declare_debug_value_i64_import(
    module: &mut impl Module,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, "stasis_jit_debug_value_i64", signature, linkage)
}

fn declare_debug_value_f64_import(
    module: &mut impl Module,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, "stasis_jit_debug_value_f64", signature, linkage)
}

pub(crate) fn declare_f32_global_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_global_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_global_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_global_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_i32_array_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_i32_array_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_i32_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_array_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_array_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F32));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f32_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_array_load_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_array_store_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::F64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_f64_array_ptr_import(
    module: &mut impl Module,
    symbol: &str,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<FuncId, String> {
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    declare_runtime_helper(module, symbol, signature, linkage)
}

pub(crate) fn declare_extern_call_imports(
    module: &mut impl Module,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    linkage: RuntimeHelperLinkage<'_>,
) -> Result<BTreeMap<ExternImportKey, FuncId>, String> {
    let mut out = BTreeMap::new();
    for signatures in call_signatures.values() {
        for signature in signatures {
            let Some(symbol) = signature.extern_symbol.as_ref() else {
                continue;
            };
            let key = ExternImportKey {
                symbol: symbol.clone(),
                params: signature.params.clone(),
                return_type: signature.return_type,
            };
            if out.contains_key(&key) {
                continue;
            }
            let mut clif_signature = module.make_signature();
            for param in &signature.params {
                append_abi_params_for_type_id(
                    &mut clif_signature.params,
                    *param,
                    type_table,
                    named_struct_field_types,
                )?;
            }
            if signature.return_type != TYPE_ID_VOID {
                clif_signature
                    .returns
                    .push(AbiParam::new(clif_type_for_type_id(
                        signature.return_type,
                        type_table,
                    )?));
            }
            let func_id = declare_runtime_helper(module, symbol, clif_signature, linkage).map_err(
                |error| {
                    format!(
                        "failed to declare extern import '{}' with params {:?} return {}: {}",
                        symbol, signature.params, signature.return_type, error
                    )
                },
            )?;
            out.insert(key, func_id);
        }
    }
    Ok(out)
}

pub(crate) fn declare_new_variable(
    builder: &mut FunctionBuilder<'_>,
    next_variable: &mut u32,
    initial_value: Value,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<Variable, String> {
    let next = *next_variable;
    let variable = Variable::from_u32(next);
    *next_variable = next_variable
        .checked_add(1)
        .ok_or_else(|| "too many local variables".to_string())?;
    builder.declare_var(variable, clif_type_for_type_id(type_id, type_table)?);
    let initial_value = normalize_unsigned_value(builder, initial_value, type_id, type_table);
    builder.def_var(variable, initial_value);
    Ok(variable)
}

pub(crate) const STRUCT_VIEW_ABI_WORDS: usize = 3;
pub(crate) const STRUCT_VIEW_AOS_INDEX_SENTINEL: i32 = -1;
pub(crate) const STRUCT_VIEW_AOS_LEN_SENTINEL: i32 = 0;

pub(crate) fn append_abi_params_for_type_id(
    params: &mut Vec<AbiParam>,
    type_id: TypeId,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<(), String> {
    if is_struct_view_type(type_id, named_struct_field_types) {
        for _ in 0..STRUCT_VIEW_ABI_WORDS {
            params.push(AbiParam::new(types::I32));
        }
        Ok(())
    } else {
        params.push(AbiParam::new(clif_type_for_type_id(type_id, type_table)?));
        Ok(())
    }
}

pub(crate) fn clif_type_for_type_id(
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<cranelift_codegen::ir::Type, String> {
    match type_id {
        TYPE_ID_I32 => Ok(types::I32),
        TYPE_ID_F32 => Ok(types::F32),
        TYPE_ID_F64 => Ok(types::F64),
        TYPE_ID_BOOL | TYPE_ID_U8 | TYPE_ID_U16 | TYPE_ID_U32 => Ok(types::I32),
        TYPE_ID_VOID => Err("void is not a value type".to_string()),
        other => {
            let Some(info) = type_table.type_info(other) else {
                return Err(format!("unsupported type id {other} in current jit path"));
            };
            match info.category {
                TypeCategory::Builtin => {
                    Err(format!("unsupported type id {other} in current jit path"))
                }
                TypeCategory::Named
                | TypeCategory::ArrayFixed
                | TypeCategory::ArrayView
                | TypeCategory::AsciiFixed
                | TypeCategory::AsciiView
                | TypeCategory::Utf8Fixed
                | TypeCategory::Utf8View => Ok(types::I32),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopControlContext {
    pub(crate) continue_block: Block,
}

#[derive(Clone, Copy)]
pub(crate) struct DebugRuntimeRefs {
    pub(crate) frame_enter: FuncRef,
    pub(crate) frame_leave: FuncRef,
    pub(crate) statement: FuncRef,
    pub(crate) values_begin: FuncRef,
    pub(crate) value_i64: FuncRef,
    pub(crate) value_f64: FuncRef,
}

#[derive(Clone, Copy)]
pub(crate) struct ProfileRuntimeRefs {
    pub(crate) frame_enter: FuncRef,
    pub(crate) frame_leave: FuncRef,
}

pub(crate) fn emit_host_print_call_statement(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    if target != "print_i32"
        && target != "print_string"
        && target != "print_int"
        && target != "print_char"
    {
        return Ok(false);
    }
    if args.len() != 1 {
        return Err(format!(
            "host extern '{}' expects exactly one argument, found {}",
            target,
            args.len()
        ));
    }
    let argument = emit_simple_expression(
        builder,
        &args[0],
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if !is_i32_abi_compatible_type(argument.type_id, type_table) {
        return Err(format!(
            "host extern '{}' requires i32-abi-compatible argument, found type {}",
            target, argument.type_id
        ));
    }
    if target == "print_i32" || target == "print_int" || target == "print_char" {
        builder
            .ins()
            .call(runtime_call_refs.print_i32, &[argument.value]);
    } else {
        builder
            .ins()
            .call(runtime_call_refs.print_string, &[argument.value]);
    }
    Ok(true)
}

pub(crate) fn emit_extern_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    signature: &CallSignature,
    arg_values: &[Value],
) -> Result<Option<Value>, String> {
    let Some(symbol) = signature.extern_symbol.as_ref() else {
        return Err("extern dispatch requested for internal call signature".to_string());
    };
    let key = ExternImportKey {
        symbol: symbol.clone(),
        params: signature.params.clone(),
        return_type: signature.return_type,
    };
    let Some(func_ref) = runtime_call_refs.extern_calls.get(&key).copied() else {
        return Err(format!(
            "missing extern import binding for symbol '{}' with params {:?} return {}",
            symbol, signature.params, signature.return_type
        ));
    };
    let call = builder.ins().call(func_ref, arg_values);
    if signature.return_type == TYPE_ID_VOID {
        Ok(None)
    } else {
        let value = builder.inst_results(call).first().copied().ok_or_else(|| {
            format!(
                "extern call to '{}' expected value result but produced none",
                symbol
            )
        })?;
        Ok(Some(value))
    }
}

pub(crate) fn emit_internal_call_for_signature(
    builder: &mut FunctionBuilder<'_>,
    _runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    signature: &CallSignature,
    arg_values: &[Value],
    _arg_types: &[TypeId],
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    _target: &str,
) -> Result<Option<Value>, String> {
    if signature.extern_symbol.is_some() {
        return Err("internal direct call requested for extern signature".to_string());
    }
    let InternalCallMode::Direct(mode) = internal_calls;
    emit_direct_call_for_signature(
        builder,
        mode,
        signature,
        arg_values,
        type_table,
        named_struct_field_types,
    )
}
pub(crate) fn ensure_no_variable_shadowing(
    name: &str,
    values_by_name: &BTreeMap<String, LocalBinding>,
    foreach_bindings: &ForeachBindingMap,
    binding_kind: &str,
) -> Result<(), String> {
    if values_by_name.contains_key(name) || foreach_bindings.contains_key(name) {
        return Err(format!(
            "{} '{}' shadows existing variable",
            binding_kind, name
        ));
    }
    Ok(())
}

pub(crate) fn try_emit_indexed_struct_copy_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let AssignTarget::IndexedPath {
        collection_path: target_collection,
        index: target_index,
        suffix: target_suffix,
    } = target
    else {
        return Ok(false);
    };
    if !target_suffix.is_empty() {
        return Ok(false);
    }
    let SimpleExpr::IndexedPath {
        collection_path: source_collection,
        index: source_index,
        suffix: source_suffix,
    } = expression
    else {
        return Ok(false);
    };
    if !source_suffix.is_empty() {
        return Ok(false);
    }

    if op != AssignOp::Set {
        return Err(format!(
            "struct indexed copy assignment only supports '=' for '{}[...]'",
            target_collection
        ));
    }

    let target_index_binding = emit_simple_expression(
        builder,
        target_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    let source_index_binding = emit_simple_expression(
        builder,
        source_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;

    let local_target = values_by_name.get(target_collection).copied();
    let local_source = values_by_name.get(source_collection).copied();
    if let (Some(target_local), Some(source_local)) = (local_target, local_source) {
        let Some(target_element_type) = type_table.indexed_element_type_id(target_local.type_id)
        else {
            return Ok(false);
        };
        let Some(source_element_type) = type_table.indexed_element_type_id(source_local.type_id)
        else {
            return Ok(false);
        };
        let Some(target_fields) = named_struct_field_types.get(&target_element_type) else {
            return Ok(false);
        };
        let Some(source_fields) = named_struct_field_types.get(&source_element_type) else {
            return Ok(false);
        };
        if target_fields != source_fields {
            return Err(format!(
                "struct indexed copy assignment requires matching field layout for '{}[...]' and '{}[...]'",
                target_collection, source_collection
            ));
        }
        for field_name in target_fields.keys() {
            let source_value = emit_local_indexed_collection_load(
                builder,
                runtime_call_refs,
                type_table,
                named_struct_field_types,
                source_collection,
                source_local,
                field_name,
                source_index_binding,
            )?;
            emit_local_indexed_collection_assignment(
                builder,
                runtime_call_refs,
                type_table,
                named_struct_field_types,
                target_collection,
                target_local,
                field_name,
                target_index_binding,
                AssignOp::Set,
                source_value,
            )?;
        }
        return Ok(true);
    }

    if local_target.is_some() || local_source.is_some() {
        return Ok(false);
    }

    let Some(target_info) = collection_infos.get(target_collection) else {
        return Ok(false);
    };
    let Some(source_info) = collection_infos.get(source_collection) else {
        return Ok(false);
    };
    if target_info.field_types.is_empty() || source_info.field_types.is_empty() {
        return Ok(false);
    }
    if target_info.field_types != source_info.field_types {
        return Err(format!(
            "struct indexed copy assignment requires matching field layout for '{}[...]' and '{}[...]'",
            target_collection, source_collection
        ));
    }
    let source_bounds_proven =
        static_index_bounds_proven(source_index, source_info.len as usize, values_by_name);
    let target_bounds_proven =
        static_index_bounds_proven(target_index, target_info.len as usize, values_by_name);

    for field_name in target_info.field_types.keys() {
        let source_value = emit_indexed_collection_load(
            builder,
            runtime_call_refs,
            type_table,
            source_collection,
            source_info,
            field_name,
            source_index_binding,
            source_bounds_proven,
        )?;
        emit_indexed_collection_assignment(
            builder,
            runtime_call_refs,
            type_table,
            target_collection,
            target_info,
            field_name,
            target_index_binding,
            target_bounds_proven,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn try_emit_global_struct_copy_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    global_path_types: &GlobalPathTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let target_path = match target {
        AssignTarget::GlobalPath(path) => path.as_str(),
        AssignTarget::Local(path) => {
            if values_by_name.contains_key(path) || foreach_bindings.contains_key(path) {
                return Ok(false);
            }
            path.as_str()
        }
        AssignTarget::IndexedPath { .. } => return Ok(false),
    };
    let SimpleExpr::Identifier(source_path) = expression else {
        return Ok(false);
    };
    if values_by_name.contains_key(source_path) || foreach_bindings.contains_key(source_path) {
        return Ok(false);
    }
    if let Some(type_id) = global_path_types.get(target_path).copied() {
        let is_named_struct = type_table
            .type_info(type_id)
            .is_some_and(|info| info.category == TypeCategory::Named);
        if !is_named_struct {
            return Ok(false);
        }
    }
    if let Some(type_id) = global_path_types.get(source_path).copied() {
        let is_named_struct = type_table
            .type_info(type_id)
            .is_some_and(|info| info.category == TypeCategory::Named);
        if !is_named_struct {
            return Ok(false);
        }
    }

    let target_prefix = format!("{target_path}.");
    let source_prefix = format!("{source_path}.");
    let mut fields: Vec<(String, TypeId)> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            path.strip_prefix(&target_prefix)
                .map(|suffix| (suffix.to_string(), *type_id))
        })
        .collect();
    if fields.is_empty() {
        return Ok(false);
    }
    if op != AssignOp::Set {
        return Err(format!(
            "struct path copy assignment only supports '=' for '{}'",
            target_path
        ));
    }
    fields.sort_by(|left, right| left.0.cmp(&right.0));

    for (suffix, target_type) in &fields {
        if is_collection_handle_type(*target_type, type_table) {
            return Err(format!(
                "struct path copy assignment currently supports scalar fields only for '{}'",
                target_path
            ));
        }
        let source_field = format!("{source_prefix}{suffix}");
        let Some(source_type) = global_path_types.get(&source_field).copied() else {
            return Err(format!(
                "struct path copy assignment requires matching field path '{}'",
                source_field
            ));
        };
        if source_type != *target_type {
            return Err(format!(
                "struct path copy assignment type mismatch at field '{}': target {} source {}",
                suffix, target_type, source_type
            ));
        }
    }

    for (suffix, field_type) in fields {
        let source_field = format!("{source_prefix}{suffix}");
        let target_field = format!("{target_prefix}{suffix}");
        let source_value = emit_global_load(
            builder,
            runtime_call_refs,
            type_table,
            &source_field,
            field_type,
        )?;
        emit_global_assignment(
            builder,
            runtime_call_refs,
            type_table,
            &target_field,
            field_type,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn try_emit_struct_copy_from_indexed_to_global(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let target_path = match target {
        AssignTarget::GlobalPath(path) => path.as_str(),
        AssignTarget::Local(path) => {
            if values_by_name.contains_key(path) || foreach_bindings.contains_key(path) {
                return Ok(false);
            }
            path.as_str()
        }
        AssignTarget::IndexedPath { .. } => return Ok(false),
    };
    let SimpleExpr::IndexedPath {
        collection_path: source_collection,
        index: source_index,
        suffix: source_suffix,
    } = expression
    else {
        return Ok(false);
    };
    if !source_suffix.is_empty() {
        return Ok(false);
    }
    if values_by_name.contains_key(source_collection)
        || foreach_bindings.contains_key(source_collection)
    {
        return Ok(false);
    }
    let Some(source_info) = collection_infos.get(source_collection) else {
        return Ok(false);
    };
    if source_info.field_types.is_empty() {
        return Ok(false);
    }
    if op != AssignOp::Set {
        return Err(format!(
            "struct copy assignment from indexed source only supports '=' for '{}'",
            target_path
        ));
    }
    let target_prefix = format!("{target_path}.");
    let target_fields: BTreeMap<String, TypeId> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            path.strip_prefix(&target_prefix)
                .map(|suffix| (suffix.to_string(), *type_id))
        })
        .collect();
    if target_fields.is_empty() {
        return Ok(false);
    }
    if target_fields != source_info.field_types {
        return Err(format!(
            "struct copy assignment from indexed source requires matching field layout for '{}' and '{}[...]'",
            target_path, source_collection
        ));
    }
    for type_id in target_fields.values() {
        if is_collection_handle_type(*type_id, type_table) {
            return Err(format!(
                "struct copy assignment from indexed source currently supports scalar fields only for '{}'",
                target_path
            ));
        }
    }

    let source_index_binding = emit_simple_expression(
        builder,
        source_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    let source_bounds_proven =
        static_index_bounds_proven(source_index, source_info.len as usize, values_by_name);
    for (field_name, field_type) in &source_info.field_types {
        let source_value = emit_indexed_collection_load(
            builder,
            runtime_call_refs,
            type_table,
            source_collection,
            source_info,
            field_name,
            source_index_binding,
            source_bounds_proven,
        )?;
        let target_field = format!("{target_prefix}{field_name}");
        emit_global_assignment(
            builder,
            runtime_call_refs,
            type_table,
            &target_field,
            *field_type,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn try_emit_struct_copy_from_global_to_indexed(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    type_table: &TypeTable,
    target: &AssignTarget,
    op: AssignOp,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    call_signatures: &CallSignatureMap,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<bool, String> {
    let AssignTarget::IndexedPath {
        collection_path: target_collection,
        index: target_index,
        suffix: target_suffix,
    } = target
    else {
        return Ok(false);
    };
    if !target_suffix.is_empty() {
        return Ok(false);
    }
    if values_by_name.contains_key(target_collection)
        || foreach_bindings.contains_key(target_collection)
    {
        return Ok(false);
    }
    let SimpleExpr::Identifier(source_path) = expression else {
        return Ok(false);
    };
    if values_by_name.contains_key(source_path) || foreach_bindings.contains_key(source_path) {
        return Ok(false);
    }
    let Some(target_info) = collection_infos.get(target_collection) else {
        return Ok(false);
    };
    if target_info.field_types.is_empty() {
        return Ok(false);
    }
    if op != AssignOp::Set {
        return Err(format!(
            "struct copy assignment to indexed target only supports '=' for '{}[...]'",
            target_collection
        ));
    }
    let source_prefix = format!("{source_path}.");
    let source_fields: BTreeMap<String, TypeId> = global_path_types
        .iter()
        .filter_map(|(path, type_id)| {
            path.strip_prefix(&source_prefix)
                .map(|suffix| (suffix.to_string(), *type_id))
        })
        .collect();
    if source_fields.is_empty() {
        return Ok(false);
    }
    if source_fields != target_info.field_types {
        return Err(format!(
            "struct copy assignment to indexed target requires matching field layout for '{}[...]' and '{}'",
            target_collection, source_path
        ));
    }
    for type_id in source_fields.values() {
        if is_collection_handle_type(*type_id, type_table) {
            return Err(format!(
                "struct copy assignment to indexed target currently supports scalar fields only for '{}[...]'",
                target_collection
            ));
        }
    }

    let target_index_binding = emit_simple_expression(
        builder,
        target_index,
        None,
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    let target_bounds_proven =
        static_index_bounds_proven(target_index, target_info.len as usize, values_by_name);
    for (field_name, field_type) in &target_info.field_types {
        let source_field = format!("{source_prefix}{field_name}");
        let source_value = emit_global_load(
            builder,
            runtime_call_refs,
            type_table,
            &source_field,
            *field_type,
        )?;
        emit_indexed_collection_assignment(
            builder,
            runtime_call_refs,
            type_table,
            target_collection,
            target_info,
            field_name,
            target_index_binding,
            target_bounds_proven,
            AssignOp::Set,
            source_value,
        )?;
    }
    Ok(true)
}

pub(crate) fn debug_variable_slot(name: &str) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn emit_function_frame_boundary(
    builder: &mut FunctionBuilder<'_>,
    function: FuncRef,
    function_id: FunctionId,
) {
    let function_id = builder.ins().iconst(types::I32, i64::from(function_id));
    builder.ins().call(function, &[function_id]);
}

fn emit_debug_frame_boundary(
    builder: &mut FunctionBuilder<'_>,
    function: FuncRef,
    function_id: FunctionId,
) {
    emit_function_frame_boundary(builder, function, function_id);
}

fn emit_debug_statement(
    builder: &mut FunctionBuilder<'_>,
    debug: &DebugRuntimeRefs,
    function_id: FunctionId,
    site_id: u32,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    foreach_bindings: &ForeachBindingMap,
) -> Result<(), String> {
    builder.ins().call(debug.values_begin, &[]);
    let mut slots = HashMap::<u32, &str>::new();
    for (name, binding) in values_by_name {
        let slot = debug_variable_slot(name);
        if let Some(existing) = slots.insert(slot, name) {
            return Err(format!(
                "debug variable slot collision between '{existing}' and '{name}'"
            ));
        }
        let value = builder.use_var(binding.var);
        emit_debug_value(
            builder,
            debug,
            name,
            slot,
            binding.type_id,
            value,
            type_table,
        )?;
    }
    for (name, binding) in foreach_bindings {
        if binding.element_type.is_none() {
            continue;
        }
        let slot = debug_variable_slot(name);
        if let Some(existing) = slots.insert(slot, name) {
            return Err(format!(
                "debug variable slot collision between '{existing}' and '{name}'"
            ));
        }
        let value = emit_foreach_binding_load(builder, runtime_call_refs, type_table, binding, "")?;
        emit_debug_value(
            builder,
            debug,
            name,
            slot,
            value.type_id,
            value.value,
            type_table,
        )?;
    }
    let function_id = builder.ins().iconst(types::I32, i64::from(function_id));
    let site_id = builder.ins().iconst(types::I32, i64::from(site_id as i32));
    builder.ins().call(debug.statement, &[function_id, site_id]);
    Ok(())
}

fn emit_debug_value(
    builder: &mut FunctionBuilder<'_>,
    debug: &DebugRuntimeRefs,
    name: &str,
    slot: u32,
    type_id: TypeId,
    value: Value,
    type_table: &TypeTable,
) -> Result<(), String> {
    let slot_value = builder.ins().iconst(types::I32, i64::from(slot as i32));
    let type_value = builder.ins().iconst(types::I32, i64::from(type_id));
    match type_id {
        TYPE_ID_F32 => {
            let value = builder.ins().fpromote(types::F64, value);
            builder
                .ins()
                .call(debug.value_f64, &[slot_value, type_value, value]);
        }
        TYPE_ID_F64 => {
            builder
                .ins()
                .call(debug.value_f64, &[slot_value, type_value, value]);
        }
        _ => {
            let clif_type = clif_type_for_type_id(type_id, type_table)?;
            if clif_type != types::I32 {
                return Err(format!(
                    "unsupported debug value type id {type_id} for '{name}'"
                ));
            }
            let value = if type_id == TYPE_ID_I32 {
                builder.ins().sextend(types::I64, value)
            } else {
                builder.ins().uextend(types::I64, value)
            };
            builder
                .ins()
                .call(debug.value_i64, &[slot_value, type_value, value]);
        }
    }
    Ok(())
}

pub(crate) fn emit_simple_statements(
    builder: &mut FunctionBuilder<'_>,
    statements: &[SimpleStmt],
    debug_statements: Option<&[DebugStatement]>,
    debug_refs: Option<&DebugRuntimeRefs>,
    function_id: FunctionId,
    values_by_name: &mut BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
    loop_control: Option<&LoopControlContext>,
    expected_return_type: TypeId,
    next_variable: &mut u32,
) -> Result<bool, String> {
    if debug_refs.is_some() && debug_statements.is_none_or(|debug| debug.len() != statements.len())
    {
        return Err("debug statement metadata does not match lowered statements".to_string());
    }
    for (index, statement) in statements.iter().enumerate() {
        if let Some(debug_refs) = debug_refs {
            let debug = &debug_statements.expect("debug metadata was validated")[index];
            emit_debug_statement(
                builder,
                debug_refs,
                function_id,
                debug.source_offset,
                values_by_name,
                runtime_call_refs,
                type_table,
                foreach_bindings,
            )?;
        }
        match statement {
            SimpleStmt::Noop => {}
            SimpleStmt::Let {
                name,
                type_id,
                expression,
            } => {
                ensure_no_variable_shadowing(
                    name,
                    values_by_name,
                    foreach_bindings,
                    "let binding",
                )?;

                if let Some(struct_view) = try_emit_struct_view_value(
                    builder,
                    expression,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    let local_type_id = type_id.unwrap_or(struct_view.type_id);
                    if local_type_id != struct_view.type_id {
                        return Err(format!(
                            "let binding '{}' expected type {} view expression but found {}",
                            name, local_type_id, struct_view.type_id
                        ));
                    }
                    let mut view_bounds_proven = struct_view.bounds_proven;
                    // A struct-array view with an arbitrary index must still fail fatally. The
                    // check belongs to creation of the alias rather than every field access
                    // through that alias. This applies to both fixed globals and array-view
                    // parameters; the latter carry their runtime length alongside the handle.
                    // Once validated, all loads and stores from the same {base,index,len} view
                    // may reuse the fact.
                    if !view_bounds_proven && struct_view.storage_kind == StructViewStorageKind::Soa
                    {
                        emit_array_bounds_trap(builder, struct_view.index, struct_view.len);
                        view_bounds_proven = true;
                    }
                    let variable = declare_new_variable(
                        builder,
                        next_variable,
                        struct_view.base,
                        local_type_id,
                        type_table,
                    )?;
                    let index_var = declare_new_variable(
                        builder,
                        next_variable,
                        struct_view.index,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    let len_var = declare_new_variable(
                        builder,
                        next_variable,
                        struct_view.len,
                        TYPE_ID_I32,
                        type_table,
                    )?;
                    values_by_name.insert(
                        name.clone(),
                        LocalBinding {
                            var: variable,
                            type_id: local_type_id,
                            struct_view: Some(StructViewBinding {
                                index_var,
                                len_var,
                                storage_kind: struct_view.storage_kind,
                                known_collection_hash: struct_view.known_collection_hash,
                                bounds_proven: view_bounds_proven,
                            }),
                            proven_index_upper: None,
                        },
                    );
                    continue;
                } else if type_id
                    .is_some_and(|type_id| is_struct_view_type(type_id, named_struct_field_types))
                {
                    return Err(format!(
                        "let binding '{}' requires view initializer for struct type {}",
                        name,
                        type_id.unwrap_or(TYPE_ID_VOID)
                    ));
                }

                let binding = emit_simple_expression(
                    builder,
                    expression,
                    *type_id,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                let local_type_id = if let Some(declared_type_id) = *type_id {
                    if !are_assignment_types_compatible(
                        declared_type_id,
                        binding.type_id,
                        type_table,
                    ) {
                        let expected = type_table
                            .type_info(declared_type_id)
                            .map_or_else(|| declared_type_id.to_string(), |info| info.name.clone());
                        let found = type_table
                            .type_info(binding.type_id)
                            .map_or_else(|| binding.type_id.to_string(), |info| info.name.clone());
                        return Err(format!(
                            "let binding '{name}' expected {expected} expression but found {found}"
                        ));
                    }
                    declared_type_id
                } else {
                    binding.type_id
                };
                let variable = declare_new_variable(
                    builder,
                    next_variable,
                    binding.value,
                    local_type_id,
                    type_table,
                )?;
                values_by_name.insert(
                    name.clone(),
                    LocalBinding {
                        var: variable,
                        type_id: local_type_id,
                        struct_view: None,
                        proven_index_upper: None,
                    },
                );
            }
            SimpleStmt::Assign {
                target,
                op,
                expression,
            } => {
                if try_emit_indexed_struct_copy_assignment(
                    builder,
                    runtime_call_refs,
                    internal_calls,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    call_signatures,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                if try_emit_global_struct_copy_assignment(
                    builder,
                    runtime_call_refs,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    global_path_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                if try_emit_struct_copy_from_indexed_to_global(
                    builder,
                    runtime_call_refs,
                    internal_calls,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    call_signatures,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                if try_emit_struct_copy_from_global_to_indexed(
                    builder,
                    runtime_call_refs,
                    internal_calls,
                    type_table,
                    target,
                    *op,
                    expression,
                    values_by_name,
                    call_signatures,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    continue;
                }
                let expected_rhs_type = match target {
                    AssignTarget::Local(name) => values_by_name
                        .get(name)
                        .map(|binding| binding.type_id)
                        .or_else(|| global_path_types.get(name).copied()),
                    AssignTarget::GlobalPath(path) => {
                        global_path_types.get(path).copied().or_else(|| {
                            let (base, suffix) = path.split_once('.')?;
                            let local = values_by_name.get(base)?;
                            let field_types = named_struct_field_types.get(&local.type_id)?;
                            field_types.get(suffix).copied()
                        })
                    }
                    AssignTarget::IndexedPath {
                        collection_path,
                        suffix,
                        ..
                    } => {
                        if let Some(local_collection) = values_by_name.get(collection_path).copied()
                        {
                            Some(resolve_local_collection_value_type(
                                local_collection.type_id,
                                suffix,
                                type_table,
                                named_struct_field_types,
                            )?)
                        } else if let Some((base, field)) = collection_path.split_once('.') {
                            let collection_type = values_by_name
                                .get(base)
                                .and_then(|local| named_struct_field_types.get(&local.type_id))
                                .and_then(|fields| fields.get(field))
                                .copied();
                            if let Some(collection_type) = collection_type {
                                Some(resolve_local_collection_value_type(
                                    collection_type,
                                    suffix,
                                    type_table,
                                    named_struct_field_types,
                                )?)
                            } else if let Some(collection_info) =
                                collection_infos.get(collection_path)
                            {
                                Some(resolve_collection_value_type(collection_info, suffix)?)
                            } else {
                                None
                            }
                        } else if let Some(collection_info) = collection_infos.get(collection_path)
                        {
                            Some(resolve_collection_value_type(collection_info, suffix)?)
                        } else {
                            None
                        }
                    }
                };
                let rhs = emit_simple_expression(
                    builder,
                    expression,
                    expected_rhs_type,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                match target {
                    AssignTarget::Local(name) => {
                        if let Some(local) = values_by_name.get(name).copied() {
                            if is_struct_view_type(local.type_id, named_struct_field_types) {
                                return Err(format!(
                                    "assignment target '{}' is a view binding and is not rebindable in current jit path",
                                    name
                                ));
                            }
                            if !are_assignment_types_compatible(
                                local.type_id,
                                rhs.type_id,
                                type_table,
                            ) {
                                return Err(format!(
                                    "assignment type mismatch for '{}': target type {}, expression type {}",
                                    name, local.type_id, rhs.type_id
                                ));
                            }
                            let value = if is_collection_handle_type(local.type_id, type_table) {
                                if *op != AssignOp::Set {
                                    return Err(format!(
                                        "collection handle assignment only supports '=' in current jit path for '{}'",
                                        name
                                    ));
                                }
                                rhs.value
                            } else if is_i32_scalar_lane_type(local.type_id, type_table) {
                                let unsigned =
                                    type_table.unsigned_integer_bits(local.type_id).is_some();
                                let value = match op {
                                    AssignOp::Set => rhs.value,
                                    AssignOp::Add => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().iadd(lhs, rhs.value)
                                    }
                                    AssignOp::Sub => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().isub(lhs, rhs.value)
                                    }
                                    AssignOp::Mul => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().imul(lhs, rhs.value)
                                    }
                                    AssignOp::Div if unsigned => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().udiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod if unsigned => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().urem(lhs, rhs.value)
                                    }
                                    AssignOp::Div => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().sdiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().srem(lhs, rhs.value)
                                    }
                                };
                                normalize_unsigned_value(builder, value, local.type_id, type_table)
                            } else if local.type_id == TYPE_ID_F32 {
                                match op {
                                    AssignOp::Set => rhs.value,
                                    AssignOp::Add => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fadd(lhs, rhs.value)
                                    }
                                    AssignOp::Sub => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fsub(lhs, rhs.value)
                                    }
                                    AssignOp::Mul => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fmul(lhs, rhs.value)
                                    }
                                    AssignOp::Div => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fdiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod => {
                                        return Err(format!(
                                            "'%=' requires i32 target in current jit path for '{}'",
                                            name
                                        ));
                                    }
                                }
                            } else if local.type_id == TYPE_ID_F64 {
                                match op {
                                    AssignOp::Set => rhs.value,
                                    AssignOp::Add => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fadd(lhs, rhs.value)
                                    }
                                    AssignOp::Sub => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fsub(lhs, rhs.value)
                                    }
                                    AssignOp::Mul => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fmul(lhs, rhs.value)
                                    }
                                    AssignOp::Div => {
                                        let lhs = builder.use_var(local.var);
                                        builder.ins().fdiv(lhs, rhs.value)
                                    }
                                    AssignOp::Mod => {
                                        return Err(format!(
                                            "'%=' requires i32 target in current jit path for '{}'",
                                            name
                                        ));
                                    }
                                }
                            } else if local.type_id == TYPE_ID_BOOL {
                                if *op != AssignOp::Set {
                                    return Err(format!(
                                        "bool assignment only supports '=' in current jit path for '{}'",
                                        name
                                    ));
                                }
                                rhs.value
                            } else {
                                return Err(format!(
                                    "unsupported local assignment type {} for '{}'",
                                    local.type_id, name
                                ));
                            };
                            builder.def_var(local.var, value);
                        } else {
                            if let Some((binding, suffix)) =
                                resolve_foreach_binding_for_path(name, foreach_bindings)
                            {
                                emit_foreach_binding_assignment(
                                    builder,
                                    runtime_call_refs,
                                    type_table,
                                    binding,
                                    &suffix,
                                    *op,
                                    rhs,
                                )?;
                                continue;
                            }
                            let Some(path_type) = global_path_types.get(name).copied() else {
                                return Err(format!(
                                    "unknown assignment target '{}' in current jit path",
                                    name
                                ));
                            };
                            emit_global_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                name,
                                path_type,
                                *op,
                                rhs,
                            )?;
                        }
                    }
                    AssignTarget::GlobalPath(path) => {
                        if let Some((binding, suffix)) =
                            resolve_foreach_binding_for_path(path, foreach_bindings)
                        {
                            emit_foreach_binding_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                binding,
                                &suffix,
                                *op,
                                rhs,
                            )?;
                            continue;
                        }
                        if let Some((base, suffix)) = path.split_once('.') {
                            if let Some(local) = values_by_name.get(base).copied() {
                                if let Some(kind) = collection_meta_kind_from_suffix(suffix) {
                                    if suffix == "max_length" {
                                        return Err(format!(
                                            "assignment target '{}.{}' is read-only in current jit path",
                                            base, suffix
                                        ));
                                    }
                                    if suffix == "byte_length" {
                                        return Err(format!(
                                            "assignment target '{}.{}' is read-only (assign to '{}.length') in current jit path",
                                            base, suffix, base
                                        ));
                                    }
                                    if !is_collection_handle_type(local.type_id, type_table) {
                                        return Err(format!(
                                            "assignment target '{}.{}' requires collection handle base in current jit path",
                                            base, suffix
                                        ));
                                    }
                                    if rhs.type_id != TYPE_ID_I32 {
                                        return Err(format!(
                                            "assignment type mismatch for '{}.{}': expected i32 expression but found {}",
                                            base, suffix, rhs.type_id
                                        ));
                                    }

                                    let base_value = builder.use_var(local.var);
                                    let kind_value =
                                        builder.ins().iconst(types::I32, i64::from(kind as i32));

                                    let value = match op {
                                        AssignOp::Set => rhs.value,
                                        AssignOp::Add
                                        | AssignOp::Sub
                                        | AssignOp::Mul
                                        | AssignOp::Div
                                        | AssignOp::Mod => {
                                            let current = builder.ins().call(
                                                runtime_call_refs.collection_i32_load,
                                                &[base_value, kind_value],
                                            );
                                            let current_value = builder.inst_results(current)[0];
                                            match op {
                                                AssignOp::Add => {
                                                    builder.ins().iadd(current_value, rhs.value)
                                                }
                                                AssignOp::Sub => {
                                                    builder.ins().isub(current_value, rhs.value)
                                                }
                                                AssignOp::Mul => {
                                                    builder.ins().imul(current_value, rhs.value)
                                                }
                                                AssignOp::Div => {
                                                    builder.ins().sdiv(current_value, rhs.value)
                                                }
                                                AssignOp::Mod => {
                                                    builder.ins().srem(current_value, rhs.value)
                                                }
                                                AssignOp::Set => unreachable!(),
                                            }
                                        }
                                    };

                                    builder.ins().call(
                                        runtime_call_refs.collection_i32_store,
                                        &[base_value, kind_value, value],
                                    );
                                    continue;
                                }
                                if let Some(field_types) =
                                    named_struct_field_types.get(&local.type_id)
                                {
                                    let Some(field_type) = field_types.get(suffix).copied() else {
                                        return Err(format!(
                                            "unknown local struct field path '{}.{}' in current jit path",
                                            base, suffix
                                        ));
                                    };
                                    if is_collection_handle_type(field_type, type_table) {
                                        return Err(format!(
                                            "local struct field assignment to collection handle '{}.{}' is unsupported in current jit path",
                                            base, suffix
                                        ));
                                    }
                                    if !are_assignment_types_compatible(
                                        field_type,
                                        rhs.type_id,
                                        type_table,
                                    ) {
                                        return Err(format!(
                                            "assignment type mismatch for local struct field '{}.{}': target type {}, expression type {}",
                                            base, suffix, field_type, rhs.type_id
                                        ));
                                    }

                                    let base_hash = builder.use_var(local.var);
                                    if let Some(struct_view) = local.struct_view {
                                        emit_struct_view_field_assignment(
                                            builder,
                                            runtime_call_refs,
                                            type_table,
                                            struct_view,
                                            base_hash,
                                            suffix,
                                            field_type,
                                            *op,
                                            rhs,
                                        )?;
                                        continue;
                                    }

                                    let path_hash = emit_local_struct_field_path_hash(
                                        base_hash, suffix, builder,
                                    );
                                    if is_i32_scalar_lane_type(field_type, type_table) {
                                        let lhs = if *op == AssignOp::Set {
                                            None
                                        } else {
                                            let call = builder.ins().call(
                                                runtime_call_refs.global_i32_load,
                                                &[path_hash],
                                            );
                                            Some(builder.inst_results(call)[0])
                                        };
                                        let value = emit_integer_assignment_value(
                                            builder, lhs, rhs.value, *op, type_table, field_type,
                                        );
                                        builder.ins().call(
                                            runtime_call_refs.global_i32_store,
                                            &[path_hash, value],
                                        );
                                        continue;
                                    }
                                    if field_type == TYPE_ID_BOOL {
                                        if *op != AssignOp::Set {
                                            return Err(format!(
                                                "bool local struct field assignment only supports '=' for '{}.{}'",
                                                base, suffix
                                            ));
                                        }
                                        builder.ins().call(
                                            runtime_call_refs.global_i32_store,
                                            &[path_hash, rhs.value],
                                        );
                                        continue;
                                    }
                                    if field_type == TYPE_ID_F32 {
                                        let value = match op {
                                            AssignOp::Set => rhs.value,
                                            AssignOp::Add => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fadd(lhs, rhs.value)
                                            }
                                            AssignOp::Sub => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fsub(lhs, rhs.value)
                                            }
                                            AssignOp::Mul => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fmul(lhs, rhs.value)
                                            }
                                            AssignOp::Div => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f32_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fdiv(lhs, rhs.value)
                                            }
                                            AssignOp::Mod => {
                                                return Err(format!(
                                                    "'%=' is unsupported for f32 local struct field '{}.{}'",
                                                    base, suffix
                                                ));
                                            }
                                        };
                                        builder.ins().call(
                                            runtime_call_refs.global_f32_store,
                                            &[path_hash, value],
                                        );
                                        continue;
                                    }
                                    if field_type == TYPE_ID_F64 {
                                        let value = match op {
                                            AssignOp::Set => rhs.value,
                                            AssignOp::Add => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fadd(lhs, rhs.value)
                                            }
                                            AssignOp::Sub => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fsub(lhs, rhs.value)
                                            }
                                            AssignOp::Mul => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fmul(lhs, rhs.value)
                                            }
                                            AssignOp::Div => {
                                                let call = builder.ins().call(
                                                    runtime_call_refs.global_f64_load,
                                                    &[path_hash],
                                                );
                                                let lhs = builder.inst_results(call)[0];
                                                builder.ins().fdiv(lhs, rhs.value)
                                            }
                                            AssignOp::Mod => {
                                                return Err(format!(
                                                    "'%=' is unsupported for f64 local struct field '{}.{}'",
                                                    base,
                                                    suffix
                                                ));
                                            }
                                        };
                                        builder.ins().call(
                                            runtime_call_refs.global_f64_store,
                                            &[path_hash, value],
                                        );
                                        continue;
                                    }
                                    return Err(format!(
                                        "unsupported local struct field type {} for '{}.{}'",
                                        field_type, base, suffix
                                    ));
                                }
                            }
                        }
                        let Some(path_type) = global_path_types.get(path).copied() else {
                            return Err(format!(
                                "unknown global path '{}' in current jit path",
                                path
                            ));
                        };
                        emit_global_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            path,
                            path_type,
                            *op,
                            rhs,
                        )?;
                    }
                    AssignTarget::IndexedPath {
                        collection_path,
                        index,
                        suffix,
                    } => {
                        if let Some(local_collection) = values_by_name.get(collection_path).copied()
                        {
                            let index_binding = emit_simple_expression(
                                builder,
                                index,
                                Some(TYPE_ID_I32),
                                values_by_name,
                                runtime_call_refs,
                                internal_calls,
                                call_signatures,
                                type_table,
                                global_path_types,
                                constant_values,
                                collection_infos,
                                named_struct_field_types,
                                foreach_bindings,
                            )?;
                            emit_local_indexed_collection_assignment(
                                builder,
                                runtime_call_refs,
                                type_table,
                                named_struct_field_types,
                                collection_path,
                                local_collection,
                                suffix,
                                index_binding,
                                *op,
                                rhs,
                            )?;
                            continue;
                        }
                        if let Some((base, field)) = collection_path.split_once('.') {
                            if let Some(local) = values_by_name.get(base).copied() {
                                if let Some(collection_type) = named_struct_field_types
                                    .get(&local.type_id)
                                    .and_then(|fields| fields.get(field))
                                    .copied()
                                {
                                    if let Some(element_type) =
                                        type_table.indexed_element_type_id(collection_type)
                                    {
                                        if suffix.is_empty()
                                            && element_type == TYPE_ID_F32
                                            && *op == AssignOp::Mod
                                        {
                                            return Err(format!(
                                                "'%=' is unsupported for f32 indexed receiver assignment '{}[...]'",
                                                collection_path
                                            ));
                                        }
                                        let index_binding = emit_simple_expression(
                                            builder,
                                            index,
                                            Some(TYPE_ID_I32),
                                            values_by_name,
                                            runtime_call_refs,
                                            internal_calls,
                                            call_signatures,
                                            type_table,
                                            global_path_types,
                                            constant_values,
                                            collection_infos,
                                            named_struct_field_types,
                                            foreach_bindings,
                                        )?;
                                        let index_binding =
                                            normalize_index_binding(index_binding, type_table)?;
                                        emit_fixed_collection_bounds_trap(
                                            builder,
                                            index_binding.value,
                                            collection_type,
                                            collection_path,
                                            type_table,
                                        )?;
                                        let collection_hash = emit_local_struct_field_path_hash(
                                            builder.use_var(local.var),
                                            field,
                                            builder,
                                        );
                                        emit_local_indexed_collection_assignment_for_handle(
                                            builder,
                                            runtime_call_refs,
                                            type_table,
                                            named_struct_field_types,
                                            collection_path,
                                            collection_type,
                                            collection_hash,
                                            suffix,
                                            index_binding,
                                            *op,
                                            rhs,
                                            true,
                                        )?;
                                        continue;
                                    }
                                }
                            }
                        }
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown indexed assignment collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        let index_binding = emit_simple_expression(
                            builder,
                            index,
                            Some(TYPE_ID_I32),
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
                            foreach_bindings,
                        )?;
                        let bounds_proven = static_index_bounds_proven(
                            index,
                            collection_info.len as usize,
                            values_by_name,
                        );
                        emit_indexed_collection_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            collection_path,
                            collection_info,
                            suffix,
                            index_binding,
                            bounds_proven,
                            *op,
                            rhs,
                        )?;
                    }
                }
            }
            SimpleStmt::Convert {
                target,
                kind,
                source,
            } => {
                let expected_source_type = match kind {
                    ConversionKind::FromI32 => Some(TYPE_ID_I32),
                    ConversionKind::FromF32 => Some(TYPE_ID_F32),
                    ConversionKind::FromF64 => Some(TYPE_ID_F64),
                };
                let source_binding = emit_simple_expression(
                    builder,
                    source,
                    expected_source_type,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                match target {
                    AssignTarget::Local(name) => {
                        let Some(local) = values_by_name.get(name).copied() else {
                            return Err(format!(
                                "conversion target '{}' is not a local binding",
                                name
                            ));
                        };
                        let converted = emit_conversion_assignment_value(
                            builder,
                            *kind,
                            source_binding,
                            local.type_id,
                            name,
                        )?;
                        builder.def_var(local.var, converted.value);
                    }
                    AssignTarget::GlobalPath(path) => {
                        let Some(path_type) = global_path_types.get(path).copied() else {
                            return Err(format!(
                                "unknown global path '{}' in current jit path",
                                path
                            ));
                        };
                        let converted = emit_conversion_assignment_value(
                            builder,
                            *kind,
                            source_binding,
                            path_type,
                            path,
                        )?;
                        emit_global_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            path,
                            path_type,
                            AssignOp::Set,
                            converted,
                        )?;
                    }
                    AssignTarget::IndexedPath {
                        collection_path,
                        index,
                        suffix,
                    } => {
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown indexed conversion collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        let target_type = resolve_collection_value_type(collection_info, suffix)?;
                        let target_name = format!("{collection_path}[...].{suffix}");
                        let converted = emit_conversion_assignment_value(
                            builder,
                            *kind,
                            source_binding,
                            target_type,
                            &target_name,
                        )?;
                        let index_binding = emit_simple_expression(
                            builder,
                            index,
                            Some(TYPE_ID_I32),
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
                            foreach_bindings,
                        )?;
                        let bounds_proven = static_index_bounds_proven(
                            index,
                            collection_info.len as usize,
                            values_by_name,
                        );
                        emit_indexed_collection_assignment(
                            builder,
                            runtime_call_refs,
                            type_table,
                            collection_path,
                            collection_info,
                            suffix,
                            index_binding,
                            bounds_proven,
                            AssignOp::Set,
                            converted,
                        )?;
                    }
                }
            }
            SimpleStmt::Expr(expression) => {
                if let SimpleExpr::Call { target, args } = expression {
                    let handled = emit_host_print_call_statement(
                        builder,
                        runtime_call_refs,
                        internal_calls,
                        type_table,
                        target,
                        args,
                        values_by_name,
                        call_signatures,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?;
                    if handled {
                        continue;
                    }
                    if try_emit_typed_pool_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_queue_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_priority_queue_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_ring_buffer_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_stable_pool_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_map_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_set_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_grid_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    if try_emit_typed_bitset_call(
                        builder,
                        target,
                        args,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?
                    .is_some()
                    {
                        continue;
                    }
                    let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
                    let mut arg_types: Vec<TypeId> = Vec::with_capacity(args.len());
                    let expected_params =
                        unambiguous_call_params(target, args.len(), call_signatures);
                    for (arg_index, arg) in args.iter().enumerate() {
                        if let Some(struct_view) = try_emit_struct_view_value(
                            builder,
                            arg,
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
                            foreach_bindings,
                        )? {
                            arg_values.push(struct_view.base);
                            arg_values.push(struct_view.index);
                            arg_values.push(struct_view.len);
                            arg_types.push(struct_view.type_id);
                            continue;
                        }
                        let binding = emit_simple_expression(
                            builder,
                            arg,
                            expected_params
                                .as_ref()
                                .and_then(|params| params.get(arg_index).copied()),
                            values_by_name,
                            runtime_call_refs,
                            internal_calls,
                            call_signatures,
                            type_table,
                            global_path_types,
                            constant_values,
                            collection_infos,
                            named_struct_field_types,
                            foreach_bindings,
                        )?;
                        arg_values.push(binding.value);
                        arg_types.push(binding.type_id);
                    }
                    let signature = resolve_call_signature(
                        target,
                        &arg_types,
                        call_signatures,
                        type_table,
                        named_struct_field_types,
                    )?;
                    if signature.return_type == TYPE_ID_VOID {
                        if signature.extern_symbol.is_some() {
                            let _ = emit_extern_call_for_signature(
                                builder,
                                runtime_call_refs,
                                signature,
                                &arg_values,
                            )?;
                        } else {
                            let _ = emit_internal_call_for_signature(
                                builder,
                                runtime_call_refs,
                                internal_calls,
                                signature,
                                &arg_values,
                                &arg_types,
                                type_table,
                                named_struct_field_types,
                                target,
                            )?;
                        }
                        continue;
                    }
                }
                let _ = emit_simple_expression(
                    builder,
                    expression,
                    None,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
            }
            SimpleStmt::Continue => {
                let Some(loop_control) = loop_control else {
                    return Err("continue statement is only valid inside loops".to_string());
                };
                builder.ins().jump(loop_control.continue_block, &[]);
                return Ok(true);
            }
            SimpleStmt::Return(expression) => {
                let binding = emit_simple_expression(
                    builder,
                    expression,
                    Some(expected_return_type),
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                if !are_assignment_types_compatible(
                    expected_return_type,
                    binding.type_id,
                    type_table,
                ) {
                    let expected = type_table.type_info(expected_return_type).map_or_else(
                        || expected_return_type.to_string(),
                        |info| info.name.clone(),
                    );
                    let found = type_table
                        .type_info(binding.type_id)
                        .map_or_else(|| binding.type_id.to_string(), |info| info.name.clone());
                    return Err(format!(
                        "return expression expected {expected} but found {found}"
                    ));
                }
                let value = normalize_unsigned_value(
                    builder,
                    binding.value,
                    expected_return_type,
                    type_table,
                );
                if let Some(debug) = debug_refs {
                    emit_debug_frame_boundary(builder, debug.frame_leave, function_id);
                }
                if let Some(profile) = runtime_call_refs.profile.as_ref() {
                    emit_function_frame_boundary(builder, profile.frame_leave, function_id);
                }
                builder.ins().return_(&[value]);
                return Ok(true);
            }
            SimpleStmt::ReturnVoid => {
                if expected_return_type == TYPE_ID_VOID {
                    if let Some(debug) = debug_refs {
                        emit_debug_frame_boundary(builder, debug.frame_leave, function_id);
                    }
                    if let Some(profile) = runtime_call_refs.profile.as_ref() {
                        emit_function_frame_boundary(builder, profile.frame_leave, function_id);
                    }
                    builder.ins().return_(&[]);
                    return Ok(true);
                }
                return Err("void return statement is not allowed in non-void function".to_string());
            }
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                // Only a direct positive receiver preflight can establish a
                // typed-collection proof.  The proof is an emitter-local
                // Cranelift value bundle; it is consumed by one matching
                // action in the then arm and never reaches the runtime ABI.
                let incoming_guard_proofs = internal_calls.typed_collection_guard_proofs();
                if let SimpleCondition::Expr(SimpleExpr::Call { target, args }) = condition {
                    if target.starts_with("can_") {
                        internal_calls.begin_typed_collection_guard_capture(target, args);
                    }
                }
                let condition_value = emit_simple_condition(
                    builder,
                    condition,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                let then_block = builder.create_block();
                let else_block = builder.create_block();
                let continue_block = builder.create_block();
                builder
                    .ins()
                    .brif(condition_value, then_block, &[], else_block, &[]);
                builder.seal_block(then_block);
                builder.switch_to_block(then_block);

                let expected_children = then_statements.len()
                    + else_statements
                        .as_ref()
                        .map_or(0, |statements| statements.len());
                let nested_debug = debug_statements.map(|debug| debug[index].children.as_slice());
                if nested_debug.is_some_and(|debug| debug.len() != expected_children) {
                    return Err("if debug metadata does not match branch statements".to_string());
                }
                let then_debug = nested_debug.map(|debug| &debug[..then_statements.len()]);
                let else_debug = nested_debug.map(|debug| &debug[then_statements.len()..]);

                let mut then_values = values_by_name.clone();
                let then_terminated = emit_simple_statements(
                    builder,
                    then_statements,
                    then_debug,
                    debug_refs,
                    function_id,
                    &mut then_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    loop_control,
                    expected_return_type,
                    next_variable,
                )?;
                if !then_terminated {
                    builder.ins().jump(continue_block, &[]);
                }

                // A proof is scoped to the guarded then arm.  In particular,
                // the else arm must never inherit values computed by the
                // preflight.
                internal_calls.restore_typed_collection_guard_proofs(incoming_guard_proofs);
                builder.seal_block(else_block);
                builder.switch_to_block(else_block);
                let else_terminated = if let Some(else_statements) = else_statements {
                    let mut else_values = values_by_name.clone();
                    emit_simple_statements(
                        builder,
                        else_statements,
                        else_debug,
                        debug_refs,
                        function_id,
                        &mut else_values,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                        loop_control,
                        expected_return_type,
                        next_variable,
                    )?
                } else {
                    false
                };
                if !else_terminated {
                    builder.ins().jump(continue_block, &[]);
                }
                if then_terminated && else_terminated {
                    internal_calls.clear_typed_collection_guard();
                    return Ok(true);
                }

                builder.seal_block(continue_block);
                builder.switch_to_block(continue_block);
                internal_calls.clear_typed_collection_guard();
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                // A pre-loop proof cannot authorize a repeated action. Each
                // iteration must establish its own direct can_* guard.
                internal_calls.clear_typed_collection_guard();
                let mut loop_values = values_by_name.clone();
                emit_for_control_statement(
                    builder,
                    init.as_ref(),
                    function_id,
                    &mut loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                if let Some((index_name, upper)) = canonical_fixed_array_loop_bound(
                    init,
                    condition,
                    step,
                    body_statements,
                    collection_infos,
                ) {
                    if let Some(binding) = loop_values.get_mut(&index_name) {
                        binding.proven_index_upper = Some(upper);
                    }
                }

                let condition_block = builder.create_block();
                let body_block = builder.create_block();
                let step_block = builder.create_block();
                let exit_block = builder.create_block();
                let loop_control = LoopControlContext {
                    continue_block: step_block,
                };

                builder.ins().jump(condition_block, &[]);
                builder.switch_to_block(condition_block);

                let condition_value = emit_simple_condition(
                    builder,
                    condition,
                    &loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                builder
                    .ins()
                    .brif(condition_value, body_block, &[], exit_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    debug_statements.map(|debug| debug[index].children.as_slice()),
                    debug_refs,
                    function_id,
                    &mut loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    Some(&loop_control),
                    expected_return_type,
                    next_variable,
                )?;
                if !body_terminated {
                    builder.ins().jump(step_block, &[]);
                }

                builder.seal_block(step_block);
                builder.switch_to_block(step_block);
                emit_for_control_statement(
                    builder,
                    step.as_ref(),
                    function_id,
                    &mut loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                    expected_return_type,
                    next_variable,
                )?;
                builder.ins().jump(condition_block, &[]);
                builder.seal_block(condition_block);

                builder.seal_block(exit_block);
                builder.switch_to_block(exit_block);
            }
            SimpleStmt::Foreach {
                item_name,
                index_name,
                collection_path,
                body_statements,
            } => {
                // Foreach bodies repeat, so caller-owned proofs must be
                // established inside the body for each action.
                internal_calls.clear_typed_collection_guard();
                ensure_no_variable_shadowing(
                    item_name,
                    values_by_name,
                    foreach_bindings,
                    "foreach item binding",
                )?;
                if let Some(index_name) = index_name {
                    if index_name == item_name {
                        return Err(format!(
                            "foreach index binding '{}' shadows existing variable",
                            index_name
                        ));
                    }
                    ensure_no_variable_shadowing(
                        index_name,
                        values_by_name,
                        foreach_bindings,
                        "foreach index binding",
                    )?;
                }
                let (collection_info, collection_handle, collection_struct_type_id) =
                    if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                        let info = build_local_foreach_collection_info(
                            collection_path,
                            local_collection.type_id,
                            type_table,
                            named_struct_field_types,
                        )?;
                        let element_type =
                            type_table.indexed_element_type_id(local_collection.type_id);
                        let struct_type_id = element_type
                            .filter(|type_id| named_struct_field_types.contains_key(type_id));
                        (
                            info,
                            ForeachCollectionHandle::LocalVar(local_collection.var),
                            struct_type_id,
                        )
                    } else {
                        let Some(collection_info) = collection_infos.get(collection_path) else {
                            return Err(format!(
                                "unknown foreach collection '{}' in current jit path",
                                collection_path
                            ));
                        };
                        let collection_type = global_path_types
                            .get(collection_path)
                            .copied()
                            .ok_or_else(|| {
                                format!(
                                    "unknown foreach collection '{}' in current jit path",
                                    collection_path
                                )
                            })?;
                        let element_type = type_table.indexed_element_type_id(collection_type);
                        let struct_type_id = element_type
                            .filter(|type_id| named_struct_field_types.contains_key(type_id));
                        (
                            collection_info.clone(),
                            ForeachCollectionHandle::PathHash(hash_global_path(collection_path)),
                            struct_type_id,
                        )
                    };
                let initial_index_value = builder.ins().iconst(types::I32, 0);
                let index_var = declare_new_variable(
                    builder,
                    next_variable,
                    initial_index_value,
                    TYPE_ID_I32,
                    type_table,
                )?;

                // Cache collection field pointers once per foreach loop, so hot inner loops can
                // use direct loads/stores instead of calling runtime helpers on every iteration.
                let collection_hash_value =
                    emit_foreach_collection_handle_value(builder, collection_handle);
                let len_value = builder
                    .ins()
                    .iconst(types::I32, i64::from(collection_info.len));
                let mut loop_len_value = len_value;
                let mut i32_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut u8_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut u16_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut f32_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                let mut f64_array_base_ptrs: BTreeMap<String, Value> = BTreeMap::new();
                if collection_info.element_type.is_some_and(|type_id| {
                    is_i32_abi_compatible_type(type_id, type_table)
                        && !is_u8_lane(type_table, type_id)
                        && type_id != TYPE_ID_U16
                }) {
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), String::new()))
                        })
                        .copied();
                    let base = if let Some(direct) = direct {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        emit_direct_slot_data_ptr(builder, direct.slot)
                    } else {
                        let field_hash_value = builder.ins().iconst(types::I32, 0);
                        let call = builder.ins().call(
                            runtime_call_refs.global_i32_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        builder.inst_results(call)[0]
                    };
                    i32_array_base_ptrs.insert(String::new(), base);
                }
                if collection_info
                    .element_type
                    .is_some_and(|type_id| is_u8_lane(type_table, type_id))
                {
                    if let Some(direct) =
                        matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                            .then(|| runtime_call_refs.direct_storage.as_ref())
                            .flatten()
                            .and_then(|bindings| {
                                bindings
                                    .arrays
                                    .get(&(collection_path.clone(), String::new()))
                            })
                            .copied()
                    {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        u8_array_base_ptrs.insert(
                            String::new(),
                            emit_direct_slot_data_ptr(builder, direct.slot),
                        );
                    }
                }
                if collection_info.element_type == Some(TYPE_ID_U16) {
                    if let Some(direct) =
                        matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                            .then(|| runtime_call_refs.direct_storage.as_ref())
                            .flatten()
                            .and_then(|bindings| {
                                bindings
                                    .arrays
                                    .get(&(collection_path.clone(), String::new()))
                            })
                            .copied()
                    {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        u16_array_base_ptrs.insert(
                            String::new(),
                            emit_direct_slot_data_ptr(builder, direct.slot),
                        );
                    }
                }
                if collection_info.element_type == Some(TYPE_ID_F32) {
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), String::new()))
                        })
                        .copied();
                    let base = if let Some(direct) = direct {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        emit_direct_slot_data_ptr(builder, direct.slot)
                    } else {
                        let field_hash_value = builder.ins().iconst(types::I32, 0);
                        let call = builder.ins().call(
                            runtime_call_refs.global_f32_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        builder.inst_results(call)[0]
                    };
                    f32_array_base_ptrs.insert(String::new(), base);
                }
                if collection_info.element_type == Some(TYPE_ID_F64) {
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), String::new()))
                        })
                        .copied();
                    let base = if let Some(direct) = direct {
                        loop_len_value =
                            emit_bounded_direct_array_len(builder, direct, loop_len_value);
                        emit_direct_slot_data_ptr(builder, direct.slot)
                    } else {
                        let field_hash_value = builder.ins().iconst(types::I32, 0);
                        let call = builder.ins().call(
                            runtime_call_refs.global_f64_array_ptr,
                            &[collection_hash_value, field_hash_value, len_value],
                        );
                        builder.inst_results(call)[0]
                    };
                    f64_array_base_ptrs.insert(String::new(), base);
                }
                for (suffix, type_id) in &collection_info.field_types {
                    let field_hash = hash_foreach_field_suffix(suffix);
                    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
                    let direct = matches!(collection_handle, ForeachCollectionHandle::PathHash(_))
                        .then(|| runtime_call_refs.direct_storage.as_ref())
                        .flatten()
                        .and_then(|bindings| {
                            bindings
                                .arrays
                                .get(&(collection_path.clone(), suffix.clone()))
                        })
                        .copied();
                    if is_i32_abi_compatible_type(*type_id, type_table)
                        && !is_u8_lane(type_table, *type_id)
                        && *type_id != TYPE_ID_U16
                    {
                        let base = if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            emit_direct_slot_data_ptr(builder, direct.slot)
                        } else {
                            let call = builder.ins().call(
                                runtime_call_refs.global_i32_array_ptr,
                                &[collection_hash_value, field_hash_value, len_value],
                            );
                            builder.inst_results(call)[0]
                        };
                        i32_array_base_ptrs.insert(suffix.clone(), base);
                    } else if is_u8_lane(type_table, *type_id) {
                        if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            u8_array_base_ptrs.insert(
                                suffix.clone(),
                                emit_direct_slot_data_ptr(builder, direct.slot),
                            );
                        }
                    } else if *type_id == TYPE_ID_U16 {
                        if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            u16_array_base_ptrs.insert(
                                suffix.clone(),
                                emit_direct_slot_data_ptr(builder, direct.slot),
                            );
                        }
                    } else if *type_id == TYPE_ID_F32 {
                        let base = if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            emit_direct_slot_data_ptr(builder, direct.slot)
                        } else {
                            let call = builder.ins().call(
                                runtime_call_refs.global_f32_array_ptr,
                                &[collection_hash_value, field_hash_value, len_value],
                            );
                            builder.inst_results(call)[0]
                        };
                        f32_array_base_ptrs.insert(suffix.clone(), base);
                    } else if *type_id == TYPE_ID_F64 {
                        let base = if let Some(direct) = direct {
                            loop_len_value =
                                emit_bounded_direct_array_len(builder, direct, loop_len_value);
                            emit_direct_slot_data_ptr(builder, direct.slot)
                        } else {
                            let call = builder.ins().call(
                                runtime_call_refs.global_f64_array_ptr,
                                &[collection_hash_value, field_hash_value, len_value],
                            );
                            builder.inst_results(call)[0]
                        };
                        f64_array_base_ptrs.insert(suffix.clone(), base);
                    }
                }

                let mut loop_values = values_by_name.clone();
                if let Some(index_name) = index_name {
                    loop_values.insert(
                        index_name.clone(),
                        LocalBinding {
                            var: index_var,
                            type_id: TYPE_ID_I32,
                            struct_view: None,
                            proven_index_upper: Some(collection_info.len as usize),
                        },
                    );
                }
                let mut loop_foreach_bindings = foreach_bindings.clone();
                loop_foreach_bindings.insert(
                    item_name.clone(),
                    ForeachBinding {
                        collection_handle,
                        index_var,
                        len: collection_info.len,
                        element_type: collection_info.element_type,
                        struct_type_id: collection_struct_type_id,
                        field_types: collection_info.field_types.clone(),
                        u8_array_base_ptrs,
                        u16_array_base_ptrs,
                        i32_array_base_ptrs,
                        f32_array_base_ptrs,
                        f64_array_base_ptrs,
                    },
                );

                let condition_block = builder.create_block();
                let body_block = builder.create_block();
                let step_block = builder.create_block();
                let exit_block = builder.create_block();
                let loop_control = LoopControlContext {
                    continue_block: step_block,
                };

                builder.ins().jump(condition_block, &[]);
                builder.switch_to_block(condition_block);

                let index_value = builder.use_var(index_var);
                let condition_value =
                    builder
                        .ins()
                        .icmp(IntCC::SignedLessThan, index_value, loop_len_value);
                builder
                    .ins()
                    .brif(condition_value, body_block, &[], exit_block, &[]);

                builder.seal_block(body_block);
                builder.switch_to_block(body_block);
                let body_terminated = emit_simple_statements(
                    builder,
                    body_statements,
                    debug_statements.map(|debug| debug[index].children.as_slice()),
                    debug_refs,
                    function_id,
                    &mut loop_values,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    &loop_foreach_bindings,
                    Some(&loop_control),
                    expected_return_type,
                    next_variable,
                )?;
                if !body_terminated {
                    builder.ins().jump(step_block, &[]);
                }

                builder.seal_block(step_block);
                builder.switch_to_block(step_block);
                let current_index = builder.use_var(index_var);
                let next_index = builder.ins().iadd_imm(current_index, 1);
                builder.def_var(index_var, next_index);
                builder.ins().jump(condition_block, &[]);
                builder.seal_block(condition_block);

                builder.seal_block(exit_block);
                builder.switch_to_block(exit_block);
            }
        }
    }
    Ok(false)
}

pub(crate) fn emit_conversion_assignment_value(
    builder: &mut FunctionBuilder<'_>,
    kind: ConversionKind,
    source: ValueBinding,
    target_type: TypeId,
    target_name: &str,
) -> Result<ValueBinding, String> {
    match kind {
        ConversionKind::FromI32 => {
            if source.type_id != TYPE_ID_I32 {
                return Err("from_i32 source expression must be i32".to_string());
            }
            if target_type == TYPE_ID_F32 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_from_sint(types::F32, source.value),
                    type_id: TYPE_ID_F32,
                });
            }
            if target_type == TYPE_ID_F64 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_from_sint(types::F64, source.value),
                    type_id: TYPE_ID_F64,
                });
            }
            Err(format!(
                "from_i32 target '{}' must be f32 or f64",
                target_name
            ))
        }
        ConversionKind::FromF32 => {
            if source.type_id != TYPE_ID_F32 {
                return Err("from_f32 source expression must be f32".to_string());
            }
            if target_type == TYPE_ID_I32 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_to_sint(types::I32, source.value),
                    type_id: TYPE_ID_I32,
                });
            }
            if target_type == TYPE_ID_F64 {
                return Ok(ValueBinding {
                    value: builder.ins().fpromote(types::F64, source.value),
                    type_id: TYPE_ID_F64,
                });
            }
            Err(format!(
                "from_f32 target '{}' must be i32 or f64",
                target_name
            ))
        }
        ConversionKind::FromF64 => {
            if source.type_id != TYPE_ID_F64 {
                return Err("from_f64 source expression must be f64".to_string());
            }
            if target_type == TYPE_ID_I32 {
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_to_sint(types::I32, source.value),
                    type_id: TYPE_ID_I32,
                });
            }
            if target_type == TYPE_ID_F32 {
                return Ok(ValueBinding {
                    value: builder.ins().fdemote(types::F32, source.value),
                    type_id: TYPE_ID_F32,
                });
            }
            Err(format!(
                "from_f64 target '{}' must be i32 or f32",
                target_name
            ))
        }
    }
}

pub(crate) fn emit_for_control_statement(
    builder: &mut FunctionBuilder<'_>,
    statement: &SimpleStmt,
    function_id: FunctionId,
    values_by_name: &mut BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
    expected_return_type: TypeId,
    next_variable: &mut u32,
) -> Result<(), String> {
    match statement {
        SimpleStmt::Noop => Ok(()),
        SimpleStmt::Let { .. }
        | SimpleStmt::Assign { .. }
        | SimpleStmt::Convert { .. }
        | SimpleStmt::Expr(_) => {
            let terminated = emit_simple_statements(
                builder,
                std::slice::from_ref(statement),
                None,
                None,
                function_id,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
                None,
                expected_return_type,
                next_variable,
            )?;
            if terminated {
                return Err("for-loop control statement cannot terminate function".to_string());
            }
            Ok(())
        }
        other => Err(format!(
            "unsupported for-loop control statement in current jit path: {:?}",
            other
        )),
    }
}

fn collect_call_targets_from_hir(hir: &FunctionHIR) -> BTreeSet<String> {
    fn expression(value: &SimpleExpr, out: &mut BTreeSet<String>) {
        match value {
            SimpleExpr::Condition(condition) => condition_targets(condition, out),
            SimpleExpr::IndexedPath { index, .. } => expression(index, out),
            SimpleExpr::Call { target, args } => {
                out.insert(target.clone());
                for argument in args {
                    expression(argument, out);
                }
            }
            SimpleExpr::Binary { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleExpr::DefaultValue(_)
            | SimpleExpr::Int(_)
            | SimpleExpr::Float(_)
            | SimpleExpr::Bool(_)
            | SimpleExpr::StringLiteral(_)
            | SimpleExpr::Identifier(_) => {}
        }
    }

    fn condition_targets(condition: &SimpleCondition, out: &mut BTreeSet<String>) {
        match condition {
            SimpleCondition::Comparison { lhs, rhs, .. } => {
                expression(lhs, out);
                expression(rhs, out);
            }
            SimpleCondition::Expr(expression_value) => expression(expression_value, out),
            SimpleCondition::And(lhs, rhs) | SimpleCondition::Or(lhs, rhs) => {
                condition_targets(lhs, out);
                condition_targets(rhs, out);
            }
            SimpleCondition::Not(inner) => condition_targets(inner, out),
        }
    }

    fn statement(value: &SimpleStmt, out: &mut BTreeSet<String>) {
        match value {
            SimpleStmt::Let {
                expression: value, ..
            }
            | SimpleStmt::Assign {
                expression: value, ..
            }
            | SimpleStmt::Expr(value)
            | SimpleStmt::Return(value) => expression(value, out),
            SimpleStmt::Convert { source, .. } => expression(source, out),
            SimpleStmt::If {
                condition,
                then_statements,
                else_statements,
            } => {
                condition_targets(condition, out);
                for nested in then_statements {
                    statement(nested, out);
                }
                if let Some(nested_statements) = else_statements {
                    for nested in nested_statements {
                        statement(nested, out);
                    }
                }
            }
            SimpleStmt::For {
                init,
                condition,
                step,
                body_statements,
            } => {
                statement(init, out);
                condition_targets(condition, out);
                statement(step, out);
                for nested in body_statements {
                    statement(nested, out);
                }
            }
            SimpleStmt::Foreach {
                body_statements, ..
            } => {
                for nested in body_statements {
                    statement(nested, out);
                }
            }
            SimpleStmt::Noop | SimpleStmt::Continue | SimpleStmt::ReturnVoid => {}
        }
    }

    let mut targets = BTreeSet::new();
    for statement_value in &hir.statements {
        statement(statement_value, &mut targets);
    }
    targets
}

pub(crate) fn try_emit_struct_view_value(
    builder: &mut FunctionBuilder<'_>,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<StructViewValue>, String> {
    match expression {
        SimpleExpr::Identifier(name) => {
            if let Some(local) = values_by_name.get(name).copied() {
                if is_struct_view_type(local.type_id, named_struct_field_types) {
                    let Some(struct_view) = local.struct_view else {
                        return Err(format!(
                            "struct local '{}' is missing struct view metadata in current jit path",
                            name
                        ));
                    };
                    return Ok(Some(StructViewValue {
                        type_id: local.type_id,
                        base: builder.use_var(local.var),
                        index: builder.use_var(struct_view.index_var),
                        len: builder.use_var(struct_view.len_var),
                        storage_kind: struct_view.storage_kind,
                        known_collection_hash: struct_view.known_collection_hash,
                        bounds_proven: struct_view.bounds_proven,
                    }));
                }
            }
            if let Some(binding) = foreach_bindings.get(name) {
                if let Some(struct_type_id) = binding.struct_type_id {
                    let base =
                        emit_foreach_collection_handle_value(builder, binding.collection_handle);
                    let index = builder.use_var(binding.index_var);
                    let len = builder.ins().iconst(types::I32, i64::from(binding.len));
                    return Ok(Some(StructViewValue {
                        type_id: struct_type_id,
                        base,
                        index,
                        len,
                        storage_kind: StructViewStorageKind::Soa,
                        known_collection_hash: match binding.collection_handle {
                            ForeachCollectionHandle::PathHash(hash) => Some(hash),
                            ForeachCollectionHandle::LocalVar(_) => None,
                        },
                        bounds_proven: true,
                    }));
                }
            }
            if let Some(path_type) = global_path_types.get(name).copied() {
                if is_struct_view_type(path_type, named_struct_field_types) {
                    let base = builder
                        .ins()
                        .iconst(types::I32, i64::from(hash_global_path(name)));
                    let index = builder
                        .ins()
                        .iconst(types::I32, i64::from(STRUCT_VIEW_AOS_INDEX_SENTINEL));
                    let len = builder
                        .ins()
                        .iconst(types::I32, i64::from(STRUCT_VIEW_AOS_LEN_SENTINEL));
                    return Ok(Some(StructViewValue {
                        type_id: path_type,
                        base,
                        index,
                        len,
                        storage_kind: StructViewStorageKind::Aos,
                        known_collection_hash: None,
                        bounds_proven: true,
                    }));
                }
            }
            Ok(None)
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if !suffix.is_empty() {
                return Ok(None);
            }

            let (collection_handle, collection_type_id, known_len) =
                if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                    let len = type_table.fixed_collection_len(local_collection.type_id);
                    (
                        builder.use_var(local_collection.var),
                        local_collection.type_id,
                        len,
                    )
                } else {
                    let Some(collection_type_id) = global_path_types.get(collection_path).copied()
                    else {
                        return Ok(None);
                    };
                    let len = collection_infos
                        .get(collection_path)
                        .map(|info| info.len)
                        .or_else(|| type_table.fixed_collection_len(collection_type_id));
                    (
                        builder
                            .ins()
                            .iconst(types::I32, i64::from(hash_global_path(collection_path))),
                        collection_type_id,
                        len,
                    )
                };

            let Some(element_type_id) = type_table.indexed_element_type_id(collection_type_id)
            else {
                return Ok(None);
            };
            if !is_struct_view_type(element_type_id, named_struct_field_types) {
                return Ok(None);
            }

            let index_binding = emit_simple_expression(
                builder,
                index,
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let index_binding = normalize_index_binding(index_binding, type_table)?;

            let len_value = if let Some(known_len) = known_len {
                builder.ins().iconst(types::I32, i64::from(known_len))
            } else {
                let kind_value = builder
                    .ins()
                    .iconst(types::I32, i64::from(CollectionMetaKind::MaxLength as i32));
                let call = builder.ins().call(
                    runtime_call_refs.collection_i32_load,
                    &[collection_handle, kind_value],
                );
                builder.inst_results(call)[0]
            };

            Ok(Some(StructViewValue {
                type_id: element_type_id,
                base: collection_handle,
                index: index_binding.value,
                len: len_value,
                storage_kind: StructViewStorageKind::Soa,
                known_collection_hash: (!values_by_name.contains_key(collection_path))
                    .then(|| hash_global_path(collection_path)),
                bounds_proven: known_len.is_some_and(|len| {
                    static_index_bounds_proven(index, len as usize, values_by_name)
                }),
            }))
        }
        _ => Ok(None),
    }
}

#[derive(Clone, Copy)]
enum TypedPoolCallResult {
    Value(ValueBinding),
    Void,
}

#[derive(Clone, Copy)]
enum TypedPoolCallKind {
    Push,
    Remove,
    Count,
    Capacity,
    Clear,
    CanPush,
    CanRemove,
}

impl TypedPoolCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "push" => Some(Self::Push),
            "remove" => Some(Self::Remove),
            "count" => Some(Self::Count),
            "capacity" => Some(Self::Capacity),
            "clear" => Some(Self::Clear),
            "can_push" => Some(Self::CanPush),
            "can_remove" => Some(Self::CanRemove),
            _ => None,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::Push => 2,
            Self::Remove | Self::CanRemove => 2,
            Self::Count | Self::Capacity | Self::Clear | Self::CanPush => 1,
        }
    }
}

fn is_typed_collection_receiver(
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
    expected_kind: TypedCollectionKind,
) -> Result<bool, String> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Ok(false);
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Ok(false);
    }
    let Some(type_id) = global_path_types.get(path).copied() else {
        return Ok(false);
    };
    if !type_table.is_typed_collection_type(type_id) {
        return Ok(false);
    }
    let Some(type_name) = type_table.type_info(type_id).map(|info| info.name.as_str()) else {
        return Ok(false);
    };
    Ok(type_table
        .parse_typed_collection_descriptor(type_name)?
        .is_some_and(|descriptor| descriptor.kind == expected_kind))
}

fn typed_pool_storage_bindings(
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<(String, usize, DirectStorageRef, DirectArrayStorageRef), String> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    }
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "{target} first argument '{}' is not a persistent typed collection path",
            path
        )
    })?;
    if !type_table.is_typed_collection_type(type_id) {
        return Err(format!(
            "{target} requires persistent pool path '{}', found non-typed state type {}",
            path, type_id
        ));
    }
    let type_name = type_table
        .type_info(type_id)
        .map(|info| info.name.as_str())
        .ok_or_else(|| format!("{target} path '{}' has unknown type {}", path, type_id))?;
    let descriptor = type_table
        .parse_typed_collection_descriptor(type_name)?
        .ok_or_else(|| {
            format!(
                "{target} path '{}' has no compiler-owned typed collection descriptor",
                path
            )
        })?;
    if descriptor.kind != TypedCollectionKind::Pool {
        return Err(format!(
            "{target} requires persistent pool path '{}', found {}",
            path,
            descriptor.kind_name()
        ));
    }
    if descriptor.element_type != Some(TYPE_ID_I32) {
        return Err(format!(
            "{target} requires pool path '{}' with i32 payload",
            path
        ));
    }
    let capacity = usize::try_from(descriptor.capacity).map_err(|_| {
        format!(
            "{target} pool path '{}' capacity {} does not fit the target index type",
            path, descriptor.capacity
        )
    })?;
    let direct_storage = runtime_call_refs.direct_storage.as_ref().ok_or_else(|| {
        format!(
            "{target} for typed pool '{}' requires direct storage bindings",
            path
        )
    })?;
    let count_path = format!("{path}.count");
    let count = direct_storage
        .scalars
        .get(&count_path)
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed pool '{}' is missing direct scalar binding '{}.count'",
                path, path
            )
        })?;
    let values = direct_storage
        .arrays
        .get(&(path.to_string(), String::from("values")))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed pool '{}' is missing direct array binding '{}.values'",
                path, path
            )
        })?;
    if values.storage_bytes != 4 || values.static_len != Some(capacity) {
        return Err(format!(
            "{target} for typed pool '{}' requires an i32 values binding with static length {}, found {} bytes and length {:?}",
            path, capacity, values.storage_bytes, values.static_len
        ));
    }
    Ok((path.clone(), capacity, count, values))
}

fn emit_direct_i32_store(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    value: Value,
) {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    builder.ins().store(MemFlags::new(), value, data, 0);
}

fn emit_typed_pool_push_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    value: Value,
) -> Result<ValueBinding, String> {
    emit_direct_array_store(
        builder,
        values_ref.slot,
        count,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    Ok(ValueBinding {
        value: count,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_pool_remove_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    index: Value,
    type_table: &TypeTable,
) -> Result<(), String> {
    let last_index = builder.ins().iadd_imm(count, -1);
    let removing_last = builder.ins().icmp(IntCC::Equal, index, last_index);
    let zero_last_block = builder.create_block();
    let move_last_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(removing_last, zero_last_block, &[], move_last_block, &[]);

    builder.seal_block(move_last_block);
    builder.switch_to_block(move_last_block);
    let moved_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        last_index,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        moved_value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    builder.ins().jump(zero_last_block, &[]);

    builder.seal_block(zero_last_block);
    builder.switch_to_block(zero_last_block);
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        last_index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_i32_store(builder, count_ref, last_index);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

fn emit_typed_pool_clear(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
) -> Result<(), String> {
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_i32_store(builder, count_ref, zero);
    if capacity == 0 {
        return Ok(());
    }
    let capacity_i32 = i32::try_from(capacity)
        .map_err(|_| format!("typed pool capacity {capacity} exceeds i32 operation range"))?;
    let condition_block = builder.create_block();
    let body_block = builder.create_block();
    let exit_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    builder.ins().jump(condition_block, &[zero]);

    builder.switch_to_block(condition_block);
    let index = builder.block_params(condition_block)[0];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(capacity_i32));
    builder.ins().brif(more, body_block, &[], exit_block, &[]);

    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next = builder.ins().iadd_imm(index, 1);
    builder.ins().jump(condition_block, &[next]);
    builder.seal_block(condition_block);

    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

#[derive(Clone, Copy)]
enum TypedCircularCallResult {
    Value(ValueBinding),
    Void,
}

#[derive(Clone, Copy)]
enum TypedQueueCallKind {
    Push,
    Pop,
    Peek,
    PhysicalIndex,
    CanPush,
    CanPop,
    CanPeek,
    Count,
    Capacity,
    Clear,
    OverwriteOldest,
}

impl TypedQueueCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "push" => Some(Self::Push),
            "pop" => Some(Self::Pop),
            "peek" => Some(Self::Peek),
            "physical_index" => Some(Self::PhysicalIndex),
            "can_push" => Some(Self::CanPush),
            "can_pop" => Some(Self::CanPop),
            "can_peek" => Some(Self::CanPeek),
            "count" => Some(Self::Count),
            "capacity" => Some(Self::Capacity),
            "clear" => Some(Self::Clear),
            "overwrite_oldest" => Some(Self::OverwriteOldest),
            _ => None,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::Push
            | Self::Peek
            | Self::PhysicalIndex
            | Self::CanPeek
            | Self::OverwriteOldest => 2,
            Self::Pop
            | Self::CanPush
            | Self::CanPop
            | Self::Count
            | Self::Capacity
            | Self::Clear => 1,
        }
    }
}

fn typed_circular_storage_bindings(
    target: &str,
    args: &[SimpleExpr],
    expected_kind: TypedCollectionKind,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<
    (
        String,
        usize,
        DirectStorageRef,
        DirectStorageRef,
        DirectArrayStorageRef,
    ),
    String,
> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    }
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "{target} first argument '{}' is not a persistent typed collection path",
            path
        )
    })?;
    if !type_table.is_typed_collection_type(type_id) {
        return Err(format!(
            "{target} requires persistent {} path '{}', found non-typed state type {}",
            expected_kind.as_str(),
            path,
            type_id
        ));
    }
    let type_name = type_table
        .type_info(type_id)
        .map(|info| info.name.as_str())
        .ok_or_else(|| format!("{target} path '{}' has unknown type {}", path, type_id))?;
    let descriptor = type_table
        .parse_typed_collection_descriptor(type_name)?
        .ok_or_else(|| {
            format!(
                "{target} path '{}' has no compiler-owned typed collection descriptor",
                path
            )
        })?;
    if descriptor.kind != expected_kind {
        return Err(format!(
            "{target} requires persistent {} path '{}', found {}",
            expected_kind.as_str(),
            path,
            descriptor.kind_name()
        ));
    }
    if descriptor.element_type != Some(TYPE_ID_I32) {
        return Err(format!(
            "{target} requires {} path '{}' with i32 payload",
            expected_kind.as_str(),
            path
        ));
    }
    let capacity = usize::try_from(descriptor.capacity).map_err(|_| {
        format!(
            "{target} {} path '{}' capacity {} does not fit the target index type",
            expected_kind.as_str(),
            path,
            descriptor.capacity
        )
    })?;
    let direct_storage = runtime_call_refs.direct_storage.as_ref().ok_or_else(|| {
        format!(
            "{target} for typed {} '{}' requires direct storage bindings",
            expected_kind.as_str(),
            path
        )
    })?;
    let count_path = format!("{path}.count");
    let count = direct_storage
        .scalars
        .get(&count_path)
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct scalar binding '{}.count'",
                expected_kind.as_str(),
                path,
                path
            )
        })?;
    let head_path = format!("{path}.head");
    let head = direct_storage
        .scalars
        .get(&head_path)
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct scalar binding '{}.head'",
                expected_kind.as_str(),
                path,
                path
            )
        })?;
    let values = direct_storage
        .arrays
        .get(&(path.to_string(), String::from("values")))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct array binding '{}.values'",
                expected_kind.as_str(),
                path,
                path
            )
        })?;
    if values.storage_bytes != 4 || values.static_len != Some(capacity) {
        return Err(format!(
            "{target} for typed {} '{}' requires an i32 values binding with static length {}, found {} bytes and length {:?}",
            expected_kind.as_str(),
            path,
            capacity,
            values.storage_bytes,
            values.static_len
        ));
    }
    Ok((path.clone(), capacity, count, head, values))
}

fn emit_typed_circular_metadata_valid(
    builder: &mut FunctionBuilder<'_>,
    count: Value,
    head: Value,
    capacity: i32,
) -> Value {
    let count_non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, count, 0);
    let count_within_capacity =
        builder
            .ins()
            .icmp_imm(IntCC::SignedLessThanOrEqual, count, i64::from(capacity));
    let head_non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, head, 0);
    let head_below_capacity =
        builder
            .ins()
            .icmp_imm(IntCC::SignedLessThan, head, i64::from(capacity));
    let count_valid = builder
        .ins()
        .band(count_non_negative, count_within_capacity);
    let head_valid = builder.ins().band(head_non_negative, head_below_capacity);
    builder.ins().band(count_valid, head_valid)
}

fn emit_typed_circular_physical_index(
    builder: &mut FunctionBuilder<'_>,
    head: Value,
    logical_index: Value,
    capacity: i32,
) -> Value {
    // Keep the addition and modulo in i64 even though the public ABI is i32.
    // Metadata and logical-index validation happens before this value is used
    // for a bounds-proven storage access.
    let head_i64 = builder.ins().sextend(types::I64, head);
    let logical_i64 = builder.ins().sextend(types::I64, logical_index);
    let offset = builder.ins().iadd(head_i64, logical_i64);
    let capacity_i64 = builder.ins().iconst(types::I64, i64::from(capacity));
    let remainder = builder.ins().urem(offset, capacity_i64);
    builder.ins().ireduce(types::I32, remainder)
}

fn emit_typed_circular_overwrite_oldest(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    head_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
    value: Value,
    type_table: &TypeTable,
) -> Result<(), String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed circular collection capacity {capacity} exceeds i32 operation range")
    })?;
    if capacity_i32 == 0 {
        return Err("overwrite_oldest requires positive capacity".to_string());
    }

    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let head = emit_direct_scalar_load(builder, head_ref, TYPE_ID_I32, type_table)?;
    let metadata_valid = emit_typed_circular_metadata_valid(builder, count, head, capacity_i32);
    emit_typed_collection_metadata_trap(builder, metadata_valid);
    let full = builder
        .ins()
        .icmp_imm(IntCC::Equal, count, i64::from(capacity_i32));
    let full_block = builder.create_block();
    let nonfull_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(full, full_block, &[], nonfull_block, &[]);

    builder.seal_block(nonfull_block);
    builder.switch_to_block(nonfull_block);
    let physical = emit_typed_circular_physical_index(builder, head, count, capacity_i32);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        physical,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(full_block);
    builder.switch_to_block(full_block);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        head,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let one = builder.ins().iconst(types::I32, 1);
    let next_head = emit_typed_circular_physical_index(builder, head, one, capacity_i32);
    emit_direct_i32_store(builder, head_ref, next_head);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

fn emit_typed_circular_guard_state(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    head_ref: DirectStorageRef,
    capacity: usize,
    mode: TypedCollectionGuardAction,
    logical_index: Option<Value>,
    type_table: &TypeTable,
) -> Result<(ValueBinding, Option<TypedCollectionGuardState>), String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed circular collection capacity {capacity} exceeds i32 operation range")
    })?;
    if capacity_i32 <= 0 {
        let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
        let head = emit_direct_scalar_load(builder, head_ref, TYPE_ID_I32, type_table)?;
        let count_zero = builder.ins().icmp_imm(IntCC::Equal, count, 0);
        let head_zero = builder.ins().icmp_imm(IntCC::Equal, head, 0);
        let metadata_valid = builder.ins().band(count_zero, head_zero);
        emit_typed_collection_metadata_trap(builder, metadata_valid);
        let physical = logical_index.map(|_| builder.ins().iconst(types::I32, 0));
        return Ok((
            ValueBinding {
                value: builder.ins().iconst(types::I32, 0),
                type_id: TYPE_ID_BOOL,
            },
            Some(TypedCollectionGuardState::Circular {
                count,
                head,
                physical,
            }),
        ));
    }
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let head = emit_direct_scalar_load(builder, head_ref, TYPE_ID_I32, type_table)?;
    let metadata_valid = emit_typed_circular_metadata_valid(builder, count, head, capacity_i32);
    emit_typed_collection_metadata_trap(builder, metadata_valid);
    let (valid, physical) = if let Some(logical_index) = logical_index {
        let logical_non_negative =
            builder
                .ins()
                .icmp_imm(IntCC::SignedGreaterThanOrEqual, logical_index, 0);
        let logical_below_count = builder
            .ins()
            .icmp(IntCC::SignedLessThan, logical_index, count);
        let logical_below_capacity = builder.ins().icmp_imm(
            IntCC::SignedLessThan,
            logical_index,
            i64::from(capacity_i32),
        );
        let logical_valid = builder
            .ins()
            .band(logical_non_negative, logical_below_count);
        let logical_valid = builder.ins().band(logical_valid, logical_below_capacity);
        let physical =
            emit_typed_circular_physical_index(builder, head, logical_index, capacity_i32);
        (
            builder.ins().band(metadata_valid, logical_valid),
            Some(physical),
        )
    } else {
        let predicate = match mode {
            TypedCollectionGuardAction::QueuePop | TypedCollectionGuardAction::RingBufferPop => {
                builder.ins().icmp_imm(IntCC::SignedGreaterThan, count, 0)
            }
            TypedCollectionGuardAction::QueuePush | TypedCollectionGuardAction::RingBufferPush => {
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedLessThan, count, i64::from(capacity_i32))
            }
            _ => builder.ins().iconst(types::I32, 0),
        };
        (builder.ins().band(metadata_valid, predicate), None)
    };
    let state = TypedCollectionGuardState::Circular {
        count,
        head,
        physical,
    };
    Ok((
        ValueBinding {
            value: valid,
            type_id: TYPE_ID_BOOL,
        },
        Some(state),
    ))
}

fn emit_typed_circular_push_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
    count: Value,
    head: Value,
    value: Value,
) -> Result<(), String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed circular collection capacity {capacity} exceeds i32 operation range")
    })?;
    let physical = emit_typed_circular_physical_index(builder, head, count, capacity_i32);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        physical,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    Ok(())
}

fn emit_typed_circular_pop_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    head_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
    count: Value,
    head: Value,
) -> Result<(), String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed circular collection capacity {capacity} exceeds i32 operation range")
    })?;
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        head,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, -1);
    emit_direct_i32_store(builder, count_ref, next_count);
    let now_empty = builder.ins().icmp_imm(IntCC::Equal, next_count, 0);
    let reset_head_block = builder.create_block();
    let advance_head_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(now_empty, reset_head_block, &[], advance_head_block, &[]);

    builder.seal_block(reset_head_block);
    builder.switch_to_block(reset_head_block);
    emit_direct_i32_store(builder, head_ref, zero);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(advance_head_block);
    builder.switch_to_block(advance_head_block);
    let one = builder.ins().iconst(types::I32, 1);
    let next_head = emit_typed_circular_physical_index(builder, head, one, capacity_i32);
    emit_direct_i32_store(builder, head_ref, next_head);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

fn emit_typed_circular_peek_fast(
    builder: &mut FunctionBuilder<'_>,
    values_ref: DirectArrayStorageRef,
    physical: Value,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    Ok(ValueBinding {
        value: emit_direct_array_load(
            builder,
            values_ref.slot,
            physical,
            TYPE_ID_I32,
            type_table,
            values_ref.storage_bytes,
            values_ref.static_len,
            true,
        )?,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_circular_clear(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    head_ref: DirectStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
) -> Result<(), String> {
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_i32_store(builder, count_ref, zero);
    emit_direct_i32_store(builder, head_ref, zero);
    if capacity == 0 {
        return Ok(());
    }
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed circular collection capacity {capacity} exceeds i32 operation range")
    })?;
    let condition_block = builder.create_block();
    let body_block = builder.create_block();
    let exit_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    builder.ins().jump(condition_block, &[zero]);

    builder.switch_to_block(condition_block);
    let index = builder.block_params(condition_block)[0];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(capacity_i32));
    builder.ins().brif(more, body_block, &[], exit_block, &[]);

    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next = builder.ins().iadd_imm(index, 1);
    builder.ins().jump(condition_block, &[next]);
    builder.seal_block(condition_block);

    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

fn emit_typed_circular_count(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    head_ref: DirectStorageRef,
    capacity: usize,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed circular collection capacity {capacity} exceeds i32 operation range")
    })?;
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let head = emit_direct_scalar_load(builder, head_ref, TYPE_ID_I32, type_table)?;
    let valid = if capacity_i32 == 0 {
        let count_zero = builder.ins().icmp_imm(IntCC::Equal, count, 0);
        let head_zero = builder.ins().icmp_imm(IntCC::Equal, head, 0);
        builder.ins().band(count_zero, head_zero)
    } else {
        emit_typed_circular_metadata_valid(builder, count, head, capacity_i32)
    };
    emit_typed_collection_metadata_trap(builder, valid);
    Ok(ValueBinding {
        value: count,
        type_id: TYPE_ID_I32,
    })
}

fn try_emit_typed_queue_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedCircularCallResult>, String> {
    let Some(kind) = TypedQueueCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::Queue,
    )? {
        return Ok(None);
    }
    let expected_arity = kind.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, head_ref, values_ref) = typed_circular_storage_bindings(
        target,
        args,
        TypedCollectionKind::Queue,
        values_by_name,
        runtime_call_refs,
        global_path_types,
        type_table,
    )?;
    match kind {
        TypedQueueCallKind::Push => {
            let value = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::QueuePush, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    count,
                    head,
                    physical: None,
                }) => emit_typed_circular_push_fast(
                    builder,
                    count_ref,
                    values_ref,
                    capacity,
                    count,
                    head,
                    value.value,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for queue path '{path}' received an invalid can_push proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for queue path '{path}' requires its exact direct can_push guard"
                    ));
                }
            }
            Ok(Some(TypedCircularCallResult::Void))
        }
        TypedQueueCallKind::OverwriteOldest => {
            if capacity == 0 {
                return Err(format!(
                    "{target} cannot be called on zero-capacity queue path '{path}'"
                ));
            }
            let value = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            emit_typed_circular_overwrite_oldest(
                builder,
                count_ref,
                head_ref,
                values_ref,
                capacity,
                value.value,
                type_table,
            )?;
            Ok(Some(TypedCircularCallResult::Void))
        }
        TypedQueueCallKind::CanPush => {
            let (value, state) = emit_typed_circular_guard_state(
                builder,
                count_ref,
                head_ref,
                capacity,
                TypedCollectionGuardAction::QueuePush,
                None,
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::QueuePush,
                    args,
                    state,
                );
            }
            Ok(Some(TypedCircularCallResult::Value(value)))
        }
        TypedQueueCallKind::CanPop => {
            let (value, state) = emit_typed_circular_guard_state(
                builder,
                count_ref,
                head_ref,
                capacity,
                TypedCollectionGuardAction::QueuePop,
                None,
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::QueuePop,
                    args,
                    state,
                );
            }
            Ok(Some(TypedCircularCallResult::Value(value)))
        }
        TypedQueueCallKind::CanPeek => {
            let logical_index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if logical_index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} logical index argument must have exact i32 type, found {}",
                    logical_index.type_id
                ));
            }
            let (value, state) = emit_typed_circular_guard_state(
                builder,
                count_ref,
                head_ref,
                capacity,
                TypedCollectionGuardAction::QueuePeek,
                Some(logical_index.value),
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::QueuePeek,
                    args,
                    state,
                );
            }
            Ok(Some(TypedCircularCallResult::Value(value)))
        }
        TypedQueueCallKind::Pop => {
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::QueuePop, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    count,
                    head,
                    physical: None,
                }) => emit_typed_circular_pop_fast(
                    builder, count_ref, head_ref, values_ref, capacity, count, head,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for queue path '{path}' received an invalid can_pop proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for queue path '{path}' requires its exact direct can_pop guard"
                    ));
                }
            }
            Ok(Some(TypedCircularCallResult::Void))
        }
        TypedQueueCallKind::Peek => {
            let logical_index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if logical_index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} logical index argument must have exact i32 type, found {}",
                    logical_index.type_id
                ));
            }
            let result = match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::QueuePeek, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    physical: Some(physical),
                    ..
                }) => emit_typed_circular_peek_fast(builder, values_ref, physical, type_table)?,
                Some(_) => {
                    return Err(format!(
                        "{target} for queue path '{path}' received an invalid can_peek proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for queue path '{path}' requires its exact direct can_peek guard"
                    ));
                }
            };
            Ok(Some(TypedCircularCallResult::Value(result)))
        }
        TypedQueueCallKind::PhysicalIndex => {
            let logical_index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if logical_index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} logical index argument must have exact i32 type, found {}",
                    logical_index.type_id
                ));
            }
            let physical = match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::QueuePeek, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    physical: Some(physical),
                    ..
                }) => physical,
                Some(_) => {
                    return Err(format!(
                        "{target} for queue path '{path}' received an invalid can_peek proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for queue path '{path}' requires its exact direct can_peek guard"
                    ));
                }
            };
            Ok(Some(TypedCircularCallResult::Value(ValueBinding {
                value: physical,
                type_id: TYPE_ID_I32,
            })))
        }
        TypedQueueCallKind::Count => Ok(Some(TypedCircularCallResult::Value(
            emit_typed_circular_count(builder, count_ref, head_ref, capacity, type_table)?,
        ))),
        TypedQueueCallKind::Capacity => {
            let capacity = i32::try_from(capacity).map_err(|_| {
                format!("{target} queue path '{path}' capacity exceeds i32 operation range")
            })?;
            Ok(Some(TypedCircularCallResult::Value(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(capacity)),
                type_id: TYPE_ID_I32,
            })))
        }
        TypedQueueCallKind::Clear => {
            emit_typed_circular_clear(builder, count_ref, head_ref, values_ref, capacity)?;
            Ok(Some(TypedCircularCallResult::Void))
        }
    }
}

#[derive(Clone, Copy)]
enum TypedPriorityQueueCallResult {
    Value(ValueBinding),
    Void,
}

#[derive(Clone, Copy)]
enum TypedPriorityQueueCallKind {
    Push,
    Pop,
    Peek,
    PeekPriority,
    Count,
    Capacity,
    Clear,
    CanPush,
    CanPop,
    CanPeek,
}

impl TypedPriorityQueueCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "push" => Some(Self::Push),
            "pop" => Some(Self::Pop),
            "peek" => Some(Self::Peek),
            "peek_priority" => Some(Self::PeekPriority),
            "count" => Some(Self::Count),
            "capacity" => Some(Self::Capacity),
            "clear" => Some(Self::Clear),
            "can_push" => Some(Self::CanPush),
            "can_pop" => Some(Self::CanPop),
            "can_peek" => Some(Self::CanPeek),
            _ => None,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::Push => 3,
            Self::Pop
            | Self::Peek
            | Self::PeekPriority
            | Self::Count
            | Self::Capacity
            | Self::Clear
            | Self::CanPush
            | Self::CanPop
            | Self::CanPeek => 1,
        }
    }
}

fn typed_priority_queue_storage_bindings(
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<
    (
        String,
        usize,
        DirectStorageRef,
        DirectStorageRef,
        DirectArrayStorageRef,
        DirectArrayStorageRef,
        DirectArrayStorageRef,
    ),
    String,
> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    }
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "{target} first argument '{}' is not a persistent typed collection path",
            path
        )
    })?;
    if !type_table.is_typed_collection_type(type_id) {
        return Err(format!(
            "{target} requires persistent priority_queue path '{}', found non-typed state type {}",
            path, type_id
        ));
    }
    let type_name = type_table
        .type_info(type_id)
        .map(|info| info.name.as_str())
        .ok_or_else(|| format!("{target} path '{}' has unknown type {}", path, type_id))?;
    let descriptor = type_table
        .parse_typed_collection_descriptor(type_name)?
        .ok_or_else(|| {
            format!(
                "{target} path '{}' has no compiler-owned typed collection descriptor",
                path
            )
        })?;
    if descriptor.kind != TypedCollectionKind::PriorityQueue {
        return Err(format!(
            "{target} requires persistent priority_queue path '{}', found {}",
            path,
            descriptor.kind_name()
        ));
    }
    if descriptor.element_type != Some(TYPE_ID_I32) {
        return Err(format!(
            "{target} requires priority_queue path '{}' with i32 payload",
            path
        ));
    }
    let capacity = usize::try_from(descriptor.capacity).map_err(|_| {
        format!(
            "{target} priority_queue path '{}' capacity {} does not fit the target index type",
            path, descriptor.capacity
        )
    })?;
    let lanes_match = descriptor.lanes.len() == 5
        && descriptor.lanes[0].name == "count"
        && descriptor.lanes[0].type_id == TYPE_ID_I32
        && descriptor.lanes[0].element_count == 1
        && descriptor.lanes[1].name == "next_order"
        && descriptor.lanes[1].type_id == TYPE_ID_U32
        && descriptor.lanes[1].element_count == 1
        && descriptor.lanes[2].name == "priority"
        && descriptor.lanes[2].type_id == TYPE_ID_I32
        && descriptor.lanes[2].element_count == u64::from(descriptor.capacity)
        && descriptor.lanes[3].name == "order"
        && descriptor.lanes[3].type_id == TYPE_ID_U32
        && descriptor.lanes[3].element_count == u64::from(descriptor.capacity)
        && descriptor.lanes[4].name == "values"
        && descriptor.lanes[4].type_id == TYPE_ID_I32
        && descriptor.lanes[4].element_count == u64::from(descriptor.capacity);
    if !lanes_match {
        return Err(format!(
            "{target} priority_queue path '{}' requires exact count:i32, next_order:u32, priority:i32[N], order:u32[N], and values:i32[N] lanes",
            path
        ));
    }
    let direct_storage = runtime_call_refs.direct_storage.as_ref().ok_or_else(|| {
        format!(
            "{target} for typed priority_queue '{}' requires direct storage bindings",
            path
        )
    })?;
    let scalar = |name: &str| {
        let lane_path = format!("{path}.{name}");
        direct_storage
            .scalars
            .get(&lane_path)
            .copied()
            .ok_or_else(|| {
                format!(
                "{target} for typed priority_queue '{}' is missing direct scalar binding '{}.{}'",
                path, path, name
            )
            })
    };
    let array = |name: &str, type_name: &str| {
        let binding = direct_storage
            .arrays
            .get(&(path.to_string(), name.to_string()))
            .copied()
            .ok_or_else(|| {
                format!(
                    "{target} for typed priority_queue '{}' is missing direct array binding '{}.{}'",
                    path, path, name
                )
            })?;
        if binding.storage_bytes != 4 || binding.static_len != Some(capacity) {
            return Err(format!(
                "{target} for typed priority_queue '{}' requires a {} binding with static length {}, found {} bytes and length {:?}",
                path,
                type_name,
                capacity,
                binding.storage_bytes,
                binding.static_len
            ));
        }
        Ok(binding)
    };
    Ok((
        path.clone(),
        capacity,
        scalar("count")?,
        scalar("next_order")?,
        array("priority", "i32")?,
        array("order", "u32")?,
        array("values", "i32")?,
    ))
}

fn is_priority_queue_receiver(
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<bool, String> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Ok(false);
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Ok(false);
    }
    let Some(type_id) = global_path_types.get(path).copied() else {
        return Ok(false);
    };
    if !type_table.is_typed_collection_type(type_id) {
        return Ok(false);
    }
    let Some(type_name) = type_table.type_info(type_id).map(|info| info.name.as_str()) else {
        return Ok(false);
    };
    Ok(type_table
        .parse_typed_collection_descriptor(type_name)?
        .is_some_and(|descriptor| descriptor.kind == TypedCollectionKind::PriorityQueue))
}

fn emit_typed_priority_queue_metadata_valid(
    builder: &mut FunctionBuilder<'_>,
    count: Value,
    capacity: i32,
) -> Value {
    let count_non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, count, 0);
    let count_within_capacity =
        builder
            .ins()
            .icmp_imm(IntCC::SignedLessThanOrEqual, count, i64::from(capacity));
    builder
        .ins()
        .band(count_non_negative, count_within_capacity)
}

fn emit_typed_priority_queue_before(
    builder: &mut FunctionBuilder<'_>,
    priority: Value,
    order: Value,
    other_priority: Value,
    other_order: Value,
) -> Value {
    let priority_less = builder
        .ins()
        .icmp(IntCC::SignedLessThan, priority, other_priority);
    let priority_equal = builder.ins().icmp(IntCC::Equal, priority, other_priority);
    let order_less = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, order, other_order);
    let tie_less = builder.ins().band(priority_equal, order_less);
    builder.ins().bor(priority_less, tie_less)
}

fn emit_typed_priority_queue_sift_up(
    builder: &mut FunctionBuilder<'_>,
    index: Value,
    priority_ref: DirectArrayStorageRef,
    order_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    type_table: &TypeTable,
) -> Result<(), String> {
    let condition_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    let body_block = builder.create_block();
    builder.append_block_param(body_block, types::I32);
    let swap_block = builder.create_block();
    for _ in 0..8 {
        builder.append_block_param(swap_block, types::I32);
    }
    let exit_block = builder.create_block();

    builder.ins().jump(condition_block, &[index]);

    builder.switch_to_block(condition_block);
    let current_index = builder.block_params(condition_block)[0];
    let has_parent = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThan, current_index, 0);
    builder
        .ins()
        .brif(has_parent, body_block, &[current_index], exit_block, &[]);

    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    let current_index = builder.block_params(body_block)[0];
    let parent_minus_one = builder.ins().iadd_imm(current_index, -1);
    let two = builder.ins().iconst(types::I32, 2);
    let parent = builder.ins().udiv(parent_minus_one, two);
    let current_priority = emit_direct_array_load(
        builder,
        priority_ref.slot,
        current_index,
        TYPE_ID_I32,
        type_table,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    let current_order = emit_direct_array_load(
        builder,
        order_ref.slot,
        current_index,
        TYPE_ID_U32,
        type_table,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    let current_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        current_index,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let parent_priority = emit_direct_array_load(
        builder,
        priority_ref.slot,
        parent,
        TYPE_ID_I32,
        type_table,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    let parent_order = emit_direct_array_load(
        builder,
        order_ref.slot,
        parent,
        TYPE_ID_U32,
        type_table,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    let parent_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        parent,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let should_swap = emit_typed_priority_queue_before(
        builder,
        current_priority,
        current_order,
        parent_priority,
        parent_order,
    );
    builder.ins().brif(
        should_swap,
        swap_block,
        &[
            current_index,
            parent,
            current_priority,
            current_order,
            current_value,
            parent_priority,
            parent_order,
            parent_value,
        ],
        exit_block,
        &[],
    );

    builder.seal_block(swap_block);
    builder.switch_to_block(swap_block);
    let params = builder.block_params(swap_block).to_vec();
    let current_index = params[0];
    let parent = params[1];
    let current_priority = params[2];
    let current_order = params[3];
    let current_value = params[4];
    let parent_priority = params[5];
    let parent_order = params[6];
    let parent_value = params[7];
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        parent,
        current_priority,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        parent,
        current_order,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        parent,
        current_value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        current_index,
        parent_priority,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        current_index,
        parent_order,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        current_index,
        parent_value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    builder.ins().jump(condition_block, &[parent]);

    builder.seal_block(condition_block);
    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

fn emit_typed_priority_queue_sift_down(
    builder: &mut FunctionBuilder<'_>,
    index: Value,
    limit: Value,
    priority_ref: DirectArrayStorageRef,
    order_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    type_table: &TypeTable,
) -> Result<(), String> {
    let condition_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    builder.append_block_param(condition_block, types::I32);
    let body_block = builder.create_block();
    builder.append_block_param(body_block, types::I32);
    builder.append_block_param(body_block, types::I32);
    let children_block = builder.create_block();
    for _ in 0..6 {
        builder.append_block_param(children_block, types::I32);
    }
    let choose_block = builder.create_block();
    for _ in 0..9 {
        builder.append_block_param(choose_block, types::I32);
    }
    let right_block = builder.create_block();
    for _ in 0..9 {
        builder.append_block_param(right_block, types::I32);
    }
    let swap_block = builder.create_block();
    for _ in 0..9 {
        builder.append_block_param(swap_block, types::I32);
    }
    let exit_block = builder.create_block();

    builder.ins().jump(condition_block, &[index, limit]);

    builder.switch_to_block(condition_block);
    let current_index = builder.block_params(condition_block)[0];
    let current_limit = builder.block_params(condition_block)[1];
    let has_current = builder
        .ins()
        .icmp(IntCC::SignedLessThan, current_index, current_limit);
    builder.ins().brif(
        has_current,
        body_block,
        &[current_index, current_limit],
        exit_block,
        &[],
    );

    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    let current_index = builder.block_params(body_block)[0];
    let current_limit = builder.block_params(body_block)[1];
    let current_priority = emit_direct_array_load(
        builder,
        priority_ref.slot,
        current_index,
        TYPE_ID_I32,
        type_table,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    let current_order = emit_direct_array_load(
        builder,
        order_ref.slot,
        current_index,
        TYPE_ID_U32,
        type_table,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    let current_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        current_index,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let current_index_i64 = builder.ins().sextend(types::I64, current_index);
    let shifted_index = builder.ins().ishl_imm(current_index_i64, 1);
    let left_i64 = builder.ins().iadd_imm(shifted_index, 1);
    let limit_i64 = builder.ins().sextend(types::I64, current_limit);
    let has_left = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, left_i64, limit_i64);
    let left = builder.ins().ireduce(types::I32, left_i64);
    builder.ins().brif(
        has_left,
        children_block,
        &[
            current_index,
            current_limit,
            left,
            current_priority,
            current_order,
            current_value,
        ],
        exit_block,
        &[],
    );

    builder.seal_block(children_block);
    builder.switch_to_block(children_block);
    let children_args = builder.block_params(children_block).to_vec();
    let current_index = children_args[0];
    let current_limit = children_args[1];
    let left = children_args[2];
    let current_priority = children_args[3];
    let current_order = children_args[4];
    let current_value = children_args[5];
    let right = builder.ins().iadd_imm(left, 1);
    let has_right = builder
        .ins()
        .icmp(IntCC::SignedLessThan, right, current_limit);
    let left_priority = emit_direct_array_load(
        builder,
        priority_ref.slot,
        left,
        TYPE_ID_I32,
        type_table,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    let left_order = emit_direct_array_load(
        builder,
        order_ref.slot,
        left,
        TYPE_ID_U32,
        type_table,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    let left_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        left,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let child_args = [
        current_index,
        current_limit,
        left,
        current_priority,
        current_order,
        current_value,
        left_priority,
        left_order,
        left_value,
    ];
    builder.ins().brif(
        has_right,
        right_block,
        &child_args,
        choose_block,
        &child_args,
    );

    builder.seal_block(right_block);
    builder.switch_to_block(right_block);
    let right_args = builder.block_params(right_block).to_vec();
    let current_index = right_args[0];
    let current_limit = right_args[1];
    let left = right_args[2];
    let current_priority = right_args[3];
    let current_order = right_args[4];
    let current_value = right_args[5];
    let left_priority = right_args[6];
    let left_order = right_args[7];
    let left_value = right_args[8];
    let right = builder.ins().iadd_imm(left, 1);
    let right_priority = emit_direct_array_load(
        builder,
        priority_ref.slot,
        right,
        TYPE_ID_I32,
        type_table,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    let right_order = emit_direct_array_load(
        builder,
        order_ref.slot,
        right,
        TYPE_ID_U32,
        type_table,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    let right_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        right,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let right_before_left = emit_typed_priority_queue_before(
        builder,
        right_priority,
        right_order,
        left_priority,
        left_order,
    );
    let child_index = builder.ins().select(right_before_left, right, left);
    let child_priority = builder
        .ins()
        .select(right_before_left, right_priority, left_priority);
    let child_order = builder
        .ins()
        .select(right_before_left, right_order, left_order);
    let child_value = builder
        .ins()
        .select(right_before_left, right_value, left_value);
    builder.ins().jump(
        choose_block,
        &[
            current_index,
            current_limit,
            child_index,
            current_priority,
            current_order,
            current_value,
            child_priority,
            child_order,
            child_value,
        ],
    );

    builder.seal_block(choose_block);
    builder.switch_to_block(choose_block);
    let choose_args = builder.block_params(choose_block).to_vec();
    let current_index = choose_args[0];
    let current_limit = choose_args[1];
    let child_index = choose_args[2];
    let current_priority = choose_args[3];
    let current_order = choose_args[4];
    let current_value = choose_args[5];
    let child_priority = choose_args[6];
    let child_order = choose_args[7];
    let child_value = choose_args[8];
    let should_swap = emit_typed_priority_queue_before(
        builder,
        child_priority,
        child_order,
        current_priority,
        current_order,
    );
    builder.ins().brif(
        should_swap,
        swap_block,
        &[
            current_index,
            current_limit,
            child_index,
            current_priority,
            current_order,
            current_value,
            child_priority,
            child_order,
            child_value,
        ],
        exit_block,
        &[],
    );

    builder.seal_block(swap_block);
    builder.switch_to_block(swap_block);
    let swap_args = builder.block_params(swap_block).to_vec();
    let current_index = swap_args[0];
    let current_limit = swap_args[1];
    let child_index = swap_args[2];
    let current_priority = swap_args[3];
    let current_order = swap_args[4];
    let current_value = swap_args[5];
    let child_priority = swap_args[6];
    let child_order = swap_args[7];
    let child_value = swap_args[8];
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        current_index,
        child_priority,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        current_index,
        child_order,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        current_index,
        child_value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        child_index,
        current_priority,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        child_index,
        current_order,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        child_index,
        current_value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    builder
        .ins()
        .jump(condition_block, &[child_index, current_limit]);

    builder.seal_block(condition_block);
    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

fn emit_typed_priority_queue_push_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    next_order_ref: DirectStorageRef,
    priority_ref: DirectArrayStorageRef,
    order_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    next_order: Value,
    priority: Value,
    value: Value,
    type_table: &TypeTable,
) -> Result<(), String> {
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        count,
        priority,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        count,
        next_order,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        count,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_typed_priority_queue_sift_up(
        builder,
        count,
        priority_ref,
        order_ref,
        values_ref,
        type_table,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    let next_sequence = builder.ins().iadd_imm(next_order, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    emit_direct_i32_store(builder, next_order_ref, next_sequence);
    Ok(())
}

fn emit_typed_priority_queue_pop_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    priority_ref: DirectArrayStorageRef,
    order_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    type_table: &TypeTable,
) -> Result<(), String> {
    let last_index = builder.ins().iadd_imm(count, -1);
    let zero = builder.ins().iconst(types::I32, 0);
    let single = builder.ins().icmp_imm(IntCC::Equal, count, 1);
    let single_block = builder.create_block();
    let multi_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(single, single_block, &[], multi_block, &[]);

    builder.seal_block(single_block);
    builder.switch_to_block(single_block);
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        last_index,
        zero,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        last_index,
        zero,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        last_index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_i32_store(builder, count_ref, zero);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(multi_block);
    builder.switch_to_block(multi_block);
    let last_priority = emit_direct_array_load(
        builder,
        priority_ref.slot,
        last_index,
        TYPE_ID_I32,
        type_table,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    let last_order = emit_direct_array_load(
        builder,
        order_ref.slot,
        last_index,
        TYPE_ID_U32,
        type_table,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    let last_value = emit_direct_array_load(
        builder,
        values_ref.slot,
        last_index,
        TYPE_ID_I32,
        type_table,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        zero,
        last_priority,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        zero,
        last_order,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        zero,
        last_value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        last_index,
        zero,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        last_index,
        zero,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        last_index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, -1);
    emit_direct_i32_store(builder, count_ref, next_count);
    emit_typed_priority_queue_sift_down(
        builder,
        zero,
        next_count,
        priority_ref,
        order_ref,
        values_ref,
        type_table,
    )?;
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

fn emit_typed_priority_queue_peek_fast(
    builder: &mut FunctionBuilder<'_>,
    values_ref: DirectArrayStorageRef,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    let root = builder.ins().iconst(types::I32, 0);
    Ok(ValueBinding {
        value: emit_direct_array_load(
            builder,
            values_ref.slot,
            root,
            TYPE_ID_I32,
            type_table,
            values_ref.storage_bytes,
            values_ref.static_len,
            true,
        )?,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_priority_queue_peek_priority_fast(
    builder: &mut FunctionBuilder<'_>,
    priority_ref: DirectArrayStorageRef,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    let root = builder.ins().iconst(types::I32, 0);
    Ok(ValueBinding {
        value: emit_direct_array_load(
            builder,
            priority_ref.slot,
            root,
            TYPE_ID_I32,
            type_table,
            priority_ref.storage_bytes,
            priority_ref.static_len,
            true,
        )?,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_priority_queue_guard_state(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    next_order_ref: DirectStorageRef,
    capacity: usize,
    mode: TypedCollectionGuardAction,
    type_table: &TypeTable,
) -> Result<(ValueBinding, Option<TypedCollectionGuardState>), String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed priority_queue capacity {capacity} exceeds i32 operation range")
    })?;
    if capacity_i32 <= 0 {
        let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
        let metadata_valid = builder.ins().icmp_imm(IntCC::Equal, count, 0);
        emit_typed_collection_metadata_trap(builder, metadata_valid);
        let next_order = if mode == TypedCollectionGuardAction::PriorityQueuePush {
            Some(emit_direct_scalar_load(
                builder,
                next_order_ref,
                TYPE_ID_U32,
                type_table,
            )?)
        } else {
            None
        };
        return Ok((
            ValueBinding {
                value: builder.ins().iconst(types::I32, 0),
                type_id: TYPE_ID_BOOL,
            },
            Some(TypedCollectionGuardState::PriorityQueue { count, next_order }),
        ));
    }
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let metadata_valid = emit_typed_priority_queue_metadata_valid(builder, count, capacity_i32);
    emit_typed_collection_metadata_trap(builder, metadata_valid);
    let (value, next_order) = match mode {
        TypedCollectionGuardAction::PriorityQueuePush => {
            let next_order =
                emit_direct_scalar_load(builder, next_order_ref, TYPE_ID_U32, type_table)?;
            let has_room =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedLessThan, count, i64::from(capacity_i32));
            let order_available = builder.ins().icmp_imm(IntCC::NotEqual, next_order, -1);
            let can_push = builder.ins().band(metadata_valid, has_room);
            (
                builder.ins().band(can_push, order_available),
                Some(next_order),
            )
        }
        TypedCollectionGuardAction::PriorityQueuePop
        | TypedCollectionGuardAction::PriorityQueuePeek => {
            let nonempty = builder.ins().icmp_imm(IntCC::SignedGreaterThan, count, 0);
            (builder.ins().band(metadata_valid, nonempty), None)
        }
        _ => (builder.ins().iconst(types::I32, 0), None),
    };
    Ok((
        ValueBinding {
            value,
            type_id: TYPE_ID_BOOL,
        },
        Some(TypedCollectionGuardState::PriorityQueue { count, next_order }),
    ))
}

fn emit_typed_priority_queue_count(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    capacity: usize,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed priority_queue capacity {capacity} exceeds i32 operation range")
    })?;
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let valid = if capacity_i32 == 0 {
        builder.ins().icmp_imm(IntCC::Equal, count, 0)
    } else {
        emit_typed_priority_queue_metadata_valid(builder, count, capacity_i32)
    };
    emit_typed_collection_metadata_trap(builder, valid);
    Ok(ValueBinding {
        value: count,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_priority_queue_clear(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    next_order_ref: DirectStorageRef,
    priority_ref: DirectArrayStorageRef,
    order_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
) -> Result<(), String> {
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_i32_store(builder, count_ref, zero);
    emit_direct_i32_store(builder, next_order_ref, zero);
    if capacity == 0 {
        return Ok(());
    }
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed priority_queue capacity {capacity} exceeds i32 operation range")
    })?;
    let condition_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    let body_block = builder.create_block();
    let exit_block = builder.create_block();
    builder.ins().jump(condition_block, &[zero]);

    builder.switch_to_block(condition_block);
    let index = builder.block_params(condition_block)[0];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(capacity_i32));
    builder.ins().brif(more, body_block, &[], exit_block, &[]);

    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    emit_direct_array_store(
        builder,
        priority_ref.slot,
        index,
        zero,
        TYPE_ID_I32,
        priority_ref.storage_bytes,
        priority_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        order_ref.slot,
        index,
        zero,
        TYPE_ID_U32,
        order_ref.storage_bytes,
        order_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next = builder.ins().iadd_imm(index, 1);
    builder.ins().jump(condition_block, &[next]);

    builder.seal_block(condition_block);
    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

fn try_emit_typed_priority_queue_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedPriorityQueueCallResult>, String> {
    let Some(kind) = TypedPriorityQueueCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_priority_queue_receiver(args, values_by_name, global_path_types, type_table)? {
        return Ok(None);
    }
    let expected_arity = kind.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, next_order_ref, priority_ref, order_ref, values_ref) =
        typed_priority_queue_storage_bindings(
            target,
            args,
            values_by_name,
            runtime_call_refs,
            global_path_types,
            type_table,
        )?;
    match kind {
        TypedPriorityQueueCallKind::Push => {
            let priority = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if priority.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} priority argument must have exact i32 type, found {}",
                    priority.type_id
                ));
            }
            let value = emit_simple_expression(
                builder,
                &args[2],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            match internal_calls.take_typed_collection_guard_state(
                TypedCollectionGuardAction::PriorityQueuePush,
                args,
            ) {
                Some(TypedCollectionGuardState::PriorityQueue {
                    count,
                    next_order: Some(next_order),
                }) => emit_typed_priority_queue_push_fast(
                    builder,
                    count_ref,
                    next_order_ref,
                    priority_ref,
                    order_ref,
                    values_ref,
                    count,
                    next_order,
                    priority.value,
                    value.value,
                    type_table,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' received an invalid can_push proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' requires its exact direct can_push guard"
                    ));
                }
            }
            Ok(Some(TypedPriorityQueueCallResult::Void))
        }
        TypedPriorityQueueCallKind::Pop => {
            match internal_calls.take_typed_collection_guard_state(
                TypedCollectionGuardAction::PriorityQueuePop,
                args,
            ) {
                Some(TypedCollectionGuardState::PriorityQueue {
                    count,
                    next_order: None,
                }) => emit_typed_priority_queue_pop_fast(
                    builder,
                    count_ref,
                    priority_ref,
                    order_ref,
                    values_ref,
                    count,
                    type_table,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' received an invalid can_pop proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' requires its exact direct can_pop guard"
                    ));
                }
            }
            Ok(Some(TypedPriorityQueueCallResult::Void))
        }
        TypedPriorityQueueCallKind::Peek => {
            let result = match internal_calls.take_typed_collection_guard_state(
                TypedCollectionGuardAction::PriorityQueuePeek,
                args,
            ) {
                Some(TypedCollectionGuardState::PriorityQueue {
                    next_order: None, ..
                }) => emit_typed_priority_queue_peek_fast(builder, values_ref, type_table)?,
                Some(_) => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' received an invalid can_peek proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' requires its exact direct can_peek guard"
                    ));
                }
            };
            Ok(Some(TypedPriorityQueueCallResult::Value(result)))
        }
        TypedPriorityQueueCallKind::PeekPriority => {
            let result = match internal_calls.take_typed_collection_guard_state(
                TypedCollectionGuardAction::PriorityQueuePeek,
                args,
            ) {
                Some(TypedCollectionGuardState::PriorityQueue {
                    next_order: None, ..
                }) => {
                    emit_typed_priority_queue_peek_priority_fast(builder, priority_ref, type_table)?
                }
                Some(_) => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' received an invalid can_peek proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for priority_queue path '{path}' requires its exact direct can_peek guard"
                    ));
                }
            };
            Ok(Some(TypedPriorityQueueCallResult::Value(result)))
        }
        TypedPriorityQueueCallKind::Count => Ok(Some(TypedPriorityQueueCallResult::Value(
            emit_typed_priority_queue_count(builder, count_ref, capacity, type_table)?,
        ))),
        TypedPriorityQueueCallKind::Capacity => {
            let capacity = i32::try_from(capacity).map_err(|_| {
                format!(
                    "{target} priority_queue path '{path}' capacity exceeds i32 operation range"
                )
            })?;
            Ok(Some(TypedPriorityQueueCallResult::Value(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(capacity)),
                type_id: TYPE_ID_I32,
            })))
        }
        TypedPriorityQueueCallKind::Clear => {
            emit_typed_priority_queue_clear(
                builder,
                count_ref,
                next_order_ref,
                priority_ref,
                order_ref,
                values_ref,
                capacity,
            )?;
            Ok(Some(TypedPriorityQueueCallResult::Void))
        }
        TypedPriorityQueueCallKind::CanPush => {
            let (value, state) = emit_typed_priority_queue_guard_state(
                builder,
                count_ref,
                next_order_ref,
                capacity,
                TypedCollectionGuardAction::PriorityQueuePush,
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::PriorityQueuePush,
                    args,
                    state,
                );
            }
            Ok(Some(TypedPriorityQueueCallResult::Value(value)))
        }
        TypedPriorityQueueCallKind::CanPop | TypedPriorityQueueCallKind::CanPeek => {
            let action = if matches!(kind, TypedPriorityQueueCallKind::CanPop) {
                TypedCollectionGuardAction::PriorityQueuePop
            } else {
                TypedCollectionGuardAction::PriorityQueuePeek
            };
            let (value, state) = emit_typed_priority_queue_guard_state(
                builder,
                count_ref,
                next_order_ref,
                capacity,
                action,
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(target, action, args, state);
            }
            Ok(Some(TypedPriorityQueueCallResult::Value(value)))
        }
    }
}

#[derive(Clone, Copy)]
enum TypedRingBufferCallKind {
    Push,
    Pop,
    Peek,
    PhysicalIndex,
    CanPush,
    CanPop,
    CanPeek,
    Count,
    Capacity,
    Clear,
    OverwriteOldest,
}

impl TypedRingBufferCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "push" => Some(Self::Push),
            "pop" => Some(Self::Pop),
            "peek" => Some(Self::Peek),
            "physical_index" => Some(Self::PhysicalIndex),
            "can_push" => Some(Self::CanPush),
            "can_pop" => Some(Self::CanPop),
            "can_peek" => Some(Self::CanPeek),
            "count" => Some(Self::Count),
            "capacity" => Some(Self::Capacity),
            "clear" => Some(Self::Clear),
            "overwrite_oldest" => Some(Self::OverwriteOldest),
            _ => None,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::Push
            | Self::Peek
            | Self::PhysicalIndex
            | Self::CanPeek
            | Self::OverwriteOldest => 2,
            Self::Pop
            | Self::CanPush
            | Self::CanPop
            | Self::Count
            | Self::Capacity
            | Self::Clear => 1,
        }
    }
}

fn try_emit_typed_ring_buffer_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedCircularCallResult>, String> {
    let Some(kind) = TypedRingBufferCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::RingBuffer,
    )? {
        return Ok(None);
    }
    let expected_arity = kind.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, head_ref, values_ref) = typed_circular_storage_bindings(
        target,
        args,
        TypedCollectionKind::RingBuffer,
        values_by_name,
        runtime_call_refs,
        global_path_types,
        type_table,
    )?;
    match kind {
        TypedRingBufferCallKind::Push => {
            let value = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::RingBufferPush, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    count,
                    head,
                    physical: None,
                }) => emit_typed_circular_push_fast(
                    builder,
                    count_ref,
                    values_ref,
                    capacity,
                    count,
                    head,
                    value.value,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' received an invalid can_push proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' requires its exact direct can_push guard"
                    ));
                }
            }
            Ok(Some(TypedCircularCallResult::Void))
        }
        TypedRingBufferCallKind::OverwriteOldest => {
            if capacity == 0 {
                return Err(format!(
                    "{target} cannot be called on zero-capacity ring_buffer path '{path}'"
                ));
            }
            let value = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            emit_typed_circular_overwrite_oldest(
                builder,
                count_ref,
                head_ref,
                values_ref,
                capacity,
                value.value,
                type_table,
            )?;
            Ok(Some(TypedCircularCallResult::Void))
        }
        TypedRingBufferCallKind::CanPush => {
            let (value, state) = emit_typed_circular_guard_state(
                builder,
                count_ref,
                head_ref,
                capacity,
                TypedCollectionGuardAction::RingBufferPush,
                None,
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::RingBufferPush,
                    args,
                    state,
                );
            }
            Ok(Some(TypedCircularCallResult::Value(value)))
        }
        TypedRingBufferCallKind::CanPop => {
            let (value, state) = emit_typed_circular_guard_state(
                builder,
                count_ref,
                head_ref,
                capacity,
                TypedCollectionGuardAction::RingBufferPop,
                None,
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::RingBufferPop,
                    args,
                    state,
                );
            }
            Ok(Some(TypedCircularCallResult::Value(value)))
        }
        TypedRingBufferCallKind::CanPeek => {
            let logical_index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if logical_index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} logical index argument must have exact i32 type, found {}",
                    logical_index.type_id
                ));
            }
            let (value, state) = emit_typed_circular_guard_state(
                builder,
                count_ref,
                head_ref,
                capacity,
                TypedCollectionGuardAction::RingBufferPeek,
                Some(logical_index.value),
                type_table,
            )?;
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::RingBufferPeek,
                    args,
                    state,
                );
            }
            Ok(Some(TypedCircularCallResult::Value(value)))
        }
        TypedRingBufferCallKind::Pop => {
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::RingBufferPop, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    count,
                    head,
                    physical: None,
                }) => emit_typed_circular_pop_fast(
                    builder, count_ref, head_ref, values_ref, capacity, count, head,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' received an invalid can_pop proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' requires its exact direct can_pop guard"
                    ));
                }
            }
            Ok(Some(TypedCircularCallResult::Void))
        }
        TypedRingBufferCallKind::Peek => {
            let logical_index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if logical_index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} logical index argument must have exact i32 type, found {}",
                    logical_index.type_id
                ));
            }
            let result = match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::RingBufferPeek, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    physical: Some(physical),
                    ..
                }) => emit_typed_circular_peek_fast(builder, values_ref, physical, type_table)?,
                Some(_) => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' received an invalid can_peek proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' requires its exact direct can_peek guard"
                    ));
                }
            };
            Ok(Some(TypedCircularCallResult::Value(result)))
        }
        TypedRingBufferCallKind::PhysicalIndex => {
            let logical_index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if logical_index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} logical index argument must have exact i32 type, found {}",
                    logical_index.type_id
                ));
            }
            let physical = match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::RingBufferPeek, args)
            {
                Some(TypedCollectionGuardState::Circular {
                    physical: Some(physical),
                    ..
                }) => physical,
                Some(_) => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' received an invalid can_peek proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for ring_buffer path '{path}' requires its exact direct can_peek guard"
                    ));
                }
            };
            Ok(Some(TypedCircularCallResult::Value(ValueBinding {
                value: physical,
                type_id: TYPE_ID_I32,
            })))
        }
        TypedRingBufferCallKind::Count => Ok(Some(TypedCircularCallResult::Value(
            emit_typed_circular_count(builder, count_ref, head_ref, capacity, type_table)?,
        ))),
        TypedRingBufferCallKind::Capacity => {
            let capacity = i32::try_from(capacity).map_err(|_| {
                format!("{target} ring_buffer path '{path}' capacity exceeds i32 operation range")
            })?;
            Ok(Some(TypedCircularCallResult::Value(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(capacity)),
                type_id: TYPE_ID_I32,
            })))
        }
        TypedRingBufferCallKind::Clear => {
            emit_typed_circular_clear(builder, count_ref, head_ref, values_ref, capacity)?;
            Ok(Some(TypedCircularCallResult::Void))
        }
    }
}

#[derive(Clone, Copy)]
enum TypedStablePoolCallResult {
    Value(ValueBinding),
    Void,
}

#[derive(Clone, Copy)]
enum TypedStablePoolCallKind {
    Insert,
    Remove,
    CanInsert,
    CanRemove,
    Count,
    Capacity,
    Clear,
}

impl TypedStablePoolCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "insert" => Some(Self::Insert),
            "remove" => Some(Self::Remove),
            "can_insert" => Some(Self::CanInsert),
            "can_remove" => Some(Self::CanRemove),
            "count" => Some(Self::Count),
            "capacity" => Some(Self::Capacity),
            "clear" => Some(Self::Clear),
            _ => None,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::Insert | Self::Remove | Self::CanRemove => 2,
            Self::CanInsert | Self::Count | Self::Capacity | Self::Clear => 1,
        }
    }
}

fn typed_stable_pool_storage_bindings(
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<
    (
        String,
        usize,
        DirectStorageRef,
        DirectArrayStorageRef,
        DirectArrayStorageRef,
    ),
    String,
> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    }
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "{target} first argument '{}' is not a persistent typed collection path",
            path
        )
    })?;
    if !type_table.is_typed_collection_type(type_id) {
        return Err(format!(
            "{target} requires persistent stable_pool path '{}', found non-typed state type {}",
            path, type_id
        ));
    }
    let type_name = type_table
        .type_info(type_id)
        .map(|info| info.name.as_str())
        .ok_or_else(|| format!("{target} path '{}' has unknown type {}", path, type_id))?;
    let descriptor = type_table
        .parse_typed_collection_descriptor(type_name)?
        .ok_or_else(|| {
            format!(
                "{target} path '{}' has no compiler-owned typed collection descriptor",
                path
            )
        })?;
    if descriptor.kind != TypedCollectionKind::StablePool {
        return Err(format!(
            "{target} requires persistent stable_pool path '{}', found {}",
            path,
            descriptor.kind_name()
        ));
    }
    if descriptor.element_type != Some(TYPE_ID_I32) {
        return Err(format!(
            "{target} requires stable_pool path '{}' with i32 payload",
            path
        ));
    }
    let capacity = usize::try_from(descriptor.capacity).map_err(|_| {
        format!(
            "{target} stable_pool path '{}' capacity {} does not fit the target index type",
            path, descriptor.capacity
        )
    })?;
    let direct_storage = runtime_call_refs.direct_storage.as_ref().ok_or_else(|| {
        format!(
            "{target} for typed stable_pool '{}' requires direct storage bindings",
            path
        )
    })?;
    let count_path = format!("{path}.count");
    let count = direct_storage
        .scalars
        .get(&count_path)
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed stable_pool '{}' is missing direct scalar binding '{}.count'",
                path, path
            )
        })?;
    let occupied = direct_storage
        .arrays
        .get(&(path.to_string(), String::from("occupied")))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed stable_pool '{}' is missing direct array binding '{}.occupied'",
                path, path
            )
        })?;
    if occupied.storage_bytes != 1 || occupied.static_len != Some(capacity) {
        return Err(format!(
            "{target} for typed stable_pool '{}' requires a u8 occupied binding with static length {}, found {} bytes and length {:?}",
            path,
            capacity,
            occupied.storage_bytes,
            occupied.static_len
        ));
    }
    let values = direct_storage
        .arrays
        .get(&(path.to_string(), String::from("values")))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed stable_pool '{}' is missing direct array binding '{}.values'",
                path, path
            )
        })?;
    if values.storage_bytes != 4 || values.static_len != Some(capacity) {
        return Err(format!(
            "{target} for typed stable_pool '{}' requires an i32 values binding with static length {}, found {} bytes and length {:?}",
            path,
            capacity,
            values.storage_bytes,
            values.static_len
        ));
    }
    Ok((path.clone(), capacity, count, occupied, values))
}

fn emit_typed_stable_pool_scan(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    capacity: usize,
    type_table: &TypeTable,
) -> Result<(Value, Value, Value), String> {
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed stable_pool capacity {capacity} exceeds i32 operation range")
    })?;
    if capacity_i32 <= 0 {
        return Err("typed stable_pool metadata scan requires positive capacity".to_string());
    }
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let count_non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, count, 0);
    let count_within_capacity =
        builder
            .ins()
            .icmp_imm(IntCC::SignedLessThanOrEqual, count, i64::from(capacity_i32));
    let count_valid = builder
        .ins()
        .band(count_non_negative, count_within_capacity);

    let scan_block = builder.create_block();
    builder.append_block_param(scan_block, types::I32);
    builder.append_block_param(scan_block, types::I32);
    builder.append_block_param(scan_block, types::I32);
    let body_block = builder.create_block();
    builder.append_block_param(body_block, types::I32);
    builder.append_block_param(body_block, types::I32);
    builder.append_block_param(body_block, types::I32);
    let invalid_count_block = builder.create_block();
    let invalid_occupied_block = builder.create_block();
    let result_block = builder.create_block();
    builder.append_block_param(result_block, types::I32);
    builder.append_block_param(result_block, types::I32);
    builder.append_block_param(result_block, types::I8);
    let initial_index = builder.ins().iconst(types::I32, 0);
    let initial_total = builder.ins().iconst(types::I32, 0);
    let initial_first_free = builder.ins().iconst(types::I32, -1);
    builder.ins().brif(
        count_valid,
        scan_block,
        &[initial_index, initial_total, initial_first_free],
        invalid_count_block,
        &[],
    );

    builder.seal_block(invalid_count_block);
    builder.switch_to_block(invalid_count_block);
    let zero = builder.ins().iconst(types::I32, 0);
    let invalid_first_free = builder.ins().iconst(types::I32, -1);
    let invalid = builder.ins().iconst(types::I8, 0);
    builder
        .ins()
        .jump(result_block, &[zero, invalid_first_free, invalid]);

    builder.switch_to_block(scan_block);
    let index = builder.block_params(scan_block)[0];
    let occupied_total = builder.block_params(scan_block)[1];
    let first_free = builder.block_params(scan_block)[2];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(capacity_i32));
    let scan_valid = builder.ins().iconst(types::I8, 1);
    builder.ins().brif(
        more,
        body_block,
        &[index, occupied_total, first_free],
        result_block,
        &[occupied_total, first_free, scan_valid],
    );

    builder.switch_to_block(body_block);
    let body_index = builder.block_params(body_block)[0];
    let body_total = builder.block_params(body_block)[1];
    let body_first_free = builder.block_params(body_block)[2];
    let occupied_value = emit_direct_array_load(
        builder,
        occupied_ref.slot,
        body_index,
        TYPE_ID_U8,
        type_table,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    let is_zero = builder.ins().icmp_imm(IntCC::Equal, occupied_value, 0);
    let is_one = builder.ins().icmp_imm(IntCC::Equal, occupied_value, 1);
    let occupied_valid = builder.ins().bor(is_zero, is_one);
    let next_index = builder.ins().iadd_imm(body_index, 1);
    let next_total = builder.ins().iadd(body_total, occupied_value);
    let no_free_recorded = builder.ins().icmp_imm(IntCC::Equal, body_first_free, -1);
    let record_free = builder.ins().band(no_free_recorded, is_zero);
    let next_first_free = builder
        .ins()
        .select(record_free, body_index, body_first_free);
    builder.ins().brif(
        occupied_valid,
        scan_block,
        &[next_index, next_total, next_first_free],
        invalid_occupied_block,
        &[],
    );

    builder.seal_block(invalid_occupied_block);
    builder.switch_to_block(invalid_occupied_block);
    let zero = builder.ins().iconst(types::I32, 0);
    let invalid_first_free = builder.ins().iconst(types::I32, -1);
    let invalid = builder.ins().iconst(types::I8, 0);
    builder
        .ins()
        .jump(result_block, &[zero, invalid_first_free, invalid]);

    builder.seal_block(scan_block);
    builder.seal_block(body_block);
    builder.seal_block(result_block);
    builder.switch_to_block(result_block);
    let scanned_total = builder.block_params(result_block)[0];
    let scanned_first_free = builder.block_params(result_block)[1];
    let scanned_valid = builder.block_params(result_block)[2];
    let count_matches = builder.ins().icmp(IntCC::Equal, scanned_total, count);
    let metadata_valid = builder.ins().band(scanned_valid, count_matches);
    Ok((count, scanned_first_free, metadata_valid))
}

fn emit_typed_stable_pool_insert_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    first_free: Value,
    value: Value,
) -> Result<ValueBinding, String> {
    let occupied_one = builder.ins().iconst(types::I32, 1);
    emit_direct_array_store(
        builder,
        occupied_ref.slot,
        first_free,
        occupied_one,
        TYPE_ID_U8,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        first_free,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    Ok(ValueBinding {
        value: first_free,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_stable_pool_remove_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    index: Value,
) -> Result<(), String> {
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_array_store(
        builder,
        occupied_ref.slot,
        index,
        zero,
        TYPE_ID_U8,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, -1);
    emit_direct_i32_store(builder, count_ref, next_count);
    Ok(())
}

fn emit_typed_stable_pool_clear(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    capacity: usize,
) -> Result<(), String> {
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_i32_store(builder, count_ref, zero);
    if capacity == 0 {
        return Ok(());
    }
    let capacity_i32 = i32::try_from(capacity).map_err(|_| {
        format!("typed stable_pool capacity {capacity} exceeds i32 operation range")
    })?;
    let condition_block = builder.create_block();
    let body_block = builder.create_block();
    let exit_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    builder.ins().jump(condition_block, &[zero]);

    builder.switch_to_block(condition_block);
    let index = builder.block_params(condition_block)[0];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(capacity_i32));
    builder.ins().brif(more, body_block, &[], exit_block, &[]);

    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    emit_direct_array_store(
        builder,
        occupied_ref.slot,
        index,
        zero,
        TYPE_ID_U8,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        zero,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next = builder.ins().iadd_imm(index, 1);
    builder.ins().jump(condition_block, &[next]);
    builder.seal_block(condition_block);

    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

fn emit_typed_stable_pool_count(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    capacity: usize,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    let (count, metadata_valid) = if capacity == 0 {
        let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
        let valid = builder.ins().icmp_imm(IntCC::Equal, count, 0);
        (count, valid)
    } else {
        let (count, _first_free, valid) =
            emit_typed_stable_pool_scan(builder, count_ref, occupied_ref, capacity, type_table)?;
        (count, valid)
    };
    emit_typed_collection_metadata_trap(builder, metadata_valid);
    Ok(ValueBinding {
        value: count,
        type_id: TYPE_ID_I32,
    })
}

fn try_emit_typed_stable_pool_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedStablePoolCallResult>, String> {
    let Some(kind) = TypedStablePoolCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::StablePool,
    )? {
        return Ok(None);
    }
    let expected_arity = kind.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, occupied_ref, values_ref) = typed_stable_pool_storage_bindings(
        target,
        args,
        values_by_name,
        runtime_call_refs,
        global_path_types,
        type_table,
    )?;
    match kind {
        TypedStablePoolCallKind::Insert => {
            let value = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            let result = match internal_calls.take_typed_collection_guard_state(
                TypedCollectionGuardAction::StablePoolInsert,
                args,
            ) {
                Some(TypedCollectionGuardState::StablePool {
                    count,
                    index_or_first_free,
                }) => emit_typed_stable_pool_insert_fast(
                    builder,
                    count_ref,
                    occupied_ref,
                    values_ref,
                    count,
                    index_or_first_free,
                    value.value,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for stable_pool path '{path}' received an invalid can_insert proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for stable_pool path '{path}' requires its exact direct can_insert guard"
                    ));
                }
            };
            Ok(Some(TypedStablePoolCallResult::Value(result)))
        }
        TypedStablePoolCallKind::Remove => {
            let index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} index argument must have exact i32 type, found {}",
                    index.type_id
                ));
            }
            match internal_calls.take_typed_collection_guard_state(
                TypedCollectionGuardAction::StablePoolRemove,
                args,
            ) {
                Some(TypedCollectionGuardState::StablePool {
                    count,
                    index_or_first_free,
                }) => emit_typed_stable_pool_remove_fast(
                    builder,
                    count_ref,
                    occupied_ref,
                    values_ref,
                    count,
                    index_or_first_free,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for stable_pool path '{path}' received an invalid can_remove proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for stable_pool path '{path}' requires its exact direct can_remove guard"
                    ));
                }
            }
            Ok(Some(TypedStablePoolCallResult::Void))
        }
        TypedStablePoolCallKind::Count => Ok(Some(TypedStablePoolCallResult::Value(
            emit_typed_stable_pool_count(builder, count_ref, occupied_ref, capacity, type_table)?,
        ))),
        TypedStablePoolCallKind::Capacity => {
            let capacity = i32::try_from(capacity).map_err(|_| {
                format!("{target} stable_pool path '{path}' capacity exceeds i32 operation range")
            })?;
            Ok(Some(TypedStablePoolCallResult::Value(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(capacity)),
                type_id: TYPE_ID_I32,
            })))
        }
        TypedStablePoolCallKind::Clear => {
            emit_typed_stable_pool_clear(builder, count_ref, occupied_ref, values_ref, capacity)?;
            Ok(Some(TypedStablePoolCallResult::Void))
        }
        TypedStablePoolCallKind::CanInsert => {
            let (valid, proof_state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                let first_free = builder.ins().iconst(types::I32, -1);
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::StablePool {
                        count,
                        index_or_first_free: first_free,
                    }),
                )
            } else {
                let (count, first_free, metadata_valid) = emit_typed_stable_pool_scan(
                    builder,
                    count_ref,
                    occupied_ref,
                    capacity,
                    type_table,
                )?;
                emit_typed_collection_metadata_trap(builder, metadata_valid);
                let has_free =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, first_free, 0);
                let count_has_capacity = builder.ins().icmp_imm(
                    IntCC::SignedLessThan,
                    count,
                    i64::try_from(capacity).unwrap_or(i64::MAX),
                );
                let can_insert = builder.ins().band(has_free, count_has_capacity);
                (
                    builder.ins().band(metadata_valid, can_insert),
                    Some(TypedCollectionGuardState::StablePool {
                        count,
                        index_or_first_free: first_free,
                    }),
                )
            };
            if let Some(state) = proof_state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::StablePoolInsert,
                    args,
                    state,
                );
            }
            Ok(Some(TypedStablePoolCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
        TypedStablePoolCallKind::CanRemove => {
            let index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} index argument must have exact i32 type, found {}",
                    index.type_id
                ));
            }
            let (valid, proof_state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::StablePool {
                        count,
                        index_or_first_free: index.value,
                    }),
                )
            } else {
                let (count, _first_free, metadata_valid) = emit_typed_stable_pool_scan(
                    builder,
                    count_ref,
                    occupied_ref,
                    capacity,
                    type_table,
                )?;
                emit_typed_collection_metadata_trap(builder, metadata_valid);
                let index_non_negative =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, index.value, 0);
                let index_below_capacity = builder.ins().icmp_imm(
                    IntCC::SignedLessThan,
                    index.value,
                    i64::try_from(capacity).unwrap_or(i64::MAX),
                );
                let index_valid = builder.ins().band(index_non_negative, index_below_capacity);
                let candidate = builder.ins().band(metadata_valid, index_valid);
                let load_block = builder.create_block();
                let reject_block = builder.create_block();
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I32);
                builder
                    .ins()
                    .brif(candidate, load_block, &[], reject_block, &[]);
                builder.seal_block(load_block);
                builder.switch_to_block(load_block);
                let occupied = emit_direct_array_load(
                    builder,
                    occupied_ref.slot,
                    index.value,
                    TYPE_ID_U8,
                    type_table,
                    occupied_ref.storage_bytes,
                    occupied_ref.static_len,
                    true,
                )?;
                let occupied = builder.ins().icmp_imm(IntCC::Equal, occupied, 1);
                let occupied = builder.ins().uextend(types::I32, occupied);
                builder.ins().jump(merge_block, &[occupied]);
                builder.seal_block(reject_block);
                builder.switch_to_block(reject_block);
                let false_value = builder.ins().iconst(types::I32, 0);
                builder.ins().jump(merge_block, &[false_value]);
                builder.seal_block(merge_block);
                builder.switch_to_block(merge_block);
                (
                    builder.block_params(merge_block)[0],
                    Some(TypedCollectionGuardState::StablePool {
                        count,
                        index_or_first_free: index.value,
                    }),
                )
            };
            if let Some(state) = proof_state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::StablePoolRemove,
                    args,
                    state,
                );
            }
            Ok(Some(TypedStablePoolCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
    }
}

#[derive(Clone, Copy)]
enum TypedKeyedCallResult {
    Value(ValueBinding),
    Void,
}

#[derive(Clone, Copy)]
enum TypedMapCallKind {
    Put,
    Get,
    Contains,
    Remove,
    CanPut,
    CanGet,
    CanRemove,
}

impl TypedMapCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "put" => Some(Self::Put),
            "get" => Some(Self::Get),
            "contains" => Some(Self::Contains),
            "remove" => Some(Self::Remove),
            "can_put" => Some(Self::CanPut),
            "can_get" => Some(Self::CanGet),
            "can_remove" => Some(Self::CanRemove),
            _ => None,
        }
    }

    fn expected_arity(self) -> usize {
        match self {
            Self::Put => 3,
            Self::Get
            | Self::Contains
            | Self::Remove
            | Self::CanGet
            | Self::CanRemove
            | Self::CanPut => 2,
        }
    }
}

#[derive(Clone, Copy)]
enum TypedSetCallKind {
    Add,
    Contains,
    Remove,
    CanAdd,
    CanRemove,
}

impl TypedSetCallKind {
    fn from_target(target: &str) -> Option<Self> {
        match target {
            "add" => Some(Self::Add),
            "contains" => Some(Self::Contains),
            "remove" => Some(Self::Remove),
            "can_add" => Some(Self::CanAdd),
            "can_remove" => Some(Self::CanRemove),
            _ => None,
        }
    }
}

fn typed_keyed_storage_bindings(
    target: &str,
    args: &[SimpleExpr],
    expected_kind: TypedCollectionKind,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<
    (
        String,
        usize,
        DirectStorageRef,
        DirectArrayStorageRef,
        DirectArrayStorageRef,
        Option<DirectArrayStorageRef>,
    ),
    String,
> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    }
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "{target} first argument '{}' is not a persistent typed collection path",
            path
        )
    })?;
    if !type_table.is_typed_collection_type(type_id) {
        return Err(format!(
            "{target} requires persistent {} path '{}', found non-typed state type {}",
            expected_kind.as_str(),
            path,
            type_id
        ));
    }
    let type_name = type_table
        .type_info(type_id)
        .map(|info| info.name.as_str())
        .ok_or_else(|| format!("{target} path '{}' has unknown type {}", path, type_id))?;
    let descriptor = type_table
        .parse_typed_collection_descriptor(type_name)?
        .ok_or_else(|| {
            format!(
                "{target} path '{}' has no compiler-owned typed collection descriptor",
                path
            )
        })?;
    if descriptor.kind != expected_kind {
        return Err(format!(
            "{target} requires persistent {} path '{}', found {}",
            expected_kind.as_str(),
            path,
            descriptor.kind_name()
        ));
    }
    if descriptor.key_type != Some(TYPE_ID_I32) {
        return Err(format!(
            "{target} requires {} path '{}' with i32 keys",
            expected_kind.as_str(),
            path
        ));
    }
    if expected_kind == TypedCollectionKind::Map && descriptor.value_type != Some(TYPE_ID_I32) {
        return Err(format!(
            "{target} requires map path '{}' with i32 values",
            path
        ));
    }
    let capacity = usize::try_from(descriptor.capacity).map_err(|_| {
        format!(
            "{target} {} path '{}' capacity {} does not fit the target index type",
            expected_kind.as_str(),
            path,
            descriptor.capacity
        )
    })?;
    let direct_storage = runtime_call_refs.direct_storage.as_ref().ok_or_else(|| {
        format!(
            "{target} for typed {} '{}' requires direct storage bindings",
            expected_kind.as_str(),
            path
        )
    })?;
    let count_path = format!("{path}.count");
    let count = direct_storage
        .scalars
        .get(&count_path)
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct scalar binding '{}.count'",
                expected_kind.as_str(),
                path,
                path
            )
        })?;
    let occupied = direct_storage
        .arrays
        .get(&(path.to_string(), String::from("occupied")))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct array binding '{}.occupied'",
                expected_kind.as_str(),
                path,
                path
            )
        })?;
    if occupied.storage_bytes != 1 || occupied.static_len != Some(capacity) {
        return Err(format!(
            "{target} for typed {} '{}' requires a u8 occupied binding with static length {}, found {} bytes and length {:?}",
            expected_kind.as_str(),
            path,
            capacity,
            occupied.storage_bytes,
            occupied.static_len
        ));
    }
    let keys = direct_storage
        .arrays
        .get(&(path.to_string(), String::from("keys")))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct array binding '{}.keys'",
                expected_kind.as_str(),
                path,
                path
            )
        })?;
    if keys.storage_bytes != 4 || keys.static_len != Some(capacity) {
        return Err(format!(
            "{target} for typed {} '{}' requires an i32 keys binding with static length {}, found {} bytes and length {:?}",
            expected_kind.as_str(),
            path,
            capacity,
            keys.storage_bytes,
            keys.static_len
        ));
    }
    let values = if expected_kind == TypedCollectionKind::Map {
        let values = direct_storage
            .arrays
            .get(&(path.to_string(), String::from("values")))
            .copied()
            .ok_or_else(|| {
                format!(
                    "{target} for typed map '{}' is missing direct array binding '{}.values'",
                    path, path
                )
            })?;
        if values.storage_bytes != 4 || values.static_len != Some(capacity) {
            return Err(format!(
                "{target} for typed map '{}' requires an i32 values binding with static length {}, found {} bytes and length {:?}",
                path,
                capacity,
                values.storage_bytes,
                values.static_len
            ));
        }
        Some(values)
    } else {
        None
    };
    Ok((path.clone(), capacity, count, occupied, keys, values))
}

fn emit_typed_keyed_scan(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    capacity: usize,
    target_key: Value,
    type_table: &TypeTable,
) -> Result<(Value, Value, Value, Value), String> {
    let capacity_i32 = i32::try_from(capacity)
        .map_err(|_| format!("typed keyed collection capacity {capacity} exceeds i32 range"))?;
    if capacity_i32 <= 0 {
        return Err("typed keyed collection scan requires positive capacity".to_string());
    }
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let count_non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, count, 0);
    let count_within_capacity =
        builder
            .ins()
            .icmp_imm(IntCC::SignedLessThanOrEqual, count, i64::from(capacity_i32));
    let count_valid = builder
        .ins()
        .band(count_non_negative, count_within_capacity);

    let scan_block = builder.create_block();
    builder.append_block_param(scan_block, types::I32);
    builder.append_block_param(scan_block, types::I32);
    builder.append_block_param(scan_block, types::I32);
    builder.append_block_param(scan_block, types::I32);
    let body_block = builder.create_block();
    builder.append_block_param(body_block, types::I32);
    builder.append_block_param(body_block, types::I32);
    builder.append_block_param(body_block, types::I32);
    builder.append_block_param(body_block, types::I32);
    let invalid_count_block = builder.create_block();
    let invalid_occupied_block = builder.create_block();
    let result_block = builder.create_block();
    builder.append_block_param(result_block, types::I32);
    builder.append_block_param(result_block, types::I32);
    builder.append_block_param(result_block, types::I32);
    builder.append_block_param(result_block, types::I8);

    let initial_index = builder.ins().iconst(types::I32, 0);
    let initial_total = builder.ins().iconst(types::I32, 0);
    let initial_existing = builder.ins().iconst(types::I32, -1);
    let initial_free = builder.ins().iconst(types::I32, -1);
    builder.ins().brif(
        count_valid,
        scan_block,
        &[initial_index, initial_total, initial_existing, initial_free],
        invalid_count_block,
        &[],
    );

    builder.seal_block(invalid_count_block);
    builder.switch_to_block(invalid_count_block);
    let zero = builder.ins().iconst(types::I32, 0);
    let invalid_index = builder.ins().iconst(types::I32, -1);
    let invalid = builder.ins().iconst(types::I8, 0);
    builder
        .ins()
        .jump(result_block, &[zero, invalid_index, invalid_index, invalid]);

    builder.switch_to_block(scan_block);
    let index = builder.block_params(scan_block)[0];
    let occupied_total = builder.block_params(scan_block)[1];
    let existing = builder.block_params(scan_block)[2];
    let first_free = builder.block_params(scan_block)[3];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(capacity_i32));
    let scan_valid = builder.ins().iconst(types::I8, 1);
    builder.ins().brif(
        more,
        body_block,
        &[index, occupied_total, existing, first_free],
        result_block,
        &[occupied_total, existing, first_free, scan_valid],
    );

    builder.switch_to_block(body_block);
    let body_index = builder.block_params(body_block)[0];
    let body_total = builder.block_params(body_block)[1];
    let body_existing = builder.block_params(body_block)[2];
    let body_first_free = builder.block_params(body_block)[3];
    let occupied_value = emit_direct_array_load(
        builder,
        occupied_ref.slot,
        body_index,
        TYPE_ID_U8,
        type_table,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    let key_value = emit_direct_array_load(
        builder,
        keys_ref.slot,
        body_index,
        TYPE_ID_I32,
        type_table,
        keys_ref.storage_bytes,
        keys_ref.static_len,
        true,
    )?;
    let is_zero = builder.ins().icmp_imm(IntCC::Equal, occupied_value, 0);
    let is_one = builder.ins().icmp_imm(IntCC::Equal, occupied_value, 1);
    let occupied_valid = builder.ins().bor(is_zero, is_one);
    let key_matches = builder.ins().icmp(IntCC::Equal, key_value, target_key);
    let is_existing = builder.ins().band(is_one, key_matches);
    let no_existing_recorded = builder.ins().icmp_imm(IntCC::Equal, body_existing, -1);
    let record_existing = builder.ins().band(no_existing_recorded, is_existing);
    let next_existing = builder
        .ins()
        .select(record_existing, body_index, body_existing);
    let no_free_recorded = builder.ins().icmp_imm(IntCC::Equal, body_first_free, -1);
    let record_free = builder.ins().band(no_free_recorded, is_zero);
    let next_first_free = builder
        .ins()
        .select(record_free, body_index, body_first_free);
    let next_index = builder.ins().iadd_imm(body_index, 1);
    let next_total = builder.ins().iadd(body_total, occupied_value);
    builder.ins().brif(
        occupied_valid,
        scan_block,
        &[next_index, next_total, next_existing, next_first_free],
        invalid_occupied_block,
        &[],
    );

    builder.seal_block(invalid_occupied_block);
    builder.switch_to_block(invalid_occupied_block);
    let zero = builder.ins().iconst(types::I32, 0);
    let invalid_index = builder.ins().iconst(types::I32, -1);
    let invalid = builder.ins().iconst(types::I8, 0);
    builder
        .ins()
        .jump(result_block, &[zero, invalid_index, invalid_index, invalid]);

    builder.seal_block(scan_block);
    builder.seal_block(body_block);
    builder.seal_block(result_block);
    builder.switch_to_block(result_block);
    let scanned_total = builder.block_params(result_block)[0];
    let scanned_existing = builder.block_params(result_block)[1];
    let scanned_first_free = builder.block_params(result_block)[2];
    let scanned_valid = builder.block_params(result_block)[3];
    let count_matches = builder.ins().icmp(IntCC::Equal, scanned_total, count);
    let metadata_valid = builder.ins().band(scanned_valid, count_matches);
    Ok((count, scanned_existing, scanned_first_free, metadata_valid))
}

fn emit_typed_keyed_unique(
    builder: &mut FunctionBuilder<'_>,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    capacity: usize,
    type_table: &TypeTable,
) -> Result<Value, String> {
    let capacity_i32 = i32::try_from(capacity)
        .map_err(|_| format!("typed keyed collection capacity {capacity} exceeds i32 range"))?;
    if capacity_i32 <= 0 {
        return Err(
            "typed keyed collection uniqueness scan requires positive capacity".to_string(),
        );
    }
    let outer_block = builder.create_block();
    builder.append_block_param(outer_block, types::I32);
    builder.append_block_param(outer_block, types::I8);
    let outer_body = builder.create_block();
    builder.append_block_param(outer_body, types::I32);
    builder.append_block_param(outer_body, types::I8);
    let inner_block = builder.create_block();
    builder.append_block_param(inner_block, types::I32);
    builder.append_block_param(inner_block, types::I8);
    builder.append_block_param(inner_block, types::I32);
    builder.append_block_param(inner_block, types::I32);
    let inner_body = builder.create_block();
    builder.append_block_param(inner_body, types::I32);
    builder.append_block_param(inner_body, types::I8);
    builder.append_block_param(inner_body, types::I32);
    builder.append_block_param(inner_body, types::I32);
    let result_block = builder.create_block();
    builder.append_block_param(result_block, types::I8);

    let zero = builder.ins().iconst(types::I32, 0);
    let one = builder.ins().iconst(types::I8, 1);
    builder.ins().jump(outer_block, &[zero, one]);

    builder.switch_to_block(outer_block);
    let outer_index = builder.block_params(outer_block)[0];
    let outer_valid = builder.block_params(outer_block)[1];
    let outer_more =
        builder
            .ins()
            .icmp_imm(IntCC::SignedLessThan, outer_index, i64::from(capacity_i32));
    builder.ins().brif(
        outer_more,
        outer_body,
        &[outer_index, outer_valid],
        result_block,
        &[outer_valid],
    );

    builder.switch_to_block(outer_body);
    let body_index = builder.block_params(outer_body)[0];
    let body_valid = builder.block_params(outer_body)[1];
    let occupied_value = emit_direct_array_load(
        builder,
        occupied_ref.slot,
        body_index,
        TYPE_ID_U8,
        type_table,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    let key_value = emit_direct_array_load(
        builder,
        keys_ref.slot,
        body_index,
        TYPE_ID_I32,
        type_table,
        keys_ref.storage_bytes,
        keys_ref.static_len,
        true,
    )?;
    let occupied = builder.ins().icmp_imm(IntCC::Equal, occupied_value, 1);
    let next_outer_index = builder.ins().iadd_imm(body_index, 1);
    let inner_zero = builder.ins().iconst(types::I32, 0);
    let inner_one = builder.ins().iconst(types::I8, 1);
    builder.ins().brif(
        occupied,
        inner_block,
        &[inner_zero, inner_one, key_value, body_index],
        outer_block,
        &[next_outer_index, body_valid],
    );

    builder.switch_to_block(inner_block);
    let inner_index = builder.block_params(inner_block)[0];
    let inner_valid = builder.block_params(inner_block)[1];
    let current_key = builder.block_params(inner_block)[2];
    let current_outer_index = builder.block_params(inner_block)[3];
    let prior_exists = builder
        .ins()
        .icmp(IntCC::SignedLessThan, inner_index, current_outer_index);
    let after_inner_valid = builder.ins().band(body_valid, inner_valid);
    builder.ins().brif(
        prior_exists,
        inner_body,
        &[
            inner_index,
            after_inner_valid,
            current_key,
            current_outer_index,
        ],
        outer_block,
        &[next_outer_index, after_inner_valid],
    );

    builder.switch_to_block(inner_body);
    let prior_index = builder.block_params(inner_body)[0];
    let prior_valid = builder.block_params(inner_body)[1];
    let current_key = builder.block_params(inner_body)[2];
    let current_outer_index = builder.block_params(inner_body)[3];
    let prior_occupied_value = emit_direct_array_load(
        builder,
        occupied_ref.slot,
        prior_index,
        TYPE_ID_U8,
        type_table,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    let prior_key = emit_direct_array_load(
        builder,
        keys_ref.slot,
        prior_index,
        TYPE_ID_I32,
        type_table,
        keys_ref.storage_bytes,
        keys_ref.static_len,
        true,
    )?;
    let prior_occupied = builder
        .ins()
        .icmp_imm(IntCC::Equal, prior_occupied_value, 1);
    let same_key = builder.ins().icmp(IntCC::Equal, prior_key, current_key);
    let duplicate = builder.ins().band(prior_occupied, same_key);
    let not_duplicate = builder.ins().icmp_imm(IntCC::Equal, duplicate, 0);
    let next_valid = builder.ins().band(prior_valid, not_duplicate);
    let next_index = builder.ins().iadd_imm(prior_index, 1);
    builder.ins().jump(
        inner_block,
        &[next_index, next_valid, current_key, current_outer_index],
    );

    builder.seal_block(outer_block);
    builder.seal_block(outer_body);
    builder.seal_block(inner_block);
    builder.seal_block(inner_body);
    builder.seal_block(result_block);
    builder.switch_to_block(result_block);
    Ok(builder.block_params(result_block)[0])
}

fn emit_typed_keyed_state(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    capacity: usize,
    target_key: Value,
    type_table: &TypeTable,
) -> Result<(Value, Value, Value, Value), String> {
    let (count, existing, first_free, metadata_valid) = emit_typed_keyed_scan(
        builder,
        count_ref,
        occupied_ref,
        keys_ref,
        capacity,
        target_key,
        type_table,
    )?;
    let valid_block = builder.create_block();
    let invalid_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I32);
    builder.append_block_param(merge_block, types::I32);
    builder.append_block_param(merge_block, types::I32);
    builder.append_block_param(merge_block, types::I8);
    builder
        .ins()
        .brif(metadata_valid, valid_block, &[], invalid_block, &[]);
    builder.seal_block(invalid_block);
    builder.switch_to_block(invalid_block);
    let invalid = builder.ins().iconst(types::I8, 0);
    builder
        .ins()
        .jump(merge_block, &[count, existing, first_free, invalid]);

    builder.switch_to_block(valid_block);
    let unique_valid =
        emit_typed_keyed_unique(builder, occupied_ref, keys_ref, capacity, type_table)?;
    builder
        .ins()
        .jump(merge_block, &[count, existing, first_free, unique_valid]);

    builder.seal_block(valid_block);
    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    let metadata_valid = builder.block_params(merge_block)[3];
    emit_typed_collection_metadata_trap(builder, metadata_valid);
    Ok((
        builder.block_params(merge_block)[0],
        builder.block_params(merge_block)[1],
        builder.block_params(merge_block)[2],
        metadata_valid,
    ))
}

fn emit_typed_map_contains(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    capacity: usize,
    key: Value,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    if capacity == 0 {
        emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
        return Ok(ValueBinding {
            value: builder.ins().iconst(types::I32, 0),
            type_id: TYPE_ID_BOOL,
        });
    }
    let (_count, existing, _first_free, metadata_valid) = emit_typed_keyed_state(
        builder,
        count_ref,
        occupied_ref,
        keys_ref,
        capacity,
        key,
        type_table,
    )?;
    let has_existing = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
    let result_value = builder.ins().band(metadata_valid, has_existing);
    Ok(ValueBinding {
        value: result_value,
        type_id: TYPE_ID_BOOL,
    })
}

fn emit_typed_set_contains(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    capacity: usize,
    key: Value,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    if capacity == 0 {
        emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
        return Ok(ValueBinding {
            value: builder.ins().iconst(types::I32, 0),
            type_id: TYPE_ID_BOOL,
        });
    }
    let (_count, existing, _first_free, metadata_valid) = emit_typed_keyed_state(
        builder,
        count_ref,
        occupied_ref,
        keys_ref,
        capacity,
        key,
        type_table,
    )?;
    let has_existing = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
    let result_value = builder.ins().band(metadata_valid, has_existing);
    Ok(ValueBinding {
        value: result_value,
        type_id: TYPE_ID_BOOL,
    })
}

fn emit_typed_map_put_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    values_ref: DirectArrayStorageRef,
    count: Value,
    existing: Value,
    first_free: Value,
    key: Value,
    value: Value,
) -> Result<(), String> {
    let has_existing = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
    let existing_block = builder.create_block();
    let insert_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(has_existing, existing_block, &[], insert_block, &[]);

    builder.seal_block(existing_block);
    builder.switch_to_block(existing_block);
    emit_direct_array_store(
        builder,
        values_ref.slot,
        existing,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(insert_block);
    builder.switch_to_block(insert_block);
    let occupied_one = builder.ins().iconst(types::I32, 1);
    emit_direct_array_store(
        builder,
        occupied_ref.slot,
        first_free,
        occupied_one,
        TYPE_ID_U8,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        keys_ref.slot,
        first_free,
        key,
        TYPE_ID_I32,
        keys_ref.storage_bytes,
        keys_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        values_ref.slot,
        first_free,
        value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    builder.ins().jump(merge_block, &[]);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

fn emit_typed_map_get_fast(
    builder: &mut FunctionBuilder<'_>,
    values_ref: DirectArrayStorageRef,
    existing: Value,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    Ok(ValueBinding {
        value: emit_direct_array_load(
            builder,
            values_ref.slot,
            existing,
            TYPE_ID_I32,
            type_table,
            values_ref.storage_bytes,
            values_ref.static_len,
            true,
        )?,
        type_id: TYPE_ID_I32,
    })
}

fn emit_typed_keyed_remove_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    values_ref: Option<DirectArrayStorageRef>,
    count: Value,
    existing: Value,
) -> Result<(), String> {
    let zero = builder.ins().iconst(types::I32, 0);
    emit_direct_array_store(
        builder,
        occupied_ref.slot,
        existing,
        zero,
        TYPE_ID_U8,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        keys_ref.slot,
        existing,
        zero,
        TYPE_ID_I32,
        keys_ref.storage_bytes,
        keys_ref.static_len,
        true,
    )?;
    if let Some(values_ref) = values_ref {
        emit_direct_array_store(
            builder,
            values_ref.slot,
            existing,
            zero,
            TYPE_ID_I32,
            values_ref.storage_bytes,
            values_ref.static_len,
            true,
        )?;
    }
    let next_count = builder.ins().iadd_imm(count, -1);
    emit_direct_i32_store(builder, count_ref, next_count);
    Ok(())
}

fn emit_typed_set_add_fast(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    occupied_ref: DirectArrayStorageRef,
    keys_ref: DirectArrayStorageRef,
    count: Value,
    existing: Value,
    first_free: Value,
    key: Value,
) -> Result<(), String> {
    let has_existing = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
    let existing_block = builder.create_block();
    let insert_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(has_existing, existing_block, &[], insert_block, &[]);
    builder.seal_block(existing_block);
    builder.switch_to_block(existing_block);
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(insert_block);
    builder.switch_to_block(insert_block);
    let occupied_one = builder.ins().iconst(types::I32, 1);
    emit_direct_array_store(
        builder,
        occupied_ref.slot,
        first_free,
        occupied_one,
        TYPE_ID_U8,
        occupied_ref.storage_bytes,
        occupied_ref.static_len,
        true,
    )?;
    emit_direct_array_store(
        builder,
        keys_ref.slot,
        first_free,
        key,
        TYPE_ID_I32,
        keys_ref.storage_bytes,
        keys_ref.static_len,
        true,
    )?;
    let next_count = builder.ins().iadd_imm(count, 1);
    emit_direct_i32_store(builder, count_ref, next_count);
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

fn try_emit_typed_map_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedKeyedCallResult>, String> {
    let Some(kind) = TypedMapCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::Map,
    )? {
        return Ok(None);
    }
    let expected_arity = kind.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, occupied_ref, keys_ref, values_ref) =
        typed_keyed_storage_bindings(
            target,
            args,
            TypedCollectionKind::Map,
            values_by_name,
            runtime_call_refs,
            global_path_types,
            type_table,
        )?;
    let values_ref = values_ref.ok_or_else(|| {
        format!(
            "{target} map path '{}' has no compiler-owned values lane",
            path
        )
    })?;
    let key = emit_simple_expression(
        builder,
        &args[1],
        Some(TYPE_ID_I32),
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if key.type_id != TYPE_ID_I32 {
        return Err(format!(
            "{target} key argument must have exact i32 type, found {}",
            key.type_id
        ));
    }
    match kind {
        TypedMapCallKind::Put => {
            let value = emit_simple_expression(
                builder,
                &args[2],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::MapPut, args)
            {
                Some(TypedCollectionGuardState::Keyed {
                    count,
                    existing,
                    first_free,
                }) => emit_typed_map_put_fast(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    values_ref,
                    count,
                    existing,
                    first_free,
                    key.value,
                    value.value,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for map path '{path}' received an invalid can_put proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for map path '{path}' requires its exact direct can_put guard"
                    ));
                }
            }
            Ok(Some(TypedKeyedCallResult::Void))
        }
        TypedMapCallKind::Get => {
            let result = match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::MapGet, args)
            {
                Some(TypedCollectionGuardState::Keyed { existing, .. }) => {
                    emit_typed_map_get_fast(builder, values_ref, existing, type_table)?
                }
                Some(_) => {
                    return Err(format!(
                        "{target} for map path '{path}' received an invalid can_get proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for map path '{path}' requires its exact direct can_get guard"
                    ));
                }
            };
            Ok(Some(TypedKeyedCallResult::Value(result)))
        }
        TypedMapCallKind::Contains => {
            Ok(Some(TypedKeyedCallResult::Value(emit_typed_map_contains(
                builder,
                count_ref,
                occupied_ref,
                keys_ref,
                capacity,
                key.value,
                type_table,
            )?)))
        }
        TypedMapCallKind::Remove => {
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::MapRemove, args)
            {
                Some(TypedCollectionGuardState::Keyed {
                    count, existing, ..
                }) => emit_typed_keyed_remove_fast(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    Some(values_ref),
                    count,
                    existing,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for map path '{path}' received an invalid can_remove proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for map path '{path}' requires its exact direct can_remove guard"
                    ));
                }
            }
            Ok(Some(TypedKeyedCallResult::Void))
        }
        TypedMapCallKind::CanPut => {
            let (valid, state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                let missing = builder.ins().iconst(types::I32, -1);
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing: missing,
                        first_free: missing,
                    }),
                )
            } else {
                let (count, existing, first_free, metadata_valid) = emit_typed_keyed_state(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    capacity,
                    key.value,
                    type_table,
                )?;
                let has_existing =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
                let has_free =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, first_free, 0);
                let has_capacity = builder.ins().icmp_imm(
                    IntCC::SignedLessThan,
                    count,
                    i64::try_from(capacity).unwrap_or(i64::MAX),
                );
                let can_insert = builder.ins().band(has_free, has_capacity);
                let can_put = builder.ins().bor(has_existing, can_insert);
                (
                    builder.ins().band(metadata_valid, can_put),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing,
                        first_free,
                    }),
                )
            };
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::MapPut,
                    args,
                    state,
                );
            }
            Ok(Some(TypedKeyedCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
        TypedMapCallKind::CanGet => {
            let (valid, state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                let missing = builder.ins().iconst(types::I32, -1);
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing: missing,
                        first_free: missing,
                    }),
                )
            } else {
                let (count, existing, first_free, metadata_valid) = emit_typed_keyed_state(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    capacity,
                    key.value,
                    type_table,
                )?;
                let has_existing =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
                (
                    builder.ins().band(metadata_valid, has_existing),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing,
                        first_free,
                    }),
                )
            };
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::MapGet,
                    args,
                    state,
                );
            }
            Ok(Some(TypedKeyedCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
        TypedMapCallKind::CanRemove => {
            let (valid, state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                let missing = builder.ins().iconst(types::I32, -1);
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing: missing,
                        first_free: missing,
                    }),
                )
            } else {
                let (count, existing, first_free, metadata_valid) = emit_typed_keyed_state(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    capacity,
                    key.value,
                    type_table,
                )?;
                let has_existing =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
                (
                    builder.ins().band(metadata_valid, has_existing),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing,
                        first_free,
                    }),
                )
            };
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::MapRemove,
                    args,
                    state,
                );
            }
            Ok(Some(TypedKeyedCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
    }
}

fn try_emit_typed_set_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedKeyedCallResult>, String> {
    let Some(kind) = TypedSetCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::Set,
    )? {
        return Ok(None);
    }
    if args.len() != 2 {
        return Err(format!(
            "{target} expects 2 argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, occupied_ref, keys_ref, _values_ref) =
        typed_keyed_storage_bindings(
            target,
            args,
            TypedCollectionKind::Set,
            values_by_name,
            runtime_call_refs,
            global_path_types,
            type_table,
        )?;
    let key = emit_simple_expression(
        builder,
        &args[1],
        Some(TYPE_ID_I32),
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if key.type_id != TYPE_ID_I32 {
        return Err(format!(
            "{target} key argument must have exact i32 type, found {}",
            key.type_id
        ));
    }
    match kind {
        TypedSetCallKind::Add => {
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::SetAdd, args)
            {
                Some(TypedCollectionGuardState::Keyed {
                    count,
                    existing,
                    first_free,
                }) => emit_typed_set_add_fast(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    count,
                    existing,
                    first_free,
                    key.value,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for set path '{path}' received an invalid can_add proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for set path '{path}' requires its exact direct can_add guard"
                    ));
                }
            }
            Ok(Some(TypedKeyedCallResult::Void))
        }
        TypedSetCallKind::Contains => {
            Ok(Some(TypedKeyedCallResult::Value(emit_typed_set_contains(
                builder,
                count_ref,
                occupied_ref,
                keys_ref,
                capacity,
                key.value,
                type_table,
            )?)))
        }
        TypedSetCallKind::Remove => {
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::SetRemove, args)
            {
                Some(TypedCollectionGuardState::Keyed {
                    count, existing, ..
                }) => emit_typed_keyed_remove_fast(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    None,
                    count,
                    existing,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for set path '{path}' received an invalid can_remove proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for set path '{path}' requires its exact direct can_remove guard"
                    ));
                }
            }
            Ok(Some(TypedKeyedCallResult::Void))
        }
        TypedSetCallKind::CanAdd => {
            let (valid, state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                let missing = builder.ins().iconst(types::I32, -1);
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing: missing,
                        first_free: missing,
                    }),
                )
            } else {
                let (count, existing, first_free, metadata_valid) = emit_typed_keyed_state(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    capacity,
                    key.value,
                    type_table,
                )?;
                let has_existing =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
                let has_free =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, first_free, 0);
                let has_capacity = builder.ins().icmp_imm(
                    IntCC::SignedLessThan,
                    count,
                    i64::try_from(capacity).unwrap_or(i64::MAX),
                );
                let can_insert = builder.ins().band(has_free, has_capacity);
                let can_add = builder.ins().bor(has_existing, can_insert);
                (
                    builder.ins().band(metadata_valid, can_add),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing,
                        first_free,
                    }),
                )
            };
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::SetAdd,
                    args,
                    state,
                );
            }
            Ok(Some(TypedKeyedCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
        TypedSetCallKind::CanRemove => {
            let (valid, state) = if capacity == 0 {
                emit_typed_zero_capacity_count_trap(builder, count_ref, type_table)?;
                let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
                let missing = builder.ins().iconst(types::I32, -1);
                (
                    builder.ins().iconst(types::I32, 0),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing: missing,
                        first_free: missing,
                    }),
                )
            } else {
                let (count, existing, first_free, metadata_valid) = emit_typed_keyed_state(
                    builder,
                    count_ref,
                    occupied_ref,
                    keys_ref,
                    capacity,
                    key.value,
                    type_table,
                )?;
                let has_existing =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, existing, 0);
                (
                    builder.ins().band(metadata_valid, has_existing),
                    Some(TypedCollectionGuardState::Keyed {
                        count,
                        existing,
                        first_free,
                    }),
                )
            };
            if let Some(state) = state {
                internal_calls.capture_typed_collection_guard_proof(
                    target,
                    TypedCollectionGuardAction::SetRemove,
                    args,
                    state,
                );
            }
            Ok(Some(TypedKeyedCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
    }
}

#[derive(Clone, Copy)]
enum TypedLinearCallResult {
    Value(ValueBinding),
    Void,
}

fn typed_linear_storage_binding(
    target: &str,
    args: &[SimpleExpr],
    expected_kind: TypedCollectionKind,
    expected_lane: &str,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    global_path_types: &GlobalPathTypeMap,
    type_table: &TypeTable,
) -> Result<
    (
        String,
        usize,
        TypedCollectionDescriptor,
        DirectArrayStorageRef,
    ),
    String,
> {
    let Some(SimpleExpr::Identifier(path)) = args.first() else {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    };
    let root = path.split('.').next().unwrap_or(path);
    if values_by_name.contains_key(root) {
        return Err(format!(
            "{target} first argument must be an exact persistent typed collection path"
        ));
    }
    let type_id = global_path_types.get(path).copied().ok_or_else(|| {
        format!(
            "{target} first argument '{}' is not a persistent typed collection path",
            path
        )
    })?;
    let type_name = type_table
        .type_info(type_id)
        .map(|info| info.name.as_str())
        .ok_or_else(|| format!("{target} path '{}' has unknown type {}", path, type_id))?;
    let descriptor = type_table
        .parse_typed_collection_descriptor(type_name)?
        .ok_or_else(|| {
            format!(
                "{target} path '{}' has no compiler-owned typed collection descriptor",
                path
            )
        })?;
    if descriptor.kind != expected_kind {
        return Err(format!(
            "{target} requires persistent {} path '{}', found {}",
            expected_kind.as_str(),
            path,
            descriptor.kind_name()
        ));
    }
    let capacity = usize::try_from(descriptor.capacity).map_err(|_| {
        format!(
            "{target} {} path '{}' capacity {} does not fit the target index type",
            expected_kind.as_str(),
            path,
            descriptor.capacity
        )
    })?;
    let direct_storage = runtime_call_refs.direct_storage.as_ref().ok_or_else(|| {
        format!(
            "{target} for typed {} '{}' requires direct storage bindings",
            expected_kind.as_str(),
            path
        )
    })?;
    let lane = descriptor
        .lanes
        .iter()
        .find(|lane| lane.name == expected_lane)
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing descriptor lane '{}'",
                expected_kind.as_str(),
                path,
                expected_lane
            )
        })?;
    let expected_len = usize::try_from(lane.element_count).map_err(|_| {
        format!(
            "{target} for typed {} '{}' lane '{}' length does not fit the target index type",
            expected_kind.as_str(),
            path,
            expected_lane
        )
    })?;
    let storage = direct_storage
        .arrays
        .get(&(path.to_string(), expected_lane.to_string()))
        .copied()
        .ok_or_else(|| {
            format!(
                "{target} for typed {} '{}' is missing direct array binding '{}.{}'",
                expected_kind.as_str(),
                path,
                path,
                expected_lane
            )
        })?;
    if storage.static_len != Some(expected_len) {
        return Err(format!(
            "{target} for typed {} '{}' lane '{}' requires static length {}, found {:?}",
            expected_kind.as_str(),
            path,
            expected_lane,
            expected_len,
            storage.static_len
        ));
    }
    Ok((path.clone(), capacity, descriptor, storage))
}

#[allow(clippy::too_many_arguments)]
fn emit_typed_i32_argument(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    role: &str,
    expression: &SimpleExpr,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<ValueBinding, String> {
    let value = emit_simple_expression(
        builder,
        expression,
        Some(TYPE_ID_I32),
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if value.type_id != TYPE_ID_I32 {
        return Err(format!(
            "{target} {role} argument must have exact i32 type, found {}",
            value.type_id
        ));
    }
    Ok(value)
}

fn emit_typed_linear_clear(
    builder: &mut FunctionBuilder<'_>,
    storage: DirectArrayStorageRef,
    len: usize,
    type_id: TypeId,
) -> Result<(), String> {
    if len == 0 {
        return Ok(());
    }
    let len_i32 = i32::try_from(len)
        .map_err(|_| format!("typed collection lane length {len} exceeds i32 range"))?;
    let zero = builder.ins().iconst(types::I32, 0);
    let condition_block = builder.create_block();
    let body_block = builder.create_block();
    let exit_block = builder.create_block();
    builder.append_block_param(condition_block, types::I32);
    builder.ins().jump(condition_block, &[zero]);
    builder.switch_to_block(condition_block);
    let index = builder.block_params(condition_block)[0];
    let more = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index, i64::from(len_i32));
    builder.ins().brif(more, body_block, &[], exit_block, &[]);
    builder.seal_block(body_block);
    builder.switch_to_block(body_block);
    emit_direct_array_store(
        builder,
        storage.slot,
        index,
        zero,
        type_id,
        storage.storage_bytes,
        storage.static_len,
        true,
    )?;
    let next = builder.ins().iadd_imm(index, 1);
    builder.ins().jump(condition_block, &[next]);
    builder.seal_block(condition_block);
    builder.seal_block(exit_block);
    builder.switch_to_block(exit_block);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn try_emit_typed_grid_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedLinearCallResult>, String> {
    let expected_arity = match target {
        "can_access" | "get" => 3,
        "set" => 4,
        "capacity" | "clear" => 1,
        _ => return Ok(None),
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::Grid,
    )? {
        return Ok(None);
    }
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, descriptor, values_ref) = typed_linear_storage_binding(
        target,
        args,
        TypedCollectionKind::Grid,
        "values",
        values_by_name,
        runtime_call_refs,
        global_path_types,
        type_table,
    )?;
    if descriptor.element_type != Some(TYPE_ID_I32) || values_ref.storage_bytes != 4 {
        return Err(format!(
            "{target} requires grid path '{}' with i32 values",
            path
        ));
    }
    if target == "capacity" {
        let capacity = i32::try_from(capacity)
            .map_err(|_| format!("{target} grid path '{path}' capacity exceeds i32 range"))?;
        return Ok(Some(TypedLinearCallResult::Value(ValueBinding {
            value: builder.ins().iconst(types::I32, i64::from(capacity)),
            type_id: TYPE_ID_I32,
        })));
    }
    if target == "clear" {
        emit_typed_linear_clear(builder, values_ref, capacity, TYPE_ID_I32)?;
        return Ok(Some(TypedLinearCallResult::Void));
    }
    let x = emit_typed_i32_argument(
        builder,
        target,
        "x",
        &args[1],
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    let y = emit_typed_i32_argument(
        builder,
        target,
        "y",
        &args[2],
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    let width = descriptor.width.unwrap_or_default();
    let height = descriptor.height.unwrap_or_default();
    if target == "can_access" {
        let x_non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, x.value, 0);
        let x_below = builder
            .ins()
            .icmp_imm(IntCC::SignedLessThan, x.value, i64::from(width));
        let y_non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, y.value, 0);
        let y_below = builder
            .ins()
            .icmp_imm(IntCC::SignedLessThan, y.value, i64::from(height));
        let valid_x = builder.ins().band(x_non_negative, x_below);
        let valid_y = builder.ins().band(y_non_negative, y_below);
        let valid = builder.ins().band(valid_x, valid_y);
        let row = builder.ins().imul_imm(y.value, i64::from(width));
        let index = builder.ins().iadd(row, x.value);
        internal_calls.capture_typed_collection_guard_proof(
            target,
            TypedCollectionGuardAction::GridAccess,
            args,
            TypedCollectionGuardState::Indexed { index },
        );
        return Ok(Some(TypedLinearCallResult::Value(ValueBinding {
            value: valid,
            type_id: TYPE_ID_BOOL,
        })));
    }
    let Some(TypedCollectionGuardState::Indexed { index }) = internal_calls
        .take_typed_collection_guard_state(TypedCollectionGuardAction::GridAccess, args)
    else {
        return Err(format!(
            "{target} for grid path '{path}' requires its exact direct can_access guard"
        ));
    };
    if target == "get" {
        let value = emit_direct_array_load(
            builder,
            values_ref.slot,
            index,
            TYPE_ID_I32,
            type_table,
            values_ref.storage_bytes,
            values_ref.static_len,
            true,
        )?;
        return Ok(Some(TypedLinearCallResult::Value(ValueBinding {
            value,
            type_id: TYPE_ID_I32,
        })));
    }
    let value = emit_simple_expression(
        builder,
        &args[3],
        Some(TYPE_ID_I32),
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if value.type_id != TYPE_ID_I32 {
        return Err(format!(
            "{target} value argument must have exact i32 type, found {}",
            value.type_id
        ));
    }
    emit_direct_array_store(
        builder,
        values_ref.slot,
        index,
        value.value,
        TYPE_ID_I32,
        values_ref.storage_bytes,
        values_ref.static_len,
        true,
    )?;
    Ok(Some(TypedLinearCallResult::Void))
}

#[allow(clippy::too_many_arguments)]
fn try_emit_typed_bitset_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedLinearCallResult>, String> {
    let expected_arity = match target {
        "can_access" | "test" => 2,
        "set" => 3,
        "capacity" | "clear" => 1,
        _ => return Ok(None),
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::Bitset,
    )? {
        return Ok(None);
    }
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, descriptor, words_ref) = typed_linear_storage_binding(
        target,
        args,
        TypedCollectionKind::Bitset,
        "words",
        values_by_name,
        runtime_call_refs,
        global_path_types,
        type_table,
    )?;
    if words_ref.storage_bytes != 4 {
        return Err(format!(
            "{target} for bitset path '{path}' requires u32 word storage"
        ));
    }
    if target == "capacity" {
        let capacity = i32::try_from(capacity)
            .map_err(|_| format!("{target} bitset path '{path}' capacity exceeds i32 range"))?;
        return Ok(Some(TypedLinearCallResult::Value(ValueBinding {
            value: builder.ins().iconst(types::I32, i64::from(capacity)),
            type_id: TYPE_ID_I32,
        })));
    }
    if target == "clear" {
        let word_count = descriptor
            .lanes
            .iter()
            .find(|lane| lane.name == "words")
            .and_then(|lane| usize::try_from(lane.element_count).ok())
            .ok_or_else(|| format!("{target} bitset path '{path}' has invalid word count"))?;
        emit_typed_linear_clear(builder, words_ref, word_count, TYPE_ID_U32)?;
        return Ok(Some(TypedLinearCallResult::Void));
    }
    let index = emit_typed_i32_argument(
        builder,
        target,
        "index",
        &args[1],
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if target == "can_access" {
        let non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index.value, 0);
        let below = builder
            .ins()
            .icmp_imm(IntCC::SignedLessThan, index.value, capacity as i64);
        let valid = builder.ins().band(non_negative, below);
        internal_calls.capture_typed_collection_guard_proof(
            target,
            TypedCollectionGuardAction::BitsetAccess,
            args,
            TypedCollectionGuardState::Indexed { index: index.value },
        );
        return Ok(Some(TypedLinearCallResult::Value(ValueBinding {
            value: valid,
            type_id: TYPE_ID_BOOL,
        })));
    }
    let Some(TypedCollectionGuardState::Indexed { index }) = internal_calls
        .take_typed_collection_guard_state(TypedCollectionGuardAction::BitsetAccess, args)
    else {
        return Err(format!(
            "{target} for bitset path '{path}' requires its exact direct can_access guard"
        ));
    };
    let word_index = builder.ins().ushr_imm(index, 5);
    let bit_index = builder.ins().band_imm(index, 31);
    let one = builder.ins().iconst(types::I32, 1);
    let mask = builder.ins().ishl(one, bit_index);
    let word = emit_direct_array_load(
        builder,
        words_ref.slot,
        word_index,
        TYPE_ID_U32,
        type_table,
        words_ref.storage_bytes,
        words_ref.static_len,
        true,
    )?;
    if target == "test" {
        let selected = builder.ins().band(word, mask);
        let value = builder.ins().icmp_imm(IntCC::NotEqual, selected, 0);
        return Ok(Some(TypedLinearCallResult::Value(ValueBinding {
            value,
            type_id: TYPE_ID_BOOL,
        })));
    }
    let value = emit_simple_expression(
        builder,
        &args[2],
        Some(TYPE_ID_BOOL),
        values_by_name,
        runtime_call_refs,
        internal_calls,
        call_signatures,
        type_table,
        global_path_types,
        constant_values,
        collection_infos,
        named_struct_field_types,
        foreach_bindings,
    )?;
    if value.type_id != TYPE_ID_BOOL {
        return Err(format!(
            "{target} value argument must have exact bool type, found {}",
            value.type_id
        ));
    }
    let value_is_true = builder.ins().icmp_imm(IntCC::NotEqual, value.value, 0);
    let cleared = builder.ins().band_not(word, mask);
    let zero = builder.ins().iconst(types::I32, 0);
    let selected = builder.ins().select(value_is_true, mask, zero);
    let updated = builder.ins().bor(cleared, selected);
    let tail_bits = capacity % 32;
    let updated = if tail_bits == 0 {
        updated
    } else {
        let tail_mask = (1_i64 << tail_bits) - 1;
        let masked = builder.ins().band_imm(updated, tail_mask);
        let last_word = words_ref
            .static_len
            .and_then(|len| len.checked_sub(1))
            .ok_or_else(|| format!("set bitset path '{path}' has no physical word storage"))?;
        let is_last_word = builder
            .ins()
            .icmp_imm(IntCC::Equal, word_index, last_word as i64);
        builder.ins().select(is_last_word, masked, updated)
    };
    emit_direct_array_store(
        builder,
        words_ref.slot,
        word_index,
        updated,
        TYPE_ID_U32,
        words_ref.storage_bytes,
        words_ref.static_len,
        true,
    )?;
    Ok(Some(TypedLinearCallResult::Void))
}

fn try_emit_typed_pool_call(
    builder: &mut FunctionBuilder<'_>,
    target: &str,
    args: &[SimpleExpr],
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Option<TypedPoolCallResult>, String> {
    let Some(kind) = TypedPoolCallKind::from_target(target) else {
        return Ok(None);
    };
    if !is_typed_collection_receiver(
        args,
        values_by_name,
        global_path_types,
        type_table,
        TypedCollectionKind::Pool,
    )? {
        return Ok(None);
    }
    let expected_arity = kind.expected_arity();
    if args.len() != expected_arity {
        return Err(format!(
            "{target} expects {expected_arity} argument(s), found {}",
            args.len()
        ));
    }
    let (path, capacity, count_ref, values_ref) = typed_pool_storage_bindings(
        target,
        args,
        values_by_name,
        runtime_call_refs,
        global_path_types,
        type_table,
    )?;
    match kind {
        TypedPoolCallKind::Push => {
            let value = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if value.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} value argument must have exact i32 type, found {}",
                    value.type_id
                ));
            }
            let result = match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::PoolPush, args)
            {
                Some(TypedCollectionGuardState::Pool { count, index: None }) => {
                    emit_typed_pool_push_fast(builder, count_ref, values_ref, count, value.value)?
                }
                Some(_) => {
                    return Err(format!(
                        "{target} for pool path '{path}' received an invalid can_push proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for pool path '{path}' requires its exact direct can_push guard"
                    ));
                }
            };
            Ok(Some(TypedPoolCallResult::Value(result)))
        }
        TypedPoolCallKind::Remove => {
            let index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} index argument must have exact i32 type, found {}",
                    index.type_id
                ));
            }
            match internal_calls
                .take_typed_collection_guard_state(TypedCollectionGuardAction::PoolRemove, args)
            {
                Some(TypedCollectionGuardState::Pool {
                    count,
                    index: Some(proven_index),
                }) => emit_typed_pool_remove_fast(
                    builder,
                    count_ref,
                    values_ref,
                    count,
                    proven_index,
                    type_table,
                )?,
                Some(_) => {
                    return Err(format!(
                        "{target} for pool path '{path}' received an invalid can_remove proof"
                    ));
                }
                None => {
                    return Err(format!(
                        "{target} for pool path '{path}' requires its exact direct can_remove guard"
                    ));
                }
            }
            Ok(Some(TypedPoolCallResult::Void))
        }
        TypedPoolCallKind::Count => {
            let value = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
            let capacity_i32 = i32::try_from(capacity).map_err(|_| {
                format!("{target} pool path '{path}' capacity exceeds i32 operation range")
            })?;
            let non_negative = builder
                .ins()
                .icmp_imm(IntCC::SignedGreaterThanOrEqual, value, 0);
            let within_capacity = builder.ins().icmp_imm(
                IntCC::SignedLessThanOrEqual,
                value,
                i64::from(capacity_i32),
            );
            let metadata_valid = builder.ins().band(non_negative, within_capacity);
            emit_typed_collection_metadata_trap(builder, metadata_valid);
            Ok(Some(TypedPoolCallResult::Value(ValueBinding {
                value,
                type_id: TYPE_ID_I32,
            })))
        }
        TypedPoolCallKind::Capacity => {
            let capacity = i32::try_from(capacity).map_err(|_| {
                format!("{target} pool path '{path}' capacity exceeds i32 operation range")
            })?;
            Ok(Some(TypedPoolCallResult::Value(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(capacity)),
                type_id: TYPE_ID_I32,
            })))
        }
        TypedPoolCallKind::Clear => {
            emit_typed_pool_clear(builder, count_ref, values_ref, capacity)?;
            Ok(Some(TypedPoolCallResult::Void))
        }
        TypedPoolCallKind::CanPush => {
            let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
            let capacity_i32 = i32::try_from(capacity).map_err(|_| {
                format!("{target} pool path '{path}' capacity exceeds i32 operation range")
            })?;
            let non_negative = builder
                .ins()
                .icmp_imm(IntCC::SignedGreaterThanOrEqual, count, 0);
            let within_capacity = builder.ins().icmp_imm(
                IntCC::SignedLessThanOrEqual,
                count,
                i64::from(capacity_i32),
            );
            let metadata_valid = builder.ins().band(non_negative, within_capacity);
            emit_typed_collection_metadata_trap(builder, metadata_valid);
            let valid =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedLessThan, count, i64::from(capacity_i32));
            internal_calls.capture_typed_collection_guard_proof(
                target,
                TypedCollectionGuardAction::PoolPush,
                args,
                TypedCollectionGuardState::Pool { count, index: None },
            );
            Ok(Some(TypedPoolCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
        TypedPoolCallKind::CanRemove => {
            let index = emit_simple_expression(
                builder,
                &args[1],
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if index.type_id != TYPE_ID_I32 {
                return Err(format!(
                    "{target} index argument must have exact i32 type, found {}",
                    index.type_id
                ));
            }
            let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
            let capacity_i32 = i32::try_from(capacity).map_err(|_| {
                format!("{target} pool path '{path}' capacity exceeds i32 operation range")
            })?;
            let count_non_negative =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThanOrEqual, count, 0);
            let count_within_capacity = builder.ins().icmp_imm(
                IntCC::SignedLessThanOrEqual,
                count,
                i64::from(capacity_i32),
            );
            let count_valid = builder
                .ins()
                .band(count_non_negative, count_within_capacity);
            emit_typed_collection_metadata_trap(builder, count_valid);
            let valid = if capacity_i32 <= 0 {
                builder.ins().iconst(types::I32, 0)
            } else {
                let index_non_negative =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, index.value, 0);
                let index_below_count =
                    builder
                        .ins()
                        .icmp(IntCC::SignedLessThan, index.value, count);
                let index_below_capacity = builder.ins().icmp_imm(
                    IntCC::SignedLessThan,
                    index.value,
                    i64::from(capacity_i32),
                );
                let index_valid = builder.ins().band(index_non_negative, index_below_count);
                let index_valid = builder.ins().band(index_valid, index_below_capacity);
                index_valid
            };
            internal_calls.capture_typed_collection_guard_proof(
                target,
                TypedCollectionGuardAction::PoolRemove,
                args,
                TypedCollectionGuardState::Pool {
                    count,
                    index: Some(index.value),
                },
            );
            Ok(Some(TypedPoolCallResult::Value(ValueBinding {
                value: valid,
                type_id: TYPE_ID_BOOL,
            })))
        }
    }
}

pub(crate) fn emit_simple_expression(
    builder: &mut FunctionBuilder<'_>,
    expression: &SimpleExpr,
    expected_type: Option<TypeId>,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<ValueBinding, String> {
    match expression {
        SimpleExpr::DefaultValue(type_id) => {
            let value = match *type_id {
                TYPE_ID_F32 => builder.ins().f32const(Ieee32::with_float(0.0)),
                TYPE_ID_F64 => builder.ins().f64const(Ieee64::with_float(0.0)),
                _ => builder.ins().iconst(types::I32, 0),
            };
            Ok(ValueBinding {
                value,
                type_id: *type_id,
            })
        }
        SimpleExpr::Int(value) => {
            let literal_type = expected_type
                .filter(|type_id| type_table.is_integer(*type_id))
                .unwrap_or(TYPE_ID_I32);
            let bits = type_table.unsigned_integer_bits(literal_type);
            let value = match bits {
                Some(bits) => {
                    let maximum = if bits == 32 {
                        i64::from(u32::MAX)
                    } else {
                        (1i64 << bits) - 1
                    };
                    if *value < 0 || *value > maximum {
                        return Err(format!(
                            "integer literal {value} is outside u{bits} range 0..={maximum}"
                        ));
                    }
                    *value as u32 as i32
                }
                None => i32::try_from(*value).map_err(|_| {
                    format!("integer literal out of i32 range in expression: {value}")
                })?,
            };
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(value)),
                type_id: literal_type,
            })
        }
        SimpleExpr::Float(value) => {
            if expected_type == Some(TYPE_ID_F64) {
                Ok(ValueBinding {
                    value: builder.ins().f64const(Ieee64::with_float(*value)),
                    type_id: TYPE_ID_F64,
                })
            } else {
                Ok(ValueBinding {
                    value: builder.ins().f32const(Ieee32::with_float(*value as f32)),
                    type_id: TYPE_ID_F32,
                })
            }
        }
        SimpleExpr::Bool(value) => Ok(ValueBinding {
            value: builder
                .ins()
                .iconst(types::I32, if *value { 1_i64 } else { 0_i64 }),
            type_id: TYPE_ID_BOOL,
        }),
        SimpleExpr::StringLiteral(value) => {
            let literal_id = hash_string_literal(value);
            stasis_dynload::upsert_jit_string_literal(literal_id, value);
            let string_type_id = type_table.string_literal_type_id().unwrap_or(TYPE_ID_I32);
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(literal_id)),
                type_id: string_type_id,
            })
        }
        SimpleExpr::Condition(condition) => {
            let bool_value = emit_simple_condition(
                builder,
                condition,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let one = builder.ins().iconst(types::I32, 1_i64);
            let zero = builder.ins().iconst(types::I32, 0_i64);
            let value = builder.ins().select(bool_value, one, zero);
            Ok(ValueBinding {
                value,
                type_id: TYPE_ID_BOOL,
            })
        }
        SimpleExpr::Identifier(name) => {
            if let Some((base, suffix)) = name.split_once('.') {
                if let Some(local) = values_by_name.get(base).copied() {
                    if let Some(kind) = collection_meta_kind_from_suffix(suffix) {
                        if is_collection_handle_type(local.type_id, type_table) {
                            let base_value = builder.use_var(local.var);
                            let kind_value =
                                builder.ins().iconst(types::I32, i64::from(kind as i32));
                            let call = builder.ins().call(
                                runtime_call_refs.collection_i32_load,
                                &[base_value, kind_value],
                            );
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: TYPE_ID_I32,
                            });
                        }
                    }
                    if let Some(field_types) = named_struct_field_types.get(&local.type_id) {
                        let Some(field_type) = field_types.get(suffix).copied() else {
                            return Err(format!(
                                "unknown local struct field path '{}.{}' in current jit path",
                                base, suffix
                            ));
                        };
                        let base_hash = builder.use_var(local.var);
                        if let Some(struct_view) = local.struct_view {
                            return emit_struct_view_field_load(
                                builder,
                                runtime_call_refs,
                                type_table,
                                struct_view,
                                base_hash,
                                suffix,
                                field_type,
                            );
                        }

                        let path_hash =
                            emit_local_struct_field_path_hash(base_hash, suffix, builder);
                        if is_collection_handle_type(field_type, type_table) {
                            return Ok(ValueBinding {
                                value: path_hash,
                                type_id: field_type,
                            });
                        }
                        if is_i32_abi_compatible_type(field_type, type_table) {
                            let call = builder
                                .ins()
                                .call(runtime_call_refs.global_i32_load, &[path_hash]);
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: field_type,
                            });
                        }
                        if field_type == TYPE_ID_F32 {
                            let call = builder
                                .ins()
                                .call(runtime_call_refs.global_f32_load, &[path_hash]);
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: TYPE_ID_F32,
                            });
                        }
                        if field_type == TYPE_ID_F64 {
                            let call = builder
                                .ins()
                                .call(runtime_call_refs.global_f64_load, &[path_hash]);
                            return Ok(ValueBinding {
                                value: builder.inst_results(call)[0],
                                type_id: TYPE_ID_F64,
                            });
                        }
                        return Err(format!(
                            "unsupported local struct field type {} for '{}.{}'",
                            field_type, base, suffix
                        ));
                    }
                }
            }
            if let Some(local) = values_by_name.get(name).copied() {
                Ok(ValueBinding {
                    value: builder.use_var(local.var),
                    type_id: local.type_id,
                })
            } else if let Some((binding, suffix)) =
                resolve_foreach_binding_for_path(name, foreach_bindings)
            {
                emit_foreach_binding_load(builder, runtime_call_refs, type_table, binding, &suffix)
            } else if let Some(constant) = constant_values.get(name) {
                emit_constant_value(builder, constant)
            } else {
                if let Some(collection_path) = name.strip_suffix(".max_length") {
                    if let Some(max_length) = global_path_types
                        .get(collection_path)
                        .and_then(|type_id| type_table.fixed_collection_len(*type_id))
                    {
                        return Ok(ValueBinding {
                            value: builder.ins().iconst(types::I32, i64::from(max_length)),
                            type_id: TYPE_ID_I32,
                        });
                    }
                }
                let Some(path_type) = global_path_types.get(name).copied() else {
                    return Err(format!("unknown identifier '{}' in current jit path", name));
                };
                if named_struct_field_types.contains_key(&path_type) {
                    return Ok(ValueBinding {
                        value: builder
                            .ins()
                            .iconst(types::I32, i64::from(hash_global_path(name))),
                        type_id: path_type,
                    });
                }
                emit_global_load(builder, runtime_call_refs, type_table, name, path_type)
            }
        }
        SimpleExpr::IndexedPath {
            collection_path,
            index,
            suffix,
        } => {
            if let Some(local_collection) = values_by_name.get(collection_path).copied() {
                let index_binding = emit_simple_expression(
                    builder,
                    index,
                    Some(TYPE_ID_I32),
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                return emit_local_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_path,
                    local_collection,
                    suffix,
                    index_binding,
                );
            }
            if let Some((base, field)) = collection_path.split_once('.') {
                if let Some(local) = values_by_name.get(base).copied() {
                    if let Some(collection_type) = named_struct_field_types
                        .get(&local.type_id)
                        .and_then(|fields| fields.get(field))
                        .copied()
                    {
                        if type_table
                            .indexed_element_type_id(collection_type)
                            .is_some()
                        {
                            let index_binding = emit_simple_expression(
                                builder,
                                index,
                                Some(TYPE_ID_I32),
                                values_by_name,
                                runtime_call_refs,
                                internal_calls,
                                call_signatures,
                                type_table,
                                global_path_types,
                                constant_values,
                                collection_infos,
                                named_struct_field_types,
                                foreach_bindings,
                            )?;
                            let index_binding = normalize_index_binding(index_binding, type_table)?;
                            emit_fixed_collection_bounds_trap(
                                builder,
                                index_binding.value,
                                collection_type,
                                collection_path,
                                type_table,
                            )?;
                            let collection_hash = emit_local_struct_field_path_hash(
                                builder.use_var(local.var),
                                field,
                                builder,
                            );
                            return emit_local_indexed_collection_load_for_handle(
                                builder,
                                runtime_call_refs,
                                type_table,
                                named_struct_field_types,
                                collection_path,
                                collection_type,
                                collection_hash,
                                suffix,
                                index_binding,
                                true,
                            );
                        }
                    }
                }
            }
            let Some(collection_info) = collection_infos.get(collection_path) else {
                return Err(format!(
                    "unknown indexed collection '{}' in current jit path",
                    collection_path
                ));
            };
            let index_binding = emit_simple_expression(
                builder,
                index,
                Some(TYPE_ID_I32),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let bounds_proven =
                static_index_bounds_proven(index, collection_info.len as usize, values_by_name);
            emit_indexed_collection_load(
                builder,
                runtime_call_refs,
                type_table,
                collection_path,
                collection_info,
                suffix,
                index_binding,
                bounds_proven,
            )
        }
        SimpleExpr::Call { target, args } => {
            if let Some(result) = try_emit_typed_pool_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedPoolCallResult::Value(value) => Ok(value),
                    TypedPoolCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_queue_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedCircularCallResult::Value(value) => Ok(value),
                    TypedCircularCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_priority_queue_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedPriorityQueueCallResult::Value(value) => Ok(value),
                    TypedPriorityQueueCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_ring_buffer_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedCircularCallResult::Value(value) => Ok(value),
                    TypedCircularCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_stable_pool_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedStablePoolCallResult::Value(value) => Ok(value),
                    TypedStablePoolCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_map_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedKeyedCallResult::Value(value) => Ok(value),
                    TypedKeyedCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_set_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedKeyedCallResult::Value(value) => Ok(value),
                    TypedKeyedCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_grid_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedLinearCallResult::Value(value) => Ok(value),
                    TypedLinearCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            if let Some(result) = try_emit_typed_bitset_call(
                builder,
                target,
                args,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )? {
                return match result {
                    TypedLinearCallResult::Value(value) => Ok(value),
                    TypedLinearCallResult::Void => Err(format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )),
                };
            }
            let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
            let mut arg_types: Vec<TypeId> = Vec::with_capacity(args.len());
            let expected_params = unambiguous_call_params(target, args.len(), call_signatures);
            for (arg_index, arg) in args.iter().enumerate() {
                if let Some(struct_view) = try_emit_struct_view_value(
                    builder,
                    arg,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )? {
                    arg_values.push(struct_view.base);
                    arg_values.push(struct_view.index);
                    arg_values.push(struct_view.len);
                    arg_types.push(struct_view.type_id);
                    continue;
                }

                let binding = emit_simple_expression(
                    builder,
                    arg,
                    expected_params
                        .as_ref()
                        .and_then(|params| params.get(arg_index).copied()),
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                arg_values.push(binding.value);
                arg_types.push(binding.type_id);
            }
            if matches!(
                target.as_str(),
                "fixed32_from_i32"
                    | "fixed32_to_i32"
                    | "fixed32_mul"
                    | "fixed32_div"
                    | "fixed32_from_ratio"
            ) {
                let expected_arity = if target == "fixed32_from_i32" || target == "fixed32_to_i32" {
                    1
                } else {
                    2
                };
                if arg_values.len() != expected_arity {
                    return Err(format!(
                        "deterministic numeric intrinsic '{target}' expects {expected_arity} argument(s), found {}",
                        arg_values.len()
                    ));
                }
                if let Some(type_id) = arg_types
                    .iter()
                    .copied()
                    .find(|type_id| *type_id != TYPE_ID_I32)
                {
                    return Err(format!(
                        "deterministic numeric intrinsic '{target}' requires exact i32 arguments, found type {type_id}"
                    ));
                }
                let value = match target.as_str() {
                    "fixed32_from_i32" => builder.ins().ishl_imm(arg_values[0], 16),
                    "fixed32_to_i32" => {
                        let scale = builder.ins().iconst(types::I32, 65_536);
                        builder.ins().sdiv(arg_values[0], scale)
                    }
                    "fixed32_mul" => {
                        let lhs = builder.ins().sextend(types::I64, arg_values[0]);
                        let rhs = builder.ins().sextend(types::I64, arg_values[1]);
                        let product = builder.ins().imul(lhs, rhs);
                        let scale = builder.ins().iconst(types::I64, 65_536);
                        let scaled = builder.ins().sdiv(product, scale);
                        builder.ins().ireduce(types::I32, scaled)
                    }
                    "fixed32_div" | "fixed32_from_ratio" => {
                        let lhs = builder.ins().sextend(types::I64, arg_values[0]);
                        let numerator = builder.ins().ishl_imm(lhs, 16);
                        let denominator = builder.ins().sextend(types::I64, arg_values[1]);
                        let quotient = builder.ins().sdiv(numerator, denominator);
                        builder.ins().ireduce(types::I32, quotient)
                    }
                    _ => unreachable!(),
                };
                return Ok(ValueBinding {
                    value,
                    type_id: TYPE_ID_I32,
                });
            }
            if target == "i32_to_f32" {
                if arg_values.len() != 1 {
                    return Err(format!(
                        "math intrinsic 'i32_to_f32' expects exactly one argument, found {}",
                        arg_values.len()
                    ));
                }
                if arg_types[0] != TYPE_ID_I32 {
                    return Err(format!(
                        "math intrinsic 'i32_to_f32' requires exact i32 argument, found type {}",
                        arg_types[0]
                    ));
                }
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_from_sint(types::F32, arg_values[0]),
                    type_id: TYPE_ID_F32,
                });
            }
            if target == "f32_to_i32" {
                if arg_values.len() != 1 {
                    return Err(format!(
                        "math intrinsic 'f32_to_i32' expects exactly one argument, found {}",
                        arg_values.len()
                    ));
                }
                if arg_types[0] != TYPE_ID_F32 {
                    return Err(format!(
                        "math intrinsic 'f32_to_i32' requires f32 argument, found type {}",
                        arg_types[0]
                    ));
                }
                return Ok(ValueBinding {
                    value: builder.ins().fcvt_to_sint(types::I32, arg_values[0]),
                    type_id: TYPE_ID_I32,
                });
            }
            if (target == "sin_fast" || target == "cos_fast") && arg_values.len() != 1 {
                return Err(format!(
                    "math intrinsic '{}' expects exactly one argument, found {}",
                    target,
                    arg_values.len()
                ));
            }
            if (target == "sin_fast" || target == "cos_fast") && arg_values.len() == 1 {
                if arg_types[0] != TYPE_ID_F32 {
                    return Err(format!(
                        "math intrinsic '{}' requires f32 argument, found type {}",
                        target, arg_types[0]
                    ));
                }
                let call = if target == "sin_fast" {
                    builder
                        .ins()
                        .call(runtime_call_refs.sin_fast, &[arg_values[0]])
                } else {
                    builder
                        .ins()
                        .call(runtime_call_refs.cos_fast, &[arg_values[0]])
                };
                return Ok(ValueBinding {
                    value: builder.inst_results(call)[0],
                    type_id: TYPE_ID_F32,
                });
            }
            let signature = resolve_call_signature(
                target,
                &arg_types,
                call_signatures,
                type_table,
                named_struct_field_types,
            )?;
            if signature.extern_symbol.is_some() {
                let result = emit_extern_call_for_signature(
                    builder,
                    runtime_call_refs,
                    signature,
                    &arg_values,
                )?;
                let value = result.ok_or_else(|| {
                    format!(
                        "void call target '{}' cannot be used in value expression",
                        target
                    )
                })?;
                return Ok(ValueBinding {
                    value,
                    type_id: signature.return_type,
                });
            }
            let value = emit_internal_call_for_signature(
                builder,
                runtime_call_refs,
                internal_calls,
                signature,
                &arg_values,
                &arg_types,
                type_table,
                named_struct_field_types,
                target,
            )?
            .ok_or_else(|| {
                format!(
                    "void call target '{}' cannot be used in value expression",
                    target
                )
            })?;
            Ok(ValueBinding {
                value,
                type_id: signature.return_type,
            })
        }
        SimpleExpr::Binary { lhs, op, rhs } => {
            let child_expected = match expected_type {
                Some(TYPE_ID_F32) => Some(TYPE_ID_F32),
                Some(TYPE_ID_F64) => Some(TYPE_ID_F64),
                Some(type_id) if type_table.is_integer(type_id) => Some(type_id),
                _ => None,
            };
            let (lhs_value, rhs_value) =
                if child_expected.is_none() && matches!(lhs.as_ref(), SimpleExpr::Int(_)) {
                    let rhs_value = emit_simple_expression(
                        builder,
                        rhs,
                        None,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?;
                    let lhs_expected = type_table
                        .unsigned_integer_bits(rhs_value.type_id)
                        .is_some()
                        .then_some(rhs_value.type_id);
                    let lhs_value = emit_simple_expression(
                        builder,
                        lhs,
                        lhs_expected,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?;
                    (lhs_value, rhs_value)
                } else {
                    let lhs_value = emit_simple_expression(
                        builder,
                        lhs,
                        child_expected,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?;
                    let rhs_expected = child_expected.or_else(|| {
                        type_table
                            .unsigned_integer_bits(lhs_value.type_id)
                            .is_some()
                            .then_some(lhs_value.type_id)
                    });
                    let rhs_value = emit_simple_expression(
                        builder,
                        rhs,
                        rhs_expected,
                        values_by_name,
                        runtime_call_refs,
                        internal_calls,
                        call_signatures,
                        type_table,
                        global_path_types,
                        constant_values,
                        collection_infos,
                        named_struct_field_types,
                        foreach_bindings,
                    )?;
                    (lhs_value, rhs_value)
                };
            if is_i32_numeric_type(lhs_value.type_id, type_table)
                && is_i32_numeric_type(rhs_value.type_id, type_table)
            {
                let result_type = integer_binary_result_type(
                    expected_type,
                    lhs_value.type_id,
                    rhs_value.type_id,
                    type_table,
                );
                let unsigned = type_table.unsigned_integer_bits(result_type).is_some();
                let value = match op {
                    '+' => builder.ins().iadd(lhs_value.value, rhs_value.value),
                    '-' => builder.ins().isub(lhs_value.value, rhs_value.value),
                    '*' => builder.ins().imul(lhs_value.value, rhs_value.value),
                    '/' if unsigned => builder.ins().udiv(lhs_value.value, rhs_value.value),
                    '%' if unsigned => builder.ins().urem(lhs_value.value, rhs_value.value),
                    '/' => builder.ins().sdiv(lhs_value.value, rhs_value.value),
                    '%' => builder.ins().srem(lhs_value.value, rhs_value.value),
                    other => {
                        return Err(format!(
                            "unsupported binary operator '{other}' in expression"
                        ))
                    }
                };
                return Ok(ValueBinding {
                    value: normalize_unsigned_value(builder, value, result_type, type_table),
                    type_id: result_type,
                });
            }

            if lhs_value.type_id == TYPE_ID_F64 || rhs_value.type_id == TYPE_ID_F64 {
                let (lhs_f64, rhs_f64) =
                    coerce_numeric_operands_to_f64(builder, lhs_value, rhs_value, *op, type_table)?;
                let value = match op {
                    '+' => builder.ins().fadd(lhs_f64, rhs_f64),
                    '-' => builder.ins().fsub(lhs_f64, rhs_f64),
                    '*' => builder.ins().fmul(lhs_f64, rhs_f64),
                    '/' => builder.ins().fdiv(lhs_f64, rhs_f64),
                    '%' => {
                        return Err(
                            "unsupported '%' operator for f64 expression in current jit path"
                                .to_string(),
                        )
                    }
                    other => {
                        return Err(format!(
                            "unsupported binary operator '{other}' in expression"
                        ))
                    }
                };
                return Ok(ValueBinding {
                    value,
                    type_id: TYPE_ID_F64,
                });
            }

            let (lhs_f32, rhs_f32) =
                coerce_numeric_operands_to_f32(builder, lhs_value, rhs_value, *op, type_table)?;
            let value = match op {
                '+' => builder.ins().fadd(lhs_f32, rhs_f32),
                '-' => builder.ins().fsub(lhs_f32, rhs_f32),
                '*' => builder.ins().fmul(lhs_f32, rhs_f32),
                '/' => builder.ins().fdiv(lhs_f32, rhs_f32),
                '%' => {
                    return Err(
                        "unsupported '%' operator for f32 expression in current jit path"
                            .to_string(),
                    )
                }
                other => {
                    return Err(format!(
                        "unsupported binary operator '{other}' in expression"
                    ))
                }
            };
            Ok(ValueBinding {
                value,
                type_id: TYPE_ID_F32,
            })
        }
    }
}

pub(crate) fn coerce_numeric_operands_to_f32(
    builder: &mut FunctionBuilder<'_>,
    lhs: ValueBinding,
    rhs: ValueBinding,
    op: char,
    type_table: &TypeTable,
) -> Result<(Value, Value), String> {
    let lhs_value = if lhs.type_id == TYPE_ID_F32 {
        lhs.value
    } else if is_i32_numeric_type(lhs.type_id, type_table) {
        if type_table.unsigned_integer_bits(lhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F32, lhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F32, lhs.value)
        }
    } else {
        return Err(format!(
            "unsupported lhs type {} for '{}' expression",
            lhs.type_id, op
        ));
    };
    let rhs_value = if rhs.type_id == TYPE_ID_F32 {
        rhs.value
    } else if is_i32_numeric_type(rhs.type_id, type_table) {
        if type_table.unsigned_integer_bits(rhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F32, rhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F32, rhs.value)
        }
    } else {
        return Err(format!(
            "unsupported rhs type {} for '{}' expression",
            rhs.type_id, op
        ));
    };
    Ok((lhs_value, rhs_value))
}

pub(crate) fn coerce_numeric_operands_to_f64(
    builder: &mut FunctionBuilder<'_>,
    lhs: ValueBinding,
    rhs: ValueBinding,
    op: char,
    type_table: &TypeTable,
) -> Result<(Value, Value), String> {
    let lhs_value = if lhs.type_id == TYPE_ID_F64 {
        lhs.value
    } else if lhs.type_id == TYPE_ID_F32 {
        builder.ins().fpromote(types::F64, lhs.value)
    } else if is_i32_numeric_type(lhs.type_id, type_table) {
        if type_table.unsigned_integer_bits(lhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F64, lhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F64, lhs.value)
        }
    } else {
        return Err(format!(
            "unsupported lhs type {} for '{}' expression",
            lhs.type_id, op
        ));
    };
    let rhs_value = if rhs.type_id == TYPE_ID_F64 {
        rhs.value
    } else if rhs.type_id == TYPE_ID_F32 {
        builder.ins().fpromote(types::F64, rhs.value)
    } else if is_i32_numeric_type(rhs.type_id, type_table) {
        if type_table.unsigned_integer_bits(rhs.type_id).is_some() {
            builder.ins().fcvt_from_uint(types::F64, rhs.value)
        } else {
            builder.ins().fcvt_from_sint(types::F64, rhs.value)
        }
    } else {
        return Err(format!(
            "unsupported rhs type {} for '{}' expression",
            rhs.type_id, op
        ));
    };
    Ok((lhs_value, rhs_value))
}

pub(crate) fn emit_constant_value(
    builder: &mut FunctionBuilder<'_>,
    constant: &ConstantValue,
) -> Result<ValueBinding, String> {
    match constant {
        ConstantValue::I32 { value, type_id } => Ok(ValueBinding {
            value: builder.ins().iconst(types::I32, i64::from(*value)),
            type_id: *type_id,
        }),
        ConstantValue::F32(value) => Ok(ValueBinding {
            value: builder.ins().f32const(Ieee32::with_float(*value)),
            type_id: TYPE_ID_F32,
        }),
        ConstantValue::F64(value) => Ok(ValueBinding {
            value: builder.ins().f64const(Ieee64::with_float(*value)),
            type_id: TYPE_ID_F64,
        }),
        ConstantValue::Bool(value) => Ok(ValueBinding {
            value: builder
                .ins()
                .iconst(types::I32, if *value { 1_i64 } else { 0_i64 }),
            type_id: TYPE_ID_BOOL,
        }),
        ConstantValue::String { value, type_id } => {
            let literal_id = hash_string_literal(value);
            stasis_dynload::upsert_jit_string_literal(literal_id, value);
            Ok(ValueBinding {
                value: builder.ins().iconst(types::I32, i64::from(literal_id)),
                type_id: *type_id,
            })
        }
    }
}

pub(crate) fn resolve_foreach_binding_for_path<'a>(
    path: &str,
    foreach_bindings: &'a ForeachBindingMap,
) -> Option<(&'a ForeachBinding, String)> {
    let mut segments = path.splitn(2, '.');
    let alias = segments.next()?;
    let suffix = segments.next().unwrap_or("").to_string();
    let binding = foreach_bindings.get(alias)?;
    Some((binding, suffix))
}

pub(crate) fn build_local_foreach_collection_info(
    collection_path: &str,
    collection_type: TypeId,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<ForeachCollectionInfo, String> {
    let len = type_table
        .fixed_collection_len(collection_type)
        .ok_or_else(|| {
            format!(
                "local foreach collection '{}' requires fixed-length array type",
                collection_path
            )
        })?;
    let element_type = type_table
        .indexed_element_type_id(collection_type)
        .ok_or_else(|| {
            format!(
                "local foreach collection '{}' has unsupported type {}",
                collection_path, collection_type
            )
        })?;
    if let Some(field_types) = named_struct_field_types.get(&element_type) {
        return Ok(ForeachCollectionInfo {
            len,
            element_type: None,
            field_types: field_types.clone(),
            element_shape: type_table
                .type_info(element_type)
                .map_or_else(|| format!("type#{element_type}"), |info| info.name.clone()),
            fully_migratable: true,
        });
    }
    Ok(ForeachCollectionInfo {
        len,
        element_type: Some(element_type),
        field_types: BTreeMap::new(),
        element_shape: type_table
            .type_info(element_type)
            .map_or_else(|| format!("type#{element_type}"), |info| info.name.clone()),
        fully_migratable: true,
    })
}

pub(crate) fn emit_foreach_binding_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: &ForeachBinding,
    suffix: &str,
) -> Result<ValueBinding, String> {
    let resolved = resolve_foreach_binding_value_type(binding, suffix)?;
    let index_value = builder.use_var(binding.index_var);
    if is_u8_lane(type_table, resolved) {
        if let Some(base_ptr) = binding.u8_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let address = builder.ins().iadd(base_ptr, index_i64);
            let byte = builder.ins().load(types::I8, MemFlags::new(), address, 0);
            return Ok(ValueBinding {
                value: builder.ins().uextend(types::I32, byte),
                type_id: resolved,
            });
        }
    }
    if resolved == TYPE_ID_U16 {
        if let Some(base_ptr) = binding.u16_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 1);
            let address = builder.ins().iadd(base_ptr, byte_offset);
            let word = builder.ins().load(types::I16, MemFlags::new(), address, 0);
            return Ok(ValueBinding {
                value: builder.ins().uextend(types::I32, word),
                type_id: resolved,
            });
        }
    }
    if is_i32_abi_compatible_type(resolved, type_table) {
        if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let value = builder.ins().load(types::I32, MemFlags::new(), addr, 0);
            return Ok(ValueBinding {
                value,
                type_id: resolved,
            });
        }

        let field_hash = hash_foreach_field_suffix(suffix);
        let collection_hash =
            emit_foreach_collection_handle_value(builder, binding.collection_handle);
        let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: resolved,
        });
    }
    if resolved == TYPE_ID_F32 {
        if let Some(base_ptr) = binding.f32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let value = builder.ins().load(types::F32, MemFlags::new(), addr, 0);
            return Ok(ValueBinding {
                value,
                type_id: TYPE_ID_F32,
            });
        }

        let field_hash = hash_foreach_field_suffix(suffix);
        let collection_hash =
            emit_foreach_collection_handle_value(builder, binding.collection_handle);
        let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    if resolved == TYPE_ID_F64 {
        if let Some(base_ptr) = binding.f64_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 3);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let value = builder.ins().load(types::F64, MemFlags::new(), addr, 0);
            return Ok(ValueBinding {
                value,
                type_id: TYPE_ID_F64,
            });
        }

        let field_hash = hash_foreach_field_suffix(suffix);
        let collection_hash =
            emit_foreach_collection_handle_value(builder, binding.collection_handle);
        let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[collection_hash, field_hash_value, index_value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported foreach binding load type {} for suffix '{}'",
        resolved, suffix
    ))
}

pub(crate) fn emit_foreach_binding_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: &ForeachBinding,
    suffix: &str,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let path_type = resolve_foreach_binding_value_type(binding, suffix)?;
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "assignment type mismatch for foreach binding '{}': target type {}, expression type {}",
            suffix, path_type, rhs.type_id
        ));
    }
    let field_hash = hash_foreach_field_suffix(suffix);
    let collection_hash = emit_foreach_collection_handle_value(builder, binding.collection_handle);
    let field_hash_value = builder.ins().iconst(types::I32, i64::from(field_hash));
    let index_value = builder.use_var(binding.index_var);

    if is_i32_scalar_lane_type(path_type, type_table) {
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(
                emit_foreach_binding_load(builder, runtime_call_refs, type_table, binding, suffix)?
                    .value,
            )
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, path_type);
        if let Some(base_ptr) = binding.u8_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let addr = builder.ins().iadd(base_ptr, index_i64);
            let byte = builder.ins().ireduce(types::I8, value);
            builder.ins().store(MemFlags::new(), byte, addr, 0);
        } else if let Some(base_ptr) = binding.u16_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 1);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            let word = builder.ins().ireduce(types::I16, value);
            builder.ins().store(MemFlags::new(), word, addr, 0);
        } else if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash_value, index_value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool foreach binding '{}' only supports '=' assignment",
                suffix
            ));
        }
        if let Some(base_ptr) = binding.i32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), rhs.value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash_value, index_value, rhs.value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 foreach binding '{}'",
                    suffix
                ))
            }
        };
        if let Some(base_ptr) = binding.f32_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 2);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_f32_array_store,
                &[collection_hash, field_hash_value, index_value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_foreach_binding_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    binding,
                    suffix,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f64 foreach binding '{}'",
                    suffix
                ))
            }
        };
        if let Some(base_ptr) = binding.f64_array_base_ptrs.get(suffix).copied() {
            let index_i64 = builder.ins().uextend(types::I64, index_value);
            let byte_offset = builder.ins().ishl_imm(index_i64, 3);
            let addr = builder.ins().iadd(base_ptr, byte_offset);
            builder.ins().store(MemFlags::new(), value, addr, 0);
        } else {
            builder.ins().call(
                runtime_call_refs.global_f64_array_store,
                &[collection_hash, field_hash_value, index_value, value],
            );
        }
        return Ok(());
    }
    Err(format!(
        "unsupported foreach binding assignment type {} for suffix '{}'",
        path_type, suffix
    ))
}

pub(crate) fn resolve_foreach_binding_value_type(
    binding: &ForeachBinding,
    suffix: &str,
) -> Result<TypeId, String> {
    if suffix.is_empty() {
        if let Some(type_id) = binding.element_type {
            return Ok(type_id);
        }
        return Err("foreach binding requires field access for struct element".to_string());
    }
    binding
        .field_types
        .get(suffix)
        .copied()
        .ok_or_else(|| format!("unknown foreach field path '{}'", suffix))
}

pub(crate) fn hash_foreach_field_suffix(suffix: &str) -> i32 {
    if suffix.is_empty() {
        0
    } else {
        hash_global_path(suffix)
    }
}

pub(crate) fn emit_local_struct_field_path_hash(
    base_hash: Value,
    suffix: &str,
    builder: &mut FunctionBuilder<'_>,
) -> Value {
    let mut hash_value = base_hash;
    let dot = builder.ins().iconst(types::I32, i64::from(b'.'));
    hash_value = builder.ins().bxor(hash_value, dot);
    hash_value = builder.ins().imul_imm(hash_value, 16_777_619);
    for byte in suffix.bytes() {
        let byte_value = builder.ins().iconst(types::I32, i64::from(byte));
        hash_value = builder.ins().bxor(hash_value, byte_value);
        hash_value = builder.ins().imul_imm(hash_value, 16_777_619);
    }
    hash_value
}

pub(crate) fn emit_struct_view_field_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: StructViewBinding,
    base_hash: Value,
    suffix: &str,
    field_type: TypeId,
) -> Result<ValueBinding, String> {
    if is_collection_handle_type(field_type, type_table) {
        return Err(format!(
            "struct view field '{}' resolves to collection handle type {} which is unsupported in current jit path",
            suffix, field_type
        ));
    }
    let index_value = builder.use_var(binding.index_var);
    let direct_array = binding.known_collection_hash.and_then(|collection_hash| {
        runtime_call_refs
            .direct_storage
            .as_ref()
            .and_then(|storage| {
                storage
                    .arrays_by_hash
                    .get(&(collection_hash, hash_foreach_field_suffix(suffix)))
                    .copied()
            })
    });
    if binding.storage_kind == StructViewStorageKind::Aos {
        return emit_struct_view_field_load_for_storage(
            builder,
            runtime_call_refs,
            type_table,
            true,
            base_hash,
            index_value,
            suffix,
            field_type,
            None,
            true,
        );
    }
    if binding.storage_kind == StructViewStorageKind::Soa {
        return emit_struct_view_field_load_for_storage(
            builder,
            runtime_call_refs,
            type_table,
            false,
            base_hash,
            index_value,
            suffix,
            field_type,
            direct_array,
            binding.bounds_proven,
        );
    }
    let aos_condition = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index_value, 0);

    let aos_block = builder.create_block();
    let soa_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, clif_type_for_type_id(field_type, type_table)?);

    builder
        .ins()
        .brif(aos_condition, aos_block, &[], soa_block, &[]);

    builder.switch_to_block(aos_block);
    let aos_value = emit_struct_view_field_load_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        true,
        base_hash,
        index_value,
        suffix,
        field_type,
        None,
        true,
    )?
    .value;
    builder.ins().jump(merge_block, &[aos_value]);
    builder.seal_block(aos_block);

    builder.switch_to_block(soa_block);
    let soa_value = emit_struct_view_field_load_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        false,
        base_hash,
        index_value,
        suffix,
        field_type,
        direct_array,
        binding.bounds_proven,
    )?
    .value;
    builder.ins().jump(merge_block, &[soa_value]);
    builder.seal_block(soa_block);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    let value = builder
        .block_params(merge_block)
        .first()
        .copied()
        .ok_or_else(|| "struct view merge block missing value param".to_string())?;
    Ok(ValueBinding {
        value,
        type_id: field_type,
    })
}

fn emit_struct_view_field_load_for_storage(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    aos: bool,
    base_hash: Value,
    index_value: Value,
    suffix: &str,
    field_type: TypeId,
    direct_array: Option<DirectArrayStorageRef>,
    bounds_proven: bool,
) -> Result<ValueBinding, String> {
    let value = if aos {
        let path_hash = emit_local_struct_field_path_hash(base_hash, suffix, builder);
        if is_i32_abi_compatible_type(field_type, type_table) {
            let call = builder
                .ins()
                .call(runtime_call_refs.global_i32_load, &[path_hash]);
            builder.inst_results(call)[0]
        } else if field_type == TYPE_ID_F32 {
            let call = builder
                .ins()
                .call(runtime_call_refs.global_f32_load, &[path_hash]);
            builder.inst_results(call)[0]
        } else if field_type == TYPE_ID_F64 {
            let call = builder
                .ins()
                .call(runtime_call_refs.global_f64_load, &[path_hash]);
            builder.inst_results(call)[0]
        } else {
            return Err(format!(
                "unsupported struct view field type {} for suffix '{}'",
                field_type, suffix
            ));
        }
    } else if let Some(direct) = direct_array {
        emit_direct_array_load(
            builder,
            direct.slot,
            index_value,
            field_type,
            type_table,
            direct.storage_bytes,
            direct.static_len,
            bounds_proven,
        )?
    } else {
        let field_hash = builder
            .ins()
            .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));
        if is_i32_abi_compatible_type(field_type, type_table) {
            let call = builder.ins().call(
                runtime_call_refs.global_i32_array_load,
                &[base_hash, field_hash, index_value],
            );
            builder.inst_results(call)[0]
        } else if field_type == TYPE_ID_F32 {
            let call = builder.ins().call(
                runtime_call_refs.global_f32_array_load,
                &[base_hash, field_hash, index_value],
            );
            builder.inst_results(call)[0]
        } else if field_type == TYPE_ID_F64 {
            let call = builder.ins().call(
                runtime_call_refs.global_f64_array_load,
                &[base_hash, field_hash, index_value],
            );
            builder.inst_results(call)[0]
        } else {
            return Err(format!(
                "unsupported struct view field type {} for suffix '{}'",
                field_type, suffix
            ));
        }
    };
    Ok(ValueBinding {
        value,
        type_id: field_type,
    })
}

pub(crate) fn emit_struct_view_field_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    binding: StructViewBinding,
    base_hash: Value,
    suffix: &str,
    field_type: TypeId,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    if is_collection_handle_type(field_type, type_table) {
        return Err(format!(
            "struct view field '{}' resolves to collection handle type {} which is unsupported in current jit path",
            suffix, field_type
        ));
    }
    if !are_assignment_types_compatible(field_type, rhs.type_id, type_table) {
        return Err(format!(
            "assignment type mismatch for struct view field '{}': target type {}, expression type {}",
            suffix, field_type, rhs.type_id
        ));
    }

    let index_value = builder.use_var(binding.index_var);
    let direct_array = binding.known_collection_hash.and_then(|collection_hash| {
        runtime_call_refs
            .direct_storage
            .as_ref()
            .and_then(|storage| {
                storage
                    .arrays_by_hash
                    .get(&(collection_hash, hash_foreach_field_suffix(suffix)))
                    .copied()
            })
    });
    match binding.storage_kind {
        StructViewStorageKind::Aos => {
            return emit_struct_view_field_assignment_for_storage(
                builder,
                runtime_call_refs,
                type_table,
                true,
                base_hash,
                index_value,
                suffix,
                field_type,
                op,
                rhs,
                None,
                true,
            );
        }
        StructViewStorageKind::Soa => {
            return emit_struct_view_field_assignment_for_storage(
                builder,
                runtime_call_refs,
                type_table,
                false,
                base_hash,
                index_value,
                suffix,
                field_type,
                op,
                rhs,
                direct_array,
                binding.bounds_proven,
            );
        }
        StructViewStorageKind::Dynamic => {}
    }
    let aos_condition = builder
        .ins()
        .icmp_imm(IntCC::SignedLessThan, index_value, 0);
    let aos_block = builder.create_block();
    let soa_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(aos_condition, aos_block, &[], soa_block, &[]);

    builder.switch_to_block(aos_block);
    emit_struct_view_field_assignment_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        true,
        base_hash,
        index_value,
        suffix,
        field_type,
        op,
        rhs,
        None,
        true,
    )?;
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(aos_block);

    builder.switch_to_block(soa_block);
    emit_struct_view_field_assignment_for_storage(
        builder,
        runtime_call_refs,
        type_table,
        false,
        base_hash,
        index_value,
        suffix,
        field_type,
        op,
        rhs,
        direct_array,
        binding.bounds_proven,
    )?;
    builder.ins().jump(merge_block, &[]);
    builder.seal_block(soa_block);

    builder.seal_block(merge_block);
    builder.switch_to_block(merge_block);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_struct_view_field_assignment_for_storage(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    aos: bool,
    base_hash: Value,
    index_value: Value,
    suffix: &str,
    field_type: TypeId,
    op: AssignOp,
    rhs: ValueBinding,
    direct_array: Option<DirectArrayStorageRef>,
    bounds_proven: bool,
) -> Result<(), String> {
    let field_key = if aos {
        emit_local_struct_field_path_hash(base_hash, suffix, builder)
    } else {
        builder
            .ins()
            .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)))
    };

    if let Some(direct) = direct_array {
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(emit_direct_array_load(
                builder,
                direct.slot,
                index_value,
                field_type,
                type_table,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?)
        };
        let value = if is_i32_abi_compatible_type(field_type, type_table) {
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, field_type)
        } else {
            match op {
                AssignOp::Set => rhs.value,
                AssignOp::Add => builder
                    .ins()
                    .fadd(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Sub => builder
                    .ins()
                    .fsub(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Mul => builder
                    .ins()
                    .fmul(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Div => builder
                    .ins()
                    .fdiv(lhs.expect("compound assignment lhs"), rhs.value),
                AssignOp::Mod => {
                    return Err(format!(
                        "'%=' is unsupported for floating-point struct view field '{}'",
                        suffix
                    ))
                }
            }
        };
        return emit_direct_array_store(
            builder,
            direct.slot,
            index_value,
            value,
            field_type,
            direct.storage_bytes,
            direct.static_len,
            bounds_proven,
        );
    }

    if is_i32_scalar_lane_type(field_type, type_table) {
        let lhs = if op == AssignOp::Set {
            None
        } else if aos {
            let call = builder
                .ins()
                .call(runtime_call_refs.global_i32_load, &[field_key]);
            Some(builder.inst_results(call)[0])
        } else {
            let call = builder.ins().call(
                runtime_call_refs.global_i32_array_load,
                &[base_hash, field_key, index_value],
            );
            Some(builder.inst_results(call)[0])
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, field_type);
        if aos {
            builder
                .ins()
                .call(runtime_call_refs.global_i32_store, &[field_key, value]);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[base_hash, field_key, index_value, value],
            );
        }
        return Ok(());
    }

    if field_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool assignment only supports '=' in current jit path for struct view field '{}'",
                suffix
            ));
        }
        if aos {
            builder
                .ins()
                .call(runtime_call_refs.global_i32_store, &[field_key, rhs.value]);
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[base_hash, field_key, index_value, rhs.value],
            );
        }
        return Ok(());
    }

    let lhs = match (field_type, op) {
        (_, AssignOp::Set) => None,
        (TYPE_ID_F32, AssignOp::Mod) => {
            return Err(format!(
                "'%=' is unsupported for f32 struct view field '{}'",
                suffix
            ));
        }
        (TYPE_ID_F64, AssignOp::Mod) => {
            return Err(format!(
                "'%=' is unsupported for f64 struct view field '{}'",
                suffix
            ));
        }
        (TYPE_ID_F32, _) => {
            let call = if aos {
                builder
                    .ins()
                    .call(runtime_call_refs.global_f32_load, &[field_key])
            } else {
                builder.ins().call(
                    runtime_call_refs.global_f32_array_load,
                    &[base_hash, field_key, index_value],
                )
            };
            Some(builder.inst_results(call)[0])
        }
        (TYPE_ID_F64, _) => {
            let call = if aos {
                builder
                    .ins()
                    .call(runtime_call_refs.global_f64_load, &[field_key])
            } else {
                builder.ins().call(
                    runtime_call_refs.global_f64_array_load,
                    &[base_hash, field_key, index_value],
                )
            };
            Some(builder.inst_results(call)[0])
        }
        _ => {
            return Err(format!(
                "unsupported struct view field type {} for suffix '{}'",
                field_type, suffix
            ));
        }
    };
    let value = match op {
        AssignOp::Set => rhs.value,
        AssignOp::Add => builder
            .ins()
            .fadd(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Sub => builder
            .ins()
            .fsub(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Mul => builder
            .ins()
            .fmul(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Div => builder
            .ins()
            .fdiv(lhs.expect("compound assignment lhs"), rhs.value),
        AssignOp::Mod => unreachable!(),
    };
    if field_type == TYPE_ID_F32 {
        if aos {
            builder
                .ins()
                .call(runtime_call_refs.global_f32_store, &[field_key, value]);
        } else {
            builder.ins().call(
                runtime_call_refs.global_f32_array_store,
                &[base_hash, field_key, index_value, value],
            );
        }
    } else if aos {
        builder
            .ins()
            .call(runtime_call_refs.global_f64_store, &[field_key, value]);
    } else {
        builder.ins().call(
            runtime_call_refs.global_f64_array_store,
            &[base_hash, field_key, index_value, value],
        );
    }
    Ok(())
}

pub(crate) fn emit_foreach_collection_handle_value(
    builder: &mut FunctionBuilder<'_>,
    handle: ForeachCollectionHandle,
) -> Value {
    match handle {
        ForeachCollectionHandle::PathHash(hash) => {
            builder.ins().iconst(types::I32, i64::from(hash))
        }
        ForeachCollectionHandle::LocalVar(var) => builder.use_var(var),
    }
}

pub(crate) fn resolve_collection_value_type(
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
) -> Result<TypeId, String> {
    if suffix.is_empty() {
        if let Some(type_id) = collection_info.element_type {
            return Ok(type_id);
        }
        return Err(
            "indexed collection access requires field path for struct elements".to_string(),
        );
    }
    collection_info
        .field_types
        .get(suffix)
        .copied()
        .ok_or_else(|| format!("unknown indexed collection field path '{}'", suffix))
}

pub(crate) fn normalize_index_binding(
    index: ValueBinding,
    type_table: &TypeTable,
) -> Result<ValueBinding, String> {
    if is_i32_abi_compatible_type(index.type_id, type_table) {
        Ok(index)
    } else {
        Err(format!(
            "indexed collection access requires i32 index, found type {}",
            index.type_id
        ))
    }
}

pub(crate) fn resolve_local_collection_value_type(
    collection_type: TypeId,
    suffix: &str,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
) -> Result<TypeId, String> {
    let element_type = type_table
        .indexed_element_type_id(collection_type)
        .ok_or_else(|| {
            format!(
                "local indexed collection access is unsupported for type {}",
                collection_type
            )
        })?;
    if suffix.is_empty() {
        if named_struct_field_types.contains_key(&element_type) {
            return Err(
                "local indexed collection access requires field path for struct elements"
                    .to_string(),
            );
        }
        return Ok(element_type);
    }
    let Some(field_types) = named_struct_field_types.get(&element_type) else {
        return Err(format!(
            "local indexed collection access does not support field path '{}'",
            suffix
        ));
    };
    let field_type = field_types
        .get(suffix)
        .copied()
        .ok_or_else(|| format!("unknown local indexed collection field path '{}'", suffix))?;
    if named_struct_field_types.contains_key(&field_type)
        || type_table.indexed_element_type_id(field_type).is_some()
    {
        return Err(format!(
            "local indexed collection field path '{}' resolves to unsupported non-scalar type {}",
            suffix, field_type
        ));
    }
    Ok(field_type)
}

pub(crate) fn emit_local_indexed_collection_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    collection_name: &str,
    collection_binding: LocalBinding,
    suffix: &str,
    index_binding: ValueBinding,
) -> Result<ValueBinding, String> {
    let collection_handle = builder.use_var(collection_binding.var);
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if let Some(view) = collection_binding.struct_view {
        if !view.bounds_proven {
            let len = builder.use_var(view.len_var);
            emit_array_bounds_trap(builder, index_binding.value, len);
        }
    }
    emit_local_indexed_collection_load_for_handle(
        builder,
        runtime_call_refs,
        type_table,
        named_struct_field_types,
        collection_name,
        collection_binding.type_id,
        collection_handle,
        suffix,
        index_binding,
        collection_binding.struct_view.is_some(),
    )
}

fn emit_collection_handle_bounds_trap(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    collection_handle: Value,
    index: Value,
) {
    let max_length_kind = builder
        .ins()
        .iconst(types::I32, i64::from(CollectionMetaKind::MaxLength as i32));
    let call = builder.ins().call(
        runtime_call_refs.collection_i32_load,
        &[collection_handle, max_length_kind],
    );
    let len = builder.inst_results(call)[0];
    emit_array_bounds_trap(builder, index, len);
}

#[allow(clippy::too_many_arguments)]
fn emit_local_indexed_collection_load_for_handle(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    collection_name: &str,
    collection_type: TypeId,
    collection_handle: Value,
    suffix: &str,
    index_binding: ValueBinding,
    bounds_proven: bool,
) -> Result<ValueBinding, String> {
    let resolved = resolve_local_collection_value_type(
        collection_type,
        suffix,
        type_table,
        named_struct_field_types,
    )?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if !bounds_proven {
        emit_collection_handle_bounds_trap(
            builder,
            runtime_call_refs,
            collection_handle,
            index_binding.value,
        );
    }
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));
    if is_i32_abi_compatible_type(resolved, type_table) {
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[collection_handle, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: resolved,
        });
    }
    if resolved == TYPE_ID_F32 {
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_handle, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    if resolved == TYPE_ID_F64 {
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[collection_handle, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported local indexed collection load type {} for '{}[...].{}'",
        resolved, collection_name, suffix
    ))
}

pub(crate) fn emit_local_indexed_collection_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    collection_name: &str,
    collection_binding: LocalBinding,
    suffix: &str,
    index_binding: ValueBinding,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let collection_handle = builder.use_var(collection_binding.var);
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if let Some(view) = collection_binding.struct_view {
        if !view.bounds_proven {
            let len = builder.use_var(view.len_var);
            emit_array_bounds_trap(builder, index_binding.value, len);
        }
    }
    emit_local_indexed_collection_assignment_for_handle(
        builder,
        runtime_call_refs,
        type_table,
        named_struct_field_types,
        collection_name,
        collection_binding.type_id,
        collection_handle,
        suffix,
        index_binding,
        op,
        rhs,
        collection_binding.struct_view.is_some(),
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_local_indexed_collection_assignment_for_handle(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    collection_name: &str,
    collection_type: TypeId,
    collection_handle: Value,
    suffix: &str,
    index_binding: ValueBinding,
    op: AssignOp,
    rhs: ValueBinding,
    bounds_proven: bool,
) -> Result<(), String> {
    let path_type = resolve_local_collection_value_type(
        collection_type,
        suffix,
        type_table,
        named_struct_field_types,
    )?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if !bounds_proven {
        emit_collection_handle_bounds_trap(
            builder,
            runtime_call_refs,
            collection_handle,
            index_binding.value,
        );
    }
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "local indexed assignment type mismatch for '{}[...].{}': target type {}, expression type {}",
            collection_name, suffix, path_type, rhs.type_id
        ));
    }
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));

    if is_i32_scalar_lane_type(path_type, type_table) {
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(
                emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value,
            )
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, path_type);
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[collection_handle, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool local indexed assignment only supports '=' for '{}[...].{}'",
                collection_name, suffix
            ));
        }
        builder.ins().call(
            runtime_call_refs.global_i32_array_store,
            &[
                collection_handle,
                field_hash,
                index_binding.value,
                rhs.value,
            ],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 local indexed assignment '{}[...].{}'",
                    collection_name, suffix
                ));
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f32_array_store,
            &[collection_handle, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    if path_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_local_indexed_collection_load_for_handle(
                    builder,
                    runtime_call_refs,
                    type_table,
                    named_struct_field_types,
                    collection_name,
                    collection_type,
                    collection_handle,
                    suffix,
                    index_binding,
                    true,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f64 local indexed assignment '{}[...].{}'",
                    collection_name, suffix
                ));
            }
        };
        builder.ins().call(
            runtime_call_refs.global_f64_array_store,
            &[collection_handle, field_hash, index_binding.value, value],
        );
        return Ok(());
    }
    Err(format!(
        "unsupported local indexed collection assignment type {} for '{}[...].{}'",
        path_type, collection_name, suffix
    ))
}

fn is_u8_lane(type_table: &TypeTable, type_id: TypeId) -> bool {
    type_table
        .type_info(type_id)
        .is_some_and(|info| info.name == "u8")
}

fn emit_direct_array_load(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    index: Value,
    type_id: TypeId,
    type_table: &TypeTable,
    storage_bytes: u8,
    static_len: Option<usize>,
    bounds_proven: bool,
) -> Result<Value, String> {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    let len = if let Some(len) = static_len {
        builder.ins().iconst(types::I64, len as i64)
    } else {
        let slot = emit_direct_slot_address(builder, slot_ref);
        builder.ins().load(
            types::I64,
            MemFlags::new(),
            slot,
            stasis_dynload::JitStorageSlot::LEN_OFFSET,
        )
    };
    let index_i64 = builder.ins().sextend(types::I64, index);
    let result_type = if is_i32_abi_compatible_type(type_id, type_table) {
        types::I32
    } else if type_id == TYPE_ID_F32 {
        types::F32
    } else if type_id == TYPE_ID_F64 {
        types::F64
    } else {
        return Err(format!("unsupported direct array load type {type_id}"));
    };
    if !bounds_proven {
        let non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
        let below_len = builder.ins().icmp(IntCC::UnsignedLessThan, index_i64, len);
        let valid = builder.ins().band(non_negative, below_len);
        builder.ins().trapz(valid, TrapCode::HEAP_OUT_OF_BOUNDS);
    }
    let shift = match storage_bytes {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        other => return Err(format!("unsupported direct array element width {other}")),
    };
    let byte_offset = if shift == 0 {
        index_i64
    } else {
        builder.ins().ishl_imm(index_i64, shift)
    };
    let address = builder.ins().iadd(data, byte_offset);
    let value = if storage_bytes == 1 {
        let byte = builder.ins().load(types::I8, MemFlags::new(), address, 0);
        builder.ins().uextend(types::I32, byte)
    } else if storage_bytes == 2 {
        let word = builder.ins().load(types::I16, MemFlags::new(), address, 0);
        builder.ins().uextend(types::I32, word)
    } else {
        builder.ins().load(result_type, MemFlags::new(), address, 0)
    };
    Ok(value)
}

fn emit_direct_array_store(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    index: Value,
    value: Value,
    _type_id: TypeId,
    storage_bytes: u8,
    static_len: Option<usize>,
    bounds_proven: bool,
) -> Result<(), String> {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    let len = if let Some(len) = static_len {
        builder.ins().iconst(types::I64, len as i64)
    } else {
        let slot = emit_direct_slot_address(builder, slot_ref);
        builder.ins().load(
            types::I64,
            MemFlags::new(),
            slot,
            stasis_dynload::JitStorageSlot::LEN_OFFSET,
        )
    };
    let index_i64 = builder.ins().sextend(types::I64, index);
    if !bounds_proven {
        let non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
        let below_len = builder.ins().icmp(IntCC::UnsignedLessThan, index_i64, len);
        let valid = builder.ins().band(non_negative, below_len);
        builder.ins().trapz(valid, TrapCode::HEAP_OUT_OF_BOUNDS);
    }
    let shift = match storage_bytes {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        other => return Err(format!("unsupported direct array element width {other}")),
    };
    let byte_offset = if shift == 0 {
        index_i64
    } else {
        builder.ins().ishl_imm(index_i64, shift)
    };
    let address = builder.ins().iadd(data, byte_offset);
    let stored = if storage_bytes == 1 {
        builder.ins().ireduce(types::I8, value)
    } else if storage_bytes == 2 {
        builder.ins().ireduce(types::I16, value)
    } else {
        value
    };
    builder.ins().store(MemFlags::new(), stored, address, 0);
    Ok(())
}

fn emit_array_bounds_trap(builder: &mut FunctionBuilder<'_>, index: Value, len: Value) {
    let non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
    let below_len = builder.ins().icmp(IntCC::UnsignedLessThan, index, len);
    let valid = builder.ins().band(non_negative, below_len);
    builder.ins().trapz(valid, TrapCode::HEAP_OUT_OF_BOUNDS);
}

/// A typed collection's persistent metadata is part of the game state, not an
/// expected operation result.  `can_*` predicates may report an unavailable
/// operation, but they must not turn impossible metadata into a recoverable
/// false value.  Reuse the normal fatal bounds trap used by direct storage
/// accesses so corrupt metadata terminates the game execution path.
fn emit_typed_collection_metadata_trap(builder: &mut FunctionBuilder<'_>, metadata_valid: Value) {
    builder
        .ins()
        .trapz(metadata_valid, TrapCode::HEAP_OUT_OF_BOUNDS);
}

fn emit_typed_zero_capacity_count_trap(
    builder: &mut FunctionBuilder<'_>,
    count_ref: DirectStorageRef,
    type_table: &TypeTable,
) -> Result<(), String> {
    let count = emit_direct_scalar_load(builder, count_ref, TYPE_ID_I32, type_table)?;
    let metadata_valid = builder.ins().icmp_imm(IntCC::Equal, count, 0);
    emit_typed_collection_metadata_trap(builder, metadata_valid);
    Ok(())
}

fn emit_fixed_collection_bounds_trap(
    builder: &mut FunctionBuilder<'_>,
    index: Value,
    collection_type: TypeId,
    collection_path: &str,
    type_table: &TypeTable,
) -> Result<(), String> {
    let collection_len = type_table
        .fixed_collection_len(collection_type)
        .ok_or_else(|| {
            format!(
                "indexed receiver field '{}' has no fixed capacity",
                collection_path
            )
        })?;
    let collection_len = builder.ins().iconst(types::I32, collection_len as i64);
    emit_array_bounds_trap(builder, index, collection_len);
    Ok(())
}

fn static_index_bounds_proven(
    index: &SimpleExpr,
    collection_len: usize,
    values_by_name: &BTreeMap<String, LocalBinding>,
) -> bool {
    if let Some(value) = eval_const_i64(index) {
        return usize::try_from(value).is_ok_and(|value| value < collection_len);
    }
    match index {
        SimpleExpr::Identifier(name) => {
            values_by_name
                .get(name)
                .and_then(|binding| binding.proven_index_upper)
                == Some(collection_len)
        }
        _ => false,
    }
}

fn statement_assigns_local(statement: &SimpleStmt, name: &str) -> bool {
    match statement {
        SimpleStmt::Assign {
            target: AssignTarget::Local(target),
            ..
        }
        | SimpleStmt::Convert {
            target: AssignTarget::Local(target),
            ..
        } => target == name,
        SimpleStmt::If {
            then_statements,
            else_statements,
            ..
        } => {
            then_statements
                .iter()
                .any(|statement| statement_assigns_local(statement, name))
                || else_statements.as_ref().is_some_and(|statements| {
                    statements
                        .iter()
                        .any(|statement| statement_assigns_local(statement, name))
                })
        }
        SimpleStmt::For {
            init,
            step,
            body_statements,
            ..
        } => {
            statement_assigns_local(init, name)
                || statement_assigns_local(step, name)
                || body_statements
                    .iter()
                    .any(|statement| statement_assigns_local(statement, name))
        }
        SimpleStmt::Foreach {
            body_statements, ..
        } => body_statements
            .iter()
            .any(|statement| statement_assigns_local(statement, name)),
        _ => false,
    }
}

fn canonical_fixed_array_loop_bound(
    init: &SimpleStmt,
    condition: &SimpleCondition,
    step: &SimpleStmt,
    body_statements: &[SimpleStmt],
    collection_infos: &CollectionInfoMap,
) -> Option<(String, usize)> {
    let SimpleStmt::Assign {
        target: AssignTarget::Local(index_name),
        op: AssignOp::Set,
        expression: SimpleExpr::Int(0),
    } = init
    else {
        return None;
    };
    let SimpleCondition::Comparison {
        lhs: SimpleExpr::Identifier(condition_index),
        op: ComparisonOp::Lt,
        rhs: SimpleExpr::Identifier(max_length_path),
    } = condition
    else {
        return None;
    };
    let collection_path = max_length_path.strip_suffix(".max_length")?;
    let SimpleStmt::Assign {
        target: AssignTarget::Local(step_index),
        op: AssignOp::Set,
        expression: SimpleExpr::Binary { lhs, op: '+', rhs },
    } = step
    else {
        return None;
    };
    if condition_index != index_name
        || step_index != index_name
        || lhs.as_ref() != &SimpleExpr::Identifier(index_name.clone())
        || rhs.as_ref() != &SimpleExpr::Int(1)
        || body_statements
            .iter()
            .any(|statement| statement_assigns_local(statement, index_name))
    {
        return None;
    }
    collection_infos
        .get(collection_path)
        .map(|info| (index_name.clone(), info.len as usize))
}

pub(crate) fn emit_indexed_collection_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_path: &str,
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
    index_binding: ValueBinding,
    bounds_proven: bool,
) -> Result<ValueBinding, String> {
    let resolved = resolve_collection_value_type(collection_info, suffix)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if let Some(direct) = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| {
            bindings
                .arrays
                .get(&(collection_path.to_string(), suffix.to_string()))
        })
        .copied()
    {
        return Ok(ValueBinding {
            value: emit_direct_array_load(
                builder,
                direct.slot,
                index_binding.value,
                resolved,
                type_table,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven && direct.static_len == Some(collection_info.len as usize),
            )?,
            type_id: resolved,
        });
    }
    let collection_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(collection_path)));
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));
    if is_i32_abi_compatible_type(resolved, type_table) {
        let call = builder.ins().call(
            runtime_call_refs.global_i32_array_load,
            &[collection_hash, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: resolved,
        });
    }
    if resolved == TYPE_ID_F32 {
        let call = builder.ins().call(
            runtime_call_refs.global_f32_array_load,
            &[collection_hash, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    if resolved == TYPE_ID_F64 {
        let call = builder.ins().call(
            runtime_call_refs.global_f64_array_load,
            &[collection_hash, field_hash, index_binding.value],
        );
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported indexed collection load type {} for '{}[...].{}'",
        resolved, collection_path, suffix
    ))
}

pub(crate) fn emit_indexed_collection_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    collection_path: &str,
    collection_info: &ForeachCollectionInfo,
    suffix: &str,
    index_binding: ValueBinding,
    bounds_proven: bool,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    let path_type = resolve_collection_value_type(collection_info, suffix)?;
    let index_binding = normalize_index_binding(index_binding, type_table)?;
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "indexed assignment type mismatch for '{}[...].{}': target type {}, expression type {}",
            collection_path, suffix, path_type, rhs.type_id
        ));
    }
    let direct_slot = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| {
            bindings
                .arrays
                .get(&(collection_path.to_string(), suffix.to_string()))
        })
        .copied();
    let collection_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(collection_path)));
    let field_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_foreach_field_suffix(suffix)));

    if is_i32_scalar_lane_type(path_type, type_table) {
        let lhs = if op == AssignOp::Set {
            None
        } else {
            Some(
                emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value,
            )
        };
        let value =
            emit_integer_assignment_value(builder, lhs, rhs.value, op, type_table, path_type);
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash, index_binding.value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool indexed assignment only supports '=' for '{}[...].{}'",
                collection_path, suffix
            ));
        }
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                rhs.value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_i32_array_store,
                &[collection_hash, field_hash, index_binding.value, rhs.value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 indexed assignment '{}[...].{}'",
                    collection_path, suffix
                ))
            }
        };
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_f32_array_store,
                &[collection_hash, field_hash, index_binding.value, value],
            );
        }
        return Ok(());
    }
    if path_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs = emit_indexed_collection_load(
                    builder,
                    runtime_call_refs,
                    type_table,
                    collection_path,
                    collection_info,
                    suffix,
                    index_binding,
                    false,
                )?
                .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f64 indexed assignment '{}[...].{}'",
                    collection_path, suffix
                ))
            }
        };
        if let Some(direct) = direct_slot {
            emit_direct_array_store(
                builder,
                direct.slot,
                index_binding.value,
                value,
                path_type,
                direct.storage_bytes,
                direct.static_len,
                bounds_proven,
            )?;
        } else {
            builder.ins().call(
                runtime_call_refs.global_f64_array_store,
                &[collection_hash, field_hash, index_binding.value, value],
            );
        }
        return Ok(());
    }
    Err(format!(
        "unsupported indexed collection assignment type {} for '{}[...].{}'",
        path_type, collection_path, suffix
    ))
}

pub(crate) fn emit_global_load(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    path: &str,
    path_type: TypeId,
) -> Result<ValueBinding, String> {
    if let Some(slot_address) = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| bindings.scalars.get(path))
        .copied()
    {
        return Ok(ValueBinding {
            value: emit_direct_scalar_load(builder, slot_address, path_type, type_table)?,
            type_id: path_type,
        });
    }
    let path_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(path)));
    if is_collection_handle_type(path_type, type_table) {
        return Ok(ValueBinding {
            value: path_hash,
            type_id: path_type,
        });
    }
    if is_i32_abi_compatible_type(path_type, type_table) {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_i32_load, &[path_hash]);
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: path_type,
        });
    }
    if path_type == TYPE_ID_F32 {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_f32_load, &[path_hash]);
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F32,
        });
    }
    if path_type == TYPE_ID_F64 {
        let call = builder
            .ins()
            .call(runtime_call_refs.global_f64_load, &[path_hash]);
        return Ok(ValueBinding {
            value: builder.inst_results(call)[0],
            type_id: TYPE_ID_F64,
        });
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

pub(crate) fn emit_global_assignment(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    path: &str,
    path_type: TypeId,
    op: AssignOp,
    rhs: ValueBinding,
) -> Result<(), String> {
    if !are_assignment_types_compatible(path_type, rhs.type_id, type_table) {
        return Err(format!(
            "assignment type mismatch for global path '{}': target type {}, expression type {}",
            path, path_type, rhs.type_id
        ));
    }
    if is_collection_handle_type(path_type, type_table) {
        return Err(format!(
            "direct assignment to collection path '{}' is unsupported",
            path
        ));
    }
    if is_i32_scalar_lane_type(path_type, type_table) {
        let unsigned = type_table.unsigned_integer_bits(path_type).is_some();
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().iadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().isub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().imul(lhs, rhs.value)
            }
            AssignOp::Div if unsigned => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().udiv(lhs, rhs.value)
            }
            AssignOp::Mod if unsigned => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().urem(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().sdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().srem(lhs, rhs.value)
            }
        };
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            value,
        )?;
        return Ok(());
    }
    if path_type == TYPE_ID_BOOL {
        if op != AssignOp::Set {
            return Err(format!(
                "bool global path '{}' only supports '=' assignment",
                path
            ));
        }
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            rhs.value,
        )?;
        return Ok(());
    }
    if path_type == TYPE_ID_F32 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f32 global path '{}'",
                    path
                ))
            }
        };
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            value,
        )?;
        return Ok(());
    }
    if path_type == TYPE_ID_F64 {
        let value = match op {
            AssignOp::Set => rhs.value,
            AssignOp::Add => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fadd(lhs, rhs.value)
            }
            AssignOp::Sub => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fsub(lhs, rhs.value)
            }
            AssignOp::Mul => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fmul(lhs, rhs.value)
            }
            AssignOp::Div => {
                let lhs =
                    emit_global_load(builder, runtime_call_refs, type_table, path, path_type)?
                        .value;
                builder.ins().fdiv(lhs, rhs.value)
            }
            AssignOp::Mod => {
                return Err(format!(
                    "'%=' is unsupported for f64 global path '{}'",
                    path
                ))
            }
        };
        emit_global_scalar_store(
            builder,
            runtime_call_refs,
            type_table,
            path,
            path_type,
            value,
        )?;
        return Ok(());
    }
    Err(format!(
        "unsupported global path type {} for '{}'",
        path_type, path
    ))
}

fn emit_direct_slot_address(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
) -> Value {
    match slot_ref {
        DirectStorageRef::Absolute(address) => builder.ins().iconst(types::I64, address as i64),
        DirectStorageRef::Symbol(symbol) => builder.ins().global_value(types::I64, symbol),
    }
}

fn emit_direct_slot_data_ptr(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
) -> Value {
    match slot_ref {
        DirectStorageRef::Absolute(_) => {
            let slot = emit_direct_slot_address(builder, slot_ref);
            builder.ins().load(
                types::I64,
                MemFlags::new(),
                slot,
                stasis_dynload::JitStorageSlot::DATA_OFFSET,
            )
        }
        DirectStorageRef::Symbol(symbol) => builder.ins().global_value(types::I64, symbol),
    }
}

fn emit_bounded_direct_array_len(
    builder: &mut FunctionBuilder<'_>,
    direct: DirectArrayStorageRef,
    current_len: Value,
) -> Value {
    if direct.static_len.is_some() {
        return current_len;
    }
    let slot = emit_direct_slot_address(builder, direct.slot);
    let direct_len = builder.ins().load(
        types::I64,
        MemFlags::new(),
        slot,
        stasis_dynload::JitStorageSlot::LEN_OFFSET,
    );
    let current_len_i64 = builder.ins().uextend(types::I64, current_len);
    let direct_is_shorter =
        builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, direct_len, current_len_i64);
    let bounded_len = builder
        .ins()
        .select(direct_is_shorter, direct_len, current_len_i64);
    builder.ins().ireduce(types::I32, bounded_len)
}

fn emit_direct_scalar_load(
    builder: &mut FunctionBuilder<'_>,
    slot_ref: DirectStorageRef,
    type_id: TypeId,
    type_table: &TypeTable,
) -> Result<Value, String> {
    let data = emit_direct_slot_data_ptr(builder, slot_ref);
    if type_table.unsigned_integer_bits(type_id) == Some(8) {
        let value = builder.ins().load(types::I8, MemFlags::new(), data, 0);
        return Ok(builder.ins().uextend(types::I32, value));
    }
    if type_table.unsigned_integer_bits(type_id) == Some(16) {
        let value = builder.ins().load(types::I16, MemFlags::new(), data, 0);
        return Ok(builder.ins().uextend(types::I32, value));
    }
    let clif_type = if is_i32_abi_compatible_type(type_id, type_table) {
        types::I32
    } else if type_id == TYPE_ID_F32 {
        types::F32
    } else if type_id == TYPE_ID_F64 {
        types::F64
    } else {
        return Err(format!("unsupported direct scalar load type {type_id}"));
    };
    Ok(builder.ins().load(clif_type, MemFlags::new(), data, 0))
}

fn emit_global_scalar_store(
    builder: &mut FunctionBuilder<'_>,
    runtime_call_refs: &RuntimeCallRefs,
    type_table: &TypeTable,
    path: &str,
    type_id: TypeId,
    value: Value,
) -> Result<(), String> {
    if let Some(slot_ref) = runtime_call_refs
        .direct_storage
        .as_ref()
        .and_then(|bindings| bindings.scalars.get(path))
        .copied()
    {
        let data = emit_direct_slot_data_ptr(builder, slot_ref);
        let value = match type_table.unsigned_integer_bits(type_id) {
            Some(8) => builder.ins().ireduce(types::I8, value),
            Some(16) => builder.ins().ireduce(types::I16, value),
            _ => value,
        };
        builder.ins().store(MemFlags::new(), value, data, 0);
        return Ok(());
    }
    let path_hash = builder
        .ins()
        .iconst(types::I32, i64::from(hash_global_path(path)));
    let helper = if is_i32_abi_compatible_type(type_id, type_table) {
        runtime_call_refs.global_i32_store
    } else if type_id == TYPE_ID_F32 {
        runtime_call_refs.global_f32_store
    } else if type_id == TYPE_ID_F64 {
        runtime_call_refs.global_f64_store
    } else {
        return Err(format!("unsupported global scalar store type {type_id}"));
    };
    builder.ins().call(helper, &[path_hash, value]);
    Ok(())
}

pub(crate) fn hash_global_path(path: &str) -> i32 {
    let mut hash: u32 = 2166136261;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash as i32
}

pub(crate) fn hash_string_literal(value: &str) -> i32 {
    hash_global_path(value)
}

pub(crate) fn emit_simple_condition(
    builder: &mut FunctionBuilder<'_>,
    condition: &SimpleCondition,
    values_by_name: &BTreeMap<String, LocalBinding>,
    runtime_call_refs: &RuntimeCallRefs,
    internal_calls: &mut InternalCallMode<'_>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    global_path_types: &GlobalPathTypeMap,
    constant_values: &ConstantValueMap,
    collection_infos: &CollectionInfoMap,
    named_struct_field_types: &NamedStructFieldTypeMap,
    foreach_bindings: &ForeachBindingMap,
) -> Result<Value, String> {
    match condition {
        SimpleCondition::Comparison { lhs, op, rhs } => {
            let (lhs, rhs) = if matches!(lhs, SimpleExpr::Int(_)) {
                let rhs_value = emit_simple_expression(
                    builder,
                    rhs,
                    None,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                let lhs_expected = type_table
                    .unsigned_integer_bits(rhs_value.type_id)
                    .is_some()
                    .then_some(rhs_value.type_id);
                let lhs_value = emit_simple_expression(
                    builder,
                    lhs,
                    lhs_expected,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                (lhs_value, rhs_value)
            } else {
                let lhs_value = emit_simple_expression(
                    builder,
                    lhs,
                    None,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                let rhs_expected = type_table
                    .unsigned_integer_bits(lhs_value.type_id)
                    .is_some()
                    .then_some(lhs_value.type_id);
                let rhs_value = emit_simple_expression(
                    builder,
                    rhs,
                    rhs_expected,
                    values_by_name,
                    runtime_call_refs,
                    internal_calls,
                    call_signatures,
                    type_table,
                    global_path_types,
                    constant_values,
                    collection_infos,
                    named_struct_field_types,
                    foreach_bindings,
                )?;
                (lhs_value, rhs_value)
            };
            if is_i32_abi_compatible_type(lhs.type_id, type_table)
                && is_i32_abi_compatible_type(rhs.type_id, type_table)
            {
                let unsigned = type_table.unsigned_integer_bits(lhs.type_id).is_some()
                    || type_table.unsigned_integer_bits(rhs.type_id).is_some();
                let intcc = match op {
                    ComparisonOp::Eq => IntCC::Equal,
                    ComparisonOp::Ne => IntCC::NotEqual,
                    ComparisonOp::Lt if unsigned => IntCC::UnsignedLessThan,
                    ComparisonOp::Le if unsigned => IntCC::UnsignedLessThanOrEqual,
                    ComparisonOp::Gt if unsigned => IntCC::UnsignedGreaterThan,
                    ComparisonOp::Ge if unsigned => IntCC::UnsignedGreaterThanOrEqual,
                    ComparisonOp::Lt => IntCC::SignedLessThan,
                    ComparisonOp::Le => IntCC::SignedLessThanOrEqual,
                    ComparisonOp::Gt => IntCC::SignedGreaterThan,
                    ComparisonOp::Ge => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok(builder.ins().icmp(intcc, lhs.value, rhs.value));
            }

            let floatcc = match op {
                ComparisonOp::Eq => FloatCC::Equal,
                ComparisonOp::Ne => FloatCC::NotEqual,
                ComparisonOp::Lt => FloatCC::LessThan,
                ComparisonOp::Le => FloatCC::LessThanOrEqual,
                ComparisonOp::Gt => FloatCC::GreaterThan,
                ComparisonOp::Ge => FloatCC::GreaterThanOrEqual,
            };

            if lhs.type_id == TYPE_ID_F64 || rhs.type_id == TYPE_ID_F64 {
                let (lhs_f64, rhs_f64) =
                    coerce_numeric_operands_to_f64(builder, lhs, rhs, '?', type_table)?;
                return Ok(builder.ins().fcmp(floatcc, lhs_f64, rhs_f64));
            }

            let (lhs_f32, rhs_f32) =
                coerce_numeric_operands_to_f32(builder, lhs, rhs, '?', type_table)?;
            Ok(builder.ins().fcmp(floatcc, lhs_f32, rhs_f32))
        }
        SimpleCondition::Expr(expression) => {
            let binding = emit_simple_expression(
                builder,
                expression,
                Some(TYPE_ID_BOOL),
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            if binding.type_id == TYPE_ID_BOOL {
                return Ok(builder.ins().icmp_imm(IntCC::NotEqual, binding.value, 0));
            }
            Err(format!(
                "condition expression must be bool in current jit path; found type {} for expression {:?}",
                binding.type_id, expression
            ))
        }
        SimpleCondition::And(lhs, rhs) => {
            let lhs_value = emit_simple_condition(
                builder,
                lhs,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let rhs_block = builder.create_block();
            let false_block = builder.create_block();
            let merge_block = builder.create_block();
            let bool_type = builder.func.dfg.value_type(lhs_value);
            builder.append_block_param(merge_block, bool_type);
            builder
                .ins()
                .brif(lhs_value, rhs_block, &[], false_block, &[]);

            builder.seal_block(rhs_block);
            builder.switch_to_block(rhs_block);
            let rhs_value = emit_simple_condition(
                builder,
                rhs,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            builder.ins().jump(merge_block, &[rhs_value]);

            builder.seal_block(false_block);
            builder.switch_to_block(false_block);
            builder.ins().jump(merge_block, &[lhs_value]);

            builder.seal_block(merge_block);
            builder.switch_to_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        }
        SimpleCondition::Or(lhs, rhs) => {
            let lhs_value = emit_simple_condition(
                builder,
                lhs,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let true_block = builder.create_block();
            let rhs_block = builder.create_block();
            let merge_block = builder.create_block();
            let bool_type = builder.func.dfg.value_type(lhs_value);
            builder.append_block_param(merge_block, bool_type);
            builder
                .ins()
                .brif(lhs_value, true_block, &[], rhs_block, &[]);

            builder.seal_block(true_block);
            builder.switch_to_block(true_block);
            builder.ins().jump(merge_block, &[lhs_value]);

            builder.seal_block(rhs_block);
            builder.switch_to_block(rhs_block);
            let rhs_value = emit_simple_condition(
                builder,
                rhs,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            builder.ins().jump(merge_block, &[rhs_value]);

            builder.seal_block(merge_block);
            builder.switch_to_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        }
        SimpleCondition::Not(inner) => {
            let inner_value = emit_simple_condition(
                builder,
                inner,
                values_by_name,
                runtime_call_refs,
                internal_calls,
                call_signatures,
                type_table,
                global_path_types,
                constant_values,
                collection_infos,
                named_struct_field_types,
                foreach_bindings,
            )?;
            let true_block = builder.create_block();
            let false_block = builder.create_block();
            let merge_block = builder.create_block();
            let bool_type = builder.func.dfg.value_type(inner_value);
            builder.append_block_param(merge_block, bool_type);
            builder
                .ins()
                .brif(inner_value, true_block, &[], false_block, &[]);

            builder.seal_block(true_block);
            builder.switch_to_block(true_block);
            let false_value = emit_bool_constant(builder, false);
            builder.ins().jump(merge_block, &[false_value]);

            builder.seal_block(false_block);
            builder.switch_to_block(false_block);
            let true_value = emit_bool_constant(builder, true);
            builder.ins().jump(merge_block, &[true_value]);

            builder.seal_block(merge_block);
            builder.switch_to_block(merge_block);
            Ok(builder.block_params(merge_block)[0])
        }
    }
}

pub(crate) fn emit_bool_constant(builder: &mut FunctionBuilder<'_>, value: bool) -> Value {
    let literal = if value { 1 } else { 0 };
    let i32_value = builder.ins().iconst(types::I32, literal);
    builder.ins().icmp_imm(IntCC::NotEqual, i32_value, 0)
}

fn build_direct_runtime_call_import_ids(
    module: &mut impl Module,
    fallback: FuncId,
    linkage: RuntimeHelperLinkage<'_>,
    uses_runtime_storage: bool,
    uses_collection_runtime: bool,
    referenced_call_targets: &BTreeSet<String>,
    call_signatures: &CallSignatureMap,
    type_table: &TypeTable,
    named_struct_field_types: &NamedStructFieldTypeMap,
    debug_instrumentation: bool,
    profile_instrumentation: bool,
) -> Result<RuntimeCallImportIds, String> {
    let print_i32 = if referenced_call_targets
        .iter()
        .any(|target| matches!(target.as_str(), "print_i32" | "print_int" | "print_char"))
    {
        declare_void_call_import(module, "stasis_jit_print_i32", linkage, 1)?
    } else {
        fallback
    };
    let print_string = if referenced_call_targets.contains("print_string") {
        declare_void_call_import(module, "stasis_jit_print_string", linkage, 1)?
    } else {
        fallback
    };
    let sin_fast = if referenced_call_targets.contains("sin_fast") {
        declare_direct_f32_unary_import(module, "stasis_jit_sin_fast", linkage)?
    } else {
        fallback
    };
    let cos_fast = if referenced_call_targets.contains("cos_fast") {
        declare_direct_f32_unary_import(module, "stasis_jit_cos_fast", linkage)?
    } else {
        fallback
    };
    let referenced_extern_signatures: CallSignatureMap = call_signatures
        .iter()
        .filter(|(target, _)| referenced_call_targets.contains(*target))
        .map(|(target, signatures)| (target.clone(), signatures.clone()))
        .collect();
    macro_rules! storage_import {
        ($used:expr, $declaration:expr) => {
            if $used {
                $declaration?
            } else {
                fallback
            }
        };
    }
    Ok(RuntimeCallImportIds {
        print_i32,
        print_string,
        sin_fast,
        cos_fast,
        // Direct storage handles known standalone globals, but dynamic paths and
        // bounds fallbacks still use the runtime registry. Never alias a runtime
        // helper to the current function: their ABIs are unrelated.
        global_i32_load: storage_import!(
            uses_runtime_storage,
            declare_i32_call_import(module, "stasis_jit_global_i32_load", linkage, 1)
        ),
        global_i32_store: storage_import!(
            uses_runtime_storage,
            declare_void_call_import(module, "stasis_jit_global_i32_store", linkage, 2,)
        ),
        global_f32_load: storage_import!(
            uses_runtime_storage,
            declare_f32_global_load_import(module, "stasis_jit_global_f32_load", linkage,)
        ),
        global_f32_store: storage_import!(
            uses_runtime_storage,
            declare_f32_global_store_import(module, "stasis_jit_global_f32_store", linkage,)
        ),
        global_f64_load: storage_import!(
            uses_runtime_storage,
            declare_f64_global_load_import(module, "stasis_jit_global_f64_load", linkage,)
        ),
        global_f64_store: storage_import!(
            uses_runtime_storage,
            declare_f64_global_store_import(module, "stasis_jit_global_f64_store", linkage,)
        ),
        global_i32_array_load: storage_import!(
            uses_collection_runtime,
            declare_i32_array_load_import(module, "stasis_jit_global_i32_array_load", linkage,)
        ),
        global_i32_array_store: storage_import!(
            uses_collection_runtime,
            declare_i32_array_store_import(module, "stasis_jit_global_i32_array_store", linkage,)
        ),
        global_i32_array_ptr: storage_import!(
            uses_collection_runtime,
            declare_i32_array_ptr_import(module, "stasis_jit_global_i32_array_ptr", linkage,)
        ),
        global_f32_array_load: storage_import!(
            uses_collection_runtime,
            declare_f32_array_load_import(module, "stasis_jit_global_f32_array_load", linkage,)
        ),
        global_f32_array_store: storage_import!(
            uses_collection_runtime,
            declare_f32_array_store_import(module, "stasis_jit_global_f32_array_store", linkage,)
        ),
        global_f32_array_ptr: storage_import!(
            uses_collection_runtime,
            declare_f32_array_ptr_import(module, "stasis_jit_global_f32_array_ptr", linkage,)
        ),
        global_f64_array_load: storage_import!(
            uses_collection_runtime,
            declare_f64_array_load_import(module, "stasis_jit_global_f64_array_load", linkage,)
        ),
        global_f64_array_store: storage_import!(
            uses_collection_runtime,
            declare_f64_array_store_import(module, "stasis_jit_global_f64_array_store", linkage,)
        ),
        global_f64_array_ptr: storage_import!(
            uses_collection_runtime,
            declare_f64_array_ptr_import(module, "stasis_jit_global_f64_array_ptr", linkage,)
        ),
        collection_i32_load: storage_import!(
            uses_collection_runtime,
            declare_i32_call_import(module, "stasis_jit_collection_i32_load", linkage, 2,)
        ),
        collection_i32_store: storage_import!(
            uses_collection_runtime,
            declare_void_call_import(module, "stasis_jit_collection_i32_store", linkage, 3,)
        ),
        debug_frame_enter: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_frame_enter", linkage, 1))
            .transpose()?,
        debug_frame_leave: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_frame_leave", linkage, 1))
            .transpose()?,
        debug_statement: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_statement", linkage, 2))
            .transpose()?,
        debug_values_begin: debug_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_debug_values_begin", linkage, 0))
            .transpose()?,
        debug_value_i64: debug_instrumentation
            .then(|| declare_debug_value_i64_import(module, linkage))
            .transpose()?,
        debug_value_f64: debug_instrumentation
            .then(|| declare_debug_value_f64_import(module, linkage))
            .transpose()?,
        profile_frame_enter: profile_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_profile_frame_enter", linkage, 1))
            .transpose()?,
        profile_frame_leave: profile_instrumentation
            .then(|| declare_void_call_import(module, "stasis_jit_profile_frame_leave", linkage, 1))
            .transpose()?,
        extern_calls: declare_extern_call_imports(
            module,
            &referenced_extern_signatures,
            type_table,
            named_struct_field_types,
            linkage,
        )?,
    })
}

pub(crate) fn build_runtime_call_refs(
    module: &mut impl Module,
    imports: &RuntimeCallImportIds,
    func: &mut cranelift_codegen::ir::Function,
    direct_storage: Option<&DirectStorageBindings>,
) -> Result<RuntimeCallRefs, String> {
    let direct_storage = direct_storage
        .map(|bindings| resolve_direct_storage_refs(module, func, bindings))
        .transpose()?;
    let debug = imports
        .debug_frame_enter
        .zip(imports.debug_frame_leave)
        .zip(imports.debug_statement)
        .zip(imports.debug_values_begin)
        .zip(imports.debug_value_i64)
        .zip(imports.debug_value_f64)
        .map(
            |(((((frame_enter, frame_leave), statement), values_begin), value_i64), value_f64)| {
                DebugRuntimeRefs {
                    frame_enter: module.declare_func_in_func(frame_enter, func),
                    frame_leave: module.declare_func_in_func(frame_leave, func),
                    statement: module.declare_func_in_func(statement, func),
                    values_begin: module.declare_func_in_func(values_begin, func),
                    value_i64: module.declare_func_in_func(value_i64, func),
                    value_f64: module.declare_func_in_func(value_f64, func),
                }
            },
        );
    let profile = imports
        .profile_frame_enter
        .zip(imports.profile_frame_leave)
        .map(|(frame_enter, frame_leave)| ProfileRuntimeRefs {
            frame_enter: module.declare_func_in_func(frame_enter, func),
            frame_leave: module.declare_func_in_func(frame_leave, func),
        });
    Ok(RuntimeCallRefs {
        print_i32: module.declare_func_in_func(imports.print_i32, func),
        print_string: module.declare_func_in_func(imports.print_string, func),
        sin_fast: module.declare_func_in_func(imports.sin_fast, func),
        cos_fast: module.declare_func_in_func(imports.cos_fast, func),
        global_i32_load: module.declare_func_in_func(imports.global_i32_load, func),
        global_i32_store: module.declare_func_in_func(imports.global_i32_store, func),
        global_f32_load: module.declare_func_in_func(imports.global_f32_load, func),
        global_f32_store: module.declare_func_in_func(imports.global_f32_store, func),
        global_f64_load: module.declare_func_in_func(imports.global_f64_load, func),
        global_f64_store: module.declare_func_in_func(imports.global_f64_store, func),
        global_i32_array_load: module.declare_func_in_func(imports.global_i32_array_load, func),
        global_i32_array_store: module.declare_func_in_func(imports.global_i32_array_store, func),
        global_i32_array_ptr: module.declare_func_in_func(imports.global_i32_array_ptr, func),
        global_f32_array_load: module.declare_func_in_func(imports.global_f32_array_load, func),
        global_f32_array_store: module.declare_func_in_func(imports.global_f32_array_store, func),
        global_f32_array_ptr: module.declare_func_in_func(imports.global_f32_array_ptr, func),
        global_f64_array_load: module.declare_func_in_func(imports.global_f64_array_load, func),
        global_f64_array_store: module.declare_func_in_func(imports.global_f64_array_store, func),
        global_f64_array_ptr: module.declare_func_in_func(imports.global_f64_array_ptr, func),
        collection_i32_load: module.declare_func_in_func(imports.collection_i32_load, func),
        collection_i32_store: module.declare_func_in_func(imports.collection_i32_store, func),
        debug,
        profile,
        extern_calls: imports
            .extern_calls
            .iter()
            .map(|(key, id)| (key.clone(), module.declare_func_in_func(*id, func)))
            .collect(),
        direct_storage,
    })
}

fn resolve_direct_storage_refs(
    module: &mut impl Module,
    func: &mut cranelift_codegen::ir::Function,
    bindings: &DirectStorageBindings,
) -> Result<DirectStorageRefs, String> {
    fn resolve(
        module: &mut impl Module,
        func: &mut cranelift_codegen::ir::Function,
        binding: &DirectStorageBinding,
    ) -> Result<DirectStorageRef, String> {
        match binding {
            DirectStorageBinding::Absolute(address) => Ok(DirectStorageRef::Absolute(*address)),
            DirectStorageBinding::Symbol(symbol) => {
                let data_id = module
                    .declare_data(symbol, Linkage::Import, true, false)
                    .map_err(|error| {
                        format!("failed to declare direct storage symbol '{symbol}': {error}")
                    })?;
                Ok(DirectStorageRef::Symbol(
                    module.declare_data_in_func(data_id, func),
                ))
            }
        }
    }

    Ok(DirectStorageRefs {
        scalars: bindings
            .scalars
            .iter()
            .map(|(path, binding)| Ok((path.clone(), resolve(module, func, binding)?)))
            .collect::<Result<_, String>>()?,
        arrays: bindings
            .arrays
            .iter()
            .map(|(key, binding)| {
                Ok((
                    key.clone(),
                    DirectArrayStorageRef {
                        slot: resolve(module, func, &binding.slot)?,
                        storage_bytes: binding.storage_bytes,
                        static_len: binding.static_len,
                    },
                ))
            })
            .collect::<Result<_, String>>()?,
        arrays_by_hash: bindings
            .arrays
            .iter()
            .map(|((path, field), binding)| {
                Ok((
                    (hash_global_path(path), hash_foreach_field_suffix(field)),
                    DirectArrayStorageRef {
                        slot: resolve(module, func, &binding.slot)?,
                        storage_bytes: binding.storage_bytes,
                        static_len: binding.static_len,
                    },
                ))
            })
            .collect::<Result<_, String>>()?,
    })
}
