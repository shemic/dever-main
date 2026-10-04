use dever_core::source::SourceMap;
#[path = "support/temp.rs"]
mod temp;

fn sources(job: &str, app: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("content/article/job.dever", job);
    sources.add("content/article/app.dever", app);
    sources
}

fn checked(sources: &SourceMap) -> dever_core::hir::Program {
    dever_core::check(sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(sources))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

fn rejected(sources: &SourceMap, message: &str) {
    let errors = dever_core::check(sources).expect_err("invalid Job must be rejected");
    assert!(
        errors.iter().any(|error| error.message.contains(message)),
        "{errors:?}"
    );
}

#[cfg(feature = "sqlite")]
fn clock_at(millis: i64) -> dever_runtime::time::Clock {
    let clock = dever_runtime::time::Clock::for_test();
    clock.advance(millis).unwrap();
    clock
}

const JOB: &str = "database default\njob publish(input: app.Publish) () retry(5) timeout(30000) { app.apply(input) }";
const APP: &str = "type Publish { title: Text }\napply(input: Publish) () {}\ntransaction submit(input: Publish) (id: Id) { id = dever.job.enqueue(job.publish, input, input.title) }";

#[test]
fn static_jobs_and_enqueue_only_transactions_check_and_format() {
    let sources = sources(JOB, APP);
    checked(&sources);
    let source = &sources.files()[0];
    let formatted = dever_core::format::format(source).unwrap();
    let mut round_trip = SourceMap::default();
    let id = round_trip.add("content/article/job.dever", &formatted);
    assert_eq!(
        formatted,
        dever_core::format::format(round_trip.get(id)).unwrap()
    );
}

#[test]
fn jobs_enforce_private_entry_payload_and_policy_boundaries() {
    rejected(
        &sources(
            JOB,
            &APP.replace(
                "dever.job.enqueue(job.publish, input, input.title)",
                "job.publish(input)",
            ),
        ),
        "not callable",
    );
    rejected(
        &sources(&JOB.replace("retry(5)", "retry(0)"), APP),
        "retry must",
    );
    rejected(
        &sources(&JOB.replace("app.Publish", "Text"), APP),
        "payload must",
    );
    rejected(
        &sources(&JOB.replace("app.apply(input)", "model.count()"), APP),
        "unknown",
    );
    rejected(
        &sources(JOB, &APP.replace("title: Text", "title: Secret")),
        "Secret",
    );
    rejected(
        &sources(&JOB.replace("database default\n", ""), APP),
        "database binding",
    );
    rejected(
        &sources(
            &JOB.replace("app.apply(input)", "dever.system.time_now()"),
            APP,
        ),
        "same-domain App",
    );
    rejected(
        &sources(
            &JOB.replace(
                "app.apply(input)",
                "task = run(app.apply(input))\nwait(task)",
            ),
            APP,
        ),
        "same-domain App",
    );

    let mut public_enqueue = sources(JOB, APP);
    public_enqueue.add(
        "content/article/api.dever",
        "public post submit = app.submit",
    );
    rejected(&public_enqueue, "trusted user identity");
}

#[test]
fn queue_boundary_does_not_add_synchronous_recursion() {
    checked(&sources(
        "database default\njob retry_later(input: app.Work) () retry(2) timeout(1000) { id = app.apply(input) }",
        "type Work { key: Text }\napply(input: Work) (id: Id) { id = dever.job.enqueue(job.retry_later, input, input.key) }",
    ));
}

#[test]
fn schedules_validate_utc_fields_and_zero_input_targets() {
    checked(&sources(
        "database default\njob cleanup() () retry(3) timeout(1000) { app.clean() }\nschedule cleanup = \"0 * * * *\"",
        "clean() () {}",
    ));
    rejected(
        &sources(&format!("{JOB}\nschedule publish = \"0 * * * *\""), APP),
        "zero-input",
    );
    rejected(
        &sources(
            "database default\njob cleanup() () retry(3) timeout(1000) {}\nschedule cleanup = \"60 * * * *\"",
            "clean() () {}",
        ),
        "cron field",
    );
}

#[test]
fn nominal_model_id_wire_identity_is_preserved() {
    let mut sources = sources(
        JOB,
        &APP.replace("title: Text", "title: Text\narticle_id: model.id"),
    );
    sources.add(
        "content/article/model.dever",
        "type Article { title: Text }",
    );
    let program = checked(&sources);
    let schema = program
        .wire_schema("content.article.Publish", dever_core::wire::Policy::Job)
        .unwrap();
    assert!(
        schema
            .fingerprint()
            .contains("ModelId(\"content.article.model.id\")")
    );
}

#[test]
fn markdown_job_and_schedule_share_the_source_contract() {
    let markdown = "# 文章清理任务\n\n清理已失效文章。\n\n- 包：`content.article.job`\n- 公开类型：无\n- 公开方法：无\n- 使用：无\n\n## 队列连接\n\n使用默认连接。\n\n- 声明：`database default`\n\n```dever\ndatabase default\n```\n\n## 清理入口\n\n调用领域能力。\n\n- 函数：`cleanup`\n- 输入：无\n- 输出：无\n\n```dever\njob cleanup() () retry(3) timeout(1000) { app.clean() }\n```\n\n## 每小时清理\n\n按 UTC 整点运行。\n\n- 声明：`schedule cleanup`\n\n```dever\nschedule cleanup = \"0 * * * *\"\n```\n";
    let mut sources = SourceMap::default();
    let id = sources.add("content/article/job.dever.md", markdown);
    sources.add("content/article/app.dever", "clean() () {}");
    checked(&sources);
    let formatted = dever_core::format::format(sources.get(id)).unwrap();
    let mut round_trip = SourceMap::default();
    round_trip.add("content/article/job.dever.md", &formatted);
    round_trip.add("content/article/app.dever", "clean() () {}");
    checked(&round_trip);
}

#[test]
fn transaction_rejects_model_and_job_on_different_explicit_connections() {
    let mut sources = sources(
        JOB,
        "type Publish { title: Text }\napply(input: Publish) () {}\ntransaction submit(input: Publish) (id: Id) {\n article = model.create({ title = input.title })\n id = dever.job.enqueue(job.publish, input, input.title)\n}",
    );
    sources.add(
        "content/article/model.dever",
        "database report\ntype Article { title: Text }",
    );
    rejected(&sources, "multiple explicit database connections");
}

#[test]
fn tenant_model_jobs_generate_scoped_storage_and_private_schema_migration() {
    let mut sources = sources(
        "database default\njob publish(input: app.Publish) () retry(5) timeout(30000) { app.apply(input) }\njob cleanup() () retry(2) timeout(1000) { app.clean() }\nschedule cleanup = \"0 * * * *\"",
        "type Publish { title: Text }\napply(input: Publish) () { article = model.create({ title = input.title }) }\nclean() () {}\ntransaction submit(input: Publish) (id: Id) { article = model.create({ title = input.title })\nid = dever.job.enqueue(job.publish, input, input.title) }",
    );
    sources.add(
        "content/article/model.dever",
        "type Article { title: Text }",
    );
    let program = checked(&sources);
    let generated = dever_core::native::emit_project(
        &program,
        &sources,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    assert!(generated.contains("JOB_STORAGE_"));
    assert!(generated.contains("ModelScope::Tenant"));
    assert!(generated.contains("job_migrate_tenant(tenant_id"));
    assert!(generated.contains("dever_runtime::job::store::initialize"));
    assert!(!generated.contains("JOB_DATABASE_"));
    let storage_initializers = generated
        .lines()
        .filter(|line| line.contains("JOB_STORAGE_") && line.contains(".set("))
        .collect::<Vec<_>>();
    assert_eq!(storage_initializers.len(), 2);
    assert!(
        storage_initializers
            .iter()
            .all(|line| line.contains("ModelScope::Tenant"))
    );
    dever_core::native::compile_project(
        &program,
        &sources,
        std::ffi::OsStr::new("rustc"),
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
}

#[test]
fn runtime_modes_and_job_limits_are_strict_and_validate_permissions() {
    let project = temp::TemporaryDirectory::new();
    std::fs::create_dir(project.path().join("config")).unwrap();
    let file = project.path().join("config/setting.json");
    let valid = r#"{"runtime":{"mode":"worker","shutdown_ms":30000},"job":{"workers":2,"poll_ms":250,"lease_ms":60000,"retry_base_ms":1000,"retry_max_ms":300000}}"#;
    std::fs::write(&file, valid).unwrap();
    let settings = dever_runtime::config::Settings::load_project(project.path()).unwrap();
    settings.services(false, true).unwrap();
    assert!(settings.services(true, false).is_err());
    let inferred_worker = valid.replace(r#""runtime":{"mode":"worker","shutdown_ms":30000},"#, "");
    std::fs::write(&file, inferred_worker).unwrap();
    let inferred = dever_runtime::config::Settings::load_project(project.path()).unwrap();
    assert_eq!(
        inferred.services(false, true).unwrap().mode,
        dever_runtime::config::RuntimeMode::Worker
    );
    for mode in ["api", "all"] {
        let document = valid.replace("\"worker\"", &format!("\"{mode}\""));
        std::fs::write(&file, &document).unwrap();
        assert!(
            dever_runtime::config::Settings::load_project(project.path())
                .unwrap()
                .services(true, true)
                .is_err()
        );
        let document =
            document.replacen('{', "{\"http\":{\"host\":\"127.0.0.1\",\"port\":8181},", 1);
        std::fs::write(&file, document).unwrap();
        dever_runtime::config::Settings::load_project(project.path())
            .unwrap()
            .services(true, true)
            .unwrap();
    }
    for invalid in [
        valid.replace("\"worker\"", "\"unknown\""),
        valid.replace("\"workers\":2", "\"workers\":0"),
        valid.replace("\"lease_ms\":60000", "\"lease_ms\":0"),
        valid.replace("\"shutdown_ms\":30000", "\"shutdown_ms\":-1"),
        "{\"runtime\":null}".into(),
        "{\"job\":null}".into(),
    ] {
        std::fs::write(&file, invalid).unwrap();
        assert!(dever_runtime::config::Settings::load_project(project.path()).is_err());
    }
    let document = valid.replacen('{', "{\"database\":{\"default\":{\"type\":\"sqlite\",\"path\":\"data/db/jobs.db\",\"max_connections\":3}},", 1);
    std::fs::write(&file, document).unwrap();
    let settings = dever_runtime::config::Settings::load_project(project.path()).unwrap();
    settings
        .validate_job_connection(Some("default"), "content", 59_999)
        .unwrap();
    for timeout in [60_000, 60_001] {
        assert!(
            settings
                .validate_job_connection(Some("default"), "content", timeout)
                .unwrap_err()
                .contains("must exceed Job timeout_ms")
        );
    }
    std::fs::write(
        &file,
        valid.replace("\"lease_ms\":60000", "\"lease_ms\":7200000"),
    )
    .unwrap();
    dever_runtime::config::Settings::load_project(project.path()).unwrap();
    std::fs::write(
        &file,
        valid.replace("\"lease_ms\":60000", "\"lease_ms\":7200001"),
    )
    .unwrap();
    assert!(dever_runtime::config::Settings::load_project(project.path()).is_err());
}

#[test]
fn utc_cron_handles_calendar_ranges_steps_and_day_or_semantics() {
    let minute = |text| dever_runtime::time::parse_datetime(text).unwrap() / 60000;
    let cron = dever_runtime::cron::Cron::parse("*/15 3 20 9 1").unwrap();
    assert!(cron.matches_minute(minute("2026-09-20T03:30:00Z")));
    assert!(cron.matches_minute(minute("2026-09-21T03:30:00Z")));
    assert!(!cron.matches_minute(minute("2026-09-22T03:30:00Z")));
    assert!(!cron.matches_minute(minute("2026-09-20T03:31:00Z")));
    for invalid in [
        "* * * *",
        "0 24 * * *",
        "0 0 0 * *",
        "0 0 * 13 *",
        "0 0 * * 7",
        "*/0 * * * *",
        "5-2 * * * *",
        "0 0 31 2 *",
    ] {
        assert!(dever_runtime::cron::Cron::parse(invalid).is_err());
    }
}

#[test]
fn application_jobs_use_case_local_sqlite_clock_and_port_fakes() {
    let project = temp::TemporaryDirectory::new();
    let long_job = JOB.replace("timeout(30000)", "timeout(3600000)");
    let files = [
        ("module/main.dever", "main() () {}"),
        (
            "module/content/article/model.dever",
            "type Article { title: Text }",
        ),
        (
            "module/content/article/app.dever",
            "type Publish { title: Text\narticle_id: model.id }\ntype DeliveryError { error Failed(message: Text) }\ntransaction submit(title: Text) (id: Id) {\n article = model.create({ title = title })\n id = dever.job.enqueue(job.publish, Publish { title = title\narticle_id = article.id }, title)\n}\napply(input: Publish) () {\n original = model.get(input.article_id)\n article = model.create({ title = port.title(original.title) })\n}\ncount(title: Text) (total: Int) { total = model.count({ where = title == title }) }",
        ),
        ("module/content/article/job.dever", long_job.as_str()),
        (
            "module/content/article/port.dever",
            "title(value: Text) (title: Text) fails app.DeliveryError",
        ),
        (
            "module/content/article/adapter.dever",
            "port.title(value: Text) (title: Text) { title = \"production\" }",
        ),
        (
            "test/content/article/first.dever",
            "port.title(value: Text) (title: Text) { title = \"first\" }\nfirst() () {\n assert_eq(app.count(\"first\"), 0)\n id = app.submit(\"same-key\")\n assert_eq(dever.test.drain_jobs(5), 1)\n assert_eq(app.count(\"first\"), 1)\n dever.test.advance_clock(1000)\n assert_eq(dever.test.drain_jobs(5), 0)\n}",
        ),
        (
            "test/content/article/second.dever",
            "port.title(value: Text) (title: Text) { title = \"second\" }\nsecond() () {\n assert_eq(app.count(\"first\"), 0)\n id = app.submit(\"same-key\")\n assert_eq(dever.test.drain_jobs(5), 1)\n assert_eq(app.count(\"second\"), 1)\n}",
        ),
        (
            "test/content/article/empty.dever",
            "empty() () { dever.test.advance_clock(1000)\nassert_eq(dever.test.drain_jobs(5), 0) }",
        ),
    ];
    for (path, source) in files {
        let path = project.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let sources =
        SourceMap::load_project(&project.path().join("module"), &project.path().join("test"))
            .unwrap();
    let program = checked(&sources);
    assert_eq!(
        program
            .tests()
            .iter()
            .filter(|case| case.uses_database())
            .count(),
        2
    );
    let native =
        dever_core::native::compile_test_suite(&program, &sources, std::ffi::OsStr::new("rustc"))
            .unwrap();
    for index in 0..program.tests().len() {
        let case = temp::TemporaryDirectory::new();
        std::fs::create_dir(case.path().join("config")).unwrap();
        std::fs::write(case.path().join("config/setting.json"), r#"{"database":{"default":{"type":"sqlite","path":"data/db/case.db","max_connections":1}}}"#).unwrap();
        let binary = case.path().join("case");
        native.save(&binary).unwrap();
        let output = std::process::Command::new(binary)
            .arg(index.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn native_dispatch_blocks_changed_schema_corrupt_payload_and_unknown_target() {
    let project = temp::TemporaryDirectory::new();
    for (path, source) in [
        ("module/main.dever", "main() () {}"),
        (
            "module/content/article/model.dever",
            "type Article { title: Text }",
        ),
        (
            "module/content/article/app.dever",
            "type Work { title: Text }\nsubmit() (id: Id) { id = dever.job.enqueue(job.publish, Work { title = \"delivered\" }, \"same\") }\napply(input: Work) () { article = model.create({ title = input.title }) }\ncount() (total: Int) { total = model.count() }",
        ),
        (
            "module/content/article/job.dever",
            "database default\njob publish(input: app.Work) () retry(2) timeout(1000) { app.apply(input) }",
        ),
        (
            "test/content/article/prepare.dever",
            "prepare() () { id = app.submit() }",
        ),
        (
            "test/content/article/blocked.dever",
            "blocked() () { id = app.submit()\nassert_eq(dever.test.drain_jobs(5), 1)\nassert_eq(app.count(), 0) }",
        ),
        (
            "test/content/article/unknown.dever",
            "unknown() () { id = app.submit()\nassert_eq(dever.test.drain_jobs(5), 2)\nassert_eq(app.count(), 1) }",
        ),
    ] {
        let path = project.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let sources =
        SourceMap::load_project(&project.path().join("module"), &project.path().join("test"))
            .unwrap();
    let program = checked(&sources);
    let native =
        dever_core::native::compile_test_suite(&program, &sources, std::ffi::OsStr::new("rustc"))
            .unwrap();
    let index = |suffix: &str| {
        program
            .tests()
            .iter()
            .position(|case| case.name().ends_with(suffix))
            .unwrap()
            .to_string()
    };
    for (mutation, expected, entry) in [
        (
            "UPDATE _dever_jobs SET schema='old-version'",
            "schema_mismatch",
            "blocked",
        ),
        (
            "UPDATE _dever_jobs SET payload='{}'",
            "invalid_payload",
            "blocked",
        ),
        (
            "UPDATE _dever_jobs SET timeout_ms=3600001",
            "invalid_policy",
            "blocked",
        ),
        (
            "UPDATE _dever_jobs SET target='removed.job'",
            "unknown_target",
            "unknown",
        ),
    ] {
        let case = temp::TemporaryDirectory::new();
        std::fs::create_dir(case.path().join("config")).unwrap();
        std::fs::write(case.path().join("config/setting.json"), r#"{"database":{"default":{"type":"sqlite","path":"data/db/jobs.db","max_connections":1}}}"#).unwrap();
        let binary = case.path().join("case");
        native.save(&binary).unwrap();
        let run = |entry| {
            let output = std::process::Command::new(&binary)
                .arg(index(entry))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run("prepare");
        let path = case.path().join("data/db/jobs.db");
        let update_path = path.clone();
        dever_runtime::task::run_entry(async move {
            let database = dever_runtime::sqlite::Database::open(update_path, 1, 100)
                .map_err(|error| error.to_string())?;
            database
                .execute(mutation, vec![])
                .await
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .unwrap();
        run(entry);
        dever_runtime::task::run_entry(async move {
            let database = dever_runtime::sqlite::Database::open(path, 1, 100)
                .map_err(|error| error.to_string())?;
            let rows = database
                .query(
                    "SELECT error,attempt FROM _dever_jobs WHERE state='blocked'",
                    vec![],
                )
                .await
                .map_err(|error| error.to_string())?;
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].get(0).unwrap(),
                &dever_runtime::orm::Value::Text(expected.into())
            );
            assert_eq!(rows[0].get(1).unwrap(), &dever_runtime::orm::Value::Int(1));
            Ok(())
        })
        .unwrap();
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_durable_state_machine() {
    use database::{Executor, Sql};
    use dever_runtime::{
        config, database,
        job::{self, Spec, store},
        orm::Value,
        wire::Encoded,
    };
    let executable = std::env::current_exe().unwrap();
    if executable.file_stem().unwrap() != "durable-job-case" {
        // The child owns all runtime globals and its setting.json; deployment config is never read.
        let project = temp::TemporaryDirectory::new();
        std::fs::create_dir(project.path().join("config")).unwrap();
        std::fs::write(project.path().join("config/setting.json"), r#"{"database":{"default":{"type":"sqlite","path":"data/db/jobs.db","max_connections":2}}}"#).unwrap();
        let child = project.path().join("durable-job-case");
        std::fs::copy(executable, &child).unwrap();
        let output = std::process::Command::new(child)
            .args(["--exact", "sqlite_durable_state_machine", "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    config::bootstrap(
        config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
        &[(Some("default"), "content")],
        &[],
    )
    .unwrap();
    dever_runtime::task::run_entry(async {
        job::scope_system(None, async {
        let db = database::database_for(Some("default"), "content").unwrap();
        store::initialize(db.clone()).await.unwrap();
        store::initialize(db.clone()).await.unwrap();
        let spec = Spec { target: "content.article.job.publish", schema: "wire-v1:Job:Unit", attempts: 2, timeout_ms: 1000 };
        let sql = |text| Sql { sqlite: text, postgres: text };
        db.execute(sql("CREATE TABLE business (id BIGINT PRIMARY KEY)"), vec![]).await.unwrap();
        let tx = db.begin().await.unwrap();
        Executor::new(db.clone(), Some(&tx)).unwrap().execute(sql("INSERT INTO business(id) VALUES (1)"), vec![]).await.unwrap();
        job::enqueue(db.clone(), Some(&tx), &spec, Encoded::null(), "rollback", 0).await.unwrap();
        tx.rollback().await.unwrap();
        assert!(db.query(sql("SELECT id FROM business"), vec![]).await.unwrap().is_empty());
        assert!(db.query(sql("SELECT id FROM _dever_jobs"), vec![]).await.unwrap().is_empty());
        let tx = db.begin().await.unwrap();
        Executor::new(db.clone(), Some(&tx)).unwrap().execute(sql("INSERT INTO business(id) VALUES (2)"), vec![]).await.unwrap();
        let id = job::enqueue(db.clone(), Some(&tx), &spec, Encoded::null(), "commit", 1000).await.unwrap();
        tx.commit().await.unwrap();
        let (first, second) = tokio::join!(job::enqueue(db.clone(), None, &spec, Encoded::null(), "commit", 1000), job::enqueue(db.clone(), None, &spec, Encoded::null(), "commit", 1000));
        assert_eq!(first.unwrap(), id);
        assert_eq!(second.unwrap(), id);
        assert!(store::claim(db.clone(), &clock_at(999), 1000, 1).await.unwrap().is_empty());
        let claim = store::claim(db.clone(), &clock_at(1000), 1000, 1).await.unwrap().remove(0);
        assert_eq!(claim.attempt, 1);
        assert!(store::claim(db.clone(), &clock_at(1001), 1000, 1).await.unwrap().is_empty());
        let reclaimed = store::claim(db.clone(), &clock_at(2000), 1000, 1).await.unwrap().remove(0);
        assert_eq!(reclaimed.attempt, 2);
        assert_ne!(reclaimed.token, claim.token);
        assert!(!store::renew(db.clone(), &claim, &clock_at(2001), 1000).await.unwrap());
        assert!(!store::complete(db.clone(), &claim, store::Outcome::Success, &clock_at(2001), 100, 1000).await.unwrap());
        assert!(store::complete(db.clone(), &reclaimed, store::Outcome::Success, &clock_at(2001), 100, 1000).await.unwrap());
        let reuse = job::enqueue(db.clone(), None, &spec, Encoded::null(), "commit", 3000).await.unwrap();
        assert_ne!(reuse, id);
        let attempt = store::claim(db.clone(), &clock_at(3000), 1000, 1).await.unwrap().remove(0);
        assert!(store::complete(db.clone(), &attempt, store::Outcome::Retry, &clock_at(3000), 100, 1000).await.unwrap());
        assert!(store::claim(db.clone(), &clock_at(3099), 1000, 1).await.unwrap().is_empty());
        assert_eq!(store::claim(db.clone(), &clock_at(3100), 1000, 1).await.unwrap()[0].attempt, 2);
        assert!(store::claim(db.clone(), &clock_at(4100), 1000, 1).await.unwrap().is_empty());
        let states = db.query(sql("SELECT state,error FROM _dever_jobs WHERE state='dead'"), vec![]).await.unwrap();
        assert_eq!(states[0].get(1).unwrap(), &Value::Text("attempts_exhausted".into()));
        // Advance the injected clock while each operation waits for SQLite's
        // write lock. Claim starts a fresh lease; renew/finish cannot revive it.
        let clock = clock_at(5000);
        job::enqueue(db.clone(), None, &spec, Encoded::null(), "lock-wait", 5000).await.unwrap();
        let lock = db.begin().await.unwrap();
        Executor::new(db.clone(), Some(&lock)).unwrap().execute(sql("UPDATE business SET id=id WHERE id=2"), vec![]).await.unwrap();
        let pending = store::claim(db.clone(), &clock, 1000, 1);
        tokio::pin!(pending);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending).await.is_err());
        clock.advance(1000).unwrap();
        lock.commit().await.unwrap();
        let locked_claim = pending.await.unwrap().remove(0);
        let leases = db.query(sql("SELECT lease_until FROM _dever_jobs WHERE dedupe_key='business:lock-wait'"), vec![]).await.unwrap();
        assert_eq!(leases[0].get(0).unwrap(), &Value::Int(7000));
        let lock = db.begin().await.unwrap();
        Executor::new(db.clone(), Some(&lock)).unwrap().execute(sql("UPDATE business SET id=id WHERE id=2"), vec![]).await.unwrap();
        let pending = store::renew(db.clone(), &locked_claim, &clock, 1000);
        tokio::pin!(pending);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending).await.is_err());
        clock.advance(1001).unwrap();
        lock.commit().await.unwrap();
        assert!(!pending.await.unwrap());
        let reclaimed = store::claim(db.clone(), &clock, 1000, 1).await.unwrap().remove(0);
        let lock = db.begin().await.unwrap();
        Executor::new(db.clone(), Some(&lock)).unwrap().execute(sql("UPDATE business SET id=id WHERE id=2"), vec![]).await.unwrap();
        let pending = store::complete(db.clone(), &reclaimed, store::Outcome::Success, &clock, 100, 1000);
        tokio::pin!(pending);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending).await.is_err());
        clock.advance(1001).unwrap();
        lock.commit().await.unwrap();
        assert!(!pending.await.unwrap());
        static NEXT: Spec = Spec { target: "content.article.job.next", schema: "unit", attempts: 2, timeout_ms: 1000 };
        job::enqueue(db.clone(), None, &NEXT, Encoded::null(), "after-exhausted", clock.now().unwrap()).await.unwrap();
        let bindings = vec![job::worker::Binding::fixed(db.clone(), &NEXT, None)];
        assert_eq!(job::worker::drain(bindings, clock.clone(), 5, |_| async { store::Outcome::Success }).await.unwrap(), 1);
        assert!(store::claim(db.clone(), &clock, 1000, 1).await.unwrap().is_empty());
        let cron = dever_runtime::cron::Cron::parse("* * * * *").unwrap();
        assert_eq!(store::materialize(db.clone(), &spec, &cron, 60_000).await.unwrap(), 0);
        let (a, b) = tokio::join!(store::materialize(db.clone(), &spec, &cron, 120_000), store::materialize(db.clone(), &spec, &cron, 120_000));
        assert_eq!(a.unwrap() + b.unwrap(), 1);
        let slot = store::claim(db.clone(), &clock_at(120_000), 1000, 1).await.unwrap().remove(0);
        store::complete(db.clone(), &slot, store::Outcome::Success, &clock_at(120_001), 100, 1000).await.unwrap();
        assert_eq!(store::materialize(db.clone(), &spec, &cron, 120_000).await.unwrap(), 0);
        assert!(store::materialize(db.clone(), &spec, &dever_runtime::cron::Cron::parse("0 * * * *").unwrap(), 120_000).await.is_err());
        assert_eq!(store::materialize(db.clone(), &spec, &cron, 60_000 * 1000).await.unwrap(), 256);
        // A failure after inserting an occurrence must roll back both it and its cursor.
        db.execute(sql("CREATE TRIGGER reject_job_cursor BEFORE UPDATE ON _dever_job_schedule BEGIN SELECT RAISE(ABORT, 'test rollback'); END"), vec![]).await.unwrap();
        let before = db.query(sql("SELECT COUNT(*) FROM _dever_jobs"), vec![]).await.unwrap();
        assert!(store::materialize(db.clone(), &spec, &cron, 60_000 * 259).await.is_err());
        let after = db.query(sql("SELECT COUNT(*) FROM _dever_jobs"), vec![]).await.unwrap();
        assert_eq!(before[0].get(0).unwrap(), after[0].get(0).unwrap());
        db.execute(sql("DROP TRIGGER reject_job_cursor"), vec![]).await.unwrap();
        store::initialize(db.clone()).await.unwrap();
        assert_eq!(store::materialize(db.clone(), &spec, &cron, 60_000 * 259).await.unwrap(), 1);
        db.execute(sql("UPDATE _dever_job_version SET version=99"), vec![]).await.unwrap();
        assert!(store::initialize(db.clone()).await.is_err());
        Ok(())
        }).await
    }).unwrap();
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_job_identity_migration_blocks_legacy_work_and_persists_trusted_identity() {
    use dever_runtime::{
        config, database,
        job::{self, ExecutionIdentity, Spec, store},
        orm::Value,
        wire::Encoded,
    };

    let executable = std::env::current_exe().unwrap();
    if executable.file_stem().unwrap() != "job-identity-migration-case" {
        let project = temp::TemporaryDirectory::new();
        std::fs::create_dir(project.path().join("config")).unwrap();
        std::fs::write(project.path().join("config/setting.json"), r#"{"database":{"default":{"type":"sqlite","path":"data/db/jobs.db","max_connections":2}}}"#).unwrap();
        let child = project.path().join("job-identity-migration-case");
        std::fs::copy(executable, &child).unwrap();
        let output = std::process::Command::new(child)
            .args([
                "--exact",
                "sqlite_job_identity_migration_blocks_legacy_work_and_persists_trusted_identity",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    config::bootstrap(
        config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
        &[(Some("default"), "content")],
        &[],
    )
    .unwrap();
    dever_runtime::task::run_entry(async {
        let db = database::database_for(Some("default"), "content").unwrap();
        let sql = |text| database::Sql { sqlite: text, postgres: text };
        for statement in [
            "CREATE TABLE _dever_job_version (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version BIGINT NOT NULL)",
            "INSERT INTO _dever_job_version(singleton,version) VALUES (1,2)",
            "CREATE TABLE _dever_jobs (id TEXT PRIMARY KEY, tenant_id BIGINT NOT NULL, target TEXT NOT NULL, schema TEXT NOT NULL, payload TEXT NOT NULL, dedupe_key TEXT NOT NULL, state TEXT NOT NULL, run_at BIGINT NOT NULL, attempt BIGINT NOT NULL, max_attempts BIGINT NOT NULL, timeout_ms BIGINT NOT NULL, claim_token TEXT NOT NULL, lease_until BIGINT NOT NULL, error TEXT NOT NULL, updated_at BIGINT NOT NULL)",
            "CREATE UNIQUE INDEX _dever_jobs_active ON _dever_jobs(tenant_id,target,dedupe_key) WHERE state IN ('pending','running')",
            "CREATE INDEX _dever_jobs_due ON _dever_jobs(state,run_at,lease_until)",
            "CREATE TABLE _dever_job_schedule (identity TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, last_minute BIGINT NOT NULL)",
            "INSERT INTO _dever_jobs(id,tenant_id,target,schema,payload,dedupe_key,state,run_at,attempt,max_attempts,timeout_ms,claim_token,lease_until,error,updated_at) VALUES ('legacy',0,'content.article.job.publish','unit','null','business:legacy','pending',0,0,2,1000,'',0,'',0)",
        ] {
            db.execute(sql(statement), vec![]).await.unwrap();
        }

        store::initialize(db.clone()).await.unwrap();
        let migrated = db.query(sql("SELECT version,state,error,execution_kind FROM _dever_job_version CROSS JOIN _dever_jobs WHERE id='legacy'"), vec![]).await.unwrap();
        assert_eq!(migrated[0].get(0).unwrap(), &Value::Int(5));
        assert_eq!(migrated[0].get(1).unwrap(), &Value::Text("blocked".into()));
        assert_eq!(migrated[0].get(2).unwrap(), &Value::Text("missing_execution_identity".into()));
        assert_eq!(migrated[0].get(3).unwrap(), &Value::Text("legacy".into()));

        static SPEC: Spec = Spec { target: "content.article.job.publish", schema: "unit", attempts: 2, timeout_ms: 1000 };
        assert!(job::enqueue(db.clone(), None, &SPEC, Encoded::null(), "missing", 0).await.unwrap_err().to_string().contains("trusted Job execution identity"));
        job::scope_system(None, job::enqueue(db.clone(), None, &SPEC, Encoded::null(), "system", 0)).await.unwrap();
        let user = ExecutionIdentity::User {
            tenant_id: Some(7),
            provider: "account".into(),
            site: "admin".into(),
            subject: "user:42".into(),
            session: "session:9".into(),
            tenant_claim: Some("tenant-seven".into()),
            permission_key: "news.article.admin.publish".into(),
        };
        job::scope_execution(user.clone(), job::enqueue(db.clone(), None, &SPEC, Encoded::null(), "user", 0)).await.unwrap();

        let rows = db.query(sql("SELECT execution_kind,scope_tenant_id,auth_provider,auth_site,auth_subject,auth_session,auth_tenant,auth_permission FROM _dever_jobs WHERE dedupe_key='business:user'"), vec![]).await.unwrap();
        assert_eq!(rows[0].get(0).unwrap(), &Value::Text("user".into()));
        assert_eq!(rows[0].get(1).unwrap(), &Value::Int(7));
        assert_eq!(rows[0].get(2).unwrap(), &Value::Text("account".into()));
        assert_eq!(rows[0].get(3).unwrap(), &Value::Text("admin".into()));
        assert_eq!(rows[0].get(4).unwrap(), &Value::Text("user:42".into()));
        assert_eq!(rows[0].get(5).unwrap(), &Value::Text("session:9".into()));
        assert_eq!(rows[0].get(6).unwrap(), &Value::Text("tenant-seven".into()));
        assert_eq!(rows[0].get(7).unwrap(), &Value::Text("news.article.admin.publish".into()));
        let claims = store::claim(db.clone(), &job::Clock::for_test(), 1000, 2).await.unwrap();
        assert!(claims.iter().any(|claim| claim.execution == ExecutionIdentity::System { tenant_id: None }));
        assert!(claims.iter().any(|claim| claim.execution == user));

        db.execute(sql("DROP TABLE _dever_jobs"), vec![]).await.unwrap();
        db.execute(sql("UPDATE _dever_job_version SET version=4 WHERE singleton=1"), vec![]).await.unwrap();
        db.execute(sql("CREATE TABLE _dever_jobs (id TEXT PRIMARY KEY, tenant_id BIGINT NOT NULL, scope_tenant_id BIGINT NOT NULL, execution_kind TEXT NOT NULL, auth_provider TEXT NOT NULL, auth_site TEXT NOT NULL, auth_subject TEXT NOT NULL, auth_session TEXT NOT NULL, auth_tenant TEXT NOT NULL, auth_capabilities TEXT NOT NULL, target TEXT NOT NULL, schema TEXT NOT NULL, payload TEXT NOT NULL, dedupe_key TEXT NOT NULL, state TEXT NOT NULL, run_at BIGINT NOT NULL, attempt BIGINT NOT NULL, max_attempts BIGINT NOT NULL, timeout_ms BIGINT NOT NULL, claim_token TEXT NOT NULL, lease_until BIGINT NOT NULL, error TEXT NOT NULL, updated_at BIGINT NOT NULL)"), vec![]).await.unwrap();
        for (id, execution_kind) in [("system-v4", "system"), ("user-v4", "user")] {
            db.execute(
                sql("INSERT INTO _dever_jobs(id,tenant_id,scope_tenant_id,execution_kind,auth_provider,auth_site,auth_subject,auth_session,auth_tenant,auth_capabilities,target,schema,payload,dedupe_key,state,run_at,attempt,max_attempts,timeout_ms,claim_token,lease_until,error,updated_at) VALUES (?1,0,0,?2,'account','admin','user:42','session:9','','[]','content.article.job.publish','unit','null',?3,'pending',0,0,2,1000,'',0,'',0)"),
                vec![Value::Text(id.into()), Value::Text(execution_kind.into()), Value::Text(format!("v4:{id}"))],
            ).await.unwrap();
        }
        store::initialize(db.clone()).await.unwrap();
        let migrated = db.query(sql("SELECT version,id,state,error,auth_permission FROM _dever_job_version CROSS JOIN _dever_jobs ORDER BY id"), vec![]).await.unwrap();
        assert_eq!(migrated[0].get(0).unwrap(), &Value::Int(5));
        assert_eq!(migrated[0].get(1).unwrap(), &Value::Text("system-v4".into()));
        assert_eq!(migrated[0].get(2).unwrap(), &Value::Text("pending".into()));
        assert_eq!(migrated[0].get(4).unwrap(), &Value::Text(String::new()));
        assert_eq!(migrated[1].get(1).unwrap(), &Value::Text("user-v4".into()));
        assert_eq!(migrated[1].get(2).unwrap(), &Value::Text("blocked".into()));
        assert_eq!(migrated[1].get(3).unwrap(), &Value::Text("authorization_contract_changed".into()));
        assert_eq!(migrated[1].get(4).unwrap(), &Value::Text(String::new()));
        let columns = db.query(sql("PRAGMA table_info(_dever_jobs)"), vec![]).await.unwrap();
        assert!(!columns.iter().any(|row| {
            row.get(1).is_ok_and(|value| value == &Value::Text("auth_capabilities".into()))
        }));
        database::shutdown().await.unwrap();
        Ok(())
    }).unwrap();
}

#[cfg(all(feature = "sqlite", feature = "api", feature = "crypto", unix))]
#[test]
fn revoked_user_job_is_blocked_before_business_dispatch() {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn request(
        address: std::net::SocketAddr,
        path: &str,
        body: &str,
        cookie: Option<&str>,
    ) -> String {
        let mut stream =
            std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let cookie = cookie
            .map(|value| format!("Cookie: {value}\r\n"))
            .unwrap_or_default();
        write!(stream,
            "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{cookie}\r\n{body}",
            body.len(),
        ).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn sqlite_rows(path: std::path::PathBuf, sql: &'static str) -> Vec<dever_runtime::orm::Row> {
        dever_runtime::task::run_entry(async move {
            let database = dever_runtime::sqlite::Database::open(path, 1, 100)
                .map_err(|error| error.to_string())?;
            database
                .query(sql, vec![])
                .await
                .map_err(|error| error.to_string())
        })
        .unwrap()
    }

    fn sqlite_execute(path: std::path::PathBuf, sql: &'static str) {
        dever_runtime::task::run_entry(async move {
            let database = dever_runtime::sqlite::Database::open(path, 1, 100)
                .map_err(|error| error.to_string())?;
            database
                .execute(sql, vec![])
                .await
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .unwrap();
    }

    let project = temp::TemporaryDirectory::new();
    std::fs::create_dir(project.path().join("config")).unwrap();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    let setting = |mode: &str| {
        format!(
            r#"{{
      "runtime":{{"mode":"{mode}","shutdown_ms":1000}},
      "http":{{"host":"127.0.0.1","port":{}}},
      "job":{{"workers":1,"poll_ms":10,"lease_ms":2000,"retry_base_ms":10,"retry_max_ms":100}},
      "database":{{"default":{{"type":"sqlite","path":"data/db/jobs.db","max_connections":4}}}},
      "auth":{{"providers":{{"session":{{
        "verify":"user.account.verify",
        "jwtSecret":"0123456789abcdef0123456789abcdef",
        "ttlSeconds":3600,
        "cookie":"cms_session"
      }}}}}},
      "sites":{{"front":{{
        "path":"front",
        "auth":"session",
        "hosts":["127.0.0.1"],
        "origin":"http://{}"
      }}}}
    }}"#,
            address.port(),
            address
        )
    };
    std::fs::write(project.path().join("config/setting.json"), setting("api")).unwrap();
    let settings = dever_runtime::config::Settings::load_project(project.path()).unwrap();

    let mut application = SourceMap::default();
    application.add(
        "user/account/model.dever",
        "global type Account { session: Text unique\nactive: Bool default true }",
    );
    application.add(
        "user/account/model/effect.dever",
        "global type Effect { value: Text }",
    );
    application.add(
        "user/account/app.dever",
        r#"
type Identity {
  id: Text
  user_id: model.id?
  tenant_id: model.id?
}
type Work { value: Text }
login(session: Text) (result: Text) {
  stored = model.create({ session = session })
  discarded = stored
  dever.auth.issue_cookie("account:1", session, null)
  result = "ok"
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  stored = model.first({ where = session == claims.session and active == true })
  identity = verified(stored, claims)
}
verified(stored: model.Account, claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity {
    id = claims.subject
    user_id = stored.id
    tenant_id = null
  }
}
verified(stored: null, claims: dever.auth.Claims) (identity: Identity) {
  fail(dever.api.Error.Unauthorized)
}
submit() (id: Id) {
  id = dever.job.enqueue(job.deliver, Work { value = "executed" }, "revoked-user")
}
deliver(input: Work) () {
  stored = model.effect.create({ value = input.value })
  discarded = stored
}
"#,
    );
    application.add(
        "user/account/api/front/session.dever",
        "public post login = app.login\npost submit = app.submit",
    );
    application.add("user/account/job.dever", "database default\njob deliver(input: app.Work) () retry(2) timeout(1000) { app.deliver(input) }");
    let program =
        dever_core::check_with_settings(&application, &settings).unwrap_or_else(|errors| {
            panic!(
                "{}",
                errors
                    .iter()
                    .map(|error| error.render(&application))
                    .collect::<String>()
            )
        });
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native = dever_core::native::compile_project(
        &program,
        &application,
        &rustc,
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    let binary = project.path().join("application");
    native.save(&binary).unwrap();
    drop(reservation);

    let mut api = Child(
        std::process::Command::new(&binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while std::net::TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_err() {
        if api.0.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let mut stderr = String::new();
            if let Some(mut pipe) = api.0.stderr.take() {
                pipe.read_to_string(&mut stderr).unwrap();
            }
            panic!("API did not start: {stderr}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let login = request(
        address,
        "/user/account/front/session/login",
        r#"{"session":"session-1"}"#,
        None,
    );
    assert!(login.starts_with("HTTP/1.1 200"), "{login}");
    let cookie = login
        .lines()
        .find_map(|line| {
            line.strip_prefix("set-cookie: ")
                .or_else(|| line.strip_prefix("Set-Cookie: "))
                .map(|value| value.split(';').next().unwrap().to_owned())
        })
        .expect("login response Cookie");
    let database_path = project.path().join("data/db/jobs.db");
    for statement in [
        "INSERT INTO _dever_auth_role(id,site,name,all_permissions,enabled) VALUES ('submitter','front','Submitter',0,1)",
        "INSERT INTO _dever_auth_role_permission(site,role_id,permission_key) VALUES ('front','submitter','user.account.front.submit')",
        "INSERT INTO _dever_auth_user_role(user_id,site,role_id) VALUES (1,'front','submitter')",
    ] {
        sqlite_execute(database_path.clone(), statement);
    }
    let submit = request(
        address,
        "/user/account/front/session/submit",
        "{}",
        Some(&cookie),
    );
    assert!(submit.starts_with("HTTP/1.1 200"), "{submit}");
    let _ = api.0.kill();
    let _ = api.0.wait();

    sqlite_execute(
        database_path.clone(),
        "DELETE FROM _dever_auth_user_role WHERE user_id=1 AND site='front' AND role_id='submitter'",
    );

    std::fs::write(
        project.path().join("config/setting.json"),
        setting("worker"),
    )
    .unwrap();
    let mut worker = Child(
        std::process::Command::new(&binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut blocked = false;
    while Instant::now() < deadline {
        if worker.0.try_wait().unwrap().is_some() {
            break;
        }
        let rows = sqlite_rows(
            database_path.clone(),
            "SELECT state,error FROM _dever_jobs WHERE dedupe_key='business:revoked-user'",
        );
        blocked = rows.first().is_some_and(|row| {
            row.get(0).ok() == Some(&dever_runtime::orm::Value::Text("blocked".into()))
                && row.get(1).ok()
                    == Some(&dever_runtime::orm::Value::Text("identity_rejected".into()))
        });
        if blocked {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !blocked {
        let mut stderr = String::new();
        if let Some(mut pipe) = worker.0.stderr.take() {
            pipe.read_to_string(&mut stderr).unwrap();
        }
        panic!("revoked Job was not blocked: {stderr}");
    }
    assert!(
        sqlite_rows(database_path, "SELECT id FROM effect").is_empty(),
        "revoked Job executed its business write"
    );

    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &worker.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    while worker.0.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        worker
            .0
            .try_wait()
            .unwrap()
            .is_some_and(|status| status.success())
    );
}

#[cfg(all(feature = "sqlite", unix))]
#[test]
fn one_native_application_gates_api_worker_and_all_modes() {
    use std::time::{Duration, Instant};
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut sources = sources(
        "database default\njob publish() () retry(2) timeout(1000) { app.apply() }",
        "submit() (id: Id) { id = dever.job.enqueue(job.publish, \"mode\") }\napply() () { article = model.create({ title = \"delivered\" }) }",
    );
    sources.add(
        "content/article/model.dever",
        "global type Article { title: Text }",
    );
    sources.add("content/article/api.dever", "cmd submit = app.submit");
    sources.add(
        "monitor/health/app.dever",
        "health() (response: Bool) { response = true }",
    );
    sources.add("monitor/health/api.dever", "public get health = app.health");
    let program = checked(&sources);
    let native = dever_core::native::compile_project(
        &program,
        &sources,
        std::ffi::OsStr::new("rustc"),
        dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
    )
    .unwrap();
    for mode in ["api", "worker", "all"] {
        let project = temp::TemporaryDirectory::new();
        std::fs::create_dir(project.path().join("config")).unwrap();
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        let http = if mode == "worker" {
            String::new()
        } else {
            format!(
                ",\"http\":{{\"host\":\"127.0.0.1\",\"port\":{}}}",
                address.port()
            )
        };
        std::fs::write(project.path().join("config/setting.json"), format!("{{\"database\":{{\"default\":{{\"type\":\"sqlite\",\"path\":\"data/db/jobs.db\",\"max_connections\":2}}}},\"runtime\":{{\"mode\":\"{mode}\",\"shutdown_ms\":1000}},\"job\":{{\"workers\":1,\"poll_ms\":10,\"lease_ms\":1500,\"retry_base_ms\":10,\"retry_max_ms\":100}}{http}}}")).unwrap();
        let binary = project.path().join("application");
        native.save(&binary).unwrap();
        let queued = std::process::Command::new(&binary)
            .args(["content.article.submit", "{}"])
            .output()
            .unwrap();
        assert!(
            queued.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&queued.stdout),
            String::from_utf8_lossy(&queued.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&queued.stdout).unwrap();
        assert_eq!(response["code"], 0);
        drop(reservation);
        let mut child = Child(
            std::process::Command::new(binary)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let path = project.path().join("data/db/jobs.db");
        let expected = if mode == "api" {
            "pending"
        } else {
            "succeeded"
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut ready = false;
        while Instant::now() < deadline {
            if child.0.try_wait().unwrap().is_some() {
                break;
            }
            let listening =
                std::net::TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_ok();
            let state = if path.exists() {
                let path = path.clone();
                dever_runtime::task::run_entry(async move {
                    let database = dever_runtime::sqlite::Database::open(path, 1, 100)
                        .map_err(|error| error.to_string())?;
                    let rows = database
                        .query(
                            "SELECT state FROM _dever_jobs WHERE dedupe_key='business:mode'",
                            vec![],
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(rows.first().and_then(|row| row.get(0).ok()).cloned())
                })
                .ok()
                .flatten()
            } else {
                None
            };
            if listening == (mode != "worker")
                && state == Some(dever_runtime::orm::Value::Text(expected.into()))
            {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !ready {
            let _ = child.0.kill();
            let _ = child.0.wait();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.0.stderr.take() {
                use std::io::Read;
                pipe.read_to_string(&mut stderr).unwrap();
            }
            panic!("{mode} did not reach its service state: {stderr}");
        }
        assert!(
            std::process::Command::new("kill")
                .args(["-TERM", &child.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        while child.0.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            child
                .0
                .try_wait()
                .unwrap()
                .is_some_and(|status| status.success()),
            "{mode} did not drain successfully"
        );
    }
}

#[cfg(all(feature = "sqlite", unix))]
#[test]
fn worker_signal_stops_claiming_and_drains_or_cancels_before_database_close() {
    use dever_runtime::{
        config, database,
        job::{self, Clock, Spec, store, worker},
        wire::Encoded,
    };
    use std::time::{Duration, Instant};
    let executable = std::env::current_exe().unwrap();
    let name = executable.file_stem().unwrap().to_str().unwrap();
    if !name.starts_with("durable-job-signal-") {
        for scenario in ["grace", "timeout", "transaction"] {
            let project = temp::TemporaryDirectory::new();
            std::fs::create_dir(project.path().join("config")).unwrap();
            let shutdown = if scenario == "timeout" { 50 } else { 2000 };
            std::fs::write(project.path().join("config/setting.json"), format!(r#"{{"runtime":{{"mode":"worker","shutdown_ms":{shutdown}}},"job":{{"workers":1,"poll_ms":5,"lease_ms":2400,"retry_base_ms":100,"retry_max_ms":1000}},"database":{{"default":{{"type":"sqlite","path":"data/db/jobs.db","max_connections":2}}}}}}"#)).unwrap();
            let child_path = project
                .path()
                .join(format!("durable-job-signal-{scenario}"));
            std::fs::copy(&executable, &child_path).unwrap();
            let mut child = std::process::Command::new(child_path)
                .args([
                    "--exact",
                    "worker_signal_stops_claiming_and_drains_or_cancels_before_database_close",
                    "--nocapture",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            while !project.path().join("ready").exists() && Instant::now() < deadline {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if !project.path().join("ready").exists() {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "worker did not become ready: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            assert!(
                std::process::Command::new("kill")
                    .args(["-TERM", &child.id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait().unwrap().is_none() {
                let _ = child.kill();
            }
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                std::fs::read_to_string(project.path().join("closed")).unwrap(),
                scenario
            );
        }
        return;
    }
    let directory = executable.parent().unwrap().to_path_buf();
    let graceful = !name.ends_with("timeout");
    let transactional = name.ends_with("transaction");
    config::bootstrap(
        config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
        &[(Some("default"), "content")],
        &[],
    )
    .unwrap();
    dever_runtime::lifecycle::configure(false, true).unwrap();
    dever_runtime::task::run_entry(async move {
        job::scope_system(None, async move {
        let db = database::database_for(Some("default"), "content").unwrap();
        store::initialize(db.clone()).await.unwrap();
        static SPEC: Spec = Spec { target: "content.article.job.signal", schema: "unit", attempts: 3, timeout_ms: 2000 };
        config::settings().validate_job_connection(Some("default"), "content", SPEC.timeout_ms).unwrap();
        let old_policy = Spec { timeout_ms: 3000, ..SPEC };
        job::enqueue(db.clone(), None, &old_policy, Encoded::null(), "old-policy", -1).await.unwrap();
        // UUID tie-breaking is intentionally random within one millisecond; give
        // this ordering assertion distinct due times instead of assuming key order.
        for (run_at, key) in ["a", "b"].into_iter().enumerate() {
            job::enqueue(db.clone(), None, &SPEC, Encoded::null(), key, run_at as i64).await.unwrap();
        }
        let settings = config::settings().jobs().unwrap();
        let ready = directory.join("ready");
        let service_db = db.clone();
        let result = dever_runtime::lifecycle::run(async move {
            worker::serve(vec![worker::Binding::fixed(service_db.clone(), &SPEC, None)], Clock::default(), settings, move |_| {
                let ready = ready.clone();
                let db = service_db.clone();
                async move {
                    let transaction = if transactional { Some(db.begin().await.unwrap()) } else { None };
                    std::fs::write(ready, "claimed").unwrap();
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    if let Some(transaction) = transaction { transaction.commit().await.unwrap(); }
                    store::Outcome::Success
                }
            }).await
        }).await;
        assert_eq!(result.is_ok(), graceful, "{result:?}");
        let sql = database::Sql { sqlite: "SELECT state,error FROM _dever_jobs WHERE dedupe_key!='business:old-policy' ORDER BY dedupe_key", postgres: "SELECT state,error FROM _dever_jobs WHERE dedupe_key!='business:old-policy' ORDER BY dedupe_key" };
        let rows = db.query(sql, vec![]).await.unwrap();
        let expected = if graceful { "succeeded" } else { "running" };
        assert_eq!(rows[0].get(0).unwrap(), &dever_runtime::orm::Value::Text(expected.into()), "{:?}", rows[0].get(1));
        assert_eq!(rows[1].get(0).unwrap(), &dever_runtime::orm::Value::Text("pending".into()));
        let policy = db.query(database::Sql { sqlite: "SELECT state,error FROM _dever_jobs WHERE dedupe_key='business:old-policy'", postgres: "SELECT state,error FROM _dever_jobs WHERE dedupe_key='business:old-policy'" }, vec![]).await.unwrap();
        assert_eq!(policy[0].get(0).unwrap(), &dever_runtime::orm::Value::Text("blocked".into()));
        assert_eq!(policy[0].get(1).unwrap(), &dever_runtime::orm::Value::Text("invalid_policy".into()));
        database::shutdown().await.unwrap();
        assert!(db.query(database::Sql { sqlite: "SELECT 1", postgres: "SELECT 1" }, vec![]).await.is_err());
        dever_runtime::log::flush();
        std::fs::write(directory.join("closed"), if transactional { "transaction" } else if graceful { "grace" } else { "timeout" }).unwrap();
        Ok(())
        }).await
    }).unwrap();
}

#[cfg(feature = "sqlite")]
#[test]
fn tenant_jobs_share_the_business_database_and_restore_only_tenant_scope() {
    use dever_runtime::{
        config, database,
        job::{self, Clock, Spec, store, worker},
        orm::Value,
        wire::Encoded,
    };

    let executable = std::env::current_exe().unwrap();
    if executable.file_stem().unwrap() != "tenant-job-case" {
        let project = temp::TemporaryDirectory::new();
        std::fs::create_dir(project.path().join("config")).unwrap();
        std::fs::write(project.path().join("config/setting.json"), r#"{
            "tenant":{"database":"default","max_pools":2,"idle_timeout_ms":1000},
            "database":{"default":{"type":"sqlite","path":"data/platform.db","tenant_directory":"data/tenants","max_connections":2}}
        }"#).unwrap();
        let child = project.path().join("tenant-job-case");
        std::fs::copy(executable, &child).unwrap();
        let output = std::process::Command::new(child)
            .args([
                "--exact",
                "tenant_jobs_share_the_business_database_and_restore_only_tenant_scope",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    config::bootstrap_scoped(
        config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
        &[(Some("default"), "content", true)],
        &[&[(Some("default"), "content", true)]],
    )
    .unwrap();
    dever_runtime::task::run_entry(async {
        static SPEC: Spec = Spec {
            target: "content.article.job.publish",
            schema: "unit",
            attempts: 2,
            timeout_ms: 1000,
        };
        const FINGERPRINT: &str = "tenant-job-test-v1";
        dever_runtime::tenant::initialize(FINGERPRINT)
            .await
            .unwrap();

        for tenant_id in [1, 2] {
            let db = dever_runtime::tenant::begin_migration("default", tenant_id, FINGERPRINT)
                .await
                .unwrap();
            store::initialize(db.clone()).await.unwrap();
            db.execute(
                database::Sql {
                    sqlite: "CREATE TABLE business (id BIGINT PRIMARY KEY)",
                    postgres: "CREATE TABLE business (id BIGINT PRIMARY KEY)",
                },
                vec![],
            )
            .await
            .unwrap();
            dever_runtime::tenant::mark_database_ready(&db, tenant_id, FINGERPRINT)
                .await
                .unwrap();
            dever_runtime::tenant::finish_migration(tenant_id, FINGERPRINT)
                .await
                .unwrap();
        }

        let storage = Box::leak(Box::new(database::StorageBinding::new(
            Some("default"),
            "content",
            database::ModelScope::Tenant,
        )));
        let db1 = dever_runtime::tenant::scope(1, storage.database())
            .await
            .unwrap();
        let rolled_back = db1.begin().await.unwrap();
        database::Executor::new(db1.clone(), Some(&rolled_back))
            .unwrap()
            .execute(
                database::Sql {
                    sqlite: "INSERT INTO business(id) VALUES (1)",
                    postgres: "INSERT INTO business(id) VALUES (1)",
                },
                vec![],
            )
            .await
            .unwrap();
        job::scope_system(
            Some(1),
            job::enqueue(
                db1.clone(),
                Some(&rolled_back),
                &SPEC,
                Encoded::null(),
                "same",
                0,
            ),
        )
        .await
        .unwrap();
        rolled_back.rollback().await.unwrap();
        assert!(
            db1.query(
                database::Sql {
                    sqlite: "SELECT id FROM business",
                    postgres: "SELECT id FROM business"
                },
                vec![]
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert!(
            db1.query(
                database::Sql {
                    sqlite: "SELECT id FROM _dever_jobs",
                    postgres: "SELECT id FROM _dever_jobs"
                },
                vec![]
            )
            .await
            .unwrap()
            .is_empty()
        );

        let tenant1_job = dever_runtime::tenant::scope(
            1,
            job::scope_system(Some(1), async {
                let db = storage.database().await.unwrap();
                job::enqueue(db, None, &SPEC, Encoded::null(), "same", 0)
                    .await
                    .unwrap()
            }),
        )
        .await;
        let tenant2_job = dever_runtime::tenant::scope(
            2,
            job::scope_system(Some(2), async {
                let db = storage.database().await.unwrap();
                job::enqueue(db, None, &SPEC, Encoded::null(), "same", 0)
                    .await
                    .unwrap()
            }),
        )
        .await;
        assert_ne!(tenant1_job, tenant2_job);

        let binding = worker::Binding::scoped(storage, database::ModelScope::Tenant, &SPEC, None);
        let drained = dever_runtime::tenant::scope(
            1,
            worker::drain(vec![binding], Clock::for_test(), 1, |claim| async move {
                assert_eq!(claim.tenant_id, Some(1));
                assert_eq!(dever_runtime::tenant::current_id().unwrap(), 1);
                #[cfg(feature = "api")]
                assert!(dever_runtime::auth::id().is_err());
                store::Outcome::Success
            }),
        )
        .await
        .unwrap();
        assert_eq!(drained, 1);

        let db2 = dever_runtime::tenant::scope(2, storage.database())
            .await
            .unwrap();
        let rows = db2
            .query(
                database::Sql {
                    sqlite: "SELECT tenant_id FROM _dever_jobs WHERE state='pending'",
                    postgres: "SELECT tenant_id FROM _dever_jobs WHERE state='pending'",
                },
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(rows[0].get(0).unwrap(), &Value::Int(2));
        db2.execute(
            database::Sql {
                sqlite: "UPDATE _dever_jobs SET tenant_id=1 WHERE state='pending'",
                postgres: "UPDATE _dever_jobs SET tenant_id=1 WHERE state='pending'",
            },
            vec![],
        )
        .await
        .unwrap();
        assert!(
            store::claim(db2, &Clock::for_test(), 1000, 1)
                .await
                .unwrap_err()
                .to_string()
                .contains("tenant ownership")
        );

        let Err(unknown) = dever_runtime::tenant::scope(3, storage.database()).await else {
            panic!("unknown tenant must not resolve a database");
        };
        assert!(unknown.to_string().contains("not provisioned"));
        database::shutdown().await.unwrap();
        Ok(())
    })
    .unwrap();
}
