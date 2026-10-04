# CMS 业务验收

显式运行，使用随机 loopback 端口、真实登录及核心角色授权、临时 SQLite 和正常 Job Worker；不访问部署数据库，不启动压测。

```sh
python3 test/cms/acceptance.py --compiler /absolute/path/to/dever --output /new/report-directory
```

运行器复制 plain CMS，并只在副本追加已有 `test/dever-tests/fixtures/postgres_api` 的真实账号/成员建立 CMD。输出目录保存无配置/凭据的 `cms-app`，可用 `--executable /absolute/path/to/cms-app` 跳过后续重复构建；该程序必须包含同一个 `fixture_member` CMD，`--project` 提供匹配的源项目配置。输入源码、二进制和部署配置均不修改。临时密钥、密码和数据库随工作目录清理。

覆盖分类、上传与文件清理、关联文章、修订快照、过期/并发版本冲突、作者和角色权限、发布快照、定时/同刻任务、重复执行、失败回滚/重试、排队后撤权和会话撤销/到期。仅在自有 SQLite 加临时拒绝 INSERT 的 trigger 注入晚期故障；重放只重置已完成队列行的执行状态，保留原身份、租户、权限、schema 与 payload；会话到期通过把自有平台库的 admin 会话截止时间设为过去验证，不伪造身份。所有普通业务和授权经真实 API/CMD。不会将数据库回滚等同于已提交存储文件回滚。

每个 HTTP 请求、构建和进程均有时限；退出时停止并回收自有进程。此验收是有界业务正确性检查，不代表 PostgreSQL、浏览器 HTTPS、长期负载或性能测试。双源码等价由现有编译器 CMS 合同检查负责，不在此复制 Markdown 解析器。
