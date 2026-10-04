use crate::collections::List;

pub fn codepoint(value: &str) -> Option<i64> {
    let mut chars = value.chars();
    let first = chars.next()?;
    chars
        .next()
        .is_none()
        .then_some(i64::from(u32::from(first)))
}

pub fn from_codepoint(value: i64) -> Option<String> {
    char::from_u32(u32::try_from(value).ok()?).map(String::from)
}

pub fn at(value: &str, index: i64) -> Option<String> {
    value
        .chars()
        .nth(usize::try_from(index).ok()?)
        .map(String::from)
}

/// Text 的位置按 Unicode scalar 计数；Bytes API 单独使用字节偏移。
pub fn slice(value: &str, start: i64, end: i64) -> Option<String> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    if start > end {
        return None;
    }
    let mut boundaries = value
        .char_indices()
        .map(|(index, _)| index)
        .chain([value.len()]);
    let first = boundaries.nth(start)?;
    let last = if start == end {
        first
    } else {
        boundaries.nth(end - start - 1)?
    };
    Some(value[first..last].to_owned())
}

pub fn index_of(value: &str, pattern: &str) -> Option<i64> {
    value
        .find(pattern)
        .map(|offset| value[..offset].chars().count() as i64)
}

pub fn split(value: &str, separator: &str) -> List<String> {
    List::new(pieces(value, separator).collect())
}

pub fn pieces<'a>(value: &'a str, separator: &'a str) -> impl Iterator<Item = String> + 'a {
    value
        .split(separator)
        .filter(move |piece| !separator.is_empty() || !piece.is_empty())
        .map(str::to_owned)
}
