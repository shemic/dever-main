use dever_core::{check, llvm::emit_kernel, source::SourceMap};

/// Native acceptance always starts with checked source, not hand-written IR.
pub fn lower(source: &str) -> String {
    let mut sources = SourceMap::default();
    sources.add("main.dever", source);
    let program = check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    emit_kernel(&program, &sources, "main.main").unwrap()
}
