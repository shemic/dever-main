use dever_core::source::SourceMap;
use std::path::Path;

fn load(root: &Path) -> dever_core::hir::Program {
    let sources = SourceMap::load_project(&root.join("module"), &root.join("test"))
        .unwrap_or_else(|error| panic!("failed to load {}: {error:?}", root.display()));
    dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{} failed:\n{}",
            root.display(),
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

#[test]
fn cms_plain_and_markdown_have_the_same_contracts() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plain = load(&workspace.join("examples/cms/dever"));
    let markdown = load(&workspace.join("examples/cms/md"));
    assert_eq!(plain.api_snapshot(), markdown.api_snapshot());
    assert_eq!(plain.model_snapshot(), markdown.model_snapshot());
}
