use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use stasis_assets::ResolvedAssetManifest;
use stasis_compiler::backend::assets::{
    validate_asset_references, AssetDiagnostic, AssetValidationResult,
};
use stasis_compiler::backend::program_snapshot::ProgramSnapshot;

pub(crate) const ASSET_DIAGNOSTIC_PREFIX: &str = "stasis_asset_diagnostics:";

fn snapshot_asset_source_dirs(snapshot: &ProgramSnapshot) -> Vec<PathBuf> {
    snapshot
        .module_graph()
        .roots()
        .iter()
        .filter_map(|root| Path::new(root).parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn validate_snapshot_assets(
    project_dir: &Path,
    snapshot: &ProgramSnapshot,
    resolved: Option<&ResolvedAssetManifest>,
) -> Result<AssetValidationResult, String> {
    let manifest_paths = resolved.map(|manifest| {
        manifest
            .assets
            .iter()
            .map(|asset| asset.entry.path.clone())
            .collect()
    });
    let dynamic_paths = resolved
        .map(|manifest| &manifest.dynamic_assets)
        .cloned()
        .unwrap_or_default();
    let source_base_dirs = snapshot_asset_source_dirs(snapshot);
    let validation = validate_asset_references(
        project_dir,
        &source_base_dirs,
        snapshot.asset_references(),
        manifest_paths.as_ref(),
        &dynamic_paths,
    );
    if validation.diagnostics.is_empty() {
        Ok(validation)
    } else {
        Err(format_asset_diagnostics(&validation.diagnostics))
    }
}

pub(crate) fn retain_snapshot_assets(
    project_dir: &Path,
    snapshot: &ProgramSnapshot,
    resolved: &ResolvedAssetManifest,
) -> Result<ResolvedAssetManifest, String> {
    let validation = validate_snapshot_assets(project_dir, snapshot, Some(resolved))?;
    Ok(resolved.retain_paths(&validation.resolved_paths))
}

pub(crate) fn format_asset_diagnostics(diagnostics: &[AssetDiagnostic]) -> String {
    let json = serde_json::to_string(diagnostics)
        .unwrap_or_else(|_| "[{\"code\":\"asset_diagnostic_serialization_failed\"}]".to_string());
    format!("{ASSET_DIAGNOSTIC_PREFIX}{json}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_assets::{
        stable_asset_handle, AssetEntry, AssetFormat, ResolvedAsset, SpriteEncoding,
    };
    use stasis_compiler::backend::aot::AotProcess;

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

    fn retain_from_sources(
        root: &Path,
        sources: &[(String, String)],
        manifest: &ResolvedAssetManifest,
    ) -> ResolvedAssetManifest {
        let mut process = AotProcess::new();
        process
            .set_project_root(root.to_string_lossy().to_string())
            .expect("project root");
        for (path, source) in sources {
            process.upsert_file(
                root.join(path).to_string_lossy().to_string(),
                source.clone(),
            );
        }
        process.compile().expect("compile asset fixture");
        retain_snapshot_assets(
            root,
            process.program_snapshot().expect("program snapshot"),
            manifest,
        )
        .expect("retain snapshot assets")
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
            dynamic_assets: Default::default(),
            assets: vec![
                resolved(&root, "used", "assets/svg/used.svg"),
                resolved(&root, "unused", "assets/svg/unused.svg"),
            ],
        };
        let sources = vec![(
            "src/main.stasis".to_string(),
            concat!(
                "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32; ",
                "function main(): void { ",
                "gfx_load_sprite(\"../assets/svg/used.svg\", 32, 32); ",
                "gfx_load_sprite(\"assets/svg/used.svg\", 32, 32); }"
            )
            .to_string(),
        )];

        let retained = retain_from_sources(&root, &sources, &manifest);
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
            dynamic_assets: Default::default(),
            assets: vec![resolved(&root, "used", "assets/svg/used.svg")],
        };
        let sources = vec![(
            "src/main.stasis".to_string(),
            concat!(
                "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32; ",
                "function main(): void { gfx_load_sprite(\"assets/svg/used.svg\", 32, 32); }"
            )
            .to_string(),
        )];

        let retained = retain_from_sources(&root, &sources, &manifest);
        assert_eq!(retained.assets.len(), 1);
        assert_eq!(retained.assets[0].entry.id, "used");
        std::fs::remove_dir_all(root).ok();
    }
}
