# 发布计划模型

发布计划保存文章的计划时间和幂等状态。

- 包：`publishing.schedule.model`
- 公开类型：
  - `ScheduleStatus`
  - `Schedule`
- 公开方法：无
- 使用：无

## 计划状态

计划状态枚举。

- 类型：`ScheduleStatus`
- 分支：
  - `Pending`：该声明的业务值
  - `Completed`：该声明的业务值
  - `Cancelled`：该声明的业务值

```dever
type ScheduleStatus {
  Pending = "待发布"
  Completed = "已完成"
  Cancelled = "已取消"
}
```

## 发布计划

计划关联文章和执行时间。

- 类型：`Schedule`
- 字段：
  - `uuid: Uuid`：计划独立去重标识
  - `article_id: news.article.model.id`：文章标识
  - `publish_at: DateTime`：计划执行时间
  - `status: ScheduleStatus`：业务状态

```dever
type Schedule {
  uuid: Uuid generated unique
  article_id: news.article.model.id unique
  publish_at: DateTime index
  status: ScheduleStatus default ScheduleStatus.Pending index
}
```
