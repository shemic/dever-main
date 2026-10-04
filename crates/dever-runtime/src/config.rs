use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

mod command;
mod compilation;
pub use command::CommandLimits;
pub use compilation::{CompilationBindings, CompilationSite, DatabaseDriver};

const DATA_DIRECTORIES: [&str; 5] = ["db", "upload", "log", "cache", "tmp"];
const ADAPTER_FIELDS: &[&str] = &["use", "setting", "files", "command"];
static SETTINGS: OnceLock<Settings> = OnceLock::new();
pub const MAX_JOB_TIMEOUT_MS: u32 = 3_600_000;
pub const MAX_JOB_LEASE_MS: i64 = 2 * MAX_JOB_TIMEOUT_MS as i64;

pub struct Settings {
    executable_directory: PathBuf,
    dever: Option<DeverSettings>,
    lib: Vec<String>,
    package: Option<PackageSettings>,
    databases: BTreeMap<String, Database>,
    tenant: Option<TenantSettings>,
    http: Option<serde_json::Map<String, serde_json::Value>>,
    adapter: Option<Box<serde_json::value::RawValue>>,
    auth: AuthSettings,
    sites: BTreeMap<String, SiteSettings>,
    log: LogSettings,
    runtime: Option<RuntimeSettings>,
    job: Option<JobSettings>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageSettings {
    registry: String,
    #[serde(default, rename = "use")]
    roots: Vec<String>,
}

impl PackageSettings {
    pub fn registry(&self) -> &str {
        &self.registry
    }
    pub fn roots(&self) -> &[String] {
        &self.roots
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeverSettings {
    version: String,
}

impl DeverSettings {
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Debug, Default)]
pub struct AuthSettings {
    providers: BTreeMap<String, AuthProvider>,
}

#[derive(Clone, Debug)]
pub struct AuthProvider {
    verify: String,
    #[cfg_attr(not(feature = "api"), allow(dead_code))]
    jwt_secret: crate::secret::Secret,
    ttl_seconds: u64,
    cookie: String,
}

#[derive(Clone, Debug)]
pub struct SiteSettings {
    path: String,
    auth: String,
    hosts: Vec<String>,
    origin: Option<HttpOrigin>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpOrigin {
    scheme: String,
    host: String,
    port: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogSettings {
    #[serde(default = "default_log_level")]
    pub level: LogLevel,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    Api,
    Worker,
    All,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    pub mode: RuntimeMode,
    pub shutdown_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSettings {
    pub workers: usize,
    pub poll_ms: i64,
    pub lease_ms: i64,
    pub retry_base_ms: i64,
    pub retry_max_ms: i64,
}

impl JobSettings {
    pub(crate) fn validate_lease(
        &self,
        database: &Database,
        timeout_ms: i64,
    ) -> Result<(), String> {
        if !(1..=i64::from(MAX_JOB_TIMEOUT_MS)).contains(&timeout_ms) {
            return Err("invalid Job timeout_ms".into());
        }
        // A spare connection cannot bypass SQLite's single writer. The lease must
        // cover the whole bounded handler even while its transaction blocks renewal.
        if matches!(database, Database::Sqlite { .. }) && self.lease_ms <= timeout_ms {
            return Err("job.lease_ms must exceed Job timeout_ms for SQLite".into());
        }
        Ok(())
    }

    pub fn validate(self) -> Result<Self, String> {
        if !(1..=256).contains(&self.workers) {
            return Err("job.workers must be between 1 and 256".into());
        }
        for (field, value, minimum, maximum) in [
            ("poll_ms", self.poll_ms, 1, 60_000),
            ("lease_ms", self.lease_ms, 100, MAX_JOB_LEASE_MS),
            ("retry_base_ms", self.retry_base_ms, 1, 86_400_000),
            ("retry_max_ms", self.retry_max_ms, 1, 86_400_000),
        ] {
            if !(minimum..=maximum).contains(&value) {
                return Err(format!(
                    "job.{field} must be between {minimum} and {maximum}"
                ));
            }
        }
        if self.retry_base_ms > self.retry_max_ms {
            return Err("job.retry_base_ms exceeds retry_max_ms".into());
        }
        Ok(self)
    }

    pub fn for_test() -> Self {
        Self {
            workers: 1,
            poll_ms: 1,
            lease_ms: MAX_JOB_LEASE_MS,
            retry_base_ms: 1000,
            retry_max_ms: 300_000,
        }
    }
}

impl AuthSettings {
    pub fn provider(&self, key: &str) -> Option<&AuthProvider> {
        self.providers.get(key)
    }
}

impl AuthProvider {
    pub fn verify(&self) -> &str {
        &self.verify
    }
    pub fn ttl_seconds(&self) -> u64 {
        self.ttl_seconds
    }
    pub fn cookie(&self) -> &str {
        &self.cookie
    }
    #[cfg(feature = "api")]
    pub(crate) fn jwt_secret(&self) -> &crate::secret::Secret {
        &self.jwt_secret
    }
}

impl SiteSettings {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn auth(&self) -> &str {
        &self.auth
    }
    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }
    pub fn origin(&self) -> Option<&HttpOrigin> {
        self.origin.as_ref()
    }
}

impl HttpOrigin {
    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
}

#[derive(Clone, Debug)]
pub struct HttpSettings {
    pub host: String,
    pub port: i64,
    pub limits: crate::http::Limits,
    pub upload: UploadLimits,
    pub upload_directory: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadLimits {
    pub total_bytes: usize,
    pub file_bytes: usize,
    pub field_bytes: usize,
    pub parts: usize,
    pub fields: usize,
    pub header_bytes: usize,
    pub filename_bytes: usize,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            total_bytes: 67_108_864,
            file_bytes: 33_554_432,
            field_bytes: 65_536,
            parts: 32,
            fields: 16,
            header_bytes: 8192,
            filename_bytes: 255,
        }
    }
}

impl UploadLimits {
    pub fn validate(self) -> Result<(), String> {
        for (name, value, maximum) in [
            ("total_bytes", self.total_bytes, 1_073_741_824),
            ("file_bytes", self.file_bytes, self.total_bytes),
            ("field_bytes", self.field_bytes, 1_048_576),
            ("parts", self.parts, 256),
            ("fields", self.fields, self.parts),
            ("header_bytes", self.header_bytes, 65_536),
            ("filename_bytes", self.filename_bytes, 1024),
        ] {
            if value == 0 || value > maximum {
                return Err(format!(
                    "http.upload.{name} must be between 1 and {maximum}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpDocument {
    host: String,
    port: u16,
    #[serde(default = "default_http_header_bytes")]
    header_bytes: i64,
    #[serde(default = "default_http_body_bytes")]
    body_bytes: i64,
    #[serde(default = "default_http_timeout_ms")]
    timeout_ms: i64,
    #[serde(default = "default_http_connections")]
    connections: i64,
    #[serde(default)]
    upload: UploadLimits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Database {
    Sqlite {
        path: PathBuf,
        tenant_directory: Option<PathBuf>,
        max_connections: usize,
        max_page_size: usize,
    },
    Postgres {
        url: String,
        tenant_database_prefix: Option<String>,
        tls: PostgresTls,
        min_connections: usize,
        max_connections: usize,
        max_page_size: usize,
        wait_timeout_ms: u64,
        io_timeout_ms: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantSettings {
    database: String,
    max_pools: usize,
    idle_timeout_ms: u64,
}

impl TenantSettings {
    pub fn database(&self) -> &str {
        &self.database
    }
    pub fn max_pools(&self) -> usize {
        self.max_pools
    }
    pub fn idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresTls {
    System,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeProfile {
    pub sqlite: bool,
    pub postgres: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default, deserialize_with = "deserialize_present")]
    dever: Option<DeverSettings>,
    #[serde(default)]
    lib: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    package: Option<PackageSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    runtime: Option<RuntimeSettings>,
    #[serde(default, deserialize_with = "deserialize_present")]
    job: Option<JobSettings>,
    #[serde(default, deserialize_with = "deserialize_adapter")]
    adapter: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    auth: AuthDocument,
    #[serde(default)]
    sites: BTreeMap<String, SiteDocument>,
    #[serde(default)]
    performance: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    log: LogSettings,
    #[serde(default)]
    http: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, deserialize_with = "deserialize_database")]
    database: Option<BTreeMap<String, DatabaseDocument>>,
    #[serde(default, deserialize_with = "deserialize_present")]
    tenant: Option<TenantDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantDocument {
    database: String,
    #[serde(default = "default_tenant_max_pools")]
    max_pools: usize,
    #[serde(default = "default_tenant_idle_timeout_ms")]
    idle_timeout_ms: u64,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthDocument {
    #[serde(default)]
    providers: BTreeMap<String, AuthProviderDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthProviderDocument {
    verify: String,
    #[serde(rename = "jwtSecret")]
    jwt_secret: String,
    #[serde(rename = "ttlSeconds", default = "default_auth_ttl_seconds")]
    ttl_seconds: u64,
    #[serde(default = "default_auth_cookie")]
    cookie: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteDocument {
    path: String,
    auth: String,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    origin: Option<String>,
}

fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn deserialize_adapter<'de, D>(
    deserializer: D,
) -> Result<Option<Box<serde_json::value::RawValue>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Preserve an explicit null so the shared wire validator rejects it as a wrong type.
    Box::<serde_json::value::RawValue>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum DatabaseDocument {
    Sqlite {
        path: PathBuf,
        #[serde(default)]
        tenant_directory: Option<PathBuf>,
        #[serde(default = "default_sqlite_connections")]
        max_connections: usize,
        #[serde(default = "default_page_size")]
        max_page_size: usize,
    },
    Postgres {
        url: String,
        #[serde(default)]
        tenant_database_prefix: Option<String>,
        tls: TlsDocument,
        #[serde(default)]
        min_connections: usize,
        #[serde(default = "default_postgres_connections")]
        max_connections: usize,
        #[serde(default = "default_page_size")]
        max_page_size: usize,
        #[serde(default = "default_postgres_wait_timeout_ms")]
        wait_timeout_ms: u64,
        #[serde(default = "default_postgres_io_timeout_ms")]
        io_timeout_ms: u64,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TlsDocument {
    System,
    Disabled,
}

fn deserialize_database<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, DatabaseDocument>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    BTreeMap::deserialize(deserializer).map(Some)
}

pub fn validate_dever_version(version: &str) -> Result<(), String> {
    let mut parts = version.split('.');
    for _ in 0..3 {
        let part = parts
            .next()
            .ok_or_else(|| "dever.version must be an exact major.minor.patch version".to_owned())?;
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
            || part.parse::<u64>().is_err()
        {
            return Err("dever.version must be an exact major.minor.patch version".into());
        }
    }
    if parts.next().is_some() {
        return Err("dever.version must be an exact major.minor.patch version".into());
    }
    Ok(())
}

impl Settings {
    pub fn load_for_executable() -> Result<Self, String> {
        Self::load(&executable_directory()?)
    }

    pub fn load_project(project_root: &Path) -> Result<Self, String> {
        Self::load(project_root)
    }

    fn load(executable_directory: &Path) -> Result<Self, String> {
        let path = executable_directory.join("config/setting.json");
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
        crate::wire::parse(&text).map_err(|error| format!("invalid setting.json: {error}"))?;
        let document: Document = serde_json::from_str(&text)
            .map_err(|error| format!("invalid '{}': {error}", path.display()))?;
        let Document {
            dever,
            lib,
            package,
            runtime,
            job,
            adapter,
            auth,
            sites,
            performance,
            log,
            http,
            database,
            tenant,
        } = document;
        if let Some(dever) = &dever {
            validate_dever_version(dever.version())?;
        }
        if let Some(runtime) = runtime
            && !(1..=300_000).contains(&runtime.shutdown_ms)
        {
            return Err("runtime.shutdown_ms must be between 1 and 300000".into());
        }
        if let Some(job) = job {
            job.validate()?;
        }
        let _ = performance;
        let auth = normalize_auth(auth)?;
        let sites = normalize_sites(sites, &auth)?;
        if database
            .as_ref()
            .is_some_and(|database| !database.contains_key("default"))
        {
            return Err("database.default is required".into());
        }
        let mut databases = BTreeMap::new();
        for (name, database) in database.unwrap_or_default() {
            compilation::validate_database_name(&name)?;
            databases.insert(
                name.clone(),
                normalize_database(&name, database, executable_directory)?,
            );
        }
        let tenant = normalize_tenant(tenant, &databases)?;
        Ok(Self {
            executable_directory: executable_directory.to_path_buf(),
            dever,
            lib,
            package,
            databases,
            tenant,
            http,
            adapter,
            auth,
            sites,
            log,
            runtime,
            job,
        })
    }

    /// HTTP 配置只在启动 API 时必需；普通程序可保留空 http 对象。
    pub fn http(&self) -> Result<HttpSettings, String> {
        let document = self
            .http
            .clone()
            .ok_or("http.host and http.port are required for dever.api.serve()")?;
        let document: HttpDocument = serde_json::from_value(serde_json::Value::Object(document))
            .map_err(|error| format!("invalid http settings: {error}"))?;
        if document.host.parse::<std::net::IpAddr>().is_err() {
            return Err("http.host must be an explicit IPv4 or IPv6 address".into());
        }
        // 验证上限后再监听；缺省只限制资源，不猜测监听地址或端口。
        for (field, value, minimum, maximum) in [
            ("header_bytes", document.header_bytes, 8192, 1_048_576),
            ("body_bytes", document.body_bytes, 0, 67_108_864),
            ("timeout_ms", document.timeout_ms, 1, 300_000),
            ("connections", document.connections, 1, 65_536),
        ] {
            if !(minimum..=maximum).contains(&value) {
                return Err(format!(
                    "http.{field} must be between {minimum} and {maximum}"
                ));
            }
        }
        document.upload.validate()?;
        Ok(HttpSettings {
            host: document.host,
            port: i64::from(document.port),
            upload: document.upload,
            upload_directory: self.executable_directory.join("data/tmp"),
            limits: crate::http::Limits {
                header_bytes: document.header_bytes,
                body_bytes: document.body_bytes,
                timeout_ms: document.timeout_ms,
                connections: document.connections,
                http2: None,
            },
        })
    }

    pub fn services(&self, api: bool, worker: bool) -> Result<RuntimeSettings, String> {
        let runtime = self.runtime.unwrap_or(RuntimeSettings {
            mode: match (api, worker) {
                (true, true) => RuntimeMode::All,
                (false, true) => RuntimeMode::Worker,
                _ => RuntimeMode::Api,
            },
            shutdown_ms: 30_000,
        });
        if matches!(runtime.mode, RuntimeMode::Api | RuntimeMode::All) {
            if !api {
                return Err("runtime.mode requires an API declaration".into());
            }
            self.http()?;
        }
        if matches!(runtime.mode, RuntimeMode::Worker | RuntimeMode::All) {
            if !worker {
                return Err("runtime.mode requires a Job declaration".into());
            }
            self.job
                .ok_or("job settings are required for worker mode")?
                .validate()?;
        }
        Ok(runtime)
    }

    pub fn jobs(&self) -> Result<JobSettings, String> {
        self.job
            .ok_or_else(|| "job settings are required".into())
            .and_then(JobSettings::validate)
    }

    pub fn validate_job_connection(
        &self,
        explicit: Option<&str>,
        root: &str,
        timeout_ms: u32,
    ) -> Result<(), String> {
        let settings = self.jobs()?;
        let (name, database) = self.resolve_database(explicit, root)?;
        let capacity = match database {
            Database::Sqlite {
                max_connections, ..
            }
            | Database::Postgres {
                max_connections, ..
            } => *max_connections,
        };
        // Every handler may own a transaction; control and heartbeat retain one spare slot.
        if capacity <= settings.workers {
            return Err(format!(
                "database.{name}.max_connections must exceed job.workers to reserve a lease-control connection"
            ));
        }
        settings
            .validate_lease(database, i64::from(timeout_ms))
            .map_err(|error| format!("database.{name}: {error}"))
    }

    /// Port-only programs may omit the file when every binding is automatic and has no setting.
    pub fn load_adapter_settings() -> Result<Self, String> {
        let directory = executable_directory()?;
        Self::load_adapter_settings_at(&directory)
    }

    pub(crate) fn load_adapter_settings_at(directory: &Path) -> Result<Self, String> {
        match fs::metadata(directory.join("config/setting.json")) {
            Ok(_) => Self::load(directory),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                executable_directory: directory.to_owned(),
                dever: None,
                lib: Vec::new(),
                package: None,
                databases: BTreeMap::new(),
                tenant: None,
                http: None,
                adapter: None,
                auth: AuthSettings::default(),
                sites: BTreeMap::new(),
                log: LogSettings::default(),
                runtime: None,
                job: None,
            }),
            Err(error) => Err(format!("cannot inspect setting.json: {error}")),
        }
    }

    pub fn validate_adapter_names(&self, identities: &[&str]) -> Result<(), String> {
        if let Some(raw) = &self.adapter {
            crate::wire::parse(raw.get())?.fields(identities)?;
        }
        Ok(())
    }

    pub fn select_adapter(
        &self,
        identity: &str,
        candidates: &[&str],
    ) -> Result<(usize, Option<String>), String> {
        let node = self
            .adapter
            .as_ref()
            .map(|raw| crate::wire::parse(raw.get()))
            .transpose()?;
        let binding = node
            .as_ref()
            .map(|node| node.object())
            .transpose()?
            .and_then(|fields| fields.get(identity));
        let fields = binding
            .map(|node| node.fields(ADAPTER_FIELDS))
            .transpose()?;
        let selected = fields
            .and_then(|fields| fields.get("use"))
            .map(|node| node.text())
            .transpose()?;
        let index = match selected {
            Some(name) => candidates
                .iter()
                .position(|candidate| *candidate == name)
                .ok_or_else(|| format!("unknown Adapter selection for Port '{identity}'"))?,
            None if candidates.len() == 1 => 0,
            None => return Err(format!("adapter.{identity}.use is required")),
        };
        let setting = fields
            .and_then(|fields| fields.get("setting"))
            .map(|node| node.raw().to_owned());
        Ok((index, setting))
    }

    /// File access is a deployment grant for the selected Port, not Worker
    /// business settings. Paths stay below the application and appear at /data.
    #[cfg(feature = "external")]
    pub fn external_grants(
        &self,
        identity: &str,
        capabilities: &[String],
    ) -> Result<Vec<dever_sandbox::Grant>, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FileGrant {
            path: PathBuf,
            #[serde(default)]
            write: bool,
        }
        let Some(adapter) = &self.adapter else {
            return Ok(Vec::new());
        };
        let node = crate::wire::parse(adapter.get())?;
        let Some(binding) = node.object()?.get(identity) else {
            return Ok(Vec::new());
        };
        let binding = binding.fields(ADAPTER_FIELDS)?;
        let Some(files) = binding.get("files") else {
            return Ok(Vec::new());
        };
        if !capabilities.iter().any(|capability| capability == "file") {
            return Err("Adapter file grants require its checked file capability".into());
        }
        let files: BTreeMap<String, FileGrant> = serde_json::from_str(files.raw())
            .map_err(|_| "Adapter files must map names to {path, write} grants")?;
        if files.len() > 64 {
            return Err("Adapter has too many file grants".into());
        }
        let mut grants = Vec::new();
        for (name, grant) in files {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err("Adapter file grant names must contain lowercase letters, digits or underscores".into());
            }
            if grant.path.as_os_str().is_empty()
                || grant.path.is_absolute()
                || grant
                    .path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err("Adapter file grant path must be relative to the application".into());
            }
            let executable_cache = Path::new("data/cache/lib");
            if grant.write
                && (grant.path.starts_with(executable_cache)
                    || executable_cache.starts_with(&grant.path))
            {
                return Err(
                    "Adapter writable file grant overlaps the executable resource cache".into(),
                );
            }
            grants.push(dever_sandbox::Grant {
                source: self.executable_directory.join(grant.path),
                destination: Path::new("/data").join(name),
                writable: grant.write,
            });
        }
        Ok(grants)
    }

    pub fn initialize_data_directories(&self) -> Result<(), String> {
        for name in DATA_DIRECTORIES {
            let path = self.executable_directory.join("data").join(name);
            fs::create_dir_all(&path)
                .map_err(|error| format!("cannot create '{}': {error}", path.display()))?;
        }
        Ok(())
    }

    pub fn database(&self, name: &str) -> Option<&Database> {
        self.databases.get(name)
    }

    /// The launcher consumes this value before dispatch. Runtime keeps it only so
    /// one strict setting.json contract can also be read by standalone artifacts.
    pub fn dever_version(&self) -> Option<&str> {
        self.dever.as_ref().map(DeverSettings::version)
    }

    pub fn external_libs(&self) -> &[String] {
        &self.lib
    }

    pub fn packages(&self) -> Option<&PackageSettings> {
        self.package.as_ref()
    }

    pub fn tenant(&self) -> Option<&TenantSettings> {
        self.tenant.as_ref()
    }

    pub fn auth(&self) -> &AuthSettings {
        &self.auth
    }

    pub fn sites(&self) -> &BTreeMap<String, SiteSettings> {
        &self.sites
    }

    pub fn log(&self) -> LogSettings {
        self.log
    }

    pub fn site_for_directory<'a>(
        &'a self,
        directory: &[&str],
    ) -> Result<Option<(&'a str, &'a SiteSettings)>, String> {
        compilation::select_site(&self.sites, directory, |site| &site.path)
    }

    pub fn databases(&self) -> impl Iterator<Item = (&String, &Database)> {
        self.databases.iter()
    }

    pub fn resolve_database(
        &self,
        explicit: Option<&str>,
        package_root: &str,
    ) -> Result<(&str, &Database), String> {
        compilation::resolve_database(&self.databases, explicit, package_root)
    }

    pub fn profile(&self) -> RuntimeProfile {
        let mut profile = RuntimeProfile::default();
        for database in self.databases.values() {
            match database {
                Database::Sqlite { .. } => profile.sqlite = true,
                Database::Postgres { .. } => profile.postgres = true,
            }
        }
        profile
    }

    pub fn validate_transaction_bindings(
        &self,
        transactions: &[&[(Option<&str>, &str)]],
    ) -> Result<(), String> {
        for transaction in transactions {
            let mut connection = None;
            for (explicit, package_root) in *transaction {
                let name = self.resolve_database(*explicit, package_root)?.0;
                if connection
                    .replace(name)
                    .is_some_and(|current| current != name)
                {
                    return Err("a transaction resolves to multiple database connections".into());
                }
            }
        }
        Ok(())
    }

    pub fn validate_scoped_database_bindings(
        &self,
        bindings: &[(Option<&str>, &str, bool)],
        transactions: &[&[(Option<&str>, &str, bool)]],
    ) -> Result<(), String> {
        for (explicit, root, tenant_scoped) in bindings {
            let (name, database) = self.resolve_database(*explicit, root)?;
            if self.tenant.is_some() && *tenant_scoped {
                match database {
                    Database::Sqlite {
                        tenant_directory: Some(_),
                        ..
                    }
                    | Database::Postgres {
                        tenant_database_prefix: Some(_),
                        ..
                    } => {}
                    Database::Sqlite { .. } => {
                        return Err(format!(
                            "database.{name}.tenant_directory is required by a tenant Model"
                        ));
                    }
                    Database::Postgres { .. } => {
                        return Err(format!(
                            "database.{name}.tenant_database_prefix is required by a tenant Model"
                        ));
                    }
                }
            }
        }
        for transaction in transactions {
            let mut selected: Option<(&str, bool)> = None;
            for (explicit, root, tenant_scoped) in *transaction {
                let name = self.resolve_database(*explicit, root)?.0;
                if let Some((current_name, current_scope)) = selected {
                    if current_name != name {
                        return Err(
                            "a transaction resolves to multiple database connections".into()
                        );
                    }
                    if self.tenant.is_some() && current_scope != *tenant_scoped {
                        return Err("a transaction cannot mix global and tenant Models".into());
                    }
                } else {
                    selected = Some((name, *tenant_scoped));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_databases(
        &self,
        bundled: RuntimeProfile,
        bindings: &[(Option<&str>, &str, bool)],
        transactions: &[&[(Option<&str>, &str, bool)]],
    ) -> Result<(), String> {
        let actual = self.profile();
        if actual.sqlite && !bundled.sqlite {
            return Err("setting.json requires SQLite support; rebuild the application".into());
        }
        if actual.postgres && !bundled.postgres {
            return Err("setting.json requires PostgreSQL support; rebuild the application".into());
        }
        for (explicit, package_root, _) in bindings {
            self.resolve_database(*explicit, package_root)?;
        }
        self.validate_scoped_database_bindings(bindings, transactions)?;
        self.initialize_data_directories()
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) fn validate_database_entry(&self) -> Result<(), String> {
        if self.tenant.is_some() {
            return Err("this application entry does not support tenant database settings".into());
        }
        Ok(())
    }
}

pub fn executable_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the executable: {error}"))?;
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "the executable has no parent directory".to_owned())
}

pub fn bootstrap(
    bundled: RuntimeProfile,
    bindings: &[(Option<&str>, &str)],
    transactions: &[&[(Option<&str>, &str)]],
) -> Result<(), String> {
    let scoped_bindings = bindings
        .iter()
        .map(|(explicit, root)| (*explicit, *root, false))
        .collect::<Vec<_>>();
    let scoped_transactions = transactions
        .iter()
        .map(|transaction| {
            transaction
                .iter()
                .map(|(explicit, root)| (*explicit, *root, false))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let scoped_transaction_refs = scoped_transactions
        .iter()
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    bootstrap_scoped(bundled, &scoped_bindings, &scoped_transaction_refs)
}

pub fn bootstrap_scoped(
    bundled: RuntimeProfile,
    bindings: &[(Option<&str>, &str, bool)],
    transactions: &[&[(Option<&str>, &str, bool)]],
) -> Result<(), String> {
    let settings = Settings::load_for_executable()?;
    settings.prepare_databases(bundled, bindings, transactions)?;
    #[cfg(feature = "sqlite")]
    crate::sqlite::initialize(&settings)?;
    #[cfg(feature = "postgres")]
    crate::postgres::initialize(&settings)?;
    SETTINGS
        .set(settings)
        .map_err(|_| "runtime settings were initialized more than once".to_owned())
}

pub fn settings() -> &'static Settings {
    SETTINGS
        .get()
        .expect("runtime settings are initialized before entry")
}

pub fn try_settings() -> Option<&'static Settings> {
    SETTINGS.get()
}

pub enum CurrentSettings {
    Process(&'static Settings),
    #[cfg(feature = "api")]
    Application(std::sync::Arc<Settings>),
}

impl std::ops::Deref for CurrentSettings {
    type Target = Settings;
    fn deref(&self) -> &Settings {
        match self {
            Self::Process(settings) => settings,
            #[cfg(feature = "api")]
            Self::Application(settings) => settings,
        }
    }
}

pub fn try_current_settings() -> Option<CurrentSettings> {
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        return Some(CurrentSettings::Application(session.settings().clone()));
    }
    SETTINGS.get().map(CurrentSettings::Process)
}

pub fn current_settings() -> CurrentSettings {
    try_current_settings().expect("runtime settings are initialized before entry")
}

pub fn http_settings() -> Result<HttpSettings, String> {
    match try_current_settings() {
        Some(settings) => settings.http(),
        None => Settings::load_for_executable()?.http(),
    }
}

const fn default_http_header_bytes() -> i64 {
    16_384
}

const fn default_http_body_bytes() -> i64 {
    1_048_576
}

const fn default_http_timeout_ms() -> i64 {
    30_000
}

const fn default_http_connections() -> i64 {
    64
}

const fn default_tenant_max_pools() -> usize {
    64
}

const fn default_tenant_idle_timeout_ms() -> u64 {
    300_000
}

fn normalize_database(
    name: &str,
    database: DatabaseDocument,
    executable_directory: &Path,
) -> Result<Database, String> {
    match database {
        DatabaseDocument::Sqlite {
            path,
            tenant_directory,
            max_connections,
            max_page_size,
        } => {
            validate_limit(name, "max_connections", max_connections, 1, 64)?;
            validate_limit(name, "max_page_size", max_page_size, 1, 10_000)?;
            validate_relative_path(name, &path)?;
            if let Some(directory) = &tenant_directory {
                validate_relative_database_path(name, "tenant_directory", directory)?;
            }
            Ok(Database::Sqlite {
                path: executable_directory.join(path),
                tenant_directory: tenant_directory.map(|path| executable_directory.join(path)),
                max_connections,
                max_page_size,
            })
        }
        DatabaseDocument::Postgres {
            url,
            tenant_database_prefix,
            tls,
            min_connections,
            max_connections,
            max_page_size,
            wait_timeout_ms,
            io_timeout_ms,
        } => {
            if url.is_empty() {
                return Err(format!("database.{name}.url must not be empty"));
            }
            validate_limit(name, "max_connections", max_connections, 1, 256)?;
            if min_connections > max_connections {
                return Err(format!(
                    "database.{name}.min_connections must not exceed max_connections"
                ));
            }
            validate_limit(name, "max_page_size", max_page_size, 1, 10_000)?;
            validate_duration(name, "wait_timeout_ms", wait_timeout_ms)?;
            validate_duration(name, "io_timeout_ms", io_timeout_ms)?;
            if let Some(prefix) = &tenant_database_prefix
                && (!valid_name(prefix) || prefix.len() > 43)
            {
                return Err(format!(
                    "database.{name}.tenant_database_prefix must use at most 43 lower_snake_case bytes"
                ));
            }
            Ok(Database::Postgres {
                url,
                tenant_database_prefix,
                tls: match tls {
                    TlsDocument::System => PostgresTls::System,
                    TlsDocument::Disabled => PostgresTls::Disabled,
                },
                min_connections,
                max_connections,
                max_page_size,
                wait_timeout_ms,
                io_timeout_ms,
            })
        }
    }
}

fn normalize_tenant(
    document: Option<TenantDocument>,
    databases: &BTreeMap<String, Database>,
) -> Result<Option<TenantSettings>, String> {
    let Some(document) = document else {
        if databases.values().any(|database| match database {
            Database::Sqlite {
                tenant_directory, ..
            } => tenant_directory.is_some(),
            Database::Postgres {
                tenant_database_prefix,
                ..
            } => tenant_database_prefix.is_some(),
        }) {
            return Err("tenant database targets require top-level tenant settings".into());
        }
        return Ok(None);
    };
    if !valid_name(&document.database) {
        return Err("tenant.database must use lower_snake_case".into());
    }
    if !databases.contains_key(&document.database) {
        return Err(format!(
            "tenant.database references unknown database '{}'",
            document.database
        ));
    }
    if !(1..=1024).contains(&document.max_pools) {
        return Err("tenant.max_pools must be between 1 and 1024".into());
    }
    if !(1_000..=86_400_000).contains(&document.idle_timeout_ms) {
        return Err("tenant.idle_timeout_ms must be between 1000 and 86400000".into());
    }
    Ok(Some(TenantSettings {
        database: document.database,
        max_pools: document.max_pools,
        idle_timeout_ms: document.idle_timeout_ms,
    }))
}

fn validate_limit(
    connection: &str,
    field: &str,
    value: usize,
    minimum: usize,
    maximum: usize,
) -> Result<(), String> {
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "database.{connection}.{field} must be between {minimum} and {maximum}"
        ))
    }
}

fn validate_relative_path(connection: &str, path: &Path) -> Result<(), String> {
    validate_relative_database_path(connection, "path", path)
}

fn validate_relative_database_path(
    connection: &str,
    field: &str,
    path: &Path,
) -> Result<(), String> {
    let valid = !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "database.{connection}.{field} must stay below the executable directory"
        ))
    }
}

fn valid_name(name: &str) -> bool {
    name.starts_with(|character: char| character.is_ascii_lowercase())
        && name.split('_').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn normalize_auth(document: AuthDocument) -> Result<AuthSettings, String> {
    let mut providers = BTreeMap::new();
    for (key, provider) in document.providers {
        compilation::validate_provider(&key, &provider.verify)?;
        if !(32..=4096).contains(&provider.jwt_secret.len()) {
            return Err(format!(
                "auth.providers.{key}.jwtSecret must contain 32 to 4096 bytes"
            ));
        }
        if !(60..=604_800).contains(&provider.ttl_seconds) {
            return Err(format!(
                "auth.providers.{key}.ttlSeconds must be between 60 and 604800"
            ));
        }
        validate_cookie_token(&provider.cookie)
            .map_err(|error| format!("auth.providers.{key}.cookie {error}"))?;
        providers.insert(
            key,
            AuthProvider {
                verify: provider.verify,
                jwt_secret: crate::secret::Secret::from_input(provider.jwt_secret.into_bytes()),
                ttl_seconds: provider.ttl_seconds,
                cookie: provider.cookie,
            },
        );
    }
    Ok(AuthSettings { providers })
}

fn normalize_sites(
    documents: BTreeMap<String, SiteDocument>,
    auth: &AuthSettings,
) -> Result<BTreeMap<String, SiteSettings>, String> {
    let mut sites = BTreeMap::new();
    for (key, site) in documents {
        compilation::validate_site(
            &key,
            &site.path,
            &site.auth,
            auth.provider(&site.auth).is_some(),
        )?;
        let origin = site
            .origin
            .as_deref()
            .map(normalize_http_origin)
            .transpose()
            .map_err(|error| format!("sites.{key}.origin: {error}"))?;
        let mut hosts = Vec::with_capacity(site.hosts.len().max(1));
        for host in site.hosts {
            let normalized = normalize_authority_host(&host)
                .map_err(|error| format!("sites.{key}.hosts: {error}"))?;
            if hosts.iter().any(|existing| existing == &normalized) {
                return Err(format!(
                    "sites.{key}.hosts contains duplicate host '{normalized}'"
                ));
            }
            hosts.push(normalized);
        }
        if let Some(origin) = &origin {
            if hosts.is_empty() {
                hosts.push(origin.host.clone());
            } else if !hosts.iter().any(|host| host == &origin.host) {
                return Err(format!(
                    "sites.{key}.origin host must be present in sites.{key}.hosts"
                ));
            }
        }
        sites.insert(
            key,
            SiteSettings {
                path: site.path,
                auth: site.auth,
                hosts,
                origin,
            },
        );
    }

    compilation::validate_site_paths(
        sites
            .iter()
            .map(|(key, site)| (key.as_str(), site.path.as_str())),
    )?;
    Ok(sites)
}

pub(crate) fn normalize_authority_host(authority: &str) -> Result<String, String> {
    let authority = authority
        .parse::<hyper::http::uri::Authority>()
        .map_err(|_| "host must be an HTTP authority without a scheme or path".to_owned())?;
    if authority.as_str().contains('@') {
        return Err("host cannot contain credentials".into());
    }
    Ok(authority.host().to_ascii_lowercase())
}

fn normalize_http_origin(value: &str) -> Result<HttpOrigin, String> {
    let origin = value
        .parse::<hyper::Uri>()
        .map_err(|_| "must be an absolute HTTP or HTTPS origin".to_owned())?;
    let scheme = origin
        .scheme_str()
        .filter(|scheme| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| "scheme must be http or https".to_owned())?;
    if origin.path() != "/" || origin.query().is_some() {
        return Err("must not contain a path, query, or fragment".into());
    }
    let authority = origin
        .authority()
        .ok_or_else(|| "must contain a host".to_owned())?;
    if authority.as_str().contains('@') {
        return Err("cannot contain credentials".into());
    }
    Ok(HttpOrigin {
        scheme: scheme.to_owned(),
        host: authority.host().to_ascii_lowercase(),
        port: authority
            .port_u16()
            .unwrap_or(if scheme == "https" { 443 } else { 80 }),
    })
}

fn validate_cookie_token(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        ..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
    {
        return Err("must be a valid Cookie name".into());
    }
    Ok(())
}

const fn default_auth_ttl_seconds() -> u64 {
    86_400
}

fn default_auth_cookie() -> String {
    "dever_session".into()
}

const fn default_log_level() -> LogLevel {
    LogLevel::Info
}

const fn default_sqlite_connections() -> usize {
    1
}

const fn default_postgres_connections() -> usize {
    4
}

const fn default_page_size() -> usize {
    100
}

const fn default_postgres_wait_timeout_ms() -> u64 {
    5_000
}

const fn default_postgres_io_timeout_ms() -> u64 {
    30_000
}

fn validate_duration(connection: &str, field: &str, value: u64) -> Result<(), String> {
    if (1..=300_000).contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "database.{connection}.{field} must be between 1 and 300000"
        ))
    }
}
