use crate::collections::List;

pub fn arguments() -> Result<List<String>, String> {
    // argv[0] 是宿主可执行文件路径，不属于应用参数。
    std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| "process argument is not valid UTF-8".into())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(List::new)
}
