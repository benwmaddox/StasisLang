use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use stasis_assets::ResolvedAssetManifest;
use stasis_compiler::frontend::lexer::{lex, TokenKind};

pub(crate) fn retain_source_referenced_assets(
    project_dir: &Path,
    source_base_dir: &Path,
    sources: &[(String, String)],
    resolved: &ResolvedAssetManifest,
) -> Result<ResolvedAssetManifest, String> {
    let project_root = project_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize release asset project {}: {error}",
            project_dir.display()
        )
    })?;
    let asset_root = project_root
        .join("assets")
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize release asset root: {error}"))?;
    let source_base = canonical_project_path(&project_root, source_base_dir)?;
    if !source_base.is_dir() {
        return Err(format!(
            "release asset source base must be a directory: {}",
            source_base.display()
        ));
    }
    let mut paths = BTreeSet::new();
    for (relative_source_path, source) in sources {
        for token in lex(source).map_err(|error| {
            format!("failed to scan release asset paths in {relative_source_path}: {error}")
        })? {
            if token.kind != TokenKind::StringLiteral {
                continue;
            }
            let literal: String =
                serde_json::from_str(&source[token.start..token.end]).map_err(|error| {
                    format!("failed to decode string literal in {relative_source_path}: {error}")
                })?;
            let absolute_path = [source_base.join(&literal), project_root.join(&literal)]
                .into_iter()
                .filter_map(|candidate| candidate.canonicalize().ok())
                .find(|candidate| candidate.is_file() && candidate.starts_with(&asset_root));
            let Some(absolute_path) = absolute_path else {
                continue;
            };
            paths.insert(
                absolute_path
                    .strip_prefix(&project_root)
                    .map_err(|_| {
                        format!(
                            "release asset escaped project root: {}",
                            absolute_path.display()
                        )
                    })?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(resolved.retain_paths(&paths))
}

pub(crate) fn load_entry_sources(
    project_dir: &Path,
    entry_file: &Path,
) -> Result<Vec<(String, String)>, String> {
    let project_root = project_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize release asset project {}: {error}",
            project_dir.display()
        )
    })?;
    let entry_path = canonical_project_file(&project_root, entry_file)?;
    let (graph, sources) = stasis_compiler::frontend::module_graph::load_project_module_graph(
        &project_root,
        &entry_path,
    )
    .map_err(|diagnostic| diagnostic.message)?;
    let mut out = Vec::new();
    for relative in graph.modules().keys() {
        if relative.ends_with(".test.stasis") {
            continue;
        }
        let source = sources
            .get(relative)
            .cloned()
            .ok_or_else(|| format!("module graph source missing for {relative}"))?;
        out.push((relative.clone(), source));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(out)
}

fn canonical_project_file(project_root: &Path, file: &Path) -> Result<PathBuf, String> {
    let canonical = canonical_project_path(project_root, file)?;
    if !canonical.is_file() {
        return Err(format!(
            "release entry must be a file: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn canonical_project_path(project_root: &Path, path: &Path) -> Result<PathBuf, String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize release entry {}: {error}",
            candidate.display()
        )
    })?;
    if !canonical.starts_with(project_root) {
        return Err(format!(
            "release entry {} must stay under project directory {}",
            canonical.display(),
            project_root.display()
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_assets::{
        stable_asset_handle, AssetEntry, AssetFormat, ResolvedAsset, SpriteEncoding,
    };

    fn resolved(root: &Path, id: &str, path: &str) -> ResolvedAsset {
        let entry = AssetEntry {
            id: id.to_string(),
            path: path.to_string(),
            content_sha256: "0".repeat(64),
            prepared_from_sha256: None,
            format: AssetFormat::Sprite {
                encoding: SpriteEncoding::Svg,
                width: 32,
                height: 32,
            },
            prepare: None,
            dependencies: vec![],
        };
        ResolvedAsset {
            handle: stable_asset_handle(&entry),
            entry,
            absolute_path: root.join(path),
            byte_length: 1,
        }
    }

    #[test]
    fn keeps_only_literal_assets_in_the_reachable_source_set() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_release_assets_{stamp}"));
        std::fs::create_dir_all(root.join("assets/svg")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("assets/svg/used.svg"), "u").unwrap();
        std::fs::write(root.join("assets/svg/unused.svg"), "x").unwrap();
        let manifest = ResolvedAssetManifest {
            manifest_path: root.join("assets/manifest.json"),
            assets: vec![
                resolved(&root, "used", "assets/svg/used.svg"),
                resolved(&root, "unused", "assets/svg/unused.svg"),
            ],
        };
        let sources = vec![(
            "src/main.stasis".to_string(),
            concat!(
                "function main(): void { ",
                "hero.load_sprite_from(\"../assets/svg/used.svg\", 32, 32); ",
                "hero.load_sprite_from(\"assets/svg/used.svg\", 32, 32); }"
            )
            .to_string(),
        )];

        let retained =
            retain_source_referenced_assets(&root, Path::new("src"), &sources, &manifest).unwrap();
        assert_eq!(retained.assets.len(), 1);
        assert_eq!(retained.assets[0].entry.id, "used");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn project_root_literal_skips_existing_non_asset_source_candidate() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_release_asset_shadow_{stamp}"));
        std::fs::create_dir_all(root.join("assets/svg")).unwrap();
        std::fs::create_dir_all(root.join("src/assets/svg")).unwrap();
        std::fs::write(root.join("assets/svg/used.svg"), "project asset").unwrap();
        std::fs::write(root.join("src/assets/svg/used.svg"), "source shadow").unwrap();
        let manifest = ResolvedAssetManifest {
            manifest_path: root.join("assets/manifest.json"),
            assets: vec![resolved(&root, "used", "assets/svg/used.svg")],
        };
        let sources = vec![(
            "src/main.stasis".to_string(),
            "function main(): void { hero.load_sprite_from(\"assets/svg/used.svg\", 32, 32); }"
                .to_string(),
        )];

        let retained =
            retain_source_referenced_assets(&root, Path::new("src"), &sources, &manifest).unwrap();
        assert_eq!(retained.assets.len(), 1);
        assert_eq!(retained.assets[0].entry.id, "used");
        std::fs::remove_dir_all(root).ok();
    }
}
