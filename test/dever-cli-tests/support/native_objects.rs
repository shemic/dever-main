//! Small architecture-bearing link inputs for pack validation tests.
use dever_cli::toolchain::BuildTarget;

pub fn object(target: BuildTarget) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&1_u16.to_le_bytes());
    let machine: u16 = match target {
        BuildTarget::LinuxX86_64 => 62,
        BuildTarget::LinuxAarch64 => 183,
    };
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes
}

pub fn archive(target: BuildTarget, member: &str) -> Vec<u8> {
    let object = object(target);
    let header = format!(
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
        format!("{member}/"),
        0,
        0,
        0,
        "644",
        object.len()
    );
    assert_eq!(header.len(), 60);
    [b"!<arch>\n".as_slice(), header.as_bytes(), &object].concat()
}
