# 源码事实

- source.rs 只收集 `.dever`，保存原文、SourceId 和行开始位置；符号链接全部拒绝。
- lib.rs 的 parse_uncached 共用 lexer→parser；lexer 当前读取 SourceFile.text()。
- check.rs 的 check_package 固定 `.dever` 路径；注册阶段已有重复包检测。
- format.rs 从 token 注释和 AST 生成整个包；CLI 先准备全部字符串，再快照核验和逐文件原子替换。
- check/cache.rs 键含源码全文/路径，文档改动仍需重新建立准确诊断位置。
- pulldown-cmark 提供带偏移的块事件：https://docs.rs/pulldown-cmark/0.13.4/pulldown_cmark/struct.Parser.html 。当前未缓存，新增这一个编译器依赖用于避免不完整的 Markdown 自制解析器。
