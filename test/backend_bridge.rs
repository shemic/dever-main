use dever_backend_bridge::{
    BinaryResource, LinkKind, Target, emit_object, emit_object_with_resources, link,
};
use std::fs;
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

const IR: &str = "define i64 @answer() { ret i64 42 }";

#[test]
fn binary_initializers_reject_invalid_declarations_and_preserve_code_limit() {
    let resource = BinaryResource {
        symbol: "payload",
        bytes: &[1, 2],
    };
    for declaration in [
        "@other = external constant [2 x i8]",
        "@payload = external constant [3 x i8]",
        "@payload = external constant [2 x i16]",
        "@payload = external global [2 x i8]",
        "@payload = constant [2 x i8] zeroinitializer",
        "@payload = external thread_local constant [2 x i8]",
    ] {
        let ir = format!("{declaration}\ndefine ptr @read() {{ ret ptr @payload }}");
        assert!(emit_object_with_resources(&ir, Target::LinuxX86_64, &[resource]).is_err());
    }
    let ir = "@payload = external constant [2 x i8]\ndefine ptr @read() { ret ptr @payload }";
    assert!(
        emit_object_with_resources(ir, Target::LinuxX86_64, &[resource, resource])
            .unwrap_err()
            .contains("duplicate")
    );
    assert!(
        emit_object_with_resources(
            "@payload = external constant [2 x i8]",
            Target::LinuxX86_64,
            &[resource]
        )
        .unwrap_err()
        .contains("used external")
    );
    for symbol in ["", "1bad", "with.dot", "bad\0name"] {
        assert!(
            emit_object_with_resources(
                ir,
                Target::LinuxX86_64,
                &[BinaryResource { symbol, ..resource }]
            )
            .unwrap_err()
            .contains("symbol")
        );
    }
    assert!(
        emit_object_with_resources(ir, Target::LinuxX86_64, &vec![resource; 4097])
            .unwrap_err()
            .contains("4096")
    );
    assert!(
        emit_object(&" ".repeat(8 * 1024 * 1024 + 1), Target::LinuxX86_64)
            .unwrap_err()
            .contains("8388608")
    );
    let empty = BinaryResource {
        symbol: "empty",
        bytes: &[],
    };
    for target in Target::ALL {
        assert!(
            !emit_object_with_resources(
                "@empty = external constant [0 x i8]\ndefine ptr @read() { ret ptr @empty }",
                target,
                &[empty]
            )
            .unwrap()
            .is_empty()
        );
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn binary_payload_larger_than_text_limit_links_and_preserves_every_byte() {
    let bytes = (0..9 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let ir = format!(
        r#"@payload = external constant [{} x i8]
define void @_start() noreturn {{
  %written = call i64 asm sideeffect "syscall", "={{rax}},{{rax}},{{rdi}},{{rsi}},{{rdx}},~{{rcx}},~{{r11}},~{{memory}}"(i64 1, i64 1, ptr @payload, i64 {})
  %complete = icmp eq i64 %written, {}
  %failed = select i1 %complete, i64 0, i64 1
  call void asm sideeffect "syscall", "{{rax}},{{rdi}},~{{rcx}},~{{r11}},~{{memory}}"(i64 60, i64 %failed)
  unreachable
}}"#,
        bytes.len(),
        bytes.len(),
        bytes.len()
    );
    assert!(ir.len() < 1024);
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("entry.o");
    fs::write(
        &object,
        emit_object_with_resources(
            &ir,
            Target::LinuxX86_64,
            &[BinaryResource {
                symbol: "payload",
                bytes: &bytes,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    let executable = directory.path().join("program");
    link(
        Target::LinuxX86_64,
        LinkKind::Executable,
        &[object],
        &executable,
        Some("_start"),
    )
    .unwrap();
    let output = directory.path().join("bytes");
    let status = std::process::Command::new(executable)
        .env_clear()
        .stdout(fs::File::create(&output).unwrap())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read(output).unwrap(), bytes);
}

#[test]
fn six_targets_emit_real_architecture_specific_objects_and_link() {
    let directory = temp::TemporaryDirectory::new();
    for (index, target) in Target::ALL.into_iter().enumerate() {
        let bytes = emit_object(IR, target).unwrap();
        assert!(bytes.len() > 64);
        match target {
            Target::LinuxX86_64 | Target::LinuxAarch64 => {
                assert_eq!(&bytes[..4], b"\x7fELF");
                let machine = u16::from_le_bytes(bytes[18..20].try_into().unwrap());
                assert_eq!(
                    machine,
                    if target == Target::LinuxX86_64 {
                        62
                    } else {
                        183
                    }
                );
            }
            Target::MacosX86_64 | Target::MacosAarch64 => {
                assert_eq!(&bytes[..4], b"\xcf\xfa\xed\xfe");
                let cpu = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                assert_eq!(
                    cpu,
                    if target == Target::MacosX86_64 {
                        0x1000007
                    } else {
                        0x100000c
                    }
                );
            }
            Target::WindowsX86_64 | Target::WindowsAarch64 => {
                let machine = u16::from_le_bytes(bytes[..2].try_into().unwrap());
                assert_eq!(
                    machine,
                    if target == Target::WindowsX86_64 {
                        0x8664
                    } else {
                        0xaa64
                    }
                );
            }
        }
        let object = directory.path().join(format!("{index}.o"));
        let output = directory.path().join(format!("{index}.image"));
        fs::write(&object, bytes).unwrap();
        link(target, LinkKind::Shared, &[object], &output, None).unwrap();
        assert!(fs::metadata(output).unwrap().len() > 128);
    }
}

#[test]
fn malformed_ir_target_mismatch_and_failed_link_do_not_poison_later_calls() {
    assert!(emit_object("not LLVM", Target::LinuxX86_64).is_err());
    assert!(emit_object("define i64 @bad() { ret i32 3 }", Target::LinuxX86_64).is_err());
    assert!(emit_object("", Target::LinuxX86_64).is_err());
    assert!(
        emit_object("\0", Target::LinuxX86_64)
            .unwrap_err()
            .contains("NUL")
    );
    let invalid_ssa = "define i64 @bad(i1 %condition) {\nentry: br i1 %condition, label %left, label %right\nleft: %value = add i64 40, 2\nbr label %done\nright: br label %done\ndone: ret i64 %value\n}";
    assert!(
        emit_object(invalid_ssa, Target::LinuxX86_64)
            .unwrap_err()
            .contains("does not dominate")
    );
    assert!(
        emit_object(
            "target datalayout = \"e-p:32:32\"\ndefine i64 @answer() { ret i64 42 }",
            Target::LinuxX86_64
        )
        .unwrap_err()
        .contains("layout disagrees")
    );
    assert!(
        emit_object(
            "target triple = \"aarch64-unknown-linux-gnu\"\n",
            Target::LinuxX86_64
        )
        .unwrap_err()
        .contains("disagrees")
    );
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("bad.o");
    fs::write(&object, b"not object").unwrap();
    let output = directory.path().join("image");
    assert!(
        link(
            Target::LinuxX86_64,
            LinkKind::Shared,
            std::slice::from_ref(&object),
            &output,
            None
        )
        .is_err()
    );
    assert!(!output.exists());
    fs::write(&object, emit_object(IR, Target::LinuxX86_64).unwrap()).unwrap();
    link(
        Target::LinuxX86_64,
        LinkKind::Shared,
        std::slice::from_ref(&object),
        &output,
        None,
    )
    .unwrap();
    let original = fs::read(&output).unwrap();
    assert!(
        link(
            Target::LinuxX86_64,
            LinkKind::Shared,
            &[object],
            &output,
            None
        )
        .is_err()
    );
    assert_eq!(fs::read(output).unwrap(), original);
}

#[test]
fn concurrent_links_cannot_overwrite_the_same_new_output() {
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("entry.o");
    let output = directory.path().join("image");
    fs::write(&object, emit_object(IR, Target::LinuxX86_64).unwrap()).unwrap();
    std::thread::scope(|scope| {
        let execute = || {
            link(
                Target::LinuxX86_64,
                LinkKind::Shared,
                std::slice::from_ref(&object),
                &output,
                None,
            )
        };
        let left = scope.spawn(execute);
        let right = scope.spawn(execute);
        assert_eq!(
            [left.join().unwrap(), right.join().unwrap()]
                .iter()
                .filter(|result| result.is_ok())
                .count(),
            1
        );
    });
    assert!(fs::metadata(output).unwrap().len() > 128);
}

#[cfg(unix)]
#[test]
fn link_rejects_shared_directories_and_mutable_ancestors_before_claiming_output() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("entry.o");
    fs::write(&object, emit_object(IR, Target::LinuxX86_64).unwrap()).unwrap();
    let output = directory.path().join("image");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        link(
            Target::LinuxX86_64,
            LinkKind::Shared,
            std::slice::from_ref(&object),
            &output,
            None
        )
        .unwrap_err()
        .contains("private directory")
    );
    assert!(!output.exists());

    let leaf = directory.path().join("private");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(&leaf).unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let output = leaf.join("image");
    let result = link(
        Target::LinuxX86_64,
        LinkKind::Shared,
        std::slice::from_ref(&object),
        &output,
        None,
    );
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.unwrap_err().contains("publicly writable ancestor"));
    assert!(!output.exists());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn native_llvm_lld_program_executes_without_rustc_or_cargo() {
    // Test-owned freestanding process: no runtime or system library dependency.
    let ir = "define void @_start() noreturn {\ncall void asm sideeffect \"syscall\", \"{rax},{rdi},~{rcx},~{r11},~{memory}\"(i64 60, i64 42)\nunreachable\n}";
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("entry.o");
    fs::write(&object, emit_object(ir, Target::LinuxX86_64).unwrap()).unwrap();
    let output = directory.path().join("program");
    link(
        Target::LinuxX86_64,
        LinkKind::Executable,
        &[object],
        &output,
        Some("_start"),
    )
    .unwrap();
    let status = std::process::Command::new(output)
        .env_clear()
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(42));
}
