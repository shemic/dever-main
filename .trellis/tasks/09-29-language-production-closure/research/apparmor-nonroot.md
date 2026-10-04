# 受限 AppArmor 非 root Worker

2026-10-02，只读安全调查；未加载策略、改 sysctl 或安装全局程序。

## 已证实根因

旧失败 PID 4087980/4087981 的内核 audit：实际首个 exec 是私有 `ld-linux-x86-64`，创建 user namespace 后进入默认 `unprivileged_userns`，拒绝 capability 8/setpcap 与 12/net_admin，对应 `RTM_NEWADDR: Operation not permitted`。当前 `dever-sandbox/src/lib.rs` 把 bwrap 当 loader 参数，给 bwrap 路径添加 attachment 不会生效。

宿主 `apparmor_restrict_unprivileged_userns=1`、`apparmor_restrict_unprivileged_unconfined=0`，没有 Dever/bwrap profile；parser 4.0.1、ABI 4.0 可用。这些只证明环境条件，尚未证明专用 profile 兼容或非 root 验收通过。

## 最小实现

1. 签名 bwrap 采用静态 ELF（无 PT_INTERP/DT_NEEDED），直接 exec；内部 guard/语言继续使用私有 loader/libs。避免给动态 loader 授予 userns，也防止宿主 preload 先于可信入口运行。
2. 受限机器 helper 固定 `/opt/dever/sandbox/<bwrap_sha256>/bwrap`，复用现有发行签名安装 owner。整条目录链 root-owned，不可组/其他用户写，无符号链接。普通 0755，不加 setuid/filecaps。独立应用只选择与嵌入签名资产摘要相同者，旧版本并存，无 PATH/env/系统 bwrap 回退。
3. 专用 AppArmor ABI 4.0 profile 复用上游 bwrap 的 namespace 设置与 exec 子程序 deny-capabilities 堆叠，attachment 只覆盖受信目录。禁止简化为宽泛 unconfined+userns。现有 FD pin、只读挂载、grant、seccomp、后代回收保留。
4. 作者机缺 libcap.a，需制作固定来源静态 bwrap 资产；不能要求客户机安装编译依赖。规则文件可打包/离线验证，宿主加载须管理员明确确认。

## 验收

### 安全复核补充：专用 profile 的宿主前提

本机 `apparmor_restrict_unprivileged_unconfined=0` 是部署阻塞：Linux 对当前 unconfined 源直接允许显式 change_profile/change_onexec，目标的路径 attachment 不能拒绝这次进入。攻击程序可直接进入允许 capability/userns 的父 profile，尚未执行下一次 exec 时不会叠加 deny-capabilities 子 profile。不能加载当前规则后再补救，也不能仅在目标规则加 deny change_profile。

Ubuntu 官方要求管理员启用 `kernel.apparmor_restrict_unprivileged_unconfined=1`。产品非 root 受限启动必须只读检查此前提，实际 `/` 安装在发布 `/etc/apparmor.d` 文件前也必须检查（系统 reload 可能自动加载）。离线镜像可制作，但不是宿主验收。产品不自行改全局设置；主代理完成可审查代码后单独请求临时加强保护及加载新规则的验证授权，其他CI继续。回滚时先卸载本轮 profile，再恢复原严格项，避免留下可进入的宽松窗口。

依据：Linux `security/apparmor/domain.c` 的 `change_profile_perms`、`profile_onexec`；https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007 的 profile changes 节。此结论替代“只要静态 helper+专用规则就足够”的初始假设。

- 限制仍为 1 的非零 UID 运行真实三生态 Worker；audit 不再进入默认 unprivileged_userns。
- 源码/编译服务移除后独立程序仍可运行，只需匹配的受信 helper/profile 和系统 ABI。
- 默认拒绝未授权文件/网络/fork，声明 grant 各自生效；禁止 namespace/ptrace/capability；取消/超时不留后代。
- NNP 堆叠及直接 aa-exec 进入规则不能把 capability 带入任意程序。此项必须真实加载后验证，不能凭上游 profile 推断。
- 缺 helper/hash 错/可写祖先/symlink/profile 缺失失败关闭；多 UID/多版本互不污染；安装恢复保留既有版本。

## 上游依据

- https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007
- https://raw.githubusercontent.com/containers/bubblewrap/main/meson.build
- https://gitlab.com/apparmor/apparmor/-/raw/apparmor-4.0/profiles/apparmor/profiles/extras/bwrap-userns-restrict

文件 owners：`dever-sandbox/src/assets.rs` 与 `lib.rs`；CLI `toolchain/packaging/sandbox.rs` 与 `bootstrap{,/transaction,/service}.rs`；SDK 作者输入说明；根 `test/dever-sandbox-tests`、`native_release`。不新增自写 namespace helper、通用代理或新签名协议。
