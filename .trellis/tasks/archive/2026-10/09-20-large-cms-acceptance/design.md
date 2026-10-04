# CMS 业务闭环设计

## 复用与依赖
保留路径角色、生成入口与核心 RBAC。认证走配置站点/account.verify，权限目录自动生成，不新增身份注入。跨领域只调用 App。
依赖单向：content.revision、publishing.schedule 调用 news.article；news.article 可依赖 content.category、media.asset、audit.operation，不反向调用 revision/schedule。修订拥有编辑编排，文章拥有作者、状态和版本规则。普通 CRUD 保留生成 REST。

## 数据与行为
文章增加可空分类/媒体引用及受保护版本；关联校验复用叶领域 App。修订调用文章受约束更新再保存快照，复用同一租户事务；使用现有条件 update 的 affected rows 判断竞争，不用 count+1 分配版本。
手动与定时发布共用内聚实现；手动重复保持冲突，任务在已完成状态幂等成功。定时创建验证真实作者，保存必要业务引用并使用 enqueue_at。Worker 仍由核心重新验证可信身份和权限，payload 不能成为认证来源。
API/Job 保持薄适配。数据库回滚不等于任意文件回滚；区分 Upload 临时资源与已提交存储文件。

## 测试分层
- 应用源码测试验证 App/Domain 和正常存储结果，用例由运行器隔离；错误、权限和会话失效通过真实 HTTP 验证。Test/Job 不能调用身份 getter，不增加后门。
- 自有 HTTP 验收使用复制的 CMS、临时 SQLite/配置、随机 loopback 端口、真实登录与角色及正常 Worker。
- 复用 test/performance/cms.py 客户端与现有过程 owner；业务验收留在根 test/，不启动压测。
- 等价检查覆盖声明、Model/API/Job/Port 和设置，Markdown 保留真实文档合同。

## 变更边界
主要修改 examples/cms 双源码与测试、根 test/ 相关验收和 examples/README.md；复用编译器/runtime，默认不重建发行包或升级依赖。
改前备份：target/cms-workflows-before.v1FzWO/source.tar.gz。

## 已证实的核心阻塞
- SQL 返回同领域 App View，复用既有行解码；拒绝非存储类型、private 字段和未经解码证明的字段 bounds。仅两个方言均为完整单表简单投影时证明只读，其他 SQL 保持 write 效果。
- 可信用户/租户 ID 可在明确 nullable ModelId 上下文中保持名义类型，API 与 REST 均按当前 Provider 的 verify 字段验证，不把 Int 任意转换成 ID。CMS 直接读取已验证身份，避免租户写入链重读全局 Session；保留跨库事务禁止规则。
