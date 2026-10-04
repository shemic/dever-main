mod support;

use std::ffi::OsStr;

use dever_runtime::{config, database};

#[test]
#[ignore = "requires an isolated database configured in config/setting.json"]
fn postgres_authorization_preserves_site_scoped_roles_and_catalog() {
    let executable = std::env::current_exe().unwrap();
    if executable.file_stem().unwrap() != OsStr::new("authorization-postgres-case") {
        support::postgres::run_in_isolated_schema("authorization_postgres", |setting| async move {
            let project = support::temp::TemporaryDirectory::new();
            std::fs::create_dir(project.path().join("config")).unwrap();
            std::fs::write(
                project.path().join("config/setting.json"),
                setting.settings_json(),
            )
            .unwrap();
            let child = project.path().join("authorization-postgres-case");
            std::fs::copy(executable, &child).unwrap();
            let output = std::process::Command::new(child)
                .args([
                    "--ignored",
                    "--exact",
                    "postgres_authorization_preserves_site_scoped_roles_and_catalog",
                    "--nocapture",
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            Ok(())
        });
        return;
    }

    config::bootstrap(
        config::RuntimeProfile {
            sqlite: false,
            postgres: true,
        },
        &[(Some("default"), "auth")],
        &[],
    )
    .unwrap();
    dever_runtime::task::run_entry(async {
        database::prepare().await.unwrap();
        let catalog = database::database_for(Some("default"), "auth").unwrap();
        support::authorization::assert_contract(catalog.clone(), catalog).await;
        database::shutdown().await.unwrap();
        Ok(())
    })
    .unwrap();
}
