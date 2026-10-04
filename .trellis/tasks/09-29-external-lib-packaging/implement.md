# Implementation Plan

- [x] 扩展 native build graph，按可达 external Adapter 精确收集 locked environment；Adapter 声明 `lib "<ecosystem>:<name>@<version>"`，Program 导出去重后的精确请求，run/build 统一校验同一 lock。
- [x] 定义 target packager 输出和共享 embedded resource manifest。
- [x] 实现资源摘要、确定性排序和构建体积报告；当前 fixture 资源不做额外压缩。
- [x] 在 runtime 实现安全 extraction、复核、atomic publish、并发首次启动和清理。
- [x] 增加锁定依赖的传递闭包和 ArtifactStore 摘要/尺寸验证边界；Worker 启动前会拒绝缺失、环依赖或不完整 artifact。
- [x] 让 run/build 共用 Worker preparation owner，不形成开发/生产两套路径。
- [x] 实现 Python/JavaScript checked manifest、同名操作绑定、依赖/runtime 展开和受控启动；支持 Python 隔离、ESM/CommonJS、可信 SDK 优先加载。
- [x] 实现锁定 build pack 的受管 Go compile/link、目标文件选择、module ZIP、go:embed 和静态操作绑定；独立 Worker 只携带编译产物与合同。
- [ ] 交付正式签名语言 pack；自有 host-target Go pack 不代替六平台无宿主发行验收。
- [x] 把 Dever Package external requests 接入项目 lock resolution、source loading 和 package 更新检查；保留组件级所有权与无本地回退边界。
- [x] 保持 Adapter runtime selection；全部 checked exec 候选打包，只有 selected Worker 启动。
- [ ] 增加无系统 runtime 的隔离执行、资源篡改/中断/并发、Package 传递依赖和 test fake 回归（当前已覆盖资源完整性、路径逃逸和重复发布 fixture）。
- [ ] 更新 LANGUAGE、README、IMPLEMENTATION、toolchain/package/security specs。
- [ ] 运行 run/build/package 定向测试和二进制体积分项检查；不运行全量或性能压测。

## Review Gates

- 不把 `setting.json` 的当前 Adapter 选择编译进产物。
- 不在运行时联网、解析版本或调用系统包管理器。
- 不允许释放目录成为跨应用或跨用户的可写执行共享边界。
- 不改变无 external Adapter 应用的资源、启动和输出合同。

## Current implementation boundary

`dever lib` 已接真实 registry resolver、传递依赖和 daemon 资产缓存；packaging 把支持的锁定资源编译进 native application，入口经 `dever-runtime::external` 做私有 staging、SHA-256 复核和原子发布，`*.external.json` 输出分项体积报告。Python/JS preparation 生成 checked runner/SDK/launch manifest；`run` 与 `build` 共用同一份 lock/preparation 校验。raw pack/archive 在展开后仅为 config/exec 仍需的消费者保留，原生二进制输入按实际摘要去重；不同 Worker 的释放路径和清单仍隔离。

阶段收口补充：external Adapter 在声明体内列出精确 Lib 请求。编译器把请求纳入 external schema identity，CLI 从已检查 Program（含 Package 源码）收集每个 Worker 的请求，与 `config/setting.json.lib` 和 v2 `dever.lock` 精确核对。Dever Package 已接通版本/传递依赖、共享缓存、源码注入和统一 Lib 锁；不能以局部锁测试代替正式语言 pack 的发行验收。

真实 resolver、三 SDK typed manifest、Python/JS 自动 Worker、受管 Go compile/link、Dever Package 集成和 Linux 跨用户 artifact IPC 已实现。daemon build IPC、高级生态安装语义仍缺产品代码；正式签名 runtime pack 和无宿主目标验收未交付。宿主工具制作的自有 pack 定向用例不能等同于正式发行验收。

2026-09-30：实际 exec Worker 随程序嵌入，资源 identity 包含可执行位，启动绑定当前精确清单并重验字节和私有 Unix 权限。`external_resources` 6/6 通过，包括并发原子提取、路径/权限拒绝、部署目录不含源码 Worker、清空环境两次独立运行和篡改拒绝；`native_entry` 15/15、`port_adapter` 15/15、CLI `external_libs` 14/14 通过。这证明 exec 打包，不证明无 Python/Node/Go 环境的生态包执行。

## Verification

2026-10-02当前状态：上方阶段记录中的v2与“daemon编译/高级生态未实现”已过时。当前使用v5 lock/源码构建收据，Linux daemon LLVM编译、签名pack/完整ELF闭包、真实第三方高级依赖和OS隔离已实现。展开输出在所有raw消费者消失后才裁剪。增强三生态签名安装/受管编译/移除源码与机器目录后的无系统语言独立执行 **1/1通过**（755.81秒），包含Python/Node原生依赖，Go第三方UUID另有独立Worker证据。Linux签名首装/双项目/真实双UID共享 **1/1通过**（231.51秒）。公开和其他平台发行仍未完成；精确限制与日志见`09-29-language-production-closure/implement.md`。

以下保留09-29历史检查，不能覆盖上述当前结果：

- `cargo check --offline --workspace`：通过。
- `cargo test --offline -p dever-runtime --features external external::tests`：1/1 通过（含路径逃逸、缓存污染和完整性校验）。
- `cargo test --offline -p dever-cli --lib libs::tests`：4/4 通过。
- `cargo test --offline -p dever-cli --test external_libs`：2/2 通过。
- `cargo test --offline -p dever-tests --test native_entry project_entry_can_embed_verified_external_resources`：1/1 通过。
- `cargo test --offline -p dever-tests --features component --test external_component`：1/1 通过。
- `cargo test --offline -p dever-tests --test api_declarations`：16/16 通过。
- 资源 native 完整编译/启动 smoke 曾尝试，但在当前磁盘/链接器约 120 秒限制内未完成，不能记为通过。
- 未运行全量测试、真实 registry、真实 Python/Node/Go Worker、独立目标机运行和体积/内存压测。
