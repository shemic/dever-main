# Dever 开发入口

用户已授权实施：独立 `/data/project/dever/skills`、开发指引、新项目模板、`dever new`、安装引导以及 `dever update` 同步更新语言和 skill。

## 验收

- skill 根目录可独立分发，名称 `dever-language`，不依赖本机源码绝对路径，不混入 Go 框架规范；指导 AI 安装、编写、检查、测试和构建 Dever 应用。
- `dever new <new-directory> [--markdown]` 生成最小可运行 CMD 项目、配置、项目 AI 指引和定向应用测试；两种源码格式共享语义，无数据库、鉴权或第三方工具前提。
- 新项目目录不得覆盖已有内容；机器启动器创建新项目不读取尚不存在的项目配置。
- 语言和 skill 共用版本发行、完整性校验及更新失败保护；支持一次安装后持续获取活动版本的 skill。
- 应用配置只来自 `config/setting.json`，不用环境变量。用户已指定官方核心仓库 `git@github.com:shemic/dever-main.git` 和独立 skill 仓库 `git@github.com:shemic/dever-main-skills.git`。发行地址是工具内置元数据，禁止写入业务项目配置。
- 定向验证模板 check/fmt/test/run/build、安装/更新后 core 与 skill 一致、损坏输入不破坏旧版本。仅使用自有临时目录/安装镜像，不安装全局服务、不替换全局命令。用户已授权两个指定仓库提交并 push；skill 是独立 Git，核心使用 submodule 固定对应提交。

## 现有缺口与边界

复用 CLI、共享机器版本库、签名发行作者、既有 bootstrap 安装和项目命令分派。当前没有 new；update 仅选择 downloads/latest 的核心，不交付 skill。普通应用配置、语言语义、Go front、其它平台发行、全量 CI 和压测不在本任务范围内。
