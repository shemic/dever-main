//! Small ELF metadata fixtures; execution acceptance uses real author inputs.
pub fn compiler(needed: &[&str]) -> Vec<u8> {
    elf(None, needed, Some("$ORIGIN/lib"))
}

pub fn library(name: &str, needed: &[&str]) -> Vec<u8> {
    elf(Some(name), needed, None)
}

pub fn static_executable() -> Vec<u8> {
    let mut bytes = elf(None, &[], None);
    // Change the synthetic PT_INTERP entry to PT_NULL. Execution tests use the
    // real static author input; this fixture tests only ELF admission rules.
    bytes[64 + 2 * 56..68 + 2 * 56].copy_from_slice(&0u32.to_le_bytes());
    bytes
}

pub fn elf(soname: Option<&str>, needed: &[&str], rpath: Option<&str>) -> Vec<u8> {
    let mut strings = vec![0u8];
    let mut string = |text: &str| {
        let offset = strings.len() as u64;
        strings.extend_from_slice(text.as_bytes());
        strings.push(0);
        offset
    };
    let mut dynamic = needed
        .iter()
        .map(|name| (1u64, string(name)))
        .collect::<Vec<_>>();
    if let Some(name) = soname {
        dynamic.push((14, string(name)));
    }
    if let Some(path) = rpath {
        dynamic.push((15, string(path)));
    }
    let interpreter = if cfg!(target_arch = "aarch64") {
        b"/lib/ld-linux-aarch64.so.1\0".as_slice()
    } else {
        b"/lib64/ld-linux-x86-64.so.2\0".as_slice()
    };
    let header_count: u16 = if soname.is_some() { 2 } else { 3 };
    let string_offset = 64 + usize::from(header_count) * 56;
    let interpreter_offset = string_offset + strings.len();
    let dynamic_offset = (interpreter_offset + interpreter.len() + 7) & !7;
    dynamic.extend([
        (5, string_offset as u64),
        (10, strings.len() as u64),
        (0, 0),
    ]);
    let length = dynamic_offset + dynamic.len() * 16;
    let mut bytes = vec![0; length];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
    let machine = if cfg!(target_arch = "aarch64") {
        183u16
    } else {
        62u16
    };
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&header_count.to_le_bytes());
    let mut segment = |index: usize, kind: u32, offset: usize, size: usize| {
        let start = 64 + index * 56;
        bytes[start..start + 4].copy_from_slice(&kind.to_le_bytes());
        bytes[start + 4..start + 8].copy_from_slice(&4u32.to_le_bytes());
        for at in [8, 16, 24] {
            bytes[start + at..start + at + 8].copy_from_slice(&(offset as u64).to_le_bytes());
        }
        for at in [32, 40] {
            bytes[start + at..start + at + 8].copy_from_slice(&(size as u64).to_le_bytes());
        }
        bytes[start + 48..start + 56].copy_from_slice(&1u64.to_le_bytes());
    };
    segment(0, 1, 0, length);
    segment(1, 2, dynamic_offset, dynamic.len() * 16);
    if soname.is_none() {
        segment(2, 3, interpreter_offset, interpreter.len());
    }
    bytes[string_offset..interpreter_offset].copy_from_slice(&strings);
    bytes[interpreter_offset..interpreter_offset + interpreter.len()].copy_from_slice(interpreter);
    for (index, (tag, value)) in dynamic.into_iter().enumerate() {
        let start = dynamic_offset + index * 16;
        bytes[start..start + 8].copy_from_slice(&tag.to_le_bytes());
        bytes[start + 8..start + 16].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
