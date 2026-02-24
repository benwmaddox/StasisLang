use crate::compiler::{CompileReport, CompileResult, Compiler, FunctionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AotArtifact {
    pub function_id: FunctionId,
    pub object_index: u32,
    pub body_hash: u64,
}

#[derive(Debug, Default)]
pub struct AotProcess {
    compiler: Compiler,
    next_object_index: u32,
    artifacts: Vec<AotArtifact>,
}

impl AotProcess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.compiler.upsert_file(path, content);
    }

    pub fn compile(&mut self) -> CompileResult<CompileReport> {
        let (compiler, next_object_index, artifacts) = (
            &mut self.compiler,
            &mut self.next_object_index,
            &mut self.artifacts,
        );
        compiler.compile_with(|meta, _hir| {
            let object_index = *next_object_index;
            *next_object_index = next_object_index.saturating_add(1);
            artifacts.retain(|artifact| artifact.function_id != meta.id);
            artifacts.push(AotArtifact {
                function_id: meta.id,
                object_index,
                body_hash: meta.body_hash,
            });
            Ok(())
        })
    }

    pub fn artifacts(&self) -> &[AotArtifact] {
        &self.artifacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aot_process_runs_full_compile_and_records_objects() {
        let mut process = AotProcess::new();
        process.upsert_file("sample.stasis", "function main(): i32 { return 7; }\n");
        let report = process.compile().expect("aot compile");
        assert_eq!(report.index.parsed_functions, 1);
        assert_eq!(report.emit.emitted_functions, 1);
        assert_eq!(process.artifacts().len(), 1);
    }
}
