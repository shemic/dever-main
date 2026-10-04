//! Checked-source HTTP applications linked to the explicit runtime-only archive.
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    feature = "runtime-sqlite"
))]
#[path = "llvm_job_security_cases.rs"]
mod jobs;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/llvm_module.rs"]
mod llvm_module;
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    feature = "runtime-postgres"
))]
#[path = "dever-tests/tests/support/postgres.rs"]
pub mod postgres;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    feature = "runtime-sqlite"
))]
#[path = "llvm_api_security_cases.rs"]
mod security;
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    feature = "runtime-sqlite"
))]
#[path = "dever-tests/tests/support/sqlite_inspect.rs"]
mod sqlite_inspect;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::{llvm_module, process, temp};
    use dever_backend_bridge::{Target, emit_object};
    use dever_core::{hir::Program, source::SourceMap};
    use dever_runtime::config::Settings;
    use serde_json::{Value, json};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    pub(super) const TRANSPORT: &[(&str, &str)] = &[
        (
            "user/account/model.dever",
            "global type Account { email: Text(1, 254) }",
        ),
        (
            "platform/tenant/model.dever",
            "global type Tenant { name: Text(1, 64) }",
        ),
        (
            "user/account/app.dever",
            r#"type Identity {
  id: Text
  user_id: user.account.model.id?
  tenant_id: platform.tenant.model.id?
}
verify(claims: dever.auth.Claims) (identity: Identity) {
  identity = Identity { id = claims.subject
    user_id = null
    tenant_id = null }
}
"#,
        ),
        (
            "sample/echo/app.dever",
            r#"type Payload {
  name: Text
  count: Int
  tags: List<Text?>
  note: Text?
}
type Snapshot {
  request_id: Text
  method: Text
  path: Text
  header: Text?
  cookie: Text?
  peer: Text?
}
type Outcome {
  error Rejected(message: Text)
}
increment(q: Int, note: Text?) (value: Int) { value = q + 1 }
echo(payload: Payload) (value: Payload) { value = payload }
inspect() (value: Snapshot) {
  dever.api.set_header("x-response-probe", "committed")
  dever.task.sleep(1)
  value = Snapshot {
    request_id = dever.api.request_id()
    method = dever.api.method()
    path = dever.api.path()
    header = dever.api.header("x-probe")
    cookie = dever.api.cookie("probe")
    peer = dever.api.client_address()
  }
}
invalid() (value: Text) {
  dever.api.set_header("x-response-probe", "must-not-commit")
  fail(dever.api.Error.Invalid)
}
unauthorized() (value: Text) { fail(dever.api.Error.Unauthorized) }
forbidden() (value: Text) { fail(dever.api.Error.Forbidden) }
missing() (value: Text) { fail(dever.api.Error.NotFound) }
conflict() (value: Text) { fail(dever.api.Error.Conflict) }
limited() (value: Text) { fail(dever.api.Error.TooManyRequests) }
fault() (value: Text) { fail(Outcome.Rejected("must-not-appear-in-http-or-log")) }
upload(file: Upload, caption: Text) (size: Int) {
  size = dever.api.upload_size(file)
  dever.api.close_upload(file)
}
"#,
        ),
        (
            "sample/echo/api.dever",
            "public get increment = app.increment\npublic post echo = app.echo\npublic get inspect = app.inspect\npublic get invalid = app.invalid\npublic get unauthorized = app.unauthorized\npublic get forbidden = app.forbidden\npublic get missing = app.missing\npublic get conflict = app.conflict\npublic get limited = app.limited\npublic get fault = app.fault\npublic post upload = app.upload\ncmd echo = app.echo",
        ),
    ];

    fn echo_files(markdown: bool) -> Vec<(&'static str, &'static str)> {
        let mut files = TRANSPORT[..3].to_vec();
        if markdown {
            files.extend([
                (
                    "sample/echo/app.dever.md",
                    r#"# Echo application

Owned JSON echo contract.

- 包：`sample.echo.app`
- 公开类型：
  - `Payload`
- 公开方法：
  - `echo`
- 使用：
  - `sample.echo.echo(payload)`

## Payload

Keep concrete input fields.

- 类型：`Payload`
- 字段：
  - `name: Text`：Name.
  - `count: Int`：Count.
  - `tags: List<Text?>`：Tags.
  - `note: Text?`：Optional note.

```dever
type Payload { name: Text
  count: Int
  tags: List<Text?>
  note: Text? }
```

## Echo

Return the checked input.

- 函数：`echo`
- 输入：
  - `payload: Payload`：Payload.
- 输出：
  - `value: Payload`：Payload.

```dever
echo(payload: Payload) (value: Payload) { value = payload }
```
"#,
                ),
                (
                    "sample/echo/api.dever.md",
                    r#"# Echo interfaces

HTTP and command-line entry bindings.

- 包：`sample.echo.api`
- 公开类型：无
- 公开方法：无
- 使用：无

## HTTP

Anonymous typed JSON echo.

- 声明：`post echo`

```dever
public post echo = app.echo
```

## Command

Command-line JSON echo.

- 声明：`cmd echo`

```dever
cmd echo = app.echo
```
"#,
                ),
            ]);
        } else {
            files.extend([
                ("sample/echo/app.dever", "type Payload { name: Text\ncount: Int\ntags: List<Text?>\nnote: Text? }\necho(payload: Payload) (value: Payload) { value = payload }"),
                ("sample/echo/api.dever", "public post echo = app.echo\ncmd echo = app.echo"),
            ]);
        }
        files
    }

    pub(super) fn settings(port: u16) -> String {
        json!({
            "http": {"host": "127.0.0.1", "port": port},
            "runtime": {"mode": "api", "shutdown_ms": 2000},
            "log": {"level": "error"},
            "database": {"default": {"type": "sqlite", "path": "data/control.db", "max_connections": 2}},
            "auth": {"providers": {"session": {"verify": "user.account.verify", "jwtSecret": "0123456789abcdef0123456789abcdef", "cookie": "test_session"}}},
            "sites": {"test": {"path": "", "auth": "session", "hosts": ["localhost"], "origin": "https://localhost"}}
        })
        .to_string()
    }

    fn checked(files: &[(&str, &str)], setting: &str) -> (SourceMap, Program) {
        let directory = temp::TemporaryDirectory::new();
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::write(directory.path().join("config/setting.json"), setting).unwrap();
        let settings = Settings::load_project(directory.path()).unwrap();
        let mut sources = SourceMap::default();
        for (path, source) in files {
            sources.add(*path, *source);
        }
        let program =
            dever_core::check_with_settings(&sources, &settings).unwrap_or_else(|errors| {
                panic!(
                    "{}",
                    errors
                        .iter()
                        .map(|error| error.render(&sources))
                        .collect::<String>()
                )
            });
        (sources, program)
    }

    pub(super) fn application_ir(
        files: &[(&str, &str)],
        setting: &str,
        marker: bool,
        failure: Option<u32>,
    ) -> String {
        let (sources, program) = checked(files, setting);
        let mut ir = dever_core::llvm::emit_application(&program, &sources).unwrap();
        if marker {
            ir.push_str("\n@api_probe_cycle = private constant [19 x i8] c\"api-cycle-complete\\00\"\ndeclare i32 @puts(ptr)\ndeclare i32 @fflush(ptr)\n@stderr = external global ptr\ndeclare i64 @fwrite(ptr, i64, i64, ptr)\n");
        }
        ir.push_str("\ndefine i32 @dever_test_run() {\nentry:\n  %out = alloca i8\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_application_entry(ptr %out, ptr %fault)\n");
        match failure {
            Some(code) => ir.push_str(&llvm_module::fault_check(code, None)),
            None => ir.push_str("  %passed = icmp eq i32 %status, 0"),
        }
        if marker {
            // Owned fixture diagnostics only, while the failed root still owns its buffer.
            ir.push_str("\n  %diagnostic_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 4\n  %diagnostic = load { ptr, i64 }, ptr %diagnostic_ptr\n  %diagnostic_data = extractvalue { ptr, i64 } %diagnostic, 0\n  %diagnostic_size = extractvalue { ptr, i64 } %diagnostic, 1\n  %has_diagnostic = icmp ugt i64 %diagnostic_size, 0\n  %failed_root = xor i1 %passed, true\n  %print_diagnostic = and i1 %failed_root, %has_diagnostic\n  br i1 %print_diagnostic, label %diagnose, label %release_root\ndiagnose:\n  %diagnostic_stream = load ptr, ptr @stderr\n  %diagnostic_written = call i64 @fwrite(ptr %diagnostic_data, i64 1, i64 %diagnostic_size, ptr %diagnostic_stream)\n  br label %release_root\nrelease_root:\n");
        }
        ir.push_str("\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n");
        if marker {
            ir.push_str("  %printed = call i32 @puts(ptr @api_probe_cycle)\n  %flushed = call i32 @fflush(ptr null)\n");
        }
        ir.push_str("  %exit = select i1 %passed, i32 0, i32 1\n  ret i32 %exit\n}\n");
        ir
    }

    #[test]
    #[ignore = "requires explicit runtime-api archive; invalid descriptor must not bind a listener"]
    fn runtime_rejects_missing_authentication_before_serving() {
        let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let setting = settings(reservation.local_addr().unwrap().port());
        let ir = application_ir(TRANSPORT, &setting, false, Some(3));
        assert!(ir.contains("ptr @dever_api_auth_0"));
        let ir = ir.replace("ptr @dever_api_auth_0", "ptr null");
        llvm_module::execute(
            &ir,
            "invalid HTTP metadata",
            64,
            Duration::from_secs(15),
            &[],
            Some(&setting),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-api archive; occupied listener belongs to this test only"]
    fn failed_listener_start_drains_application_resources() {
        let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let setting = settings(reservation.local_addr().unwrap().port());
        let ir = application_ir(TRANSPORT, &setting, false, Some(3));
        llvm_module::execute(
            &ir,
            "occupied owned listener",
            64,
            Duration::from_secs(15),
            &[],
            Some(&setting),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-api archive; reads only owned setting.json and captured stderr"]
    fn log_only_cmd_uses_configured_owner_and_flushes_before_return() {
        let files = [
            (
                "sample/log/app.dever",
                "write() (okay: Bool) { dever.log.info(\"filtered-info\", {})\n  dever.log.warn(\"configured-warning\", {\"component\" = \"probe\"})\n  okay = true }",
            ),
            ("sample/log/api.dever", "cmd write = app.write"),
        ];
        let mut setting: Value = serde_json::from_str(&settings(8080)).unwrap();
        setting["log"]["level"] = "warn".into();
        let setting = setting.to_string();
        let ir = application_ir(&files, &setting, false, None);
        let output = llvm_module::execute(
            &ir,
            "log-only CMD configuration",
            64,
            Duration::from_secs(15),
            &["sample.log.write", "{}"],
            Some(&setting),
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("filtered-info"));
        assert_eq!(stderr.matches("configured-warning").count(), 65);
        assert!(stderr.contains("probe"));
    }

    #[test]
    fn http_and_cmd_sources_preserve_checker_and_markdown_contracts() {
        let setting = settings(8080);
        let (_, dever) = checked(&echo_files(false), &setting);
        let (_, markdown) = checked(&echo_files(true), &setting);
        assert_eq!(dever.api_snapshot(), markdown.api_snapshot());
        assert!(dever.api_snapshot().contains("function sample.echo.echo"));
    }

    #[test]
    fn http_application_emits_objects_for_all_six_targets() {
        let ir = application_ir(TRANSPORT, &settings(8080), false, None);
        for target in Target::ALL {
            assert!(!emit_object(&ir, target).unwrap().is_empty());
        }
    }

    #[test]
    fn http_lowering_rejects_unbound_authentication_metadata() {
        for declaration in ["get echo = app.echo", "public get echo = app.echo"] {
            let mut sources = SourceMap::default();
            sources.add(
                "sample/echo/app.dever",
                "echo() (value: Text) { value = \"owned\" }",
            );
            sources.add("sample/echo/api.dever", declaration);
            let program = dever_core::check(&sources).unwrap();
            let error = dever_core::llvm::emit_application(&program, &sources).unwrap_err();
            assert!(error.contains("sample/echo/api.dever:1:1"), "{error}");
            assert!(error.contains("requires a configured site"), "{error}");
        }
    }

    pub(super) struct Server {
        pub(super) directory: temp::TemporaryDirectory,
        child: Child,
        port: u16,
    }

    impl Server {
        fn start(files: &[(&str, &str)], iterations: usize) -> Self {
            Self::start_with(
                files,
                iterations,
                serde_json::from_str(&settings(8080)).unwrap(),
                &[],
            )
        }

        pub(super) fn start_with(
            files: &[(&str, &str)],
            iterations: usize,
            mut setting: Value,
            commands: &[Vec<String>],
        ) -> Self {
            let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = reservation.local_addr().unwrap().port();
            setting["http"]["port"] = port.into();
            let setting = setting.to_string();
            let ir = application_ir(files, &setting, true, None);
            let directory = temp::TemporaryDirectory::new();
            fs::create_dir(directory.path().join("config")).unwrap();
            fs::write(directory.path().join("config/setting.json"), setting).unwrap();
            if !commands.is_empty() {
                let program =
                    llvm_module::link_in(&directory, &ir, "checked management command", 1);
                for arguments in commands {
                    let stderr = directory.path().join("command-stderr");
                    let status = process::status(
                        Command::new(&program)
                            .env_clear()
                            .args(arguments)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(fs::File::create(&stderr).unwrap()),
                        Duration::from_secs(10),
                    )
                    .unwrap();
                    assert!(
                        status.success(),
                        "management command failed {arguments:?}: {status}\n{}",
                        fs::read_to_string(stderr).unwrap()
                    );
                }
            }
            let program =
                llvm_module::link_in(&directory, &ir, "checked HTTP application", iterations);
            drop(reservation);
            let child = Command::new(program)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(fs::File::create(directory.path().join("stdout")).unwrap())
                .stderr(fs::File::create(directory.path().join("stderr")).unwrap())
                .spawn()
                .unwrap();
            Self {
                directory,
                child,
                port,
            }
        }

        pub(super) fn ready(&mut self) {
            self.ready_at("/sample/echo/increment?q=1");
        }

        pub(super) fn ready_at(&mut self, path: &str) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(response) = self.request("GET", path, &[], "") {
                    assert_eq!(response.status, 200);
                    assert_eq!(response.body["data"], 2);
                    return;
                }
                if let Some(status) = self.child.try_wait().unwrap() {
                    panic!(
                        "HTTP application exited before readiness: {status}\n{}",
                        self.stderr()
                    );
                }
                assert!(
                    Instant::now() < deadline,
                    "HTTP application did not become ready\n{}",
                    self.stderr()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        pub(super) fn request(
            &self,
            method: &str,
            path: &str,
            headers: &[(&str, &str)],
            body: &str,
        ) -> std::io::Result<Reply> {
            let mut connection = TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], self.port)),
                Duration::from_secs(1),
            )?;
            connection.set_read_timeout(Some(Duration::from_secs(5)))?;
            connection.set_write_timeout(Some(Duration::from_secs(5)))?;
            write!(
                connection,
                "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
                body.len()
            )?;
            for (name, value) in headers {
                write!(connection, "{name}: {value}\r\n")?;
            }
            write!(connection, "\r\n{body}")?;
            let mut raw = Vec::new();
            connection.take(1_048_577).read_to_end(&mut raw)?;
            assert!(
                raw.len() <= 1_048_576,
                "fixture response exceeded its bound"
            );
            if !raw.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "{method} {path}: no complete HTTP response\n{}",
                        self.stderr()
                    ),
                ));
            }
            Ok(Reply::parse(&raw))
        }

        pub(super) fn stop_cycle(&mut self, completed: usize) {
            assert!(
                process::status(
                    Command::new("/usr/bin/kill")
                        .env_clear()
                        .arg("-TERM")
                        .arg(self.child.id().to_string()),
                    Duration::from_secs(2),
                )
                .unwrap()
                .success()
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let output = fs::read_to_string(self.directory.path().join("stdout")).unwrap();
                if output
                    .lines()
                    .filter(|line| *line == "api-cycle-complete")
                    .count()
                    == completed
                {
                    return;
                }
                if let Some(status) = self.child.try_wait().unwrap() {
                    panic!(
                        "HTTP application exited before completing cycle {completed}: {status}\n{}",
                        self.stderr()
                    );
                }
                assert!(
                    Instant::now() < deadline,
                    "HTTP cycle did not drain\n{}",
                    self.stderr()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        pub(super) fn finish(&mut self) {
            let status = process::wait(&mut self.child, Duration::from_secs(5)).unwrap();
            assert!(
                status.success(),
                "HTTP application failed allocation/fault oracle: {status}\n{}",
                self.stderr()
            );
        }

        pub(super) fn restart(&mut self, mode: &str) {
            assert!(
                self.child
                    .try_wait()
                    .unwrap()
                    .is_some_and(|status| status.success())
            );
            let path = self.directory.path().join("config/setting.json");
            let mut setting: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            setting["runtime"]["mode"] = mode.into();
            fs::write(path, setting.to_string()).unwrap();
            self.child = Command::new(self.directory.path().join("managed-kernel"))
                .env_clear()
                .stdin(Stdio::null())
                .stdout(fs::File::create(self.directory.path().join("stdout")).unwrap())
                .stderr(fs::File::create(self.directory.path().join("stderr")).unwrap())
                .spawn()
                .unwrap();
        }

        pub(super) fn stderr(&self) -> String {
            fs::read_to_string(self.directory.path().join("stderr")).unwrap()
        }

        fn disconnect_partial_upload(&self) {
            let directory = self.directory.path().join("data/tmp");
            let mut connection = TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], self.port)),
                Duration::from_secs(1),
            )
            .unwrap();
            connection
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            write!(connection, "POST /media/image/store HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=boundary\r\nContent-Length: 65536\r\n\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"partial.txt\"\r\n\r\n").unwrap();
            // 超过解码器的一块正文，避免请求头/正文的 TCP 合并决定是否写盘；
            // 仍少于 Content-Length，关闭连接才会结束这次不完整上传。
            connection.write_all(&[b'x'; 32 * 1024]).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !fs::read_dir(&directory)
                .unwrap()
                .any(|entry| entry.unwrap().metadata().unwrap().len() > 1024)
            {
                assert!(
                    Instant::now() < deadline,
                    "incomplete body never reached the upload owner"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            connection.shutdown(std::net::Shutdown::Both).unwrap();
            drop(connection);
            while fs::read_dir(&directory).unwrap().count() != 0 {
                assert!(
                    Instant::now() < deadline,
                    "disconnected upload retained its temporary file"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = process::terminate(&mut self.child);
            }
        }
    }

    pub(super) struct Reply {
        pub(super) status: u16,
        pub(super) headers: Vec<(String, String)>,
        pub(super) body: Value,
    }

    impl Reply {
        fn parse(raw: &[u8]) -> Self {
            let split = raw
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .expect("HTTP response headers");
            let header = std::str::from_utf8(&raw[..split]).unwrap();
            let mut lines = header.split("\r\n");
            let status = lines
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse()
                .unwrap();
            let headers = lines
                .map(|line| {
                    let (name, value) = line.split_once(':').unwrap();
                    (name.to_ascii_lowercase(), value.trim().to_owned())
                })
                .collect::<Vec<_>>();
            assert!(
                !headers
                    .iter()
                    .any(|(name, value)| name == "transfer-encoding" && value != "identity"),
                "fixture requires a bounded Content-Length response"
            );
            let body = serde_json::from_slice(&raw[split + 4..]).unwrap();
            Self {
                status,
                headers,
                body,
            }
        }
    }

    #[test]
    #[ignore = "requires explicit runtime-api archive; does not start a listener"]
    fn mixed_http_application_dispatches_cmd_without_binding_http() {
        let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let setting = settings(reservation.local_addr().unwrap().port());
        let payload = json!({"name": "owned", "count": 4, "tags": [], "note": null});
        let ir = application_ir(&echo_files(true), &setting, false, None);
        let body = json!({"payload": payload}).to_string();
        let output = llvm_module::execute(
            &ir,
            "API plus CMD",
            64,
            Duration::from_secs(15),
            &["sample.echo.echo", &body],
            Some(&setting),
        );
        let lines = std::str::from_utf8(&output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 65);
        for line in lines {
            assert_eq!(
                serde_json::from_str::<Value>(line).unwrap()["data"],
                payload
            );
        }
        let ir = application_ir(TRANSPORT, &setting, false, Some(3));
        llvm_module::execute(
            &ir,
            "API CMD rejects invalid input",
            64,
            Duration::from_secs(15),
            &["sample.echo.echo", "{}"],
            Some(&setting),
        );
    }

    #[test]
    #[ignore = "requires explicit runtime-api archive; starts only test-owned loopback services"]
    fn public_http_inputs_context_errors_and_repeated_roots_release_owners() {
        let mut server = Server::start(TRANSPORT, 64);
        let mut request_ids = std::collections::BTreeSet::new();
        for cycle in 1..=65 {
            server.ready();
            let context = server
                .request(
                    "GET",
                    "/sample/echo/inspect?ignored=1",
                    &[("x-probe", "owned"), ("cookie", "probe=ordinary")],
                    "",
                )
                .unwrap();
            assert_eq!(context.status, 400);
            let context = server
                .request(
                    "GET",
                    "/sample/echo/inspect",
                    &[("x-probe", "owned"), ("cookie", "probe=ordinary")],
                    "",
                )
                .unwrap();
            assert_eq!(context.status, 200);
            let data = &context.body["data"];
            assert_eq!(data["method"], "GET");
            assert_eq!(data["path"], "/sample/echo/inspect");
            assert_eq!(data["header"], "owned");
            assert_eq!(data["cookie"], "ordinary");
            assert_eq!(data["peer"], "127.0.0.1");
            assert!(request_ids.insert(data["request_id"].as_str().unwrap().to_owned()));
            assert!(
                context
                    .headers
                    .contains(&("x-response-probe".into(), "committed".into()))
            );
            for path in [
                "/sample/echo/increment",
                "/sample/echo/increment?q=x",
                "/sample/echo/increment?q=1&q=2",
                "/sample/echo/increment?q=1&unknown=2",
            ] {
                assert_eq!(server.request("GET", path, &[], "").unwrap().status, 400);
            }
            let payload = json!({"name": "你好 🌍", "count": 9_007_199_254_740_993_i64, "tags": ["owned", null], "note": null});
            let body = json!({"payload": payload}).to_string();
            let response = server
                .request(
                    "POST",
                    "/sample/echo/echo",
                    &[("content-type", "application/json")],
                    &body,
                )
                .unwrap();
            assert_eq!(response.status, 200, "{}", response.body);
            assert_eq!(
                response.body,
                json!({"code": 0, "message": "ok", "data": payload})
            );
            let multipart = "--boundary\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\nowned\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"owned.txt\"\r\nContent-Type: text/plain\r\n\r\n你好\r\n--boundary--\r\n";
            let response = server
                .request(
                    "POST",
                    "/sample/echo/upload",
                    &[("content-type", "multipart/form-data; boundary=boundary")],
                    multipart,
                )
                .unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.body["data"], 6);
            assert_eq!(
                fs::read_dir(server.directory.path().join("data/tmp"))
                    .unwrap()
                    .count(),
                0
            );
            for (action, status) in [
                ("invalid", 400),
                ("unauthorized", 401),
                ("forbidden", 403),
                ("missing", 404),
                ("conflict", 409),
                ("limited", 429),
                ("fault", 500),
            ] {
                let response = server
                    .request("GET", &format!("/sample/echo/{action}"), &[], "")
                    .unwrap();
                assert_eq!(response.status, status);
                assert_eq!(response.body["code"], status);
                assert_eq!(response.body["data"], Value::Null);
                assert!(
                    !response
                        .headers
                        .iter()
                        .any(|(name, _)| name == "x-response-probe")
                );
                assert!(!response.body.to_string().contains("must-not-appear"));
            }
            assert_eq!(
                server
                    .request("DELETE", "/sample/echo/increment", &[], "")
                    .unwrap()
                    .status,
                405
            );
            assert_eq!(
                server.request("GET", "/missing", &[], "").unwrap().status,
                404
            );
            server.stop_cycle(cycle);
        }
        server.finish();
        assert!(!server.stderr().contains("must-not-appear-in-http-or-log"));
    }

    #[test]
    #[ignore = "requires explicit runtime-api archive; owns listener, upload files and storage outputs"]
    fn upload_adapter_store_and_failed_input_release_affine_owners() {
        let mut files = TRANSPORT.to_vec();
        files.extend([
            (
                "media/image/app.dever",
                "accept(file: Upload) (key: Uuid) { key = port.store(file) }",
            ),
            (
                "media/image/port.dever",
                "store(file: Upload) (key: Uuid) fails dever.storage.PutResult",
            ),
            (
                "media/image/adapter.dever",
                "port.store(file: Upload) (key: Uuid) { key = dever.storage.put(file) }",
            ),
            ("media/image/api.dever", "public post store = app.accept"),
        ]);
        let mut server = Server::start(&files, 64);
        let multipart = "--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"owned.txt\"\r\nContent-Type: text/plain\r\n\r\n你好\r\n--boundary--\r\n";
        for cycle in 1..=65 {
            server.ready();
            let response = server
                .request(
                    "POST",
                    "/media/image/store",
                    &[("content-type", "multipart/form-data; boundary=boundary")],
                    multipart,
                )
                .unwrap();
            assert_eq!(response.status, 200, "{}", response.body);
            let key = response.body["data"].as_str().unwrap();
            assert_eq!(
                dever_runtime::orm::Uuid::parse(key).unwrap().to_string(),
                key
            );
            let stored = server.directory.path().join("data/upload").join(key);
            assert_eq!(fs::read(&stored).unwrap(), "你好".as_bytes());
            fs::remove_file(stored).unwrap();
            server.disconnect_partial_upload();
            // File was decoded before the missing caption was discovered.
            assert_eq!(
                server
                    .request(
                        "POST",
                        "/sample/echo/upload",
                        &[("content-type", "multipart/form-data; boundary=boundary")],
                        multipart
                    )
                    .unwrap()
                    .status,
                400
            );
            assert_eq!(
                fs::read_dir(server.directory.path().join("data/tmp"))
                    .unwrap()
                    .count(),
                0
            );
            server.stop_cycle(cycle);
        }
        server.finish();
    }
}
