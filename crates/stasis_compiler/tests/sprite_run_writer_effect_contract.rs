use stasis_compiler::backend::jit::JitProcess;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[test]
fn caller_effect_contract_must_name_writer_global_region() {
    let accepted = r#"
import "../../src/stdlib/graphics.stasis";

enum SpriteRef {
    Probe = 1,
}

global effect_writer: SpriteRunWriter;

function @effects(graphics, effect_writer) writer_lifecycle_with_region(): bool {
    if (!effect_writer.reserve(1, -1, 0, 0, 0, 0, 0)) {
        return false;
    }
    if (!effect_writer.write(SpriteRef.Probe, -1, 0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0)) {
        effect_writer.cancel();
        return false;
    }
    if (!effect_writer.finalize(1)) {
        effect_writer.cancel();
        return false;
    }
    return true;
}
"#;
    let mut positive = JitProcess::new();
    positive
        .set_project_root(repository_root().to_string_lossy())
        .expect("set positive effect project root");
    positive.set_required_emit_roots(&["writer_lifecycle_with_region".to_string()]);
    positive.upsert_file(
        "tests/stasis/sprite_run_writer_effect_positive.stasis",
        accepted,
    );
    positive
        .compile()
        .expect("writer global region satisfies caller effect contract");

    let rejected = accepted.replace("@effects(graphics, effect_writer)", "@effects(graphics)");
    let mut negative = JitProcess::new();
    negative
        .set_project_root(repository_root().to_string_lossy())
        .expect("set negative effect project root");
    negative.set_required_emit_roots(&["writer_lifecycle_with_region".to_string()]);
    negative.upsert_file(
        "tests/stasis/sprite_run_writer_effect_negative.stasis",
        rejected,
    );
    let error = negative
        .compile()
        .expect_err("writer global region must be declared by caller effect contract");
    let diagnostic = format!("{error:?}");
    assert!(
        diagnostic.contains("rejects write 'effect_writer.token'"),
        "unexpected effect diagnostic: {diagnostic}"
    );
}
