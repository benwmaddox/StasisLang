use std::collections::BTreeSet;

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::compiler::Compiler;
use stasis_compiler::SourceDiagnostic;

type File<'a> = (&'a str, &'a str);

fn compile_jit(files: &[File<'_>]) -> JitProcess {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["main".to_string()]);
    for (path, source) in files {
        process.upsert_file(*path, *source);
    }
    process.compile().expect("compile generic module fixture");
    process
}

fn diagnostic_for(files: &[File<'_>]) -> SourceDiagnostic {
    let mut compiler = Compiler::new();
    for (path, source) in files {
        compiler.upsert_file(*path, *source);
    }
    compiler.check().expect_err("fixture must be rejected");
    compiler
        .last_source_diagnostic()
        .cloned()
        .expect("rejected fixture must provide a source diagnostic")
}

fn generated_specialization_names(process: &JitProcess) -> BTreeSet<String> {
    process
        .program_snapshot()
        .expect("compiled process snapshot")
        .files()
        .iter()
        .flat_map(|file| file.content.lines())
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let kind = words.next()?;
            if kind != "struct" && kind != "function" {
                return None;
            }
            let name = words
                .next()?
                .split(['(', '{', ':'])
                .next()
                .unwrap_or_default();
            name.starts_with("__stasis_").then(|| name.to_string())
        })
        .collect()
}

#[test]
fn local_generic_struct_and_method_shadow_imported_same_name() {
    const MAIN: &str = r#"
import "library.stasis";
struct Box<N: i32> { value: i32; }
global local: Box<4>;
function score(self: Box<N>): i32 { return N + 100; }
function main(): i32 { return local.score(); }
"#;
    const LIBRARY: &str = r#"
struct Box<N: i32> { value: i32; }
function score(self: Box<N>): i32 { return N + 200; }
"#;

    let process = compile_jit(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("local generic method executes"),
        104
    );
}

#[test]
fn directly_imported_generic_method_resolves_bare_and_qualified_calls() {
    const MAIN: &str = r#"
import "library.stasis";
global item: library.Box<4>;
function main(): i32 { return score(item) + library.score(item); }
"#;
    const LIBRARY: &str = r#"
struct Box<N: i32> { value: i32; }
function score(self: Box<N>): i32 { return N + 10; }
"#;

    let process = compile_jit(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("bare and qualified imported calls execute"),
        28
    );
}

#[test]
fn unimported_sole_workspace_generic_is_rejected() {
    const MAIN: &str = r#"
global item: Box<4>;
function main(): i32 { return 0; }
"#;
    const LIBRARY: &str = "struct Box<N: i32> { value: i32; }\n";

    let diagnostic = diagnostic_for(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("Box"));
}

#[test]
fn transitive_import_allows_bare_generic_use() {
    const MAIN: &str = r#"
import "middle.stasis";
global item: Box<4>;
function main(): i32 { return score(item); }
"#;
    const MIDDLE: &str = "import \"leaf.stasis\";\n";
    const LEAF: &str = r#"
struct Box<N: i32> { value: i32; }
function score(self: Box<N>): i32 { return N + 20; }
"#;

    let process = compile_jit(&[
        ("main.stasis", MAIN),
        ("middle.stasis", MIDDLE),
        ("leaf.stasis", LEAF),
    ]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("transitive bare generic call executes"),
        24
    );
}

#[test]
fn transitive_module_qualification_is_rejected_without_a_direct_import() {
    const MAIN: &str = r#"
import "middle.stasis";
global item: Box<4>;
function main(): i32 { return leaf.score(item); }
"#;
    const MIDDLE: &str = "import \"leaf.stasis\";\n";
    const LEAF: &str = r#"
struct Box<N: i32> { value: i32; }
function score(self: Box<N>): i32 { return N + 20; }
"#;

    let diagnostic = diagnostic_for(&[
        ("main.stasis", MAIN),
        ("middle.stasis", MIDDLE),
        ("leaf.stasis", LEAF),
    ]);
    assert_eq!(diagnostic.path, "main.stasis");
    assert!(diagnostic.message.contains("score"));
}

#[test]
fn visible_same_named_generic_structs_are_deterministically_ambiguous() {
    const MAIN: &str = r#"
import "one.stasis";
import "two.stasis";
global item: Box<4>;
function main(): i32 { return 0; }
"#;
    const ONE: &str =
        "struct Box<N: i32> { value: i32; }\nfunction score(self: Box<N>): i32 { return N; }\n";
    const TWO: &str =
        "struct Box<N: i32> { value: i32; }\nfunction score(self: Box<N>): i32 { return N; }\n";

    let left = diagnostic_for(&[
        ("main.stasis", MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ]);
    let right = diagnostic_for(&[
        ("two.stasis", TWO),
        ("one.stasis", ONE),
        ("main.stasis", MAIN),
    ]);
    assert_eq!(left, right);
    assert_eq!(left.path, "main.stasis");
    assert!(left.message.contains("ambiguous generic type"));
}

#[test]
fn modules_can_repeat_constants_structs_generic_structs_and_methods_without_cross_binding() {
    const MAIN: &str = r#"
import "one.stasis";
import "two.stasis";
global one_value: one.Box<4>;
global two_value: two.Box<5>;
function main(): i32 { return one.score(one_value) + two.score(two_value); }
"#;
    const ONE: &str = r#"
const CAP: i32 = 3;
struct Shared { one_marker: i32; }
struct Box<N: i32> { values: i32[CAP]; }
function score(self: Box<N>): i32 { return CAP + N; }
"#;
    const TWO: &str = r#"
const CAP: i32 = 7;
struct Shared { two_marker: i32; }
struct Box<N: i32> { values: i32[CAP]; }
function score(self: Box<N>): i32 { return CAP + N; }
"#;

    let process = compile_jit(&[
        ("main.stasis", MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("qualified module methods execute"),
        19
    );
}

#[test]
fn generic_definition_constant_is_stable_while_call_site_argument_is_inferred() {
    const MAIN: &str = r#"
import "library.stasis";
const CAP: i32 = 11;
global item: library.Box<5>;
function main(): i32 { return library.score(item); }
"#;
    const LIBRARY: &str = r#"
const CAP: i32 = 3;
struct Box<N: i32> { values: i32[CAP]; }
function score(self: Box<N>): i32 { return CAP + N; }
"#;

    let process = compile_jit(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("definition-site constant and call-site argument execute"),
        8
    );
}

#[test]
fn insertion_order_and_unrelated_roots_do_not_change_specialization_names_or_runtime() {
    const MAIN: &str = r#"
import "library.stasis";
global item: library.Box<5>;
function main(): i32 { return library.score(item); }
"#;
    const LIBRARY: &str = r#"
struct Box<N: i32> { values: i32[3]; }
function score(self: Box<N>): i32 { return 3 + N; }
"#;
    const UNRELATED: &str = r#"
struct Box<N: i32> { values: i32[99]; }
function score(self: Box<N>): i32 { return 99 + N + 900; }
"#;

    let baseline = compile_jit(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
    let variant = compile_jit(&[
        ("unrelated.stasis", UNRELATED),
        ("library.stasis", LIBRARY),
        ("main.stasis", MAIN),
    ]);
    assert_eq!(
        baseline
            .execute_i32_noarg_by_name("main")
            .expect("baseline executes"),
        8
    );
    assert_eq!(
        variant
            .execute_i32_noarg_by_name("main")
            .expect("insertion-order variant executes"),
        8
    );
    assert_eq!(
        generated_specialization_names(&baseline),
        generated_specialization_names(&variant)
    );
}

#[test]
fn formatting_and_equivalent_call_qualification_keep_specialization_identity() {
    const BARE_MAIN: &str = r#"
import "library.stasis";
global item: library.Box<5>;
function main(): i32 { return score(item); }
"#;
    const RECEIVER_MAIN: &str = r#"
import "library.stasis";
global item: library.Box<5>;
function main(): i32 { return item.score(); }
"#;
    const COMPACT_LIBRARY: &str =
        "struct Box<N:i32>{value:i32;}\nfunction score(self:Box<N>):i32{return N;}\n";
    const FORMATTED_LIBRARY: &str = r#"
struct Box<N: i32> {
    value: i32;
}

function score(self: Box< N >): i32 {
    return N;
}
"#;

    let bare = compile_jit(&[
        ("main.stasis", BARE_MAIN),
        ("library.stasis", COMPACT_LIBRARY),
    ]);
    let receiver = compile_jit(&[
        ("library.stasis", FORMATTED_LIBRARY),
        ("main.stasis", RECEIVER_MAIN),
    ]);
    assert_eq!(
        bare.execute_i32_noarg_by_name("main")
            .expect("bare spelling executes"),
        5
    );
    assert_eq!(
        receiver
            .execute_i32_noarg_by_name("main")
            .expect("receiver spelling executes"),
        5
    );
    assert_eq!(
        generated_specialization_names(&bare),
        generated_specialization_names(&receiver)
    );
}

#[test]
fn unrelated_duplicate_ordinary_type_does_not_change_specialization_identity() {
    const MAIN: &str = r#"
import "library.stasis";
struct Item { value: i32; }
global item: library.Box<Item>;
function main(): i32 { return library.score(item); }
"#;
    const LIBRARY: &str = r#"
struct Box<T: type> { value: T; }
function score(self: Box<T>): i32 { return 8; }
"#;
    const UNRELATED: &str = "struct Item { other: i32; }\n";

    let baseline = compile_jit(&[("main.stasis", MAIN), ("library.stasis", LIBRARY)]);
    let variant = compile_jit(&[
        ("unrelated.stasis", UNRELATED),
        ("library.stasis", LIBRARY),
        ("main.stasis", MAIN),
    ]);
    assert_eq!(
        baseline
            .execute_i32_noarg_by_name("main")
            .expect("baseline ordinary argument executes"),
        8
    );
    assert_eq!(
        variant
            .execute_i32_noarg_by_name("main")
            .expect("variant ordinary argument executes"),
        8
    );
    assert_eq!(
        generated_specialization_names(&baseline),
        generated_specialization_names(&variant)
    );
}

#[test]
fn ordinary_value_identifier_does_not_trigger_type_ambiguity() {
    const MAIN: &str = r#"
import "one.stasis";
import "two.stasis";
function main(): i32 { let Shared: i32 = 7; return Shared; }
"#;
    const ONE: &str = r#"
struct Shared { one: i32; }
struct Box<N: i32> { value: i32; }
"#;
    const TWO: &str = r#"
struct Shared { two: i32; }
struct Box<N: i32> { value: i32; }
"#;

    let process = compile_jit(&[
        ("main.stasis", MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("ordinary identifier executes"),
        7
    );
}

#[test]
fn definition_site_constant_ambiguity_is_deterministic_and_owned() {
    const MAIN: &str = "import \"library.stasis\";\nfunction main(): i32 { return 0; }\n";
    const LIBRARY: &str = r#"
import "one.stasis";
import "two.stasis";
const WIDTH: i32 = CAP;
struct Box<N: i32> { values: i32[WIDTH]; }
"#;
    const ONE: &str = "const CAP: i32 = 3;\n";
    const TWO: &str = "const CAP: i32 = 7;\n";

    let left = diagnostic_for(&[
        ("main.stasis", MAIN),
        ("library.stasis", LIBRARY),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ]);
    let right = diagnostic_for(&[
        ("two.stasis", TWO),
        ("one.stasis", ONE),
        ("library.stasis", LIBRARY),
        ("main.stasis", MAIN),
    ]);
    assert_eq!(left, right);
    assert_eq!(left.path, "library.stasis");
    assert!(left.message.contains("one.stasis, two.stasis"));
}

#[test]
fn value_generic_argument_is_not_rewritten_as_same_named_ordinary_type() {
    const MAIN: &str = r#"
import "values.stasis";
import "types.stasis";
struct Box<N: i32> { values: i32[N]; }
global item: Box<CAP>;
function score(self: Box<N>): i32 { return N; }
function main(): i32 { return item.score(); }
"#;
    const VALUES: &str = "const CAP: i32 = 3;\n";
    const TYPES: &str = "struct CAP { value: i32; }\n";
    const OTHER: &str = "const CAP: i32 = 9;\n";

    let process = compile_jit(&[
        ("main.stasis", MAIN),
        ("values.stasis", VALUES),
        ("types.stasis", TYPES),
        ("other.stasis", OTHER),
    ]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("value generic argument executes"),
        3
    );
}

#[test]
fn duplicate_constant_rewrite_preserves_fields_and_imported_references() {
    const MAIN: &str = r#"
import "one.stasis";
global value: one.Shared;
function main(): i32 {
    if (CAP == 3) { return CAP + value.CAP; }
    return 0;
}
"#;
    const ONE: &str = r#"
const CAP: i32 = 3;
struct Shared { CAP: i32; }
struct CAP { value: i32; }
enum Marker { CAP = 1, }
struct Box<N: i32> { values: i32[N]; }
"#;
    const TWO: &str = "const CAP: i32 = 7;\n";

    let process = compile_jit(&[
        ("main.stasis", MAIN),
        ("one.stasis", ONE),
        ("two.stasis", TWO),
    ]);
    assert_eq!(
        process
            .execute_i32_noarg_by_name("main")
            .expect("imported constant and same-named field execute"),
        3
    );
}
