# Design

## Boundary

本任务增加机器级 Dever 管理层。它拥有稳定入口、版本目录、项目版本分派、更新事务和共享缓存服务；版本化编译器继续拥有源码检查、生成和运行语义。项目仍通过现有 CLI 合同进入编译器。

## Directory Contract

发行安装器按平台选择机器级根目录，但根内布局一致：

```text
<toolchain>/
  bin/
    dever                 stable launcher
    deverd                cache/build service
  state/
    active-version
    install.lock
  versions/
    <version>/
      dever-core
      manifest.json
      runtime/
  cache/
    native/
    downloads/
    staging/
```

`bin/dever` 是机器上唯一公开入口。`versions/*`、`state/*` 与缓存目录不加入 PATH，也不要求用户直接操作。目录根由安装器写入 launcher 自身的安装位置，运行时不读取环境变量。

## Command Flow

1. launcher 解析命令和项目根，不读取 Dever 源码。
2. 对项目命令只解析 `config/setting.json` 的 `dever.version`；存在时精确选择，否则读取机器 `active-version`。
3. launcher 校验版本 manifest 和核心可执行文件身份，然后把原参数分派给版本核心。
4. 核心完成现有 check/run/build 流程；需要原生产物时通过本机受保护 IPC 请求 `deverd`。
5. `deverd` 根据已安装版本和完整构建输入自行计算缓存键、编译或恢复产物，再将结果返回请求用户，客户端不能指定缓存文件路径。

`config/setting.json` 增加可选配置：

```json
{
  "dever": {
    "version": "0.1.0"
  }
}
```

runtime 配置解析器接受并忽略已经由 launcher 消费的 `dever` 段，确保同一配置可以同时用于 `run` 和独立构建产物；构建产物不使用该字段选择自身 runtime。

## Ownership

- `dever-cli`：稳定 launcher 命令合同、项目版本读取和版本核心分派。
- 独立 machine manager 模块：安装、更新、切换、卸载、签名验证和原子状态变更；不把这些分散进每个命令分支。
- `dever-runtime::config`：只增加严格的 `dever.version` 配置合同，不承担安装或版本选择。
- `dever-core::native::build`：把默认缓存请求交给共享服务；保留显式隔离缓存入口供测试使用。
- cache service：唯一可写共享缓存的进程，负责密码学键、编译、恢复、并发发布、租约和清理。
- 平台 adapter：只封装全局路径、权限提升、服务管理和 IPC 差异；上层状态机和版本规则只有一套。

## Install And Update Transaction

下载先进入 `cache/downloads`，验证发行 manifest 签名、包摘要、平台和版本后解包到 `cache/staging`。核心自检成功后原子发布到 `versions/<version>`，最后原子更新 `active-version`。任一步失败都删除 staging 并保留旧版本。卸载同样持有 `install.lock`，不能删除当前版本或正在服务请求的版本。

初次安装由签名的系统安装器建立全局入口、机器服务、权限和初始版本；之后统一由 `dever install/update/use/uninstall` 管理。需要管理员权限时使用平台标准提权流程，不保存凭据。

## Cache Security And Cleanup

- `deverd` 使用专用系统身份，缓存目录只允许该身份写入。
- 本地 IPC 使用操作系统凭据识别调用者；请求只包含规范化构建输入，不接受任意读写路径。
- 缓存键使用 SHA-256 等密码学摘要；命中前复核 manifest 与产物摘要。
- 客户端只有提交完整相同输入才能取得同一产物，服务不提供缓存目录列表或其他项目索引。
- 运行中的条目持有租约。清理只删除无租约的 staging、损坏条目和按策略选出的旧条目，并使用原子删除/替换。
- `cache status` 输出容量、条目数和版本占用，不输出项目源码、路径或用户信息。

## Failure Contract

全局入口、活动版本状态、项目锁定、签名、版本 manifest、IPC 或缓存完整性任一验证失败时返回明确错误。不得静默切换版本、使用用户 PATH 中的编译器、改写项目配置或执行未验证产物。

## Compatibility

开发态 `deverc` 保持为私有仓库入口直到正式发行链完成。正式 `dever` 不读取开发仓库 `target/native-artifacts`。已有项目无需新增配置；只有需要固定版本的项目才增加 `dever.version`。

## Deferred Complexity

本任务不重复实现父发行任务的原生后端、目标包生产和发布基础设施。安装管理层必须以父任务提供的签名发行包为输入；在发行包合同完成前可以实现和测试本地 fixture，但不能宣称公开安装链已完成。
