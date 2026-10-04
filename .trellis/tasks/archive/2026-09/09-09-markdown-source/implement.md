# 执行与验证

用户已在双格式方案后确认并选择 `.dever.md`，无需重复请求实施授权。

- [x] Core：程序块识别、原文位置、加载与包路径、共用解析和格式化。
- [x] 定向测试：提取/拒绝边界、中文换行定位、混用/重复包/契约、格式化保留说明和幂等。
- [x] 示例和 CLI：结算示例、空源码提示、实际 check/fmt/run/build 的隔离定向验证。
- [x] 文档规范：语言说明、实现说明、过时草案、AGENTS 源码规则。
- [x] 检查：相关 Rust 格式、定向 Clippy/Markdown/formatter 测试及独立复核。

`/root/.cargo/bin` 不在 PATH 时只设置命令级 PATH。持久测试放根 `test/`，临时程序归测试所有；无服务或网络操作，无全量/集成测试，无提交。仓库大部分文件原本未跟踪，diff 以本轮前快照为准。

## 完成证据（2026-09-09）

- `cargo test --offline -p dever-tests --test markdown_source --test formatter --test syntax_and_format`：45 项通过（12 + 11 + 22）。主代理已检查原始日志 `/tmp/dever-markdown-checks.log`。
- 同目标 `cargo-clippy clippy --offline ... -- -D warnings` 通过；CLI 单独 `-p dever-cli --bin deverc -- -D warnings` 通过。10 个修改 Rust 文件的定向 rustfmt 检查通过。
- 实际 CLI 对 `examples/markdown` 的 check/fmt/run 通过，输出 `customer = 小明`、`subtotal = 47.5`、`discount = 4.75`、`payable = 42.75`。普通 `.dever` 计价包被 Markdown 包调用。
- 根 test/ 下临时程序验证：无 rustc 时 check 仍可用、两种格式的 API 相同、普通 README 忽略、fmt --check 不写、第二文件错误阻止所有改写、CRLF/说明/围栏保持及幂等、run/build、PATH 不含工具链时独立运行、已有输出不覆盖、重复包 C002、C005 精确到 Markdown 第 11 行第 20 列、运行除零定位 Markdown 第 11 行。临时文件和二进制已清理。
- 最终 core 的实际 CLI 裸 CR/缩进围栏验证通过：中文原文 C005 第 10 行第 31 列、格式化无新增 LF、说明保持、check 和 fmt --check 通过。首次手写列号预期少算一列，按原文核对修正，非实现缺陷。
- 独立只读审查无阻塞。补充验证包头前后独立注释块、跨块 Bool 分句、同一文件各块 LF/CRLF/CR、围栏/正文保持和幂等通过。
- 新编译器依赖 `pulldown-cmark = 0.13.4`（关闭默认特性）及三个传递依赖已获取并锁定；不进入运行时，验证全部离线。复用已有 target/verification-tools 中的 rustfmt/Clippy，未安装全局组件。
- 更新过时标题语法草案、当前说明和旧主任务源码格式限制。28 个实现/文档变更文件空白检查通过；未修改现有无关删除项。

无未解决阻塞。未运行全量、集成或服务测试，未提交 Git；开发工作区改动保留供审查。
