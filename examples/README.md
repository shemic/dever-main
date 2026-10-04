# Dever 示例

`examples/` 只保留两个入口：`cms/` 是当前维护的应用示例，`old/` 是旧语言合同和回归材料的归档。旧目录不符合当前 component/domain/role 应用布局，不应直接执行 `dever check/run/build`。

CMS 同时维护 [Dever 源码](cms/dever/module/news/article/api/admin/manage.dever)和 [Markdown 源码](cms/md/module/news/article/api/admin/manage.dever.md)。两套实现使用同一目录与业务合同：

```text
module/
  platform/tenant/          平台租户
  user/account/             凭据、登录、bootstrap CMD
  user/membership/          租户成员
  user/session/             登录会话
  user/authorization/       核心角色授权的业务入口
  content/category/         分类
  content/revision/         文章编辑与修订快照
  media/asset/              上传、Storage Port/Adapter
  publishing/schedule/      定时发布与 Job
  audit/operation/          操作审计
  system/health/            健康 CMD
  news/article/
    app/editing.dever
    app/publication.dever
    api/admin/manage.dever
    api/front/browse.dever
    domain.dever
    model.dever
    model/publication.dever
test/
  content/category/lifecycle.dever
  news/article/publish.dever
  user/account/bootstrap.dever
  user/account/validation.dever
  user/session/lifecycle.dever
```

Markdown 项目使用相同的目录和 API/CMD 声明，仅把源码扩展名换成 `.dever.md` 并补齐文档合同。

编译器从 API/CMD/Job 声明生成入口。平台租户、账号、成员与会话是 `global type`，保存在共享平台库；文章、分类、修订、媒体、排期和审计属于当前租户物理库。认证 verify 只读取 global Model 并建立只读用户与租户身份；接口权限由核心根据当前站点和角色自动校验，后续 API 才进入可信 tenant scope。一个职责文件可包含一组内聚方法，没有按函数拆文件。

文章通过 `/news/article/admin/manage/create` 创建，`list/detail` 查询，`remove` 删除草稿，`publish` 发布。编辑调用 `/content/revision/admin/manage/create`，传入 `selected_article_id`、`expected_version`、标题正文和可空分类/媒体 ID；版本冲突返回 409，文章更新和修订快照在同一事务内完成。文章不声明绕过这些规则的自动 REST 写接口。列表 SQL 直接返回同领域 `app.ArticleView`，不暴露私有 Model。

`/publishing/schedule/admin/manage/schedule` 接收文章 ID 和 `publish_at`；创建时验证作者与草稿状态，按真实到期时间入队。每个排期有独立任务键，同刻文章不会合并。任务执行仍由核心复验身份和权限；发布、审计和排期状态一并提交或回滚，完成任务重放不会重复写入。手动重复发布返回 409。

在仓库根目录执行：

```bash
dever check examples/cms/dever
dever check examples/cms/md
dever test examples/cms/dever
dever test examples/cms/md
dever fmt examples/cms/dever --check
dever fmt examples/cms/md --check
dever run examples/cms/dever -- user.account.bootstrap '{"tenant_key":"tenant-one","tenant_name":"Tenant One","email":"owner@example.com","display_name":"Owner","password":"change-me-now"}'
dever tenant migrate examples/cms/dever 1
dever tenant owner examples/cms/dever 1 admin 1
dever tenant owner examples/cms/dever 1 front 1
```

`run/build` 只读取各项目的 `config/setting.json`，运行数据写入各自 `data/`。先用 bootstrap CMD 建立平台租户、业务 Owner membership 与 Argon2 密码，再用返回的真实 `tenant_id` 执行 `tenant migrate`，并用返回的 `tenant_id/user_id` 分别 provision admin/front 核心 RBAC Owner；示例中的 `1` 只是首次空库的常见结果，不能在生产脚本里硬编码。两个站点各自通过同站点的 `user/authorization/<site>/manage` 接口管理普通角色。不带 CMD 参数时启动 admin/front HTTP API。`test` 不读取部署配置；每个涉及 Model 的测试使用独立临时 SQLite，并在结束后清理。

CMS 的五个应用测试验证分类、发布规则、账号引导/规范化和会话生命周期的正常业务输出，位于各项目根 `test/`；拒绝、过期等负向行为，以及带真实登录的 HTTP、并发编辑、上传、Job 重试/回滚验收见 [业务验收说明](../test/cms/README.md)。普通测试不伪造认证上下文。编译器和 CLI 的 Rust 回归测试仍统一位于仓库根 `test/`，两者都不混入业务 `module/`。部署前必须替换示例 JWT secret，并把两个站点的 `origin` 和 Host 改成真实 HTTPS 入口；`origin` 描述反向代理外部地址，不是 Dever 内部监听地址。

旧示例仍可供编译器维护者查阅历史语义，但不能作为新 Dever 应用的目录、可见性或调用方式范本。
