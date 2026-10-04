# Design

## Build Graph

native specialization 保留所有生产 Adapter successors。构建器从可达 external Adapter 收集 locked environment，按 target 调用 ecosystem packager，得到 `{manifest, executable/runtime files, digests}`。相同 digest 的资源在应用资源表中 intern 一次。

## Embedded Resource Format

应用资源表包含 format/version/target/path/mode/length/SHA-256 和压缩 bytes。entry path 是 manifest 内部标识，不接受部署配置覆盖。生成程序启动时验证 resource table，再按需释放 selected Adapter 的完整 environment。

## Extraction

根固定为可执行文件同级 `data/cache/lib`。每个 digest 使用 `.staging-<pid>-<counter>` 私有目录，写入时拒绝 symlink、父级跳转和重复 path；逐文件 fsync/权限设置/摘要复核后原子 rename。已发布目录必须复核 manifest 和 entry digest，失败即报错，不原地修补。

## Package Resolution

Dever Package 发布内容保留 external declaration 和 requested Lib specs，不携带开发者机器的 lock/cache。应用项目统一解析所有直接与传递请求到自己的 `dever.lock`；每个 Adapter environment 独立，因此版本差异不需要强行全局合并。相同 artifact bytes 仍由机器 cache 和应用资源表去重。

## Output Contract

保持当前单个独立可执行文件合同。外部 runtime 增加体积但不改变命令；`dever build` 输出确定性 size report，避免把 Python/Node runtime 成本误认为 Dever 核心体积。
