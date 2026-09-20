use std::collections::HashMap;

pub type TypeId = u16;
pub const TYPE_ID_VOID: TypeId = 0;
pub const TYPE_ID_I32: TypeId = 1;
pub const TYPE_ID_F32: TypeId = 2;
pub const TYPE_ID_BOOL: TypeId = 3;
pub const TYPE_ID_F64: TypeId = 4;
pub const TYPE_ID_U8: TypeId = 5;
pub const TYPE_ID_U16: TypeId = 6;
pub const TYPE_ID_U32: TypeId = 7;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericArgument {
    Type(TypeId),
    I32(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BuiltinType {
    Void,
    I32,
    F32,
    Bool,
    F64,
    U8,
    U16,
    U32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TypeKey {
    Builtin(BuiltinType),
    Named(String),
    InstantiatedNominal {
        definition: String,
        arguments: Vec<GenericArgument>,
    },
    /// A compiler-owned collection application. This is intentionally a
    /// distinct key from both `Named` and `InstantiatedNominal`: collection
    /// identity includes the descriptor's dimensions and lane schema, even
    /// while the legacy public category remains source-ABI compatible for
    /// this metadata-only slice.
    TypedCollection {
        kind: TypedCollectionKind,
        element_type: Option<TypeId>,
        key_type: Option<TypeId>,
        value_type: Option<TypeId>,
        capacity: u32,
        width: Option<u32>,
        height: Option<u32>,
        lanes: Vec<TypedCollectionLane>,
    },
    ArrayFixed {
        element: TypeId,
        max_len: u32,
    },
    ArrayView {
        element: TypeId,
    },
    AsciiFixed {
        max_len: u32,
    },
    AsciiView,
    Utf8Fixed {
        max_len: u32,
    },
    Utf8View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCategory {
    Builtin,
    Named,
    ArrayFixed,
    ArrayView,
    AsciiFixed,
    AsciiView,
    Utf8Fixed,
    Utf8View,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeLayout {
    pub header_i32_words: u8,
    pub payload_size_bytes: Option<u32>,
    pub static_size_bytes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub category: TypeCategory,
    pub layout: TypeLayout,
}

/// Compiler-owned fixed-capacity collection applications.
///
/// These names are deliberately distinct from ordinary source-defined generic
/// structs. A collection descriptor owns the storage shape and lane order; it
/// is not a second spelling for an array/count struct in the stdlib.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TypedCollectionKind {
    Pool,
    StablePool,
    Queue,
    RingBuffer,
    Map,
    Set,
    PriorityQueue,
    Grid,
    Bitset,
}

impl TypedCollectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pool => "pool",
            Self::StablePool => "stable_pool",
            Self::Queue => "queue",
            Self::RingBuffer => "ring_buffer",
            Self::Map => "map",
            Self::Set => "set",
            Self::PriorityQueue => "priority_queue",
            Self::Grid => "grid",
            Self::Bitset => "bitset",
        }
    }
}

/// One statically laid out structure-of-arrays lane in a typed collection.
///
/// `element_count` is one for metadata lanes and the fixed lane capacity for
/// payload lanes.  `offset_bytes` and `byte_size` are measured in the shared
/// native state representation, after per-lane alignment has been applied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedCollectionLane {
    pub name: String,
    pub type_id: TypeId,
    pub element_count: u64,
    pub offset_bytes: u64,
    pub byte_size: u64,
    pub alignment_bytes: u64,
}

/// Frontend descriptor for one compiler-owned typed collection application.
///
/// This first implementation slice accepts scalar lanes.  Flat scalar-field
/// structs can be added later by reusing the existing explicit field-wise copy
/// contract; this descriptor never implies an aggregate copy or return ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedCollectionDescriptor {
    pub kind: TypedCollectionKind,
    pub element_type: Option<TypeId>,
    pub key_type: Option<TypeId>,
    pub value_type: Option<TypeId>,
    pub capacity: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub lanes: Vec<TypedCollectionLane>,
    pub static_size_bytes: u64,
}

impl TypedCollectionDescriptor {
    pub fn kind_name(&self) -> &'static str {
        self.kind.as_str()
    }

    /// Return a canonical type spelling for identity and layout diagnostics.
    pub fn canonical_type_name(&self, type_table: &TypeTable) -> String {
        let type_name = |type_id: Option<TypeId>| {
            type_id
                .and_then(|id| type_table.type_info(id))
                .map_or_else(|| "?".to_string(), |info| info.name.clone())
        };
        match self.kind {
            TypedCollectionKind::Pool
            | TypedCollectionKind::StablePool
            | TypedCollectionKind::Queue
            | TypedCollectionKind::RingBuffer
            | TypedCollectionKind::PriorityQueue => format!(
                "{}<{}, {}>",
                self.kind_name(),
                type_name(self.element_type),
                self.capacity
            ),
            TypedCollectionKind::Map => format!(
                "map<{}, {}, {}>",
                type_name(self.key_type),
                type_name(self.value_type),
                self.capacity
            ),
            TypedCollectionKind::Set => {
                format!("set<{}, {}>", type_name(self.key_type), self.capacity)
            }
            TypedCollectionKind::Grid => format!(
                "grid<{}, {}, {}>",
                type_name(self.element_type),
                self.width.unwrap_or_default(),
                self.height.unwrap_or_default()
            ),
            TypedCollectionKind::Bitset => format!("bitset<{}>", self.capacity),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeTable {
    types: Vec<TypeInfo>,
    type_keys: Vec<TypeKey>,
    by_key: HashMap<TypeKey, TypeId>,
}

impl TypeTable {
    pub fn new() -> Self {
        let mut table = Self {
            types: Vec::new(),
            type_keys: Vec::new(),
            by_key: HashMap::new(),
        };
        table.intern_builtin("void", BuiltinType::Void, 0);
        table.intern_builtin("i32", BuiltinType::I32, 4);
        table.intern_builtin("f32", BuiltinType::F32, 4);
        table.intern_builtin("bool", BuiltinType::Bool, 1);
        // Keep existing builtin ids stable; append new builtins at the end.
        table.intern_builtin("f64", BuiltinType::F64, 8);
        table.intern_builtin("u8", BuiltinType::U8, 1);
        table.intern_builtin("u16", BuiltinType::U16, 2);
        table.intern_builtin("u32", BuiltinType::U32, 4);
        table
    }

    pub fn resolve(&self, type_name: &str) -> Option<TypeId> {
        self.resolve_existing(type_name.trim())
    }

    pub fn resolve_or_intern(&mut self, type_name: &str) -> Result<TypeId, String> {
        self.resolve_or_intern_inner(type_name.trim())
    }

    /// Parse and validate a compiler-owned typed collection application.
    ///
    /// `Ok(None)` is returned for ordinary source-defined types, including
    /// ordinary generic applications.  This keeps existing generic behavior
    /// unchanged until the shared compiler lowering consumes the descriptor.
    /// Recognized collection names are validated eagerly, so malformed
    /// capacity, key, and payload arguments cannot silently become nominal
    /// user types. Legacy trailing overflow-policy arguments are rejected with
    /// an actionable migration diagnostic rather than normalized.
    pub fn parse_typed_collection_descriptor(
        &self,
        type_name: &str,
    ) -> Result<Option<TypedCollectionDescriptor>, String> {
        let trimmed = type_name.trim();
        let Some(open) = trimmed.find('<') else {
            return Ok(None);
        };
        let kind_name = trimmed[..open].trim();
        // Do not validate arbitrary source-defined generic syntax here.  The
        // ordinary generic elaborator owns that grammar, and applications such
        // as `Buffer<i32, 4>[2]` must remain outside this built-in recognizer.
        if parse_typed_collection_kind(kind_name).is_none() {
            return Ok(None);
        }
        let Some((kind_name, arguments)) = split_type_application(trimmed)? else {
            return Ok(None);
        };
        let Some(kind) = parse_typed_collection_kind(kind_name) else {
            return Ok(None);
        };

        let expected_arity = typed_collection_arity(kind);
        if arguments.len() == expected_arity + 1
            && is_legacy_typed_collection_policy(arguments[expected_arity])
        {
            return Err(legacy_typed_collection_policy_diagnostic(
                trimmed,
                kind,
                &arguments,
                expected_arity,
            ));
        }
        if arguments.len() != expected_arity {
            return Err(format!(
                "typed collection '{}' expects {} arguments, got {}",
                kind.as_str(),
                expected_arity,
                arguments.len()
            ));
        }

        let mut descriptor = TypedCollectionDescriptor {
            kind,
            element_type: None,
            key_type: None,
            value_type: None,
            capacity: 0,
            width: None,
            height: None,
            lanes: Vec::new(),
            static_size_bytes: 0,
        };

        match kind {
            TypedCollectionKind::Pool
            | TypedCollectionKind::StablePool
            | TypedCollectionKind::Queue
            | TypedCollectionKind::RingBuffer
            | TypedCollectionKind::PriorityQueue => {
                descriptor.element_type =
                    Some(self.resolve_typed_scalar_argument(kind, "element", arguments[0])?);
                descriptor.capacity = parse_typed_capacity(kind, arguments[1], "capacity")?;
            }
            TypedCollectionKind::Map => {
                descriptor.key_type = Some(self.resolve_typed_key_argument(kind, arguments[0])?);
                descriptor.value_type =
                    Some(self.resolve_typed_scalar_argument(kind, "value", arguments[1])?);
                descriptor.capacity = parse_typed_capacity(kind, arguments[2], "capacity")?;
            }
            TypedCollectionKind::Set => {
                descriptor.key_type = Some(self.resolve_typed_key_argument(kind, arguments[0])?);
                descriptor.capacity = parse_typed_capacity(kind, arguments[1], "capacity")?;
            }
            TypedCollectionKind::Grid => {
                descriptor.element_type =
                    Some(self.resolve_typed_scalar_argument(kind, "element", arguments[0])?);
                let width = parse_typed_capacity(kind, arguments[1], "width")?;
                let height = parse_typed_capacity(kind, arguments[2], "height")?;
                let cells = u64::from(width)
                    .checked_mul(u64::from(height))
                    .ok_or_else(|| {
                        format!(
                            "typed collection '{}' width*height overflows checked layout arithmetic",
                            kind.as_str()
                        )
                    })?;
                if cells > u64::from(i32::MAX as u32) {
                    return Err(format!(
                        "typed collection '{}' width*height exceeds i32 runtime index capacity",
                        kind.as_str()
                    ));
                }
                descriptor.capacity = cells as u32;
                descriptor.width = Some(width);
                descriptor.height = Some(height);
            }
            TypedCollectionKind::Bitset => {
                descriptor.capacity = parse_typed_capacity(kind, arguments[0], "capacity")?;
            }
        }

        descriptor.lanes = build_typed_collection_lanes(self, &descriptor)?;
        descriptor.static_size_bytes = typed_collection_total_size(&descriptor.lanes)?;
        if descriptor.static_size_bytes > u64::from(u32::MAX) {
            return Err(format!(
                "typed collection '{}' static size {} exceeds u32 layout representation",
                kind.as_str(),
                descriptor.static_size_bytes
            ));
        }
        Ok(Some(descriptor))
    }

    fn resolve_typed_scalar_argument(
        &self,
        kind: TypedCollectionKind,
        role: &str,
        type_name: &str,
    ) -> Result<TypeId, String> {
        let type_id = self.resolve(type_name).ok_or_else(|| {
            format!(
                "typed collection '{}' {} type '{}' is not a supported scalar",
                kind.as_str(),
                role,
                type_name.trim()
            )
        })?;
        if !is_typed_scalar_type(type_id) {
            return Err(format!(
                "typed collection '{}' {} type '{}' must be a scalar lane",
                kind.as_str(),
                role,
                type_name.trim()
            ));
        }
        Ok(type_id)
    }

    fn resolve_typed_key_argument(
        &self,
        kind: TypedCollectionKind,
        type_name: &str,
    ) -> Result<TypeId, String> {
        let type_id = self.resolve_typed_scalar_argument(kind, "key", type_name)?;
        if !self.is_integer(type_id) {
            return Err(format!(
                "typed collection '{}' key type '{}' must be an integer scalar",
                kind.as_str(),
                type_name.trim()
            ));
        }
        Ok(type_id)
    }

    pub fn intern_instantiated_nominal(
        &mut self,
        definition: &str,
        arguments: &[GenericArgument],
    ) -> Result<TypeId, String> {
        let definition = definition.trim();
        if definition.is_empty() {
            return Err("generic type definition cannot be empty".to_string());
        }
        if arguments.is_empty() {
            return Err(format!(
                "generic type '{definition}' requires at least one argument"
            ));
        }
        let key = TypeKey::InstantiatedNominal {
            definition: definition.to_string(),
            arguments: arguments.to_vec(),
        };
        let display_name = format!(
            "{}<{}>",
            definition,
            arguments
                .iter()
                .map(|argument| match argument {
                    GenericArgument::Type(type_id) => self
                        .type_info(*type_id)
                        .map_or_else(|| format!("type{type_id}"), |info| info.name.clone()),
                    GenericArgument::I32(value) => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        self.intern_with_info(
            key,
            TypeInfo {
                name: display_name,
                category: TypeCategory::Named,
                layout: TypeLayout {
                    header_i32_words: 0,
                    payload_size_bytes: None,
                    static_size_bytes: None,
                },
            },
        )
    }

    pub fn instantiated_nominal_identity(
        &self,
        type_id: TypeId,
    ) -> Option<(String, Vec<GenericArgument>)> {
        match self.type_key(type_id)? {
            TypeKey::InstantiatedNominal {
                definition,
                arguments,
            } => Some((definition.clone(), arguments.clone())),
            _ => None,
        }
    }

    pub fn ensure_ascii_view_id(&mut self) -> Result<TypeId, String> {
        self.resolve_or_intern_array("ascii", ArrayExtent::View)
    }

    pub fn ensure_utf8_view_id(&mut self) -> Result<TypeId, String> {
        self.resolve_or_intern_array("utf8", ArrayExtent::View)
    }

    pub fn string_literal_type_id(&self) -> Option<TypeId> {
        self.find_first_type_id_by_category(TypeCategory::Utf8View)
            .or_else(|| self.find_first_type_id_by_category(TypeCategory::AsciiView))
    }

    pub fn type_info(&self, id: TypeId) -> Option<&TypeInfo> {
        self.types.get(id as usize)
    }

    pub fn indexed_element_type_id(&self, type_id: TypeId) -> Option<TypeId> {
        match self.type_key(type_id)? {
            TypeKey::ArrayFixed { element, .. } | TypeKey::ArrayView { element } => Some(*element),
            TypeKey::AsciiFixed { .. }
            | TypeKey::AsciiView
            | TypeKey::Utf8Fixed { .. }
            | TypeKey::Utf8View => self.resolve_existing("u8").or(Some(TYPE_ID_I32)),
            _ => None,
        }
    }

    pub fn fixed_collection_len(&self, type_id: TypeId) -> Option<i32> {
        let raw = match self.type_key(type_id)? {
            TypeKey::ArrayFixed { max_len, .. }
            | TypeKey::AsciiFixed { max_len }
            | TypeKey::Utf8Fixed { max_len } => *max_len,
            _ => return None,
        };
        i32::try_from(raw).ok()
    }

    pub fn void_id(&self) -> TypeId {
        TYPE_ID_VOID
    }

    pub fn is_argument_compatible_with_param(&self, argument: TypeId, parameter: TypeId) -> bool {
        if argument == parameter {
            return true;
        }
        let Some(argument_key) = self.type_key(argument) else {
            return false;
        };
        let Some(parameter_key) = self.type_key(parameter) else {
            return false;
        };

        if are_i32_scalar_abi_compatible(argument_key, parameter_key) {
            return true;
        }
        if is_text_buffer_key(argument_key) && is_text_buffer_key(parameter_key) {
            return true;
        }
        if (is_text_buffer_key(argument_key) && is_byte_array_key(parameter_key, self))
            || (is_text_buffer_key(parameter_key) && is_byte_array_key(argument_key, self))
        {
            return true;
        }

        match (argument_key, parameter_key) {
            (TypeKey::ArrayFixed { element: lhs, .. }, TypeKey::ArrayView { element: rhs }) => {
                lhs == rhs
            }
            (TypeKey::ArrayView { element: lhs }, TypeKey::ArrayView { element: rhs }) => {
                lhs == rhs
            }
            (TypeKey::AsciiFixed { .. }, TypeKey::AsciiView) => true,
            (TypeKey::Utf8Fixed { .. }, TypeKey::Utf8View) => true,
            _ => false,
        }
    }

    fn resolve_or_intern_inner(&mut self, type_name: &str) -> Result<TypeId, String> {
        if type_name.is_empty() {
            return Err("type name cannot be empty".to_string());
        }
        if type_name == "string" {
            return self.resolve_or_intern_array("utf8", ArrayExtent::View);
        }
        if let Some(id) = self.resolve_existing(type_name) {
            return Ok(id);
        }

        if let Some((base, extent_text)) = split_array_suffix(type_name)? {
            let extent = parse_array_extent(extent_text)?;
            return self.resolve_or_intern_array(base, extent);
        }

        if let Some(descriptor) = self.parse_typed_collection_descriptor(type_name)? {
            return self.intern_typed_collection(descriptor);
        }

        self.intern_named(type_name)
    }

    fn intern_typed_collection(
        &mut self,
        descriptor: TypedCollectionDescriptor,
    ) -> Result<TypeId, String> {
        let key = TypeKey::TypedCollection {
            kind: descriptor.kind,
            element_type: descriptor.element_type,
            key_type: descriptor.key_type,
            value_type: descriptor.value_type,
            capacity: descriptor.capacity,
            width: descriptor.width,
            height: descriptor.height,
            lanes: descriptor.lanes.clone(),
        };
        if let Some(existing) = self.by_key.get(&key) {
            return Ok(*existing);
        }
        let static_size_bytes = u32::try_from(descriptor.static_size_bytes).map_err(|_| {
            format!(
                "typed collection '{}' static size {} exceeds u32 layout representation",
                descriptor.kind_name(),
                descriptor.static_size_bytes
            )
        })?;
        self.intern_with_info(
            key,
            TypeInfo {
                name: descriptor.canonical_type_name(self),
                // Keep the existing source ABI category until collection
                // operations have a dedicated lowering contract. The
                // internal TypeKey above is what prevents nominal identity
                // and scalar state fallback at the compiler-owned boundary.
                category: TypeCategory::Named,
                layout: TypeLayout {
                    header_i32_words: 0,
                    payload_size_bytes: Some(static_size_bytes),
                    static_size_bytes: Some(static_size_bytes),
                },
            },
        )
    }

    fn resolve_or_intern_array(
        &mut self,
        base: &str,
        extent: ArrayExtent,
    ) -> Result<TypeId, String> {
        let base = base.trim();
        if base.is_empty() {
            return Err("array type base cannot be empty".to_string());
        }

        if base == "ascii" {
            return match extent {
                ArrayExtent::View => self.intern_with_info(
                    TypeKey::AsciiView,
                    TypeInfo {
                        name: "ascii[]".to_string(),
                        category: TypeCategory::AsciiView,
                        layout: TypeLayout {
                            header_i32_words: 2,
                            payload_size_bytes: None,
                            static_size_bytes: None,
                        },
                    },
                ),
                ArrayExtent::Fixed(max_len) => {
                    let static_size = checked_add(checked_mul(2, 4)?, max_len)?;
                    self.intern_with_info(
                        TypeKey::AsciiFixed { max_len },
                        TypeInfo {
                            name: format!("ascii[{max_len}]"),
                            category: TypeCategory::AsciiFixed,
                            layout: TypeLayout {
                                header_i32_words: 2,
                                payload_size_bytes: Some(max_len),
                                static_size_bytes: Some(static_size),
                            },
                        },
                    )
                }
            };
        }

        if base == "utf8" || base == "string" {
            return match extent {
                ArrayExtent::View => self.intern_with_info(
                    TypeKey::Utf8View,
                    TypeInfo {
                        name: "utf8[]".to_string(),
                        category: TypeCategory::Utf8View,
                        layout: TypeLayout {
                            header_i32_words: 3,
                            payload_size_bytes: None,
                            static_size_bytes: None,
                        },
                    },
                ),
                ArrayExtent::Fixed(max_len) => {
                    let static_size = checked_add(checked_mul(3, 4)?, max_len)?;
                    self.intern_with_info(
                        TypeKey::Utf8Fixed { max_len },
                        TypeInfo {
                            name: format!("utf8[{max_len}]"),
                            category: TypeCategory::Utf8Fixed,
                            layout: TypeLayout {
                                header_i32_words: 3,
                                payload_size_bytes: Some(max_len),
                                static_size_bytes: Some(static_size),
                            },
                        },
                    )
                }
            };
        }

        let element_type = self.resolve_or_intern_inner(base)?;
        let element_info = self
            .type_info(element_type)
            .ok_or_else(|| format!("missing type metadata for type id {element_type}"))?;
        let element_name = element_info.name.clone();
        let element_static_size = element_info.layout.static_size_bytes;
        match extent {
            ArrayExtent::View => self.intern_with_info(
                TypeKey::ArrayView {
                    element: element_type,
                },
                TypeInfo {
                    name: format!("{element_name}[]"),
                    category: TypeCategory::ArrayView,
                    layout: TypeLayout {
                        header_i32_words: 1,
                        payload_size_bytes: None,
                        static_size_bytes: None,
                    },
                },
            ),
            ArrayExtent::Fixed(max_len) => {
                let payload_size = element_static_size
                    .map(|element_size| checked_mul(element_size, max_len))
                    .transpose()?;
                let static_size = payload_size
                    .map(|payload| checked_add(4, payload))
                    .transpose()?;
                self.intern_with_info(
                    TypeKey::ArrayFixed {
                        element: element_type,
                        max_len,
                    },
                    TypeInfo {
                        name: format!("{element_name}[{max_len}]"),
                        category: TypeCategory::ArrayFixed,
                        layout: TypeLayout {
                            header_i32_words: 1,
                            payload_size_bytes: payload_size,
                            static_size_bytes: static_size,
                        },
                    },
                )
            }
        }
    }

    fn resolve_existing(&self, type_name: &str) -> Option<TypeId> {
        if type_name.is_empty() {
            return None;
        }
        if type_name == "void" {
            return Some(TYPE_ID_VOID);
        }
        if type_name == "i32" {
            return Some(TYPE_ID_I32);
        }
        if type_name == "f32" {
            return Some(TYPE_ID_F32);
        }
        if type_name == "bool" {
            return Some(TYPE_ID_BOOL);
        }
        if type_name == "f64" {
            return Some(TYPE_ID_F64);
        }
        if type_name == "u8" {
            return Some(TYPE_ID_U8);
        }
        if type_name == "u16" {
            return Some(TYPE_ID_U16);
        }
        if type_name == "u32" {
            return Some(TYPE_ID_U32);
        }
        if type_name == "string" {
            return self.by_key.get(&TypeKey::Utf8View).copied();
        }

        if let Some((index, _)) = self
            .types
            .iter()
            .enumerate()
            .find(|(_, type_info)| type_info.name == type_name)
        {
            return TypeId::try_from(index).ok();
        }

        // Normalize whitespace and retain compiler-owned identity for callers
        // that use `resolve` after an application was interned through a
        // differently spaced source spelling.
        if let Ok(Some(descriptor)) = self.parse_typed_collection_descriptor(type_name) {
            let key = TypeKey::TypedCollection {
                kind: descriptor.kind,
                element_type: descriptor.element_type,
                key_type: descriptor.key_type,
                value_type: descriptor.value_type,
                capacity: descriptor.capacity,
                width: descriptor.width,
                height: descriptor.height,
                lanes: descriptor.lanes,
            };
            return self.by_key.get(&key).copied();
        }

        let split = split_array_suffix(type_name).ok()?;
        if let Some((base, extent_text)) = split {
            let extent = parse_array_extent(extent_text).ok()?;
            let base = base.trim();
            if base == "ascii" {
                return match extent {
                    ArrayExtent::View => self.by_key.get(&TypeKey::AsciiView).copied(),
                    ArrayExtent::Fixed(max_len) => {
                        self.by_key.get(&TypeKey::AsciiFixed { max_len }).copied()
                    }
                };
            }
            if base == "utf8" || base == "string" {
                return match extent {
                    ArrayExtent::View => self.by_key.get(&TypeKey::Utf8View).copied(),
                    ArrayExtent::Fixed(max_len) => {
                        self.by_key.get(&TypeKey::Utf8Fixed { max_len }).copied()
                    }
                };
            }
            let element = self.resolve_existing(base)?;
            return match extent {
                ArrayExtent::View => self.by_key.get(&TypeKey::ArrayView { element }).copied(),
                ArrayExtent::Fixed(max_len) => self
                    .by_key
                    .get(&TypeKey::ArrayFixed { element, max_len })
                    .copied(),
            };
        }
        self.by_key
            .get(&TypeKey::Named(type_name.to_string()))
            .copied()
    }

    fn intern_builtin(&mut self, name: &str, builtin: BuiltinType, static_size_bytes: u32) {
        let id = self.types.len() as TypeId;
        let key = TypeKey::Builtin(builtin);
        self.types.push(TypeInfo {
            name: name.to_string(),
            category: TypeCategory::Builtin,
            layout: TypeLayout {
                header_i32_words: 0,
                payload_size_bytes: None,
                static_size_bytes: Some(static_size_bytes),
            },
        });
        self.type_keys.push(key.clone());
        self.by_key.insert(key, id);
    }

    fn intern_named(&mut self, name: &str) -> Result<TypeId, String> {
        self.intern_with_info(
            TypeKey::Named(name.to_string()),
            TypeInfo {
                name: name.to_string(),
                category: TypeCategory::Named,
                layout: TypeLayout {
                    header_i32_words: 0,
                    payload_size_bytes: None,
                    static_size_bytes: None,
                },
            },
        )
    }

    fn intern_with_info(&mut self, key: TypeKey, info: TypeInfo) -> Result<TypeId, String> {
        if let Some(existing) = self.by_key.get(&key) {
            return Ok(*existing);
        }
        let id = u16::try_from(self.types.len()).map_err(|_| "type table exceeded u16 capacity")?;
        self.types.push(info);
        self.type_keys.push(key.clone());
        self.by_key.insert(key, id);
        Ok(id)
    }

    fn type_key(&self, id: TypeId) -> Option<&TypeKey> {
        self.type_keys.get(id as usize)
    }

    /// Whether a type id was interned from a compiler-owned typed collection
    /// application rather than from an ordinary nominal declaration.
    pub fn is_typed_collection_type(&self, type_id: TypeId) -> bool {
        matches!(
            self.type_key(type_id),
            Some(TypeKey::TypedCollection { .. })
        )
    }

    fn find_first_type_id_by_category(&self, category: TypeCategory) -> Option<TypeId> {
        self.types
            .iter()
            .position(|type_info| type_info.category == category)
            .and_then(|index| TypeId::try_from(index).ok())
    }

    pub fn unsigned_integer_bits(&self, type_id: TypeId) -> Option<u8> {
        match self.type_key(type_id)? {
            TypeKey::Builtin(BuiltinType::U8) => Some(8),
            TypeKey::Builtin(BuiltinType::U16) => Some(16),
            TypeKey::Builtin(BuiltinType::U32) => Some(32),
            _ => None,
        }
    }

    pub fn is_integer(&self, type_id: TypeId) -> bool {
        matches!(
            self.type_key(type_id),
            Some(TypeKey::Builtin(
                BuiltinType::I32 | BuiltinType::U8 | BuiltinType::U16 | BuiltinType::U32
            ))
        )
    }

    pub(crate) fn is_i32_abi_compatible(&self, type_id: TypeId) -> bool {
        if self.is_typed_collection_type(type_id) {
            return false;
        }
        if type_id == TYPE_ID_I32
            || type_id == TYPE_ID_BOOL
            || self.unsigned_integer_bits(type_id).is_some()
        {
            return true;
        }
        self.type_info(type_id).is_some_and(|info| {
            matches!(
                info.category,
                TypeCategory::Named
                    | TypeCategory::ArrayFixed
                    | TypeCategory::ArrayView
                    | TypeCategory::AsciiFixed
                    | TypeCategory::AsciiView
                    | TypeCategory::Utf8Fixed
                    | TypeCategory::Utf8View
            )
        })
    }

    /// `SpriteRef` is a compiler-owned nominal scalar. It deliberately keeps an
    /// i32 ABI lane for the host boundary, but source code may not treat it as
    /// an integer or exchange it with another i32-compatible type.
    pub(crate) fn is_sealed_sprite_ref(&self, type_id: TypeId) -> bool {
        self.type_info(type_id)
            .is_some_and(|info| info.name == "SpriteRef")
    }

    pub(crate) fn assignment_types_are_compatible(
        &self,
        target_type: TypeId,
        expression_type: TypeId,
    ) -> bool {
        if target_type == expression_type {
            return true;
        }
        if self.is_sealed_sprite_ref(target_type) {
            return false;
        }
        if target_type == TYPE_ID_BOOL || expression_type == TYPE_ID_BOOL {
            return false;
        }
        self.is_i32_abi_compatible(target_type) && self.is_i32_abi_compatible(expression_type)
    }
}

fn are_i32_scalar_abi_compatible(argument: &TypeKey, parameter: &TypeKey) -> bool {
    if is_i32_lane_builtin(argument) && is_i32_lane_builtin(parameter) {
        return true;
    }

    matches!(
        (argument, parameter),
        (
            TypeKey::Builtin(BuiltinType::I32),
            TypeKey::Builtin(BuiltinType::I32)
        ) | (
            TypeKey::Builtin(BuiltinType::Bool),
            TypeKey::Builtin(BuiltinType::Bool)
        ) | (
            TypeKey::Builtin(BuiltinType::I32),
            TypeKey::Builtin(BuiltinType::Bool)
        ) | (
            TypeKey::Builtin(BuiltinType::Bool),
            TypeKey::Builtin(BuiltinType::I32)
        )
    )
}

fn is_i32_lane_builtin(key: &TypeKey) -> bool {
    matches!(
        key,
        TypeKey::Builtin(
            BuiltinType::I32
                | BuiltinType::Bool
                | BuiltinType::U8
                | BuiltinType::U16
                | BuiltinType::U32
        )
    )
}

fn is_text_buffer_key(key: &TypeKey) -> bool {
    matches!(
        key,
        TypeKey::AsciiFixed { .. }
            | TypeKey::AsciiView
            | TypeKey::Utf8Fixed { .. }
            | TypeKey::Utf8View
    )
}

fn is_byte_array_key(key: &TypeKey, table: &TypeTable) -> bool {
    let element = match key {
        TypeKey::ArrayFixed { element, .. } | TypeKey::ArrayView { element } => *element,
        _ => return false,
    };
    matches!(
        table.type_key(element),
        Some(TypeKey::Builtin(BuiltinType::U8))
    )
}

fn parse_typed_collection_kind(name: &str) -> Option<TypedCollectionKind> {
    match name.trim() {
        "pool" => Some(TypedCollectionKind::Pool),
        "stable_pool" => Some(TypedCollectionKind::StablePool),
        "queue" => Some(TypedCollectionKind::Queue),
        "ring_buffer" => Some(TypedCollectionKind::RingBuffer),
        "map" => Some(TypedCollectionKind::Map),
        "set" => Some(TypedCollectionKind::Set),
        "priority_queue" => Some(TypedCollectionKind::PriorityQueue),
        "grid" => Some(TypedCollectionKind::Grid),
        "bitset" => Some(TypedCollectionKind::Bitset),
        _ => None,
    }
}

fn typed_collection_arity(kind: TypedCollectionKind) -> usize {
    match kind {
        TypedCollectionKind::Pool
        | TypedCollectionKind::StablePool
        | TypedCollectionKind::Queue
        | TypedCollectionKind::RingBuffer
        | TypedCollectionKind::Set
        | TypedCollectionKind::PriorityQueue => 2,
        TypedCollectionKind::Map => 3,
        TypedCollectionKind::Grid => 3,
        TypedCollectionKind::Bitset => 1,
    }
}

fn is_legacy_typed_collection_policy(text: &str) -> bool {
    matches!(text.trim(), "error" | "drop_newest" | "overwrite_oldest")
}

fn legacy_typed_collection_policy_diagnostic(
    source: &str,
    kind: TypedCollectionKind,
    arguments: &[&str],
    canonical_arity: usize,
) -> String {
    let policy = arguments[canonical_arity].trim();
    let canonical_arguments = arguments[..canonical_arity].join(", ");
    format!(
        "typed collection '{source}' uses legacy trailing overflow policy '{policy}'; remove that argument and migrate to '{}<{}>'",
        kind.as_str(),
        canonical_arguments
    )
}

fn parse_typed_capacity(kind: TypedCollectionKind, text: &str, role: &str) -> Result<u32, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "typed collection '{}' {} must be a nonnegative decimal constant, got '{}'",
            kind.as_str(),
            role,
            trimmed
        ));
    }
    let value = trimmed.parse::<u64>().map_err(|_| {
        format!(
            "typed collection '{}' {} '{}' is outside the supported integer range",
            kind.as_str(),
            role,
            trimmed
        )
    })?;
    if value > i32::MAX as u64 {
        return Err(format!(
            "typed collection '{}' {} {} exceeds i32::MAX",
            kind.as_str(),
            role,
            value
        ));
    }
    Ok(value as u32)
}

fn split_type_application(type_name: &str) -> Result<Option<(&str, Vec<&str>)>, String> {
    let trimmed = type_name.trim();
    let Some(open) = trimmed.find('<') else {
        return Ok(None);
    };
    if !trimmed.ends_with('>') {
        return Err(format!(
            "invalid typed collection application '{}': missing '>'",
            trimmed
        ));
    }
    let base = trimmed[..open].trim();
    if base.is_empty() {
        return Err(format!(
            "invalid typed collection application '{}': missing collection name",
            trimmed
        ));
    }

    let bytes = trimmed.as_bytes();
    let mut angle_depth = 0i32;
    let mut square_depth = 0i32;
    let mut argument_start = open + 1;
    let mut arguments = Vec::new();
    for index in open..bytes.len() {
        match bytes[index] {
            b'<' => angle_depth += 1,
            b'>' => {
                angle_depth -= 1;
                if angle_depth < 0 {
                    return Err(format!(
                        "invalid typed collection application '{}': unmatched '>'",
                        trimmed
                    ));
                }
                if angle_depth == 0 {
                    if square_depth != 0 {
                        return Err(format!(
                            "invalid typed collection application '{}': unmatched '['",
                            trimmed
                        ));
                    }
                    let argument = trimmed[argument_start..index].trim();
                    if argument.is_empty() {
                        return Err(format!(
                            "invalid typed collection application '{}': empty argument",
                            trimmed
                        ));
                    }
                    arguments.push(argument);
                    if index + 1 != bytes.len() {
                        return Err(format!(
                            "invalid typed collection application '{}': trailing text",
                            trimmed
                        ));
                    }
                }
            }
            b'[' => square_depth += 1,
            b']' => {
                square_depth -= 1;
                if square_depth < 0 {
                    return Err(format!(
                        "invalid typed collection application '{}': unmatched ']'",
                        trimmed
                    ));
                }
            }
            b',' if angle_depth == 1 && square_depth == 0 => {
                let argument = trimmed[argument_start..index].trim();
                if argument.is_empty() {
                    return Err(format!(
                        "invalid typed collection application '{}': empty argument",
                        trimmed
                    ));
                }
                arguments.push(argument);
                argument_start = index + 1;
            }
            _ => {}
        }
    }
    if angle_depth != 0 || square_depth != 0 || arguments.is_empty() {
        return Err(format!(
            "invalid typed collection application '{}': unbalanced or empty arguments",
            trimmed
        ));
    }
    Ok(Some((base, arguments)))
}

fn is_typed_scalar_type(type_id: TypeId) -> bool {
    matches!(
        type_id,
        TYPE_ID_I32
            | TYPE_ID_F32
            | TYPE_ID_F64
            | TYPE_ID_BOOL
            | TYPE_ID_U8
            | TYPE_ID_U16
            | TYPE_ID_U32
    )
}

fn typed_scalar_lane_layout(type_id: TypeId) -> Option<(u64, u64)> {
    match type_id {
        TYPE_ID_I32 | TYPE_ID_F32 | TYPE_ID_U32 | TYPE_ID_BOOL => Some((4, 4)),
        TYPE_ID_F64 => Some((8, 8)),
        TYPE_ID_U8 => Some((1, 1)),
        TYPE_ID_U16 => Some((2, 2)),
        _ => None,
    }
}

fn build_typed_collection_lanes(
    type_table: &TypeTable,
    descriptor: &TypedCollectionDescriptor,
) -> Result<Vec<TypedCollectionLane>, String> {
    let mut lanes = Vec::new();
    let mut offset = 0u64;
    match descriptor.kind {
        TypedCollectionKind::Pool => {
            append_typed_lane(type_table, &mut lanes, &mut offset, "count", TYPE_ID_I32, 1)?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "values",
                descriptor.element_type.expect("pool element validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::StablePool => {
            append_typed_lane(type_table, &mut lanes, &mut offset, "count", TYPE_ID_I32, 1)?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "occupied",
                TYPE_ID_U8,
                u64::from(descriptor.capacity),
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "values",
                descriptor
                    .element_type
                    .expect("stable pool element validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::Queue | TypedCollectionKind::RingBuffer => {
            append_typed_lane(type_table, &mut lanes, &mut offset, "count", TYPE_ID_I32, 1)?;
            append_typed_lane(type_table, &mut lanes, &mut offset, "head", TYPE_ID_I32, 1)?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "values",
                descriptor.element_type.expect("queue element validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::Map => {
            append_typed_lane(type_table, &mut lanes, &mut offset, "count", TYPE_ID_I32, 1)?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "occupied",
                TYPE_ID_U8,
                u64::from(descriptor.capacity),
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "keys",
                descriptor.key_type.expect("map key validated"),
                u64::from(descriptor.capacity),
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "values",
                descriptor.value_type.expect("map value validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::Set => {
            append_typed_lane(type_table, &mut lanes, &mut offset, "count", TYPE_ID_I32, 1)?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "occupied",
                TYPE_ID_U8,
                u64::from(descriptor.capacity),
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "keys",
                descriptor.key_type.expect("set key validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::PriorityQueue => {
            append_typed_lane(type_table, &mut lanes, &mut offset, "count", TYPE_ID_I32, 1)?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "next_order",
                TYPE_ID_U32,
                1,
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "priority",
                TYPE_ID_I32,
                u64::from(descriptor.capacity),
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "order",
                TYPE_ID_U32,
                u64::from(descriptor.capacity),
            )?;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "values",
                descriptor
                    .element_type
                    .expect("priority queue element validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::Grid => {
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "values",
                descriptor.element_type.expect("grid element validated"),
                u64::from(descriptor.capacity),
            )?;
        }
        TypedCollectionKind::Bitset => {
            let words = u64::from(descriptor.capacity)
                .checked_add(31)
                .ok_or_else(|| "typed bitset word-count arithmetic overflow".to_string())?
                / 32;
            append_typed_lane(
                type_table,
                &mut lanes,
                &mut offset,
                "words",
                TYPE_ID_U32,
                words,
            )?;
        }
    }
    Ok(lanes)
}

fn append_typed_lane(
    _type_table: &TypeTable,
    lanes: &mut Vec<TypedCollectionLane>,
    offset: &mut u64,
    name: &str,
    type_id: TypeId,
    element_count: u64,
) -> Result<(), String> {
    let (element_size, alignment) = typed_scalar_lane_layout(type_id).ok_or_else(|| {
        format!(
            "typed collection lane '{}' has unsupported scalar type id {}",
            name, type_id
        )
    })?;
    if element_count == 0 {
        lanes.push(TypedCollectionLane {
            name: name.to_string(),
            type_id,
            element_count,
            offset_bytes: *offset,
            byte_size: 0,
            alignment_bytes: alignment,
        });
        return Ok(());
    }
    let aligned_offset = align_typed_offset(*offset, alignment)?;
    let byte_size = element_size
        .checked_mul(element_count)
        .ok_or_else(|| format!("typed collection lane '{}' byte size overflow", name))?;
    let end = aligned_offset
        .checked_add(byte_size)
        .ok_or_else(|| format!("typed collection lane '{}' offset overflow", name))?;
    lanes.push(TypedCollectionLane {
        name: name.to_string(),
        type_id,
        element_count,
        offset_bytes: aligned_offset,
        byte_size,
        alignment_bytes: alignment,
    });
    *offset = end;
    Ok(())
}

fn typed_collection_total_size(lanes: &[TypedCollectionLane]) -> Result<u64, String> {
    let mut end = 0u64;
    let mut alignment = 1u64;
    for lane in lanes {
        end = end.max(
            lane.offset_bytes
                .checked_add(lane.byte_size)
                .ok_or_else(|| "typed collection total byte size overflow".to_string())?,
        );
        if lane.byte_size > 0 {
            alignment = alignment.max(lane.alignment_bytes);
        }
    }
    align_typed_offset(end, alignment)
}

fn align_typed_offset(offset: u64, alignment: u64) -> Result<u64, String> {
    debug_assert!(alignment.is_power_of_two());
    let adjustment = alignment
        .checked_sub(1)
        .ok_or_else(|| "typed collection alignment cannot be zero".to_string())?;
    offset
        .checked_add(adjustment)
        .map(|value| value / alignment * alignment)
        .ok_or_else(|| "typed collection alignment arithmetic overflow".to_string())
}

fn split_array_suffix(type_name: &str) -> Result<Option<(&str, &str)>, String> {
    let bytes = type_name.as_bytes();
    if bytes.last().is_none_or(|last| *last != b']') {
        return Ok(None);
    }

    let mut depth = 0i32;
    for index in (0..bytes.len()).rev() {
        match bytes[index] {
            b']' => depth += 1,
            b'[' => {
                depth -= 1;
                if depth == 0 {
                    let base = &type_name[..index];
                    let extent = &type_name[index + 1..bytes.len() - 1];
                    return Ok(Some((base, extent)));
                }
                if depth < 0 {
                    return Err("invalid type annotation: unmatched '['".to_string());
                }
            }
            _ => {}
        }
    }
    Err("invalid type annotation: missing '[' for array suffix".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayExtent {
    View,
    Fixed(u32),
}

fn parse_array_extent(extent_text: &str) -> Result<ArrayExtent, String> {
    let trimmed = extent_text.trim();
    if trimmed.is_empty() {
        return Ok(ArrayExtent::View);
    }
    if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid array extent '{trimmed}'"));
    }
    let max_len = trimmed
        .parse::<u32>()
        .map_err(|_| format!("invalid array extent '{trimmed}'"))?;
    Ok(ArrayExtent::Fixed(max_len))
}

fn checked_mul(lhs: u32, rhs: u32) -> Result<u32, String> {
    lhs.checked_mul(rhs)
        .ok_or_else(|| "type layout size overflow".to_string())
}

fn checked_add(lhs: u32, rhs: u32) -> Result<u32, String> {
    lhs.checked_add(rhs)
        .ok_or_else(|| "type layout size overflow".to_string())
}

impl Default for TypeTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_id_capacity_accepts_last_u16_id_and_rejects_next_without_mutation() {
        let mut table = TypeTable::new();
        let mut last_id = None;
        while table.types.len() <= usize::from(u16::MAX) {
            let index = table.types.len();
            last_id = Some(
                table
                    .intern_named(&format!("BoundaryType{index}"))
                    .expect("every representable TypeId is accepted"),
            );
        }

        assert_eq!(last_id, Some(u16::MAX));
        assert_eq!(table.types.len(), usize::from(u16::MAX) + 1);
        let before_types = table.types.len();
        let before_keys = table.type_keys.len();
        let before_index = table.by_key.len();
        let error = table
            .intern_named("TypeIdOverflow")
            .expect_err("the first unrepresentable TypeId must fail");
        assert!(
            error.contains("type table exceeded u16 capacity"),
            "{error}"
        );
        assert_eq!(table.types.len(), before_types);
        assert_eq!(table.type_keys.len(), before_keys);
        assert_eq!(table.by_key.len(), before_index);
        assert_eq!(table.resolve("TypeIdOverflow"), None);
    }

    #[test]
    fn fixed_array_layout_accepts_zero_and_last_u32_size_then_rejects_overflow() {
        let mut table = TypeTable::new();
        let zero = table.resolve_or_intern("i32[0]").expect("zero capacity");
        let last = table
            .resolve_or_intern("i32[1073741822]")
            .expect("last i32 array layout fitting u32");

        assert_eq!(
            table.type_info(zero).expect("zero layout").layout,
            TypeLayout {
                header_i32_words: 1,
                payload_size_bytes: Some(0),
                static_size_bytes: Some(4),
            }
        );
        assert_eq!(
            table.type_info(last).expect("last layout").layout,
            TypeLayout {
                header_i32_words: 1,
                payload_size_bytes: Some(4_294_967_288),
                static_size_bytes: Some(4_294_967_292),
            }
        );

        let before = table.types.len();
        let error = table
            .resolve_or_intern("i32[1073741823]")
            .expect_err("header addition must reject u32 overflow");
        assert!(error.contains("type layout size overflow"), "{error}");
        assert_eq!(table.types.len(), before);
        assert_eq!(table.resolve("i32[1073741823]"), None);
    }

    #[test]
    fn interns_array_view_and_fixed_types_deterministically() {
        let mut table = TypeTable::new();
        let view_1 = table.resolve_or_intern("i32[]").expect("i32[]");
        let view_2 = table.resolve_or_intern("i32[]").expect("i32[] again");
        let fixed_4 = table.resolve_or_intern("i32[4]").expect("i32[4]");
        let fixed_8 = table.resolve_or_intern("i32[8]").expect("i32[8]");

        assert_eq!(view_1, view_2);
        assert_ne!(view_1, fixed_4);
        assert_ne!(fixed_4, fixed_8);

        let fixed_4_info = table.type_info(fixed_4).expect("fixed type info");
        assert_eq!(fixed_4_info.layout.header_i32_words, 1);
        assert_eq!(fixed_4_info.layout.payload_size_bytes, Some(16));
        assert_eq!(fixed_4_info.layout.static_size_bytes, Some(20));
    }

    #[test]
    fn interns_nominal_value_applications_by_canonical_arguments() {
        let mut table = TypeTable::new();
        let first = table
            .intern_instantiated_nominal("Buffer", &[GenericArgument::I32(12)])
            .expect("Buffer<12>");
        let spelled_equivalent = table
            .intern_instantiated_nominal("Buffer", &[GenericArgument::I32(12)])
            .expect("Buffer<12> again");
        let different = table
            .intern_instantiated_nominal("Buffer", &[GenericArgument::I32(24)])
            .expect("Buffer<24>");

        assert_eq!(first, spelled_equivalent);
        assert_ne!(first, different);
        assert_eq!(table.resolve("Buffer<12>"), Some(first));
        assert_eq!(
            table.instantiated_nominal_identity(first),
            Some(("Buffer".to_string(), vec![GenericArgument::I32(12)]))
        );
    }

    #[test]
    fn models_ascii_and_utf8_headers_with_max_length() {
        let mut table = TypeTable::new();
        let ascii_fixed = table.resolve_or_intern("ascii[32]").expect("ascii[32]");
        let ascii_view = table.resolve_or_intern("ascii[]").expect("ascii[]");
        let utf8_fixed = table.resolve_or_intern("utf8[32]").expect("utf8[32]");
        let utf8_view = table.resolve_or_intern("utf8[]").expect("utf8[]");

        let ascii_fixed_info = table.type_info(ascii_fixed).expect("ascii fixed info");
        assert_eq!(ascii_fixed_info.layout.header_i32_words, 2);
        assert_eq!(ascii_fixed_info.layout.payload_size_bytes, Some(32));
        assert_eq!(ascii_fixed_info.layout.static_size_bytes, Some(40));

        let ascii_view_info = table.type_info(ascii_view).expect("ascii view info");
        assert_eq!(ascii_view_info.layout.header_i32_words, 2);
        assert_eq!(ascii_view_info.layout.payload_size_bytes, None);
        assert_eq!(ascii_view_info.layout.static_size_bytes, None);

        let utf8_fixed_info = table.type_info(utf8_fixed).expect("utf8 fixed info");
        assert_eq!(utf8_fixed_info.layout.header_i32_words, 3);
        assert_eq!(utf8_fixed_info.layout.payload_size_bytes, Some(32));
        assert_eq!(utf8_fixed_info.layout.static_size_bytes, Some(44));

        let utf8_view_info = table.type_info(utf8_view).expect("utf8 view info");
        assert_eq!(utf8_view_info.layout.header_i32_words, 3);
        assert_eq!(utf8_view_info.layout.payload_size_bytes, None);
        assert_eq!(utf8_view_info.layout.static_size_bytes, None);
    }

    #[test]
    fn narrow_unsigned_builtins_have_true_static_layouts_and_stable_ids() {
        let table = TypeTable::new();
        for (name, expected_id, expected_size) in [
            ("u8", TYPE_ID_U8, 1),
            ("u16", TYPE_ID_U16, 2),
            ("u32", TYPE_ID_U32, 4),
        ] {
            assert_eq!(table.resolve(name), Some(expected_id));
            assert_eq!(
                table
                    .type_info(expected_id)
                    .and_then(|info| info.layout.static_size_bytes),
                Some(expected_size)
            );
        }
    }

    #[test]
    fn string_alias_resolves_to_utf8_types() {
        let mut table = TypeTable::new();
        let string_view = table.resolve_or_intern("string").expect("string");
        let utf8_view = table.resolve_or_intern("utf8[]").expect("utf8[]");
        let string_fixed = table.resolve_or_intern("string[24]").expect("string[24]");
        let utf8_fixed = table.resolve_or_intern("utf8[24]").expect("utf8[24]");

        assert_eq!(string_view, utf8_view);
        assert_eq!(string_fixed, utf8_fixed);
    }

    #[test]
    fn ensure_text_view_ids_seed_string_literal_type_without_name_lookup() {
        let mut table = TypeTable::new();
        assert_eq!(table.string_literal_type_id(), None);

        let utf8_view = table.ensure_utf8_view_id().expect("ensure utf8[]");
        let ascii_view = table.ensure_ascii_view_id().expect("ensure ascii[]");

        assert_eq!(table.string_literal_type_id(), Some(utf8_view));
        assert_ne!(utf8_view, ascii_view);
    }

    #[test]
    fn rejects_invalid_array_extent_text() {
        let mut table = TypeTable::new();
        let err = table
            .resolve_or_intern("i32[abc]")
            .expect_err("expected invalid extent");
        assert!(
            err.contains("invalid array extent"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_returns_only_existing_types() {
        let mut table = TypeTable::new();
        assert!(table.resolve("enemy").is_none());
        let enemy = table.resolve_or_intern("enemy").expect("enemy");
        assert_eq!(table.resolve("enemy"), Some(enemy));
        assert!(table.resolve("enemy[]").is_none());
    }

    #[test]
    fn array_view_param_accepts_fixed_capacity_argument_of_same_element() {
        let mut table = TypeTable::new();
        let arg = table.resolve_or_intern("i32[64]").expect("i32[64]");
        let param = table.resolve_or_intern("i32[]").expect("i32[]");
        assert!(table.is_argument_compatible_with_param(arg, param));
    }

    #[test]
    fn array_view_param_rejects_fixed_capacity_argument_of_other_element() {
        let mut table = TypeTable::new();
        let arg = table.resolve_or_intern("i32[64]").expect("i32[64]");
        let param = table.resolve_or_intern("enemy[]").expect("enemy[]");
        assert!(!table.is_argument_compatible_with_param(arg, param));
    }

    #[test]
    fn fixed_capacity_array_param_requires_exact_capacity_match() {
        let mut table = TypeTable::new();
        let arg = table.resolve_or_intern("i32[8]").expect("i32[8]");
        let param = table.resolve_or_intern("i32[16]").expect("i32[16]");
        assert!(!table.is_argument_compatible_with_param(arg, param));
    }

    #[test]
    fn ascii_utf8_buffer_arguments_are_cross_compatible_for_calls() {
        let mut table = TypeTable::new();
        let ascii_fixed = table.resolve_or_intern("ascii[16]").expect("ascii[16]");
        let ascii_view = table.resolve_or_intern("ascii[]").expect("ascii[]");
        let utf8_fixed = table.resolve_or_intern("utf8[16]").expect("utf8[16]");
        let utf8_view = table.resolve_or_intern("utf8[]").expect("utf8[]");
        let u8_view = table.resolve_or_intern("u8[]").expect("u8[]");

        assert!(table.is_argument_compatible_with_param(ascii_fixed, ascii_view));
        assert!(table.is_argument_compatible_with_param(utf8_fixed, utf8_view));
        assert!(table.is_argument_compatible_with_param(ascii_fixed, utf8_view));
        assert!(table.is_argument_compatible_with_param(utf8_fixed, ascii_view));
        assert!(table.is_argument_compatible_with_param(ascii_view, utf8_view));
        assert!(table.is_argument_compatible_with_param(utf8_view, ascii_view));
        assert!(table.is_argument_compatible_with_param(utf8_view, u8_view));
        assert!(table.is_argument_compatible_with_param(u8_view, ascii_view));
    }

    #[test]
    fn indexed_element_type_reports_array_and_string_storage_elements() {
        let mut table = TypeTable::new();
        let i32_view = table.resolve_or_intern("i32[]").expect("i32[]");
        let i32_fixed = table.resolve_or_intern("i32[64]").expect("i32[64]");
        let utf8_view = table.resolve_or_intern("utf8[]").expect("utf8[]");

        assert_eq!(table.indexed_element_type_id(i32_view), Some(TYPE_ID_I32));
        assert_eq!(table.indexed_element_type_id(utf8_view), Some(TYPE_ID_U8));
        assert_eq!(table.fixed_collection_len(i32_view), None);
        assert_eq!(table.fixed_collection_len(i32_fixed), Some(64));
    }

    #[test]
    fn scalar_i32_abi_arguments_match_named_scalar_parameters() {
        let mut table = TypeTable::new();
        let u8_type = table.resolve_or_intern("u8").expect("u8");
        let ascii_view = table.resolve_or_intern("ascii[]").expect("ascii[]");

        assert!(table.is_argument_compatible_with_param(TYPE_ID_I32, u8_type));
        assert!(table.is_argument_compatible_with_param(u8_type, TYPE_ID_I32));
        assert!(!table.is_argument_compatible_with_param(TYPE_ID_I32, ascii_view));
    }

    #[test]
    fn sealed_sprite_ref_keeps_abi_lane_without_integer_source_compatibility() {
        let mut table = TypeTable::new();
        let sprite_ref = table.resolve_or_intern("SpriteRef").expect("SpriteRef");

        assert!(table.is_i32_abi_compatible(sprite_ref));
        assert!(!table.is_argument_compatible_with_param(TYPE_ID_I32, sprite_ref));
        assert!(!table.is_argument_compatible_with_param(sprite_ref, TYPE_ID_I32));
        assert!(!table.assignment_types_are_compatible(sprite_ref, TYPE_ID_I32));
        assert!(table.assignment_types_are_compatible(TYPE_ID_I32, sprite_ref));
    }

    #[test]
    fn typed_collection_descriptors_cover_all_kinds_with_exact_lane_costs() {
        let table = TypeTable::new();
        let cases = [
            ("pool<i32, 2>", TypedCollectionKind::Pool, 12, 2),
            (
                "stable_pool<u16, 3>",
                TypedCollectionKind::StablePool,
                16,
                3,
            ),
            ("queue<f64, 2>", TypedCollectionKind::Queue, 24, 2),
            ("ring_buffer<u8, 4>", TypedCollectionKind::RingBuffer, 12, 4),
            ("map<u8, f64, 3>", TypedCollectionKind::Map, 40, 3),
            ("set<u32, 3>", TypedCollectionKind::Set, 20, 3),
            (
                "priority_queue<i32, 2>",
                TypedCollectionKind::PriorityQueue,
                32,
                2,
            ),
            ("grid<u16, 2, 3>", TypedCollectionKind::Grid, 12, 6),
            ("bitset<33>", TypedCollectionKind::Bitset, 8, 33),
        ];

        for (source, expected_kind, expected_bytes, expected_capacity) in cases {
            let descriptor = table
                .parse_typed_collection_descriptor(source)
                .expect("valid typed collection source")
                .expect("recognized typed collection");
            assert_eq!(descriptor.kind, expected_kind, "{source}");
            assert_eq!(descriptor.static_size_bytes, expected_bytes, "{source}");
            assert_eq!(descriptor.capacity, expected_capacity, "{source}");
            assert_eq!(descriptor.canonical_type_name(&table), source, "{source}");
            if expected_kind == TypedCollectionKind::StablePool {
                assert_eq!(descriptor.lanes[1].offset_bytes, 4, "{source}");
                assert_eq!(descriptor.lanes[2].offset_bytes, 8, "{source}");
            }
            assert!(
                descriptor.lanes.windows(2).all(
                    |lanes| lanes[0].offset_bytes + lanes[0].byte_size <= lanes[1].offset_bytes
                ),
                "lanes overlap for {source}: {:?}",
                descriptor.lanes
            );
        }
    }

    #[test]
    fn typed_collection_descriptor_accepts_zero_capacity_for_every_kind() {
        let table = TypeTable::new();
        let cases = [
            ("pool<i32, 0>", TypedCollectionKind::Pool, 4, 0),
            ("stable_pool<i32, 0>", TypedCollectionKind::StablePool, 4, 0),
            ("queue<i32, 0>", TypedCollectionKind::Queue, 8, 0),
            ("ring_buffer<i32, 0>", TypedCollectionKind::RingBuffer, 8, 0),
            ("map<u8, i32, 0>", TypedCollectionKind::Map, 4, 0),
            ("set<u32, 0>", TypedCollectionKind::Set, 4, 0),
            (
                "priority_queue<i32, 0>",
                TypedCollectionKind::PriorityQueue,
                8,
                0,
            ),
            ("grid<u8, 0, 3>", TypedCollectionKind::Grid, 0, 0),
            ("bitset<0>", TypedCollectionKind::Bitset, 0, 0),
        ];

        for (source, expected_kind, expected_bytes, expected_capacity) in cases {
            let descriptor = table
                .parse_typed_collection_descriptor(source)
                .expect("zero capacity remains a valid type")
                .expect("recognized zero-capacity collection");
            assert_eq!(descriptor.kind, expected_kind, "{source}");
            assert_eq!(descriptor.static_size_bytes, expected_bytes, "{source}");
            assert_eq!(descriptor.capacity, expected_capacity, "{source}");
            assert_eq!(descriptor.canonical_type_name(&table), source, "{source}");
            assert!(
                descriptor
                    .lanes
                    .iter()
                    .filter(|lane| lane.element_count == 0)
                    .all(|lane| lane.byte_size == 0),
                "zero-capacity payload lanes must have zero size for {source}"
            );
        }

        assert!(table
            .parse_typed_collection_descriptor("Buffer<i32, 2>")
            .expect("ordinary generic applications remain outside this descriptor")
            .is_none());
        assert!(table
            .parse_typed_collection_descriptor("Buffer<i32, 4>[2]")
            .expect("ordinary generic array applications remain outside this descriptor")
            .is_none());
    }

    #[test]
    fn typed_collection_descriptor_rejects_invalid_contracts_and_legacy_policies() {
        let table = TypeTable::new();
        for (source, expected) in [
            ("map<f32, i32, 2>", "key type"),
            ("pool<i32, -1>", "nonnegative decimal"),
            ("queue<i32, N>", "nonnegative decimal"),
            ("pool<Entity, 2>", "supported scalar"),
            ("pool<i32, 2, unknown>", "expects 2 arguments"),
        ] {
            let error = table
                .parse_typed_collection_descriptor(source)
                .expect_err("invalid typed collection must be rejected");
            assert!(error.contains(expected), "{source}: {error}");
        }

        let legacy_cases = [
            ("pool<i32, 2>", TypedCollectionKind::Pool),
            ("stable_pool<u16, 3>", TypedCollectionKind::StablePool),
            ("queue<f64, 2>", TypedCollectionKind::Queue),
            ("ring_buffer<u8, 4>", TypedCollectionKind::RingBuffer),
            ("map<u8, f64, 3>", TypedCollectionKind::Map),
            ("set<u32, 3>", TypedCollectionKind::Set),
            ("priority_queue<i32, 2>", TypedCollectionKind::PriorityQueue),
            ("grid<u16, 2, 3>", TypedCollectionKind::Grid),
            ("bitset<33>", TypedCollectionKind::Bitset),
        ];
        for (canonical, kind) in legacy_cases {
            for policy in ["error", "drop_newest", "overwrite_oldest"] {
                let source = format!("{}, {}>", canonical.trim_end_matches('>'), policy);
                let error = table
                    .parse_typed_collection_descriptor(&source)
                    .expect_err("legacy policy syntax must require migration");
                let expected = format!(
                    "typed collection '{source}' uses legacy trailing overflow policy '{policy}'; remove that argument and migrate to '{canonical}'"
                );
                assert_eq!(error, expected, "{kind:?}, {policy}");
            }
        }
    }

    #[test]
    fn typed_collection_application_has_compiler_owned_identity() {
        let mut table = TypeTable::new();
        let typed = table
            .resolve_or_intern("pool<i32,2>")
            .expect("typed pool type");
        let same = table
            .resolve_or_intern("pool<i32, 2>")
            .expect("canonical typed pool type");
        let spaced_kind = table
            .resolve_or_intern("pool <i32, 2>")
            .expect("typed pool type with whitespace before '<'");
        let nominal = table
            .resolve_or_intern("PoolLike<i32, 2, error>")
            .expect("ordinary nominal type");

        assert_eq!(typed, same, "source whitespace must not change identity");
        assert_eq!(
            typed, spaced_kind,
            "whitespace before '<' must not change identity"
        );
        assert_eq!(table.resolve("pool <i32, 2>"), Some(typed));
        let legacy = table
            .resolve_or_intern("pool<i32, 2, error>")
            .expect_err("legacy policy must not normalize into canonical identity");
        assert!(legacy.contains("migrate to 'pool<i32, 2>'"), "{legacy}");
        assert_ne!(typed, nominal, "typed pools must not be nominal aliases");
        assert!(table.is_typed_collection_type(typed));
        assert!(!table.is_typed_collection_type(nominal));
        assert!(
            !table.is_i32_abi_compatible(typed),
            "typed collection applications must not use scalar i32 ABI fallback"
        );
        assert_eq!(
            table
                .type_info(typed)
                .expect("typed type info")
                .layout
                .static_size_bytes,
            Some(12)
        );
    }

    #[test]
    fn typed_collection_descriptor_enforces_runtime_dimension_and_layout_limits() {
        let table = TypeTable::new();
        let exact_dimension_limit = table
            .parse_typed_collection_descriptor("grid<u8, 2147483647, 1>")
            .expect("exact i32 dimension product should be representable")
            .expect("recognized grid descriptor");
        assert_eq!(exact_dimension_limit.capacity, 2_147_483_647);
        let error = table
            .parse_typed_collection_descriptor("grid<i32, 2147483647, 2>")
            .expect_err("grid product must be checked");
        assert!(error.contains("i32 runtime index capacity"), "{error}");

        let error = table
            .parse_typed_collection_descriptor("grid<u8, 2147483647, 2>")
            .expect_err("dimension product above runtime index width must be rejected");
        assert!(error.contains("i32 runtime index capacity"), "{error}");

        let fitting = table
            .parse_typed_collection_descriptor("pool<i32, 1073741822>")
            .expect("last fitting u32 static layout")
            .expect("recognized pool descriptor");
        assert_eq!(fitting.static_size_bytes, 4_294_967_292);
        let error = table
            .parse_typed_collection_descriptor("pool<i32, 1073741823>")
            .expect_err("static layout above u32 representation must be rejected");
        assert!(error.contains("u32 layout representation"), "{error}");

        let error = table
            .parse_typed_collection_descriptor("pool<i32, 2, error")
            .expect_err("unbalanced application must be rejected");
        assert!(error.contains("missing '>'"), "{error}");
    }
}
