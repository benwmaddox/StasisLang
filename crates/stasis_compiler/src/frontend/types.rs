use std::collections::HashMap;

pub type TypeId = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: &'static str,
    pub size: u16,
    pub flags: u16,
}

#[derive(Debug, Clone)]
pub struct TypeTable {
    types: Vec<TypeInfo>,
    by_name: HashMap<&'static str, TypeId>,
}

impl TypeTable {
    pub fn new() -> Self {
        let mut types = Vec::new();
        let mut by_name = HashMap::new();
        let mut add_builtin = |name: &'static str, size: u16, flags: u16| {
            let id = types.len() as TypeId;
            types.push(TypeInfo { name, size, flags });
            by_name.insert(name, id);
        };
        // `void` first so missing return annotations can default to 0.
        add_builtin("void", 0, 0);
        add_builtin("i32", 4, 0);
        Self { types, by_name }
    }

    pub fn resolve(&self, name: &str) -> Option<TypeId> {
        self.by_name.get(name).copied()
    }

    pub fn type_info(&self, id: TypeId) -> Option<TypeInfo> {
        self.types.get(id as usize).copied()
    }

    pub fn void_id(&self) -> TypeId {
        self.resolve("void").unwrap_or(0)
    }
}

impl Default for TypeTable {
    fn default() -> Self {
        Self::new()
    }
}
