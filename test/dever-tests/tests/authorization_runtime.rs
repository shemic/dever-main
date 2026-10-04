mod support;

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_authorization_uses_exact_site_scoped_role_permissions() {
    use dever_runtime::{config, database};

    let executable = std::env::current_exe().unwrap();
    if executable.file_stem().unwrap() != "authorization-runtime-case" {
        let project = support::temp::TemporaryDirectory::new();
        std::fs::create_dir(project.path().join("config")).unwrap();
        std::fs::write(
            project.path().join("config/setting.json"),
            r#"{"database":{"default":{"type":"sqlite","path":"data/db/auth.db","max_connections":2}}}"#,
        )
        .unwrap();
        let child = project.path().join("authorization-runtime-case");
        std::fs::copy(executable, &child).unwrap();
        let output = std::process::Command::new(child)
            .args([
                "--exact",
                "sqlite_authorization_uses_exact_site_scoped_role_permissions",
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
        return;
    }

    config::bootstrap(
        config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        },
        &[(Some("default"), "auth")],
        &[],
    )
    .unwrap();
    dever_runtime::task::run_entry(async {
        let catalog = database::database_for(Some("default"), "auth").unwrap();
        support::authorization::assert_contract(catalog.clone(), catalog).await;
        database::shutdown().await.unwrap();
        Ok(())
    })
    .unwrap();
}
