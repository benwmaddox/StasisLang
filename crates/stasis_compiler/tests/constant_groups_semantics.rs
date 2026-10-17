use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::compiler::Compiler;
use stasis_compiler::SourceDiagnostic;

type File<'a> = (&'a str, &'a str);

fn check_valid(files: &[File<'_>]) {
    let mut compiler = Compiler::new();
    for (path, source) in files {
        compiler.upsert_file(*path, *source);
    }
    compiler
        .check()
        .expect("constant-group fixture should check");
}

fn diagnostic_for(files: &[File<'_>]) -> SourceDiagnostic {
    let mut compiler = Compiler::new();
    for (path, source) in files {
        compiler.upsert_file(*path, *source);
    }
    compiler.check().expect_err("fixture should be rejected");
    compiler
        .last_source_diagnostic()
        .cloned()
        .expect("rejected fixture should provide a source diagnostic")
}

fn assert_diagnostic_in_source(diagnostic: &SourceDiagnostic, files: &[File<'_>]) {
    let source = files
        .iter()
        .find_map(|(path, source)| (*path == diagnostic.path).then_some(*source))
        .expect("diagnostic path belongs to a fixture source");
    assert!(
        diagnostic.start < diagnostic.end,
        "diagnostic must identify source bytes: {diagnostic:?}"
    );
    assert!(
        source.get(diagnostic.start..diagnostic.end).is_some(),
        "diagnostic range must be valid for {}: {diagnostic:?}",
        diagnostic.path
    );
}

#[test]
fn local_group_wins_and_module_qualification_selects_the_imported_owner() {
    const MAIN: &str = r#"
import "library.stasis";
const LIMITS { SELECTED: i32 = 4; }
function main(): i32 {
    let local: i32 = LIMITS.SELECTED;
    let imported: bool = library.LIMITS.SELECTED;
    if (imported) { return local; }
    return 0;
}
"#;
    const LIBRARY: &str = r#"
const LIMITS { SELECTED: bool = true; }
"#;

    check_valid(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
}

#[test]
fn one_visible_group_is_resolved_without_exposing_an_unimported_group() {
    const MAIN: &str = r#"
import "library.stasis";
function main(): i32 {
    let visible: i32 = TUNING.VALUE;
    let exact: i32 = library.TUNING.VALUE;
    return visible + exact;
}
"#;
    const LIBRARY: &str = "const TUNING { VALUE: i32 = 7; }\n";
    const UNIMPORTED: &str = "const TUNING { VALUE: bool = true; }\n";

    check_valid(&[
        ("main.stasis", MAIN),
        ("library.stasis", LIBRARY),
        ("unimported.stasis", UNIMPORTED),
    ]);
}

#[test]
fn a_selected_local_or_exact_module_owner_does_not_fall_back_on_missing_member() {
    const MAIN: &str = r#"
import "library.stasis";
const LIMITS { LOCAL_ONLY: i32 = 3; }
function main(): i32 { return LIMITS.REMOTE_ONLY; }
"#;
    const LIBRARY: &str = "const LIMITS { REMOTE_ONLY: i32 = 8; }\n";
    let files = [("main.stasis", MAIN), ("library.stasis", LIBRARY)];

    let diagnostic = diagnostic_for(&files);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("LIMITS"));
    assert!(diagnostic.message.contains("REMOTE_ONLY"));
    assert_diagnostic_in_source(&diagnostic, &files);

    const EXACT_MAIN: &str = r#"
import "library.stasis";
function main(): i32 { return library.LIMITS.MISSING; }
"#;
    let exact_files = [("main.stasis", EXACT_MAIN), ("library.stasis", LIBRARY)];
    let exact_diagnostic = diagnostic_for(&exact_files);
    assert_eq!(exact_diagnostic.path, "main.stasis");
    assert!(exact_diagnostic.message.contains("LIMITS"));
    assert!(exact_diagnostic.message.contains("MISSING"));
    assert_diagnostic_in_source(&exact_diagnostic, &exact_files);
}

#[test]
fn alias_only_missing_member_is_diagnosed_instead_of_falling_through() {
    const MAIN: &str = r#"
import "library.stasis";
function main(): i32 { return library.MISSING; }
"#;
    const LIBRARY: &str = "const PRESENT: i32 = 1;\n";
    let files = [("main.stasis", MAIN), ("library.stasis", LIBRARY)];

    let diagnostic = diagnostic_for(&files);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("MISSING"));
    assert_diagnostic_in_source(&diagnostic, &files);
}

#[test]
fn alias_without_a_flat_member_can_resolve_one_group_or_enum_candidate() {
    const GROUP_MAIN: &str = r#"
import "library.stasis";
const library { VALUE: i32 = 7; }
function main(): i32 { return library.VALUE; }
"#;
    const LIBRARY: &str = "const OTHER: i32 = 1;\n";
    check_valid(&[("main.stasis", GROUP_MAIN), ("library.stasis", LIBRARY)]);

    const ENUM_MAIN: &str = r#"
import "one.stasis";
import "two.stasis";
function main(): i32 { return one.READY; }
"#;
    const ONE: &str = "const OTHER: i32 = 1;\n";
    const TWO: &str = "enum one { READY = 3, }\n";
    check_valid(&[
        ("main.stasis", ENUM_MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ]);
}

#[test]
fn visible_same_named_groups_are_ambiguous_deterministically() {
    const MAIN: &str = r#"
import "one.stasis";
import "two.stasis";
function main(): i32 { return LIMITS.WIDTH; }
"#;
    const ONE: &str = "const LIMITS { WIDTH: i32 = 1; }\n";
    const TWO: &str = "const LIMITS { WIDTH: i32 = 2; }\n";

    let files = [
        ("main.stasis", MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ];
    let diagnostic = diagnostic_for(&files);
    let reversed = diagnostic_for(&[
        ("two.stasis", TWO),
        ("one.stasis", ONE),
        ("main.stasis", MAIN),
    ]);

    assert_eq!(diagnostic, reversed);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("ambiguous"));
    assert!(diagnostic.message.contains("LIMITS"));
    assert_diagnostic_in_source(&diagnostic, &files);
}

#[test]
fn two_part_module_flat_group_and_enum_candidates_are_ambiguous() {
    const MAIN: &str = r#"
import "one.stasis";
import "two.stasis";
const one { READY: i32 = 1; }
function main(): i32 { return one.READY; }
"#;
    const ONE: &str = "const READY: i32 = 2;\n";
    const TWO: &str = "enum one { READY = 3, }\n";
    let files = [
        ("main.stasis", MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ];

    let diagnostic = diagnostic_for(&files);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("ambiguous"));
    assert!(diagnostic.message.contains("one.READY"));
    assert_diagnostic_in_source(&diagnostic, &files);
}

#[test]
fn members_do_not_leak_into_bare_lookup_and_groups_are_not_values() {
    for (name, body, expected) in [
        (
            "bare-member",
            "function main(): i32 { return VALUE; }",
            "VALUE",
        ),
        (
            "bare-group",
            "function main(): i32 { let value: i32 = LIMITS; return value; }",
            "LIMITS",
        ),
        (
            "member-assignment",
            "function main(): i32 { LIMITS.VALUE = 2; return 0; }",
            "LIMITS",
        ),
    ] {
        let source = format!("const LIMITS {{ VALUE: i32 = 1; }}\n{body}\n");
        let files = [("main.stasis", source.as_str())];
        let diagnostic = diagnostic_for(&files);
        assert_eq!(diagnostic.path, "main.stasis", "{name}");
        assert!(
            diagnostic.message.contains(expected),
            "{name}: {diagnostic:?}"
        );
        assert_diagnostic_in_source(&diagnostic, &files);
    }
}

#[test]
fn runtime_roots_shadow_groups_for_parameters_inferred_locals_foreach_and_globals() {
    const SOURCE: &str = r#"
const ITEM { VALUE: i32 = 99; }
const NODE { VALUE: i32 = 98; }
struct Item { VALUE: i32; }
global items: Item[1];
global NODE: Item;

function parameter(ITEM: Item): i32 { return ITEM.VALUE; }
function inferred(): i32 {
    let ITEM = items[0];
    return ITEM.VALUE;
}
function foreach_item(): i32 {
    let total: i32 = 0;
    foreach (let ITEM in items) { total += ITEM.VALUE; }
    return total;
}
function main(): i32 {
    let ITEM = items[0];
    return parameter(ITEM) + inferred() + foreach_item() + NODE.VALUE;
}
"#;

    check_valid(&[("main.stasis", SOURCE)]);
}

#[test]
fn an_inferred_primitive_local_call_shadows_a_same_named_group() {
    const SOURCE: &str = r#"
const ITEM { VALUE: i32 = 99; }
function make_item(): i32 { return 1; }
function main(): i32 {
    let ITEM = make_item();
    return ITEM.VALUE;
}
"#;

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    jit.upsert_file("main.stasis", SOURCE);
    let error = jit
        .compile()
        .expect_err("an inferred i32 local must not be replaced by ITEM.VALUE");
    let message = format!("{error:?}");
    assert!(
        message.contains("VALUE") || message.contains("ITEM"),
        "compile error should identify the unresolved runtime member path: {message}"
    );
}

#[test]
fn a_runtime_global_root_shadows_an_import_alias_path() {
    const MAIN: &str = r#"
import "library.stasis";
struct Leaf { VALUE: i32; }
struct AliasRoot { OPTIONS: Leaf; }
global library: AliasRoot;
function main(): i32 { return library.OPTIONS.VALUE; }
"#;
    const LIBRARY: &str = "const OPTIONS { VALUE: bool = true; }\n";

    check_valid(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
}

#[test]
fn same_file_duplicate_group_member_and_top_level_names_are_rejected_at_source() {
    const DUPLICATE_MEMBER: &str = r#"
const LIMITS { WIDTH: i32 = 1; WIDTH: i32 = 2; }
function main(): i32 { return 0; }
"#;
    let duplicate_files = [("main.stasis", DUPLICATE_MEMBER)];
    let duplicate = diagnostic_for(&duplicate_files);
    let with_unrelated_root = diagnostic_for(&[
        ("unrelated.stasis", "function other(): i32 { return 0; }\n"),
        ("main.stasis", DUPLICATE_MEMBER),
    ]);
    assert_eq!(duplicate, with_unrelated_root);
    assert_eq!(duplicate.path, "main.stasis");
    assert!(duplicate.start < duplicate.end);
    assert!(duplicate.message.contains("WIDTH"));
    assert_diagnostic_in_source(&duplicate, &duplicate_files);
    assert!(DUPLICATE_MEMBER[duplicate.start..duplicate.end].contains("WIDTH"));

    for (name, declarations) in [
        (
            "duplicate-group",
            "const LIMITS { A: i32 = 1; } const LIMITS { B: i32 = 2; }",
        ),
        (
            "flat-constant-collision",
            "const LIMITS { A: i32 = 1; } const LIMITS: i32 = 2;",
        ),
        (
            "struct-collision",
            "const LIMITS { A: i32 = 1; } struct LIMITS { value: i32; }",
        ),
        (
            "enum-collision",
            "const LIMITS { A: i32 = 1; } enum LIMITS { A, }",
        ),
    ] {
        let source = format!("{declarations}\nfunction main(): i32 {{ return 0; }}\n");
        let files = [("main.stasis", source.as_str())];
        let diagnostic = diagnostic_for(&files);
        assert_eq!(diagnostic.path, "main.stasis", "{name}");
        assert!(
            diagnostic.message.contains("LIMITS"),
            "{name}: {diagnostic:?}"
        );
        assert_diagnostic_in_source(&diagnostic, &files);
    }
}

#[test]
fn grouped_constants_work_in_field_defaults_generic_arguments_and_array_extents() {
    const SOURCE: &str = r#"
const SIZES { WIDTH: i32 = 4; }
const WIDTH: i32 = -1;
enum Mode { Ready = 2, }
const SETTINGS { MODE: Mode = Mode.Ready; }
struct Buffer<WIDTH: i32> { values: i32[WIDTH]; }
struct Defaults { width: i32 = SIZES.WIDTH; mode: Mode = SETTINGS.MODE; }
global buffer: Buffer<SIZES.WIDTH>;
global direct: i32[SIZES.WIDTH];
global defaults: Defaults;
function main(): i32 {
    if (defaults.mode != SETTINGS.MODE) { return -1; }
    let legacy: i32 = WIDTH;
    return buffer.values[0] + direct[0] + defaults.width + legacy;
}
"#;

    check_valid(&[("main.stasis", SOURCE)]);
}

#[test]
fn invalid_group_member_types_string_expressions_and_cycles_are_diagnosed() {
    for (name, declarations, body, expected) in [
        (
            "unsupported-type",
            "const BAD { VALUE: i64 = 1; }",
            "function main(): i32 { return BAD.VALUE; }",
            "supported scalar",
        ),
        (
            "string-expression",
            "const BAD { TEXT: string = \"a\" + \"b\"; }",
            "function main(): i32 { let text: string = BAD.TEXT; return 0; }",
            "literal string initializer",
        ),
        (
            "cycle",
            "const BAD { FIRST: i32 = BAD.SECOND; SECOND: i32 = BAD.FIRST; }",
            "function main(): i32 { return BAD.FIRST; }",
            "cycle",
        ),
    ] {
        let source = format!("{declarations}\n{body}\n");
        let files = [("main.stasis", source.as_str())];
        let diagnostic = diagnostic_for(&files);
        assert_eq!(diagnostic.path, "main.stasis", "{name}");
        assert!(
            diagnostic
                .message
                .to_lowercase()
                .contains(&expected.to_lowercase()),
            "{name}: {diagnostic:?}"
        );
        assert_diagnostic_in_source(&diagnostic, &files);
    }
}

#[test]
fn unused_group_members_are_still_validated() {
    for (name, declaration, expected) in [
        (
            "division-by-zero",
            "const BAD { VALUE: i32 = 1 / 0; }",
            "division by zero",
        ),
        (
            "string-expression",
            "const BAD { TEXT: string = \"a\" + \"b\"; }",
            "literal string initializer",
        ),
        (
            "two-member-cycle",
            "const BAD { FIRST: i32 = BAD.SECOND; SECOND: i32 = BAD.FIRST; }",
            "cycle",
        ),
        (
            "unsupported-type",
            "const BAD { VALUE: i64 = 1; }",
            "supported scalar",
        ),
    ] {
        let source = format!("{declaration}\nfunction main(): i32 {{ return 0; }}\n");
        let files = [("main.stasis", source.as_str())];
        let diagnostic = diagnostic_for(&files);
        assert_eq!(diagnostic.path, "main.stasis", "{name}");
        assert!(
            diagnostic
                .message
                .to_lowercase()
                .contains(&expected.to_lowercase()),
            "{name}: {diagnostic:?}"
        );
        assert_diagnostic_in_source(&diagnostic, &files);
    }
}

#[test]
fn constant_groups_cannot_use_reserved_compiler_names() {
    const SOURCE: &str = r#"
const __stasis_const_deadbeef { VALUE: i32 = 1; }
function main(): i32 { return 0; }
"#;
    let files = [("main.stasis", SOURCE)];

    let diagnostic = diagnostic_for(&files);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("reserved"));
    assert!(diagnostic.message.contains("__stasis_const_"));
    assert_diagnostic_in_source(&diagnostic, &files);
}

#[test]
fn generic_group_arguments_keep_checked_i32_evaluation() {
    const SOURCE: &str = r#"
const SIZES { WIDTH: i32 = 4; }
struct Buffer<N: i32> { values: i32[N]; }
global invalid: Buffer<SIZES.WIDTH + 2147483647>;
function main(): i32 { return 0; }
"#;
    let files = [("main.stasis", SOURCE)];

    let diagnostic = diagnostic_for(&files);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.to_lowercase().contains("overflow"));
    assert_diagnostic_in_source(&diagnostic, &files);
}
