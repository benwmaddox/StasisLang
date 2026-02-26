use std::collections::HashMap;

pub type TypeId = u16;
pub const TYPE_ID_VOID: TypeId = 0;
pub const TYPE_ID_I32: TypeId = 1;
pub const TYPE_ID_F32: TypeId = 2;
pub const TYPE_ID_BOOL: TypeId = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BuiltinType {
    Void,
    I32,
    F32,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TypeKey {
    Builtin(BuiltinType),
    Named(String),
    ArrayFixed { element: TypeId, max_len: u32 },
    ArrayView { element: TypeId },
    AsciiFixed { max_len: u32 },
    AsciiView,
    Utf8Fixed { max_len: u32 },
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
        table
    }

    pub fn resolve(&self, type_name: &str) -> Option<TypeId> {
        self.resolve_existing(type_name.trim())
    }

    pub fn resolve_or_intern(&mut self, type_name: &str) -> Result<TypeId, String> {
        self.resolve_or_intern_inner(type_name.trim())
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

        self.intern_named(type_name)
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
        if type_name == "string" {
            return self.by_key.get(&TypeKey::Utf8View).copied();
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
}

fn are_i32_scalar_abi_compatible(argument: &TypeKey, parameter: &TypeKey) -> bool {
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
        ) | (TypeKey::Builtin(BuiltinType::I32), TypeKey::Named(_))
            | (TypeKey::Builtin(BuiltinType::Bool), TypeKey::Named(_))
            | (TypeKey::Named(_), TypeKey::Builtin(BuiltinType::I32))
            | (TypeKey::Named(_), TypeKey::Builtin(BuiltinType::Bool))
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
        Some(TypeKey::Named(name)) if name == "u8"
    )
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
        assert_eq!(table.indexed_element_type_id(utf8_view), Some(TYPE_ID_I32));
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
}
