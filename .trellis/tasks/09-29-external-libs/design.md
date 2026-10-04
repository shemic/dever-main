# Design

## Architecture

```text
App -> typed Port -> selected Adapter -> Worker supervisor -> Component Protocol -> Lib
```

Port 保持现有 compiler-owned Function ID 和闭合实现集合。外部 Adapter 是新的实现种类，不是新业务调用层；checker 从 Port 合同合成 Worker operation schema，并把显式 capability 纳入既有 effect/dependency 分析。

## Source Contract

建议外部 Adapter 保持现有 topic identity：

```text
module/ai/embedding/port.dever
module/ai/embedding/adapter/python.dever
module/ai/embedding/adapter/python/main.py
```

```dever
setting {
  model: Text
}

external pip "python/main.py" {
  lib "sentence-transformers@3.3.1"
  allow gpu
}
```

一个外部 Adapter 文件恰好有一个 `external` 声明，可选现有 typed `setting`，没有 Dever operation body/helper。它由编译器合成该 Port 的全部实现，Worker handshake 必须提供全部 operation。入口必须是所属 Adapter 目录内的规范化相对路径。

## Component Protocol

传输使用专用双向管道和四字节大端长度分帧 JSON，复用现有 wire codec 的 16 MiB、64 层、65536 值预算。stderr 只承载日志，不能进入协议。

消息固定为：`hello/ready/call/result/error/cancel/health/shutdown`。每次握手包含协议版本、Port identity、schema hash、Adapter identity 和 capability 集；不匹配立即结束 Worker。请求 ID 在单 Worker 生命周期内唯一，输出必须属于当前未完成请求。

## Worker Ownership

每个选中外部 Adapter 启动一个长驻 Worker 和有界请求队列。supervisor 属于现有应用 Scope；入口启动完成后才允许用户代码，关闭时停止 admission、取消未完成请求、发送 shutdown 并在期限内 join/kill。崩溃使当前请求 fault；后续调用可以受限重启，但绝不重放已发送请求。

## Lib Resolution

生态 resolver 共用请求/锁模型：requested spec -> exact artifacts -> content hashes -> runtime pack -> target set。resolver 只能在显式 `lib add/update` 联网，结果写 canonical `dever.lock` 并原子替换。项目命令只验证 lock 与缓存，不重新解析版本。

机器级 cache 和 runtime pack 消费 `09-28-shared-toolchain` 的全局工具链合同；在该合同完成前，生态子任务只使用测试自有 fixture store，不新增临时用户缓存协议。

## Build Packaging

每个 Adapter 生成目标专用 Worker artifact。`build` 将所有运行时可选 Adapter、runtime pack、lock manifest 和摘要作为只读资源嵌入应用。首次使用时写入 `data/cache/lib/<digest>` 的私有 staging，校验后原子发布；主程序只执行匹配 manifest 的文件。

## Extension Boundary

新增生态只实现四个固定面：spec parser、resolver、runtime/build packager、SDK。它不能扩展语法、Port 协议、业务类型系统、setting 来源或进程生命周期。

## Dependencies

- 通用协议子任务可以先实现和验证。
- 生态解析依赖机器级共享工具链目录/缓存合同。
- 生产自包含打包依赖父发行任务提供可分发 runtime/target pack；本地 fixture 可以先验证封装格式。
