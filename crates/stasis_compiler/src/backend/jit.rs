use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitArtifact {
    pub function_id: FunctionId,
    pub slot: u32,
    pub body_hash: u64,
}

#[derive(Debug, Default)]
pub struct JitProcess {
    compiler: Compiler,
    next_slot: u32,
    artifacts: Vec<JitArtifact>,
}

impl JitProcess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let (compiler, next_slot, artifacts) =
            (&mut self.compiler, &mut self.next_slot, &mut self.artifacts);
        compiler.compile_with(|meta, _hir| {
            let slot = *next_slot;
            *next_slot = next_slot.saturating_add(1);
            artifacts.retain(|artifact| artifact.function_id != meta.id);
            artifacts.push(JitArtifact {
                function_id: meta.id,
                slot,
                body_hash: meta.body_hash,
            });
            Ok(())
        })
    }

    pub fn artifacts(&self) -> &[JitArtifact] {
        &self.artifacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jit_process_runs_full_compile_and_records_slots() {
        let mut process = JitProcess::new();
        process.upsert_file(
            "sample.stasis",
            "function helper(): i32 { return 1; }\nfunction main(): i32 { return helper(); }\n",
        );
        let report = process.compile().expect("jit compile");
        assert_eq!(report.index.parsed_functions, 2);
        assert_eq!(report.emit.emitted_functions, 2);
        assert_eq!(process.artifacts().len(), 2);
    }
}
