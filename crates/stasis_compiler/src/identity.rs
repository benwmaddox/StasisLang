use std::fmt;

/// Lossless compiler-owned identity for a source declaration.
///
/// The canonical text is suitable for manifests, diagnostics, and tooling. The
/// compact [`FnId`] is derived from this text and must always be collision checked
/// when declarations are collected into one program.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId(String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalSourcePath(String);

pub type FnId = u32;

impl SymbolId {
    pub fn declaration(
        kind: &str,
        source_path: &CanonicalSourcePath,
        qualified_name: &str,
        discriminator: &str,
    ) -> Self {
        Self(format!(
            "v1|{}|{}|{}|{}",
            escape_component(kind),
            escape_component(source_path.as_str()),
            escape_component(qualified_name),
            escape_component(discriminator)
        ))
    }

    pub fn function(
        source_path: &CanonicalSourcePath,
        qualified_name: &str,
        overload_discriminator: &str,
    ) -> Self {
        Self::declaration(
            "function",
            source_path,
            qualified_name,
            overload_discriminator,
        )
    }

    pub fn canonical(&self) -> &str {
        &self.0
    }

    pub fn fn_id(&self) -> FnId {
        fnv1a32(self.0.as_bytes())
    }
}

impl CanonicalSourcePath {
    pub fn project_relative(path: &str) -> Result<Self, String> {
        canonical_source_path(None, path).map(Self)
    }

    pub fn under_project_root(project_root: &str, path: &str) -> Result<Self, String> {
        canonical_source_path(Some(project_root), path).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CanonicalSourcePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for SymbolId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub fn normalize_source_path(path: &str) -> String {
    let mut components = Vec::new();
    let replaced = path.replace('\\', "/");
    let path = replaced.strip_prefix("//?/").unwrap_or(&replaced);
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => components.push(component),
            component => components.push(component),
        }
    }
    components.join("/")
}

/// Converts a source path into the project-relative, host-independent path used
/// by SymbolId. Absolute inputs require an absolute project root and must remain
/// inside it. Canonical identity is case-sensitive on every host.
pub fn canonical_source_path(project_root: Option<&str>, path: &str) -> Result<String, String> {
    let replaced = path.replace('\\', "/");
    if replaced.to_ascii_uppercase().starts_with("//?/UNC/") {
        return Err(format!(
            "UNC source paths are not valid canonical project paths: '{replaced}'"
        ));
    }
    let path = replaced
        .strip_prefix("//?/")
        .unwrap_or(&replaced)
        .to_string();
    if path.starts_with("//") {
        return Err(format!(
            "UNC source paths are not valid canonical project paths: '{path}'"
        ));
    }
    if has_drive_prefix(&path) && !is_rooted(&path) {
        return Err(format!(
            "drive-relative source paths are not supported: '{path}'"
        ));
    }
    let rooted = is_rooted(&path);
    let relative = if rooted {
        let root = project_root.ok_or_else(|| {
            format!("absolute source path requires an explicit project root: '{path}'")
        })?;
        let root = root.replace('\\', "/");
        if !is_rooted(&root) || root.starts_with("//") {
            return Err(format!(
                "project root must be an unambiguous absolute path: '{root}'"
            ));
        }
        let root = normalize_absolute(&root)?;
        let path = normalize_absolute(&path)?;
        let prefix = format!("{}/", root.trim_end_matches('/'));
        if path == root {
            return Err("source path resolves to the project root, not a file".to_string());
        }
        path.strip_prefix(&prefix)
            .ok_or_else(|| format!("source path is outside project root: '{path}'"))?
            .to_string()
    } else {
        path
    };
    normalize_relative(&relative)
}

fn is_rooted(path: &str) -> bool {
    path.starts_with('/') || (has_drive_prefix(path) && path.as_bytes().get(2) == Some(&b'/'))
}

fn has_drive_prefix(path: &str) -> bool {
    path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(1) == Some(&b':')
}

fn normalize_absolute(path: &str) -> Result<String, String> {
    let prefix_len = if path.as_bytes().get(1) == Some(&b':') {
        2
    } else {
        0
    };
    let prefix = &path[..prefix_len];
    let tail = path[prefix_len..].trim_start_matches('/');
    let relative = normalize_relative(tail)?;
    Ok(if prefix_len == 0 {
        format!("/{relative}")
    } else {
        format!("{prefix}/{relative}")
    })
}

fn normalize_relative(path: &str) -> Result<String, String> {
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(format!("source path escapes project root: '{path}'"));
                }
            }
            component if component.contains(':') => {
                return Err(format!(
                    "invalid rooted source path component '{component}'"
                ));
            }
            component => components.push(component),
        }
    }
    if components.is_empty() {
        return Err("canonical source path is empty".to_string());
    }
    Ok(components.join("/"))
}

pub fn overload_discriminator(param_type_names: &[String]) -> String {
    if param_type_names.is_empty() {
        return "()".to_string();
    }
    format!("({})", param_type_names.join(","))
}

fn escape_component(component: &str) -> String {
    component
        .replace('%', "%25")
        .replace('|', "%7c")
        .replace('\\', "%5c")
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_function_identity_normalizes_paths_and_ignores_bodies() {
        let path = CanonicalSourcePath::under_project_root(
            "C:/Games/Brickout",
            "C:\\Games\\Brickout\\src\\main.stasis",
        )
        .expect("rooted path");
        let left = SymbolId::function(&path, "tick", "()");
        let right = SymbolId::function(
            &CanonicalSourcePath::project_relative("src/main.stasis").unwrap(),
            "tick",
            "()",
        );
        assert_eq!(left, right);
        assert_eq!(left.fn_id(), right.fn_id());
        assert_eq!(left.canonical(), "v1|function|src/main.stasis|tick|()");
    }

    #[test]
    fn canonical_paths_are_host_independent_case_sensitive_and_root_bounded() {
        assert_eq!(
            canonical_source_path(Some("C:/Games/Brickout"), "src\\main.stasis").unwrap(),
            "src/main.stasis"
        );
        assert_ne!(
            canonical_source_path(None, "src/Main.stasis").unwrap(),
            canonical_source_path(None, "src/main.stasis").unwrap()
        );
        for invalid in [
            "../main.stasis",
            "C:main.stasis",
            "C:/Other/main.stasis",
            "//server/share/main.stasis",
        ] {
            assert!(canonical_source_path(Some("C:/Games/Brickout"), invalid).is_err());
        }
    }

    #[test]
    fn public_symbol_constructors_require_a_validated_project_relative_path() {
        for invalid in [
            "../main.stasis",
            "C:main.stasis",
            "C:/main.stasis",
            "//host/main.stasis",
        ] {
            assert!(
                CanonicalSourcePath::project_relative(invalid).is_err(),
                "{invalid}"
            );
        }
        let valid = CanonicalSourcePath::project_relative("src/../main.stasis").unwrap();
        assert_eq!(valid.as_str(), "main.stasis");
        assert_eq!(
            SymbolId::function(&valid, "main", "()").canonical(),
            "v1|function|main.stasis|main|()"
        );
    }

    #[test]
    fn overload_discriminator_is_receiver_and_parameter_type_based() {
        assert_eq!(overload_discriminator(&[]), "()");
        assert_eq!(
            overload_discriminator(&["Player".to_string(), "i32".to_string()]),
            "(Player,i32)"
        );
    }
}
