use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_compiler::compiler::{CompileError, Compiler};
use stasis_compiler::{SourceDiagnostic, SourceDiagnosticCode};

const PATH: &str = "contracts/shared_value_contracts.stasis";

type ContractResult = Result<(), SourceDiagnostic>;

#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    source: &'static str,
    roots: &'static [&'static str],
}

const SCALAR_RETURNS: Case = Case {
    name: "scalar returns",
    source: r#"
function scalar_i32(): i32 { return 7; }
function scalar_f32(): f32 { return 1.5; }
function scalar_bool(): bool { return true; }

function main(): i32 {
    let converted: i32 = f32_to_i32(scalar_f32());
    if (scalar_bool()) {
        return scalar_i32() + converted;
    }
    return 0;
}
"#,
    roots: &["main"],
};

const NAMED_STRUCT_PARAMETERS_AND_RECEIVERS: Case = Case {
    name: "named-struct parameters and receivers",
    source: r#"
struct Item { score: i32; }
global item: Item;

function read(value: Item): i32 { return value.score; }
function bump(self: Item, amount: i32): void { self.score += amount; }

function main(): i32 {
    item.score = 40;
    item.bump(2);
    return read(item);
}
"#,
    roots: &["main"],
};

const FIXED_STORED_ARRAYS: Case = Case {
    name: "fixed stored arrays",
    source: r#"
struct Scores { values: i32[2]; }
global scores: Scores;

function main(): i32 {
    scores.values[0] = 20;
    scores.values[1] = 22;
    return scores.values[0] + scores.values[1];
}
"#,
    roots: &["main"],
};

const ORDINARY_STRUCT_RETURN: Case = Case {
    name: "ordinary struct return",
    source: r#"
struct Item { score: i32; }
global item: Item;

function get_item(): Item /* result contract */ { return item; }
function main(): i32 { return 0; }
"#,
    roots: &["main", "get_item"],
};

const RECEIVER_STRUCT_RETURN: Case = Case {
    name: "receiver struct return",
    source: r#"
struct Item { score: i32; }
global item: Item;

function copy(self: Item): Item { return self; }
function main(): i32 { return 0; }
"#,
    roots: &["main", "copy"],
};

const LIFECYCLE_STRUCT_RETURN: Case = Case {
    name: "lifecycle struct return",
    source: r#"
struct Item { score: i32; }
global item: Item;

function tick(): Item { return item; }
function main(): i32 { return 0; }
"#,
    roots: &["main", "tick"],
};

const EXTERN_STRUCT_RETURN: Case = Case {
    name: "extern struct return",
    source: r#"
struct Item { score: i32; }

extern function host_item(): Item /* result contract */;
function main(): i32 { return 0; }
"#,
    roots: &["main"],
};

const GENERIC_STRUCT_RETURN: Case = Case {
    name: "generic first-parameter-bound struct return",
    source: r#"
struct Box<T: type> { value: T; }
struct Item { score: i32; }
global item_box: Box<Item>;

function first(box: Box<T>): T { return box.value; }
function main(): i32 {
    first(item_box);
    return 0;
}
"#,
    roots: &["main"],
};

const NAMED_STRUCT_VIEW_RETURN: Case = Case {
    name: "named-struct view return",
    source: r#"
struct Item { score: i32; }
global items: Item[2];

function view(): Item[] { return items; }
function main(): i32 { return 0; }
"#,
    roots: &["main", "view"],
};

const NAMED_STRUCT_FIXED_ARRAY_RETURN: Case = Case {
    name: "named-struct fixed-array return",
    source: r#"
struct Item { score: i32; }
global items: Item[2];

function fixed(): Item[2] { return items; }
function main(): i32 { return 0; }
"#,
    roots: &["main", "fixed"],
};

const ORDINARY_STORED_VIEW: Case = Case {
    name: "ordinary stored view",
    source: r#"
struct Rig { bones: i32[] /* field contract */; }
global rig: Rig;

function main(): i32 { return 0; }
"#,
    roots: &["main"],
};

const GENERIC_STORED_VIEW: Case = Case {
    name: "generic stored view",
    source: r#"
struct Rig<T: type> { bones: T[]; }
global rig: Rig<i32>;

function main(): i32 { return 0; }
"#,
    roots: &["main"],
};

fn compile_check(case: Case) -> ContractResult {
    let mut compiler = Compiler::new();
    compiler.upsert_file(PATH, case.source);
    match compiler.check() {
        Ok(_) => Ok(()),
        Err(error) => Err(diagnostic_or_panic(
            compiler.last_source_diagnostic(),
            case,
            "Compiler::check",
            error,
        )),
    }
}

fn compile_jit(case: Case) -> ContractResult {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&roots(case));
    process.upsert_file(PATH, case.source);
    match process.compile() {
        Ok(_) => Ok(()),
        Err(error) => Err(diagnostic_or_panic(
            process.last_source_diagnostic(),
            case,
            "JitProcess",
            error,
        )),
    }
}

fn compile_aot(case: Case) -> ContractResult {
    let mut process = AotProcess::new();
    process.set_required_emit_roots(&roots(case));
    process.upsert_file(PATH, case.source);
    match process.compile() {
        Ok(_) => Ok(()),
        Err(error) => Err(diagnostic_or_panic(
            process.last_source_diagnostic(),
            case,
            "AotProcess",
            error,
        )),
    }
}

fn compile_wasm(case: Case) -> ContractResult {
    let mut process = WasmProcess::new();
    process.set_required_emit_roots(&roots(case));
    process.upsert_file(PATH, case.source);
    match process.compile() {
        Ok(_) => Ok(()),
        Err(error) => Err(diagnostic_or_panic(
            process.last_source_diagnostic(),
            case,
            "WasmProcess",
            error,
        )),
    }
}

fn roots(case: Case) -> Vec<String> {
    case.roots.iter().map(|root| (*root).to_string()).collect()
}

fn diagnostic_or_panic(
    diagnostic: Option<&SourceDiagnostic>,
    case: Case,
    backend: &str,
    error: CompileError,
) -> SourceDiagnostic {
    diagnostic.cloned().unwrap_or_else(|| {
        panic!(
            "{backend} rejected '{}' without a source diagnostic: {error:?}",
            case.name
        )
    })
}

fn assert_accepts(case: Case) {
    for (backend, result) in [
        ("Compiler::check", compile_check(case)),
        ("JitProcess", compile_jit(case)),
        ("AotProcess", compile_aot(case)),
        ("WasmProcess", compile_wasm(case)),
    ] {
        assert!(
            result.is_ok(),
            "{} must accept '{}', got {:?}",
            backend,
            case.name,
            result.err()
        );
    }
}

fn assert_rejects(case: Case, code: SourceDiagnosticCode, symbol: &str, type_text: &str) {
    let expected_message = match code {
        SourceDiagnosticCode::UnsupportedStructReturn => format!(
            "named-struct result '{}' is unsupported by the non-owning view ABI; pass it as a parameter and return a scalar or void",
            type_text
        ),
        SourceDiagnosticCode::StoredViewField => {
            format!(
                "struct '{}' field '{}' cannot store view type '{}'; use a fixed-capacity field or keep the view as a parameter/local",
                symbol, "bones", type_text
            )
        }
        other => panic!("unexpected shared value contract code {other:?}"),
    };
    let span_marker = match code {
        SourceDiagnosticCode::UnsupportedStructReturn => format!("): {type_text}"),
        SourceDiagnosticCode::StoredViewField => format!("bones: {type_text}"),
        _ => unreachable!("code was checked above"),
    };
    let marker_offset = span_marker
        .find(type_text)
        .expect("span marker contains expected type text");
    let expected_start = case
        .source
        .find(&span_marker)
        .map(|start| start + marker_offset)
        .unwrap_or_else(|| {
            panic!(
                "missing expected span marker '{span_marker}' in {}",
                case.name
            )
        });
    let expected_end = expected_start + type_text.len();

    let diagnostics = [
        ("Compiler::check", compile_check(case)),
        ("JitProcess", compile_jit(case)),
        ("AotProcess", compile_aot(case)),
        ("WasmProcess", compile_wasm(case)),
    ]
    .into_iter()
    .map(|(backend, result)| {
        let diagnostic = match result {
            Ok(()) => panic!("{backend} unexpectedly accepted '{}'", case.name),
            Err(diagnostic) => diagnostic,
        };
        assert_eq!(
            diagnostic.code, code,
            "{backend} diagnostic code for '{}': {diagnostic:?}",
            case.name
        );
        assert_eq!(
            diagnostic.path, PATH,
            "{backend} diagnostic path for '{}': {diagnostic:?}",
            case.name
        );
        assert_eq!(
            diagnostic.start, expected_start,
            "{backend} diagnostic start for '{}': {diagnostic:?}",
            case.name
        );
        assert_eq!(
            diagnostic.end, expected_end,
            "{backend} diagnostic end for '{}': {diagnostic:?}",
            case.name
        );
        assert_eq!(
            diagnostic.symbol, symbol,
            "{backend} diagnostic symbol for '{}': {diagnostic:?}",
            case.name
        );
        assert_eq!(
            diagnostic.message, expected_message,
            "{backend} diagnostic message for '{}': {diagnostic:?}",
            case.name
        );
        diagnostic
    })
    .collect::<Vec<_>>();

    assert!(
        diagnostics.windows(2).all(|pair| pair[0] == pair[1]),
        "diagnostic parity failed for '{}': {diagnostics:?}",
        case.name
    );
}

#[test]
fn shared_value_contract_accepts_scalars_named_parameters_receivers_and_fixed_arrays() {
    for case in [
        SCALAR_RETURNS,
        NAMED_STRUCT_PARAMETERS_AND_RECEIVERS,
        FIXED_STORED_ARRAYS,
    ] {
        assert_accepts(case);
    }
}

#[test]
fn shared_value_contract_rejects_all_non_owning_struct_results_across_backends() {
    assert_rejects(
        ORDINARY_STRUCT_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "get_item",
        "Item",
    );
    assert_rejects(
        RECEIVER_STRUCT_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "copy",
        "Item",
    );
    assert_rejects(
        LIFECYCLE_STRUCT_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "tick",
        "Item",
    );
    assert_rejects(
        EXTERN_STRUCT_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "host_item",
        "Item",
    );
    assert_rejects(
        GENERIC_STRUCT_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "first",
        "T",
    );
    assert_rejects(
        NAMED_STRUCT_VIEW_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "view",
        "Item[]",
    );
    assert_rejects(
        NAMED_STRUCT_FIXED_ARRAY_RETURN,
        SourceDiagnosticCode::UnsupportedStructReturn,
        "fixed",
        "Item[2]",
    );
}

#[test]
fn shared_value_contract_rejects_ordinary_and_generic_stored_views_across_backends() {
    assert_rejects(
        ORDINARY_STORED_VIEW,
        SourceDiagnosticCode::StoredViewField,
        "Rig",
        "i32[]",
    );
    assert_rejects(
        GENERIC_STORED_VIEW,
        SourceDiagnosticCode::StoredViewField,
        "Rig",
        "T[]",
    );
}
