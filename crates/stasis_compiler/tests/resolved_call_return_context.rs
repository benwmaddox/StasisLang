use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::compiler::CompileError;

#[test]
fn unrelated_enum_return_context_is_rejected_by_backend() {
    let mut process = JitProcess::new();
    process.upsert_file(
        "sample.stasis",
        "enum Horizontal { Left, }\nenum Vertical { Top, }\nfunction horizontal(): Horizontal { return Horizontal.Left; }\nfunction vertical(): Vertical { return horizontal(); }\nfunction main(): i32 { vertical(); return 0; }\n",
    );

    let error = process
        .compile()
        .expect_err("a Horizontal result must not satisfy a Vertical return");
    match error {
        CompileError::Backend(message) => assert!(
            message.contains("returns type") && message.contains("incompatible with expected type"),
            "unexpected backend diagnostic: {message}"
        ),
        other => panic!("expected backend rejection, got {other:?}"),
    }
}
