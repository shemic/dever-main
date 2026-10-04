//! Trust-chain regression checks touch only an explicitly owned target subtree.
use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, chown, symlink};

#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;

#[test]
fn restricted_hosts_require_strict_profile_transitions_before_helper_selection() {
    use std::io::{Error, ErrorKind};
    for userns in [None, Some("0\n")] {
        assert!(
            !host_policy(|path| {
                assert!(
                    path.ends_with("apparmor_restrict_unprivileged_userns"),
                    "unrestricted hosts must not read the strict flag"
                );
                userns
                    .map(str::to_owned)
                    .ok_or_else(|| Error::from(ErrorKind::NotFound))
            })
            .unwrap()
        );
    }
    for strict in [None, Some("0\n"), Some("1\n")] {
        let result = host_policy(|path| {
            if path.ends_with("apparmor_restrict_unprivileged_userns") {
                return Ok("1\n".into());
            }
            assert!(path.ends_with("apparmor_restrict_unprivileged_unconfined"));
            strict
                .map(str::to_owned)
                .ok_or_else(|| Error::from(ErrorKind::NotFound))
        });
        if strict == Some("1\n") {
            assert!(result.unwrap());
        } else {
            assert!(
                result
                    .unwrap_err()
                    .contains("kernel.apparmor_restrict_unprivileged_unconfined=1")
            );
        }
    }
    for failure in [ErrorKind::PermissionDenied, ErrorKind::InvalidData] {
        assert!(
            host_policy(|_| Err(Error::from(failure)))
                .unwrap_err()
                .contains("cannot inspect")
        );
    }
    assert!(
        host_policy(|_| Ok("2\n".into()))
            .unwrap_err()
            .contains("invalid AppArmor kernel flag")
    );
    assert!(
        host_policy(
            |path| if path.ends_with("apparmor_restrict_unprivileged_userns") {
                Ok("1".into())
            } else {
                Err(Error::from(ErrorKind::PermissionDenied))
            }
        )
        .unwrap_err()
        .contains("cannot inspect")
    );
}

#[test]
#[ignore = "requires root and the prepared static author bwrap; touches only owned target files"]
fn rejects_untrusted_helper_paths_bytes_modes_owners_and_capabilities() {
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()
        .unwrap();
    let root = temp::TemporaryDirectory::new_in(&target);
    let bytes = fs::read(target.join("sandbox-inputs/assets/bin/bwrap")).unwrap();
    let hash = crate::validate_bwrap(&bytes).unwrap();
    let directory = root.path().join(&hash);
    fs::create_dir(&directory).unwrap();
    let helper = directory.join("bwrap");
    assert!(
        verify_helper(&helper, &hash)
            .unwrap_err()
            .contains("missing")
    );
    fs::write(&helper, &bytes).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    verify_helper(&helper, &hash).unwrap();
    assert!(
        verify_helper(&helper, &"0".repeat(64))
            .unwrap_err()
            .contains("differs")
    );
    for mode in [0o775, 0o4755, 0o2755, 0o644] {
        fs::set_permissions(&helper, fs::Permissions::from_mode(mode)).unwrap();
        assert!(verify_helper(&helper, &hash).is_err());
    }
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    chown(&helper, Some(65534), Some(65534)).unwrap();
    assert!(verify_helper(&helper, &hash).is_err());
    chown(&helper, Some(0), Some(0)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(verify_helper(&helper, &hash).is_err());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let renamed = root.path().join("retained");
    fs::rename(&directory, &renamed).unwrap();
    symlink(&renamed, &directory).unwrap();
    assert!(verify_helper(&helper, &hash).is_err());
    fs::remove_file(&directory).unwrap();
    fs::rename(&renamed, &directory).unwrap();
    // VFS_CAP_REVISION_2 with only CAP_NET_ADMIN permitted; never execute it.
    let mut capability = [0u8; 20];
    capability[..4].copy_from_slice(&0x0200_0001u32.to_le_bytes());
    capability[4..8].copy_from_slice(&(1u32 << 12).to_le_bytes());
    rustix::fs::setxattr(
        &helper,
        "security.capability",
        &capability,
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    assert!(
        verify_helper(&helper, &hash)
            .unwrap_err()
            .contains("file capabilities")
    );
    rustix::fs::removexattr(&helper, "security.capability").unwrap();
    verify_helper(&helper, &hash).unwrap();
    // A sticky /tmp ancestor is acceptable for installer image fixtures, but
    // cannot be part of the runtime's administrator-controlled attachment path.
    let shared_ancestor = temp::TemporaryDirectory::new();
    let shared_helper = shared_ancestor.path().join("bwrap");
    fs::write(&shared_helper, &bytes).unwrap();
    fs::set_permissions(&shared_helper, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(verify_helper(&shared_helper, &hash).is_err());
}
