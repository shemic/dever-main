use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const STANDARD_SOURCE: usize = 1 << (usize::BITS - 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceOrigin {
    Application,
    Test,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub source: SourceId,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum SourceRole {
    App,
    Domain,
    Model,
    Port,
    Adapter,
    Api,
    Job,
}

impl SourceRole {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Domain => "domain",
            Self::Model => "model",
            Self::Port => "port",
            Self::Adapter => "adapter",
            Self::Api => "api",
            Self::Job => "job",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "app" => Some(Self::App),
            "domain" => Some(Self::Domain),
            "model" => Some(Self::Model),
            "port" => Some(Self::Port),
            "adapter" => Some(Self::Adapter),
            "api" => Some(Self::Api),
            "job" => Some(Self::Job),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SourceLayout {
    Main,
    Test {
        component: String,
        domain: String,
        topic: String,
    },
    Role {
        component: String,
        domain: String,
        role: SourceRole,
        topics: Vec<String>,
    },
    Loose,
}

impl SourceLayout {
    pub(crate) fn role(&self) -> Option<SourceRole> {
        match self {
            Self::Role { role, .. } => Some(*role),
            Self::Main | Self::Test { .. } | Self::Loose => None,
        }
    }

    pub(crate) fn domain(&self) -> Option<(&str, &str)> {
        match self {
            Self::Role {
                component, domain, ..
            }
            | Self::Test {
                component, domain, ..
            } => Some((component, domain)),
            Self::Main | Self::Loose => None,
        }
    }

    pub(crate) fn app_prefix(&self) -> Option<String> {
        self.domain()
            .map(|(component, domain)| format!("{component}.{domain}"))
    }
}

impl Span {
    pub(crate) fn through(self, last: Span) -> Self {
        debug_assert_eq!(self.source, last.source);
        Self {
            end: last.end,
            ..self
        }
    }
}

#[derive(Debug)]
pub struct SourceFile {
    id: SourceId,
    path: PathBuf,
    logical_path: PathBuf,
    text: String,
    line_starts: Vec<usize>,
    strict_layout: bool,
    origin: SourceOrigin,
}

impl SourceFile {
    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn logical_path(&self) -> &Path {
        &self.logical_path
    }

    pub(crate) fn origin(&self) -> SourceOrigin {
        self.origin
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_test(&self) -> bool {
        self.origin == SourceOrigin::Test
    }

    pub(crate) fn is_markdown(&self) -> bool {
        is_markdown_path(&self.path)
    }

    pub(crate) fn is_model(&self) -> bool {
        self.layout().is_ok_and(|layout| {
            layout.role() == Some(SourceRole::Model)
                || matches!(layout, SourceLayout::Loose)
                    && self
                        .path
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|name| name == "model")
        })
    }

    pub(crate) fn layout(&self) -> Result<SourceLayout, crate::diagnostic::Diagnostic> {
        let segments = self.path_segments()?;
        if self.origin == SourceOrigin::Test {
            let [component, domain, topic] = segments.as_slice() else {
                return Err(crate::diagnostic::Diagnostic::error(
                    "C003",
                    "test source must be <component>/<domain>/<topic>.dever or .dever.md",
                    self.span(0, 0),
                ));
            };
            return Ok(SourceLayout::Test {
                component: (*component).to_owned(),
                domain: (*domain).to_owned(),
                topic: (*topic).to_owned(),
            });
        }
        if segments.as_slice() == ["main"] {
            return Ok(SourceLayout::Main);
        }
        if segments.len() < 3 {
            return Ok(SourceLayout::Loose);
        }
        let Some(role) = SourceRole::from_name(segments[2]) else {
            return Ok(SourceLayout::Loose);
        };
        Ok(SourceLayout::Role {
            component: segments[0].to_owned(),
            domain: segments[1].to_owned(),
            role,
            topics: segments[3..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        })
    }

    pub(crate) fn strict_layout(&self) -> bool {
        self.strict_layout
    }

    pub(crate) fn package_path(
        &self,
    ) -> Result<crate::syntax::Path, crate::diagnostic::Diagnostic> {
        let segments = self.path_segments()?;
        Ok(segments
            .into_iter()
            .map(|segment| crate::syntax::Name {
                text: segment.to_owned(),
                span: self.span(0, 0),
            })
            .collect())
    }

    fn path_segments(&self) -> Result<Vec<&str>, crate::diagnostic::Diagnostic> {
        let invalid_path = || {
            crate::diagnostic::Diagnostic::error(
                "C003",
                "source must have a relative .dever or .dever.md path below the source root",
                self.span(0, 0),
            )
        };
        let mut segments = self
            .logical_path
            .components()
            .map(|component| match component {
                std::path::Component::Normal(segment) => segment.to_str().ok_or_else(invalid_path),
                _ => Err(invalid_path()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let filename = segments.pop().ok_or_else(invalid_path)?;
        let stem = filename
            .strip_suffix(".dever.md")
            .or_else(|| filename.strip_suffix(".dever"))
            .filter(|stem| !stem.is_empty())
            .ok_or_else(invalid_path)?;
        segments.push(stem);
        Ok(segments)
    }

    /// Returns a one-based line and Unicode scalar column for a byte boundary.
    pub fn position(&self, offset: usize) -> (usize, usize) {
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        let column = self.text[self.line_starts[line]..offset].chars().count() + 1;
        (line + 1, column)
    }

    pub fn line(&self, one_based_line: usize) -> &str {
        let start = self.line_starts[one_based_line - 1];
        let end = self
            .line_starts
            .get(one_based_line)
            .copied()
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\r', '\n'])
    }

    pub(crate) fn span(&self, start: usize, end: usize) -> Span {
        Span {
            source: self.id,
            start,
            end,
        }
    }
}

#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    strict_layout: bool,
}

/// Verified Package source. `path` has the same component-relative identity
/// as a file below the application's `module/` directory.
#[derive(Clone, Debug)]
pub struct PackageSource {
    pub path: PathBuf,
    pub display: PathBuf,
    pub text: String,
}

/// Portable application/test inputs. Standard sources are always compiler-owned.
#[derive(Clone, Debug)]
pub struct CompilationSource {
    pub logical_path: String,
    pub is_test: bool,
    pub text: String,
}

impl SourceMap {
    pub fn compilation_sources(&self) -> Result<Vec<CompilationSource>, String> {
        self.files
            .iter()
            .map(|source| {
                let logical_path = source
                    .logical_path
                    .to_str()
                    .ok_or("compilation source paths must be UTF-8")?
                    .replace('\\', "/");
                Ok(CompilationSource {
                    logical_path,
                    is_test: source.is_test(),
                    text: source.text.clone(),
                })
            })
            .collect()
    }

    pub fn from_compilation_sources(inputs: &[CompilationSource]) -> Result<Self, String> {
        let mut sources = Self {
            strict_layout: true,
            ..Self::default()
        };
        let mut paths = std::collections::BTreeSet::new();
        for input in inputs {
            let path = &input.logical_path;
            if path.is_empty()
                || path.contains(['\\', ':', '\0'])
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || !is_source_path(Path::new(path))
            {
                return Err("compilation source paths must be normalized relative .dever or .dever.md paths".into());
            }
            if !paths.insert((input.is_test, path)) {
                return Err(format!("duplicate compilation source '{path}'"));
            }
            let origin = if input.is_test {
                SourceOrigin::Test
            } else {
                SourceOrigin::Application
            };
            let display = if input.is_test {
                Path::new("test").join(path)
            } else {
                PathBuf::from(path)
            };
            sources.add_source(display, PathBuf::from(path), input.text.clone(), origin);
        }
        finish_load(sources, Vec::new()).map_err(|_| "invalid compilation sources".into())
    }

    pub fn add(&mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> SourceId {
        let path = path.into();
        self.add_source(path.clone(), path, text.into(), SourceOrigin::Application)
    }

    fn add_source(
        &mut self,
        path: PathBuf,
        logical_path: PathBuf,
        text: String,
        origin: SourceOrigin,
    ) -> SourceId {
        let mut line_starts = vec![0];
        let bytes = text.as_bytes();
        line_starts.extend(bytes.iter().enumerate().filter_map(|(offset, byte)| {
            let newline =
                *byte == b'\n' || (*byte == b'\r' && bytes.get(offset + 1) != Some(&b'\n'));
            newline.then_some(offset + 1)
        }));
        let id = SourceId(self.files.len());
        self.files.push(SourceFile {
            id,
            path,
            logical_path,
            text,
            line_starts,
            strict_layout: self.strict_layout,
            origin,
        });
        id
    }

    pub fn get(&self, id: SourceId) -> &SourceFile {
        if Self::is_standard(id) {
            &standard_sources()[id.0 & !STANDARD_SOURCE]
        } else {
            &self.files[id.0]
        }
    }

    pub fn is_standard(id: SourceId) -> bool {
        id.0 & STANDARD_SOURCE != 0
    }

    pub(crate) fn all_files(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter().chain(standard_sources())
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    pub(crate) fn strict_layout(&self) -> bool {
        self.strict_layout
    }

    pub fn load(root: &Path) -> Result<Self, Vec<LoadError>> {
        Self::load_with_packages(root, None, &[])
    }

    pub fn load_with_packages(
        module_root: &Path,
        test_root: Option<&Path>,
        packages: &[PackageSource],
    ) -> Result<Self, Vec<LoadError>> {
        let mut sources = Self {
            strict_layout: true,
            ..Self::default()
        };
        let mut errors = Vec::new();
        let package_only = !packages.is_empty()
            && fs::symlink_metadata(module_root)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if !package_only {
            sources.load_root(module_root, None, SourceOrigin::Application, &mut errors);
        }
        if let Some(test_root) = test_root.filter(|root| root.exists()) {
            sources.load_root(
                test_root,
                Some(Path::new("test")),
                SourceOrigin::Test,
                &mut errors,
            );
        }
        for package in packages {
            if package
                .path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
                || !is_source_path(&package.path)
            {
                errors.push(LoadError {
                    path: package.display.clone(),
                    message: "invalid Package source path".into(),
                });
                continue;
            }
            if sources.files.iter().any(|source| {
                source.origin == SourceOrigin::Application && source.logical_path == package.path
            }) {
                errors.push(LoadError {
                    path: package.display.clone(),
                    message: "Package source collides with another application component".into(),
                });
                continue;
            }
            sources.add_source(
                package.path.clone(),
                package.path.clone(),
                package.text.clone(),
                SourceOrigin::Application,
            );
        }
        finish_load(sources, errors)
    }

    pub fn load_project(module_root: &Path, test_root: &Path) -> Result<Self, Vec<LoadError>> {
        Self::load_with_packages(module_root, Some(test_root), &[])
    }

    fn load_root(
        &mut self,
        root: &Path,
        display_prefix: Option<&Path>,
        origin: SourceOrigin,
        errors: &mut Vec<LoadError>,
    ) {
        let mut paths = Vec::new();
        collect_paths(root, &mut paths, errors);
        paths.sort();
        for path in paths {
            match read_utf8(&path) {
                Ok(text) => {
                    let logical = path
                        .strip_prefix(root)
                        .expect("collected below source root")
                        .to_path_buf();
                    let display = display_prefix
                        .map(|prefix| prefix.join(&logical))
                        .unwrap_or_else(|| logical.clone());
                    self.add_source(display, logical, text, origin);
                }
                Err(error) => errors.push(error),
            }
        }
    }
}

fn finish_load(
    mut sources: SourceMap,
    mut errors: Vec<LoadError>,
) -> Result<SourceMap, Vec<LoadError>> {
    sources
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    for (index, source) in sources.files.iter_mut().enumerate() {
        source.id = SourceId(index);
    }
    errors.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.message.cmp(&right.message))
    });
    if errors.is_empty() {
        Ok(sources)
    } else {
        Err(errors)
    }
}

// Origins are assigned by the compiler, never inferred from user-controlled paths.
fn standard_sources() -> &'static [SourceFile] {
    static SOURCES: OnceLock<Vec<SourceFile>> = OnceLock::new();
    SOURCES.get_or_init(|| {
        let mut sources = SourceMap::default();
        for (path, text) in [
            ("text.dever", include_str!("../../../library/text.dever")),
            (
                "dever/storage.dever",
                include_str!("../../../library/dever/storage.dever"),
            ),
            (
                "dever/auth.dever",
                include_str!("../../../library/dever/auth.dever"),
            ),
            (
                "dever/site.dever",
                include_str!("../../../library/dever/site.dever"),
            ),
            ("int.dever", include_str!("../../../library/int.dever")),
            ("math.dever", include_str!("../../../library/math.dever")),
            (
                "decimal.dever",
                include_str!("../../../library/decimal.dever"),
            ),
            ("float.dever", include_str!("../../../library/float.dever")),
            ("uuid.dever", include_str!("../../../library/uuid.dever")),
            (
                "dever/api.dever",
                include_str!("../../../library/dever/api.dever"),
            ),
            (
                "dever/process.dever",
                include_str!("../../../library/dever/process.dever"),
            ),
            (
                "dever/crypto.dever",
                include_str!("../../../library/dever/crypto.dever"),
            ),
            (
                "dever/time.dever",
                include_str!("../../../library/dever/time.dever"),
            ),
            (
                "dever/task.dever",
                include_str!("../../../library/dever/task.dever"),
            ),
            (
                "dever/json.dever",
                include_str!("../../../library/dever/json.dever"),
            ),
            (
                "dever/json/value.dever",
                include_str!("../../../library/dever/json/value.dever"),
            ),
            (
                "dever/json/number.dever",
                include_str!("../../../library/dever/json/number.dever"),
            ),
            (
                "dever/json/scan.dever",
                include_str!("../../../library/dever/json/scan.dever"),
            ),
            (
                "dever/json/parse.dever",
                include_str!("../../../library/dever/json/parse.dever"),
            ),
            (
                "dever/json/write.dever",
                include_str!("../../../library/dever/json/write.dever"),
            ),
            (
                "dever/http.dever",
                include_str!("../../../library/dever/http.dever"),
            ),
            (
                "dever/sse.dever",
                include_str!("../../../library/dever/sse.dever"),
            ),
            (
                "dever/websocket.dever",
                include_str!("../../../library/dever/websocket.dever"),
            ),
            (
                "dever/job.dever",
                include_str!("../../../library/dever/job.dever"),
            ),
            (
                "dever/tls.dever",
                include_str!("../../../library/dever/tls.dever"),
            ),
            (
                "dever/io.dever",
                include_str!("../../../library/dever/io.dever"),
            ),
            (
                "dever/log.dever",
                include_str!("../../../library/dever/log.dever"),
            ),
            (
                "dever/bytes.dever",
                include_str!("../../../library/dever/bytes.dever"),
            ),
            (
                "dever/net.dever",
                include_str!("../../../library/dever/net.dever"),
            ),
            (
                "dever/id.dever",
                include_str!("../../../library/dever/id.dever"),
            ),
            (
                "dever/database.dever",
                include_str!("../../../library/dever/database.dever"),
            ),
        ] {
            sources.add(path, text);
        }
        for source in &mut sources.files {
            source.id.0 |= STANDARD_SOURCE;
        }
        sources.files
    })
}

pub(crate) fn parsed_standard(
    id: SourceId,
) -> Result<crate::syntax::Package, Vec<crate::diagnostic::Diagnostic>> {
    type Parsed = Result<crate::syntax::Package, Vec<crate::diagnostic::Diagnostic>>;
    static PACKAGES: OnceLock<Vec<Parsed>> = OnceLock::new();
    PACKAGES.get_or_init(|| {
        standard_sources()
            .iter()
            .map(crate::parse_uncached)
            .collect()
    })[id.0 & !STANDARD_SOURCE]
        .clone()
}

#[derive(Debug)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for LoadError {}

fn read_utf8(path: &Path) -> Result<String, LoadError> {
    let bytes = fs::read(path).map_err(|error| LoadError {
        path: path.into(),
        message: error.to_string(),
    })?;
    String::from_utf8(bytes).map_err(|error| LoadError {
        path: path.into(),
        message: format!("invalid UTF-8 at byte {}", error.utf8_error().valid_up_to()),
    })
}

fn collect_paths(root: &Path, paths: &mut Vec<PathBuf>, errors: &mut Vec<LoadError>) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let result = (|| -> std::io::Result<()> {
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                errors.push(LoadError {
                    path: path.clone(),
                    message: "source symbolic links are not supported".into(),
                });
            } else if metadata.is_dir() {
                for entry in fs::read_dir(&path)? {
                    pending.push(entry?.path());
                }
            } else if path == root {
                errors.push(LoadError {
                    path: path.clone(),
                    message: "expected a source directory".into(),
                });
            } else if metadata.is_file() && is_source_path(&path) {
                paths.push(path.clone());
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(LoadError {
                path,
                message: error.to_string(),
            });
        }
    }
}

fn is_source_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "dever")
        || is_markdown_path(path)
}

fn is_markdown_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".dever.md"))
}
