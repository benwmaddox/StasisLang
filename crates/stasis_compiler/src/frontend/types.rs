use std::collections::HashMap;

pub type TypeId = u16;
pub const TYPE_ID_VOID: TypeId = 0;
pub const TYPE_ID_I32: TypeId = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub size: u16,
    pub flags: u16,
}

#[derive(Debug, Clone)]
pub struct TypeTable {
    types: Vec<TypeInfo>,
    by_name: HashMap<String, TypeId>,
}

impl TypeTable {
    pub fn new() -> Self {
        let mut types = Vec::new();
        let mut by_name = HashMap::new();
        let mut add_builtin = |name: &str, size: u16, flags: u16| {
            let id = types.len() as TypeId;
            types.push(TypeInfo {
                name: name.to_string(),
                size,
                flags,
            });
            by_name.insert(name.to_string(), id);
        };
        // `void` first so missing return annotations can default to 0.
        add_builtin("void", 0, 0);
        add_builtin("i32", 4, 0);
        Self { types, by_name }
    }

    pub fn resolve(&self, name: &str) -> Option<TypeId> {
        self.by_name.get(name).copied()
    }

    pub fn resolve_or_intern(&mut self, name: &str) -> TypeId {
        if let Some(existing) = self.resolve(name) {
            return existing;
        }
        let id = self.types.len() as TypeId;
        self.types.push(TypeInfo {
            name: name.to_string(),
            size: 4,
            flags: 0,
        });
        self.by_name.insert(name.to_string(), id);
        id
    }

    pub fn type_info(&self, id: TypeId) -> Option<&TypeInfo> {
        self.types.get(id as usize)
    }

    pub fn void_id(&self) -> TypeId {
        self.resolve("void").unwrap_or(TYPE_ID_VOID)
    }
}

impl Default for TypeTable {
    fn default() -> Self {
        Self::new()
    }
}
