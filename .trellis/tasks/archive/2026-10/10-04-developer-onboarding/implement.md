# 实施记录

状态：实现与定向验证完成。核心工作提交 ac458ff，独立 skill 提交 9fd78fc；本任务按用户授权向两个指定仓库发布源码。

1. CLI 新建项目与共享模板。
2. 独立开发 skill、安装/更新引导和语言参考。
3. 发行包携带 skill，机器安装与同步更新集成。
4. 定向测试、真实模板 CLI 验证、失败保护和独立 skill 校验。

实施前现状：机器 update 仅消费本地 downloads/latest，当时尚无 new 和发布 skill；磁盘约213MiB可用，因此本次Cargo验证串行执行。

## 已完成

- CLI `new <new-directory> [--markdown]`、单一模板资源和两种等价源码格式；新目录原子发布，拒绝已有目录/文件/链接，覆盖并发失败保护。
- 独立 dever-language skill，含语言/架构/接口/数据/权限/租户/库/Markdown/安装参考，项目 AI 指引，官方源安装脚本。SKILL 元数据校验通过。
- core 主仓库和 skills 独立仓库采用用户指定 SSH remote；主仓库固定 skill submodule。官方更新地址内置 GitHub Releases，已撤销放入配置文件的早期提议。
- 发行 maker 将完整 skill 加入签名清单；`skill path [project-root]` 按活动/固定版本只读校验；`skill install` 安装永久薄入口，不单独维护可失配的技能版本指针。
- 官方下载先验签，限制源/重定向/大小/归档成员，校验完整包后发布；更新和安装不读取业务配置或环境变量。损坏包保留旧活动版本。
- Python 首装助手复用已有 bootstrap，官方资源制作工具发布三个确定性资源，Linux NOREPLACE 防止覆盖并发目标。测试仅在自有目录，首装脚本的最终 bootstrap 为 mock，未触碰宿主安装/服务。

## 验证

- Rust new_project：4/4；release_source：7/7（含自有 loopback 下载/坏签名/坏内容）；shared_toolchain 定向：3/3（完整版本切换、坏skill、真实降低UID读取）。
- native_release 两项 maker 定向：2/2（缺skill/超限拒绝，确定性完整签名）。共16项Rust定向通过。
- Python安装/资产定向：8/8；包含独立临时密钥签名、拒绝缺skill、归档坏尾/重复/链接/unsigned、镜像配置/清理和发布竞态。
- 独立按skill执行普通/Markdown项目14项new/check/fmt/test/run/build/独立程序命令全部成功；各1个应用测试通过，4次原生执行JSON为code=0、data=Hello, Dever。主代理再次生成Markdown项目并原生run得到相同结果。
- rustfmt已应用；定向clippy对lib、core/launcher、新项目/共享工具链/发行测试通过 -D warnings；skill validator通过。
- 未跑全量CI、压测、真实数据库或全局安装；未发布正式二进制资产，不能将源码push当作发行完成。官方GitHub两个仓库初始为空，core仓库已确认public。

## 本机保留

- 改前源码备份：target/onboarding-before.Vgoy97/source.tar.gz。
- 新官方发行公钥仅公开pub，私钥留在被忽略的本机目录；已验证PKCS#8 v2符合maker并匹配skill公钥，不打印/提交私钥。
- 为有限磁盘验证临时移走的6份旧CLI缓存已归档为 target/onboarding-before.Vgoy97/superseded-cli-cache.tar.gz（约21MiB），可恢复；本次自有临时目录已清理，未删除业务数据。
