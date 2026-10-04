# 设计

- `skills/` 为可独立复制/后续建 Git 的 skill 根目录，包含 SKILL.md、按需语言/安装参考、共享模板资源。名称 dever-language，避免与 shemic-dever Go skill 混淆。
- CLI 复用该目录的模板字节，生成器拥有文件系统发布边界；新项目使用 app + cmd API + 零输入测试，不生成 main/package/expose 或无用业务分层。
- new 属于非项目命令，机器启动器使用活动核心；模板与发行核心版本一致。
- skill 纳入发行签名清单并由版本库管理。安装和更新沿用现有机器权限、锁、暂存、校验和恢复机制；可发现路径必须随活动版本切换，不要求每个项目复制或手动更新技能。
- 明确区分独立 skill 安装、机器 Dever 安装、应用依赖准备。官方发行源内置为 GitHub shemic/dever-main Releases，不读取业务配置或环境变量。Git 仓库和公开二进制资产区分，未发布资产时明确报错。
- 安装到 AI 的稳定 skill 入口先执行绝对机器启动器的 `skill path`，再加载活动版本的完整 skill。继续保留唯一 active-version journal 提交点，避免核心和 skill 双指针更新撕裂。
- 实施前保存涉及源码的自有备份；并行任务按文件归属分工，Cargo 验证由主代理串行执行以控制磁盘。
