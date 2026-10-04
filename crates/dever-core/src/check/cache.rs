use std::cell::RefCell;
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;
use crate::hir::Program;
use crate::source::{SourceMap, SourceOrigin};

type CheckedProgram = Result<Program, Vec<Diagnostic>>;

struct CachedProgram {
    sources: Vec<(PathBuf, PathBuf, SourceOrigin, String)>,
    strict_layout: bool,
    result: CheckedProgram,
}

thread_local! {
    // Keep one compilation only: editors and embedding callers can reuse an unchanged
    // program without retaining every project ever checked by a long-lived process.
    static PREVIOUS: RefCell<Option<CachedProgram>> = const { RefCell::new(None) };
}

pub(super) fn check(
    sources: &SourceMap,
    compute: impl FnOnce(&SourceMap) -> CheckedProgram,
) -> CheckedProgram {
    let cached = PREVIOUS.with_borrow(|previous| {
        let previous = previous.as_ref()?;
        let matches = previous.strict_layout == sources.strict_layout()
            && previous.sources.len() == sources.files().len()
            && previous.sources.iter().zip(sources.files()).all(
                |((path, logical_path, origin, text), source)| {
                    path == source.path()
                        && logical_path == source.logical_path()
                        && *origin == source.origin()
                        && text == source.text()
                },
            );
        matches.then(|| previous.result.clone())
    });
    if let Some(result) = cached {
        return result;
    }
    let result = compute(sources);
    PREVIOUS.with_borrow_mut(|previous| {
        *previous = Some(CachedProgram {
            strict_layout: sources.strict_layout(),
            sources: sources
                .files()
                .iter()
                .map(|source| {
                    (
                        source.path().to_owned(),
                        source.logical_path().to_owned(),
                        source.origin(),
                        source.text().to_owned(),
                    )
                })
                .collect(),
            result: result.clone(),
        });
    });
    result
}
