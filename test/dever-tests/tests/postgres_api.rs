mod support;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use dever_core::source::SourceMap;
use dever_runtime::config::Settings;
use serde_json::{Value, json};

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_directory(&path, &target);
        } else if kind.is_file() {
            fs::copy(path, target).unwrap();
        } else {
            panic!("CMS source must contain only regular files and directories");
        }
    }
}

fn append_fixture(path: &Path, source: &str) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(b"\n").unwrap();
    file.write_all(source.as_bytes()).unwrap();
}

fn copy_cms_project(workspace: &Path, project: &Path) {
    let source = workspace.join("examples/cms/dever");
    fs::create_dir(project).unwrap();
    for directory in ["config", "module", "test"] {
        copy_directory(&source.join(directory), &project.join(directory));
    }
    append_fixture(
        &project.join("module/user/account/app.dever"),
        include_str!("../fixtures/postgres_api/account_app.dever"),
    );
    append_fixture(
        &project.join("module/user/membership/app.dever"),
        include_str!("../fixtures/postgres_api/membership_app.dever"),
    );
    append_fixture(
        &project.join("module/user/account/api.dever"),
        include_str!("../fixtures/postgres_api/account_api.dever"),
    );
}

#[test]
fn postgres_member_fixture_checks_without_a_database() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temporary = support::temp::TemporaryDirectory::new();
    let project = temporary.path().join("project");
    copy_cms_project(&workspace, &project);
    let sources = SourceMap::load_project(&project.join("module"), &project.join("test")).unwrap();
    let settings = Settings::load_project(&project).unwrap();
    let program = dever_core::check_with_settings(&sources, &settings).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let application_errors = program.application_errors().unwrap();
    assert!(application_errors.is_empty(), "{application_errors:?}");
    assert!(program.api_snapshot().contains("fixture_member"));
}

fn save_report(workspace: &Path, report: &Value) -> Result<PathBuf, String> {
    let directory = workspace.join("target/performance/postgres-http-09-30-v1");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = if directory.join("report.json").exists() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        directory.join(format!("report-{}-{nanos}.json", std::process::id()))
    } else {
        directory.join("report.json")
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    let document = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    file.write_all(&document)
        .map_err(|error| error.to_string())?;
    Ok(path)
}

fn result_directory(workspace: &Path) -> PathBuf {
    let parent = workspace.join("target/performance/postgres-http-09-30-v1");
    fs::create_dir_all(&parent).unwrap();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let run = parent.join(format!("run-{}-{nanos}", std::process::id()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&run).unwrap();
    run.join("result")
}

#[test]
#[ignore = "requires an isolated PostgreSQL instance configured in config/setting.json"]
fn postgres_cms_http_isolates_two_tenant_databases() {
    let summary = support::postgres::run_in_isolated_databases("postgres_api", |setting| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let compiler = workspace.join("target/debug/dever");
        assert!(
            compiler.is_file(),
            "build the current dever before PostgreSQL acceptance"
        );
        let temporary = support::temp::TemporaryDirectory::new();
        let project = temporary.path().join("project");
        copy_cms_project(&workspace, &project);
        let settings_path = project.join("config/setting.json");
        let mut settings: Value =
            serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        settings["database"] = json!({"default": setting.database_setting()});
        fs::write(
            &settings_path,
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();

        let result = result_directory(&workspace);
        let output = Command::new("python3")
            .arg("-B")
            .arg(workspace.join("test/performance/cms.py"))
            .arg("postgres-case")
            .arg("--project")
            .arg(&project)
            .arg("--output")
            .arg(&result)
            .arg("--compiler")
            .arg(&compiler)
            .arg("--articles")
            .arg("2")
            .arg("--rbac-fixture")
            .output()
            .map_err(|error| format!("cannot start PostgreSQL CMS acceptance: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "PostgreSQL CMS acceptance failed; owned logs: {}\n{}\n{}",
                result.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }
        let response: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("invalid PostgreSQL CMS result: {error}"))?;
        if response["status"] != "passed" {
            return Err(format!(
                "PostgreSQL CMS acceptance did not pass: {response}"
            ));
        }
        let report_path = response["report"]
            .as_str()
            .ok_or_else(|| "PostgreSQL CMS result has no report path".to_owned())?;
        let report_path = Path::new(report_path);
        if !report_path.starts_with(&result) {
            return Err("PostgreSQL CMS report is outside the owned result directory".into());
        }
        let report: Value =
            serde_json::from_slice(&fs::read(report_path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if report["status"] != "passed" {
            return Err(format!("PostgreSQL CMS report did not pass: {report}"));
        }
        let tenant_a = &report["tenants"]["tenant_a"];
        let tenant_b = &report["tenants"]["tenant_b"];
        let tenant_a_id = tenant_a["tenant_id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| "tenant A has no positive ID".to_owned())?;
        let tenant_b_id = tenant_b["tenant_id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| "tenant B has no positive ID".to_owned())?;
        let user_a_id = tenant_a["user_id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| "tenant A has no positive user ID".to_owned())?;
        let user_b_id = tenant_b["user_id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| "tenant B has no positive user ID".to_owned())?;
        if tenant_a_id == tenant_b_id || user_a_id == user_b_id {
            return Err("tenant A and B resolved to the same identity".into());
        }
        let titles_a = tenant_a["contract"]["titles"]
            .as_array()
            .ok_or_else(|| "tenant A has no title list".to_owned())?;
        let titles_b = tenant_b["contract"]["titles"]
            .as_array()
            .ok_or_else(|| "tenant B has no title list".to_owned())?;
        if titles_a.len() != 2
            || titles_b.len() != 2
            || titles_a == titles_b
            || tenant_a["contract"]["articles"] != 2
            || tenant_b["contract"]["articles"] != 2
            || tenant_a["contract"]["duplicate_publish_status"] != 409
            || tenant_b["contract"]["duplicate_publish_status"] != 409
            || tenant_a["slugs"] != json!(["bench-0", "bench-1"])
            || tenant_b["slugs"] != json!(["bench-0", "bench-1"])
            || report["cross_site_cookie_rejected"] != true
            || report["cross_tenant_login_rejected"] != true
        {
            return Err("PostgreSQL CMS tenant or HTTP contract is incomplete".into());
        }
        let rbac = &report["rbac"];
        let member_user_id = rbac["member_user_id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| "RBAC fixture has no positive non-Owner user ID".to_owned())?;
        if member_user_id == user_a_id || member_user_id == user_b_id {
            return Err("RBAC fixture selected an Owner identity".into());
        }
        let permission_names = ["admin_read", "admin_permissions", "front_list"];
        let mut permission_keys = Vec::new();
        for name in permission_names {
            let key = rbac["permissions"][name]
                .as_str()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| format!("RBAC report has no {name} permission key"))?;
            if permission_keys.contains(&key) {
                return Err("RBAC report reused one permission key for separate routes".into());
            }
            permission_keys.push(key);
        }
        let checks = [
            "before_grant_denied",
            "both_admin_roles_allowed",
            "revoked_role_denied",
            "second_role_still_allowed",
            "front_same_named_role_still_allowed",
        ];
        for check in checks {
            if rbac[check] != true {
                return Err(format!("RBAC HTTP check did not pass: {check}"));
            }
        }
        let summary = json!({
            "status": "passed",
            "source": report["source"],
            "binary": report["binary"],
            "ready_seconds": report["ready_seconds"],
            "scope": report["scope"],
            "tenants": {
                "tenant_a": {
                    "tenant_id": tenant_a_id, "user_id": user_a_id,
                    "articles": 2, "titles": titles_a, "slugs": tenant_a["slugs"],
                    "duplicate_publish_status": 409,
                },
                "tenant_b": {
                    "tenant_id": tenant_b_id, "user_id": user_b_id,
                    "articles": 2, "titles": titles_b, "slugs": tenant_b["slugs"],
                    "duplicate_publish_status": 409,
                },
            },
            "cross_site_cookie_rejected": true,
            "cross_tenant_login_rejected": true,
            "rbac": {
                "member_user_id": member_user_id,
                "permissions": {
                    "admin_read": permission_keys[0],
                    "admin_permissions": permission_keys[1],
                    "front_list": permission_keys[2],
                },
                "before_grant_denied": true,
                "both_admin_roles_allowed": true,
                "revoked_role_denied": true,
                "second_role_still_allowed": true,
                "front_same_named_role_still_allowed": true,
            },
        });
        Ok(summary)
    });
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let saved = save_report(&workspace, &summary).unwrap();
    println!("PostgreSQL CMS acceptance: {}", saved.display());
    println!("{summary}");
}
