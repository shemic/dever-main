# 发布计划应用

发布计划应用在事务中写入计划并提交持久化 Job。

- 包：`publishing.schedule.app`
- 公开类型：
  - `PublishRequest`
  - `ScheduleView`
- 公开方法：
  - `create`
  - `publish`
  - `complete`
  - `publish_claimed`
- 使用：
  - `publishing.schedule.create(article_id, publish_at)`
  - `publishing.schedule.publish(input)`
  - `publishing.schedule.complete(status, schedule_id)`
  - `publishing.schedule.publish_claimed(changed, article_id)`

## 发布请求

Job 只携带计划 ID。

- 类型：`PublishRequest`
- 字段：
  - `schedule_id: model.id`：持久计划标识

```dever
type PublishRequest {
  schedule_id: model.id
}
```

## 计划视图

返回计划的公开字段。

- 类型：`ScheduleView`
- 字段：
  - `id: model.id`：记录标识
  - `article_id: news.article.model.id`：文章标识
  - `publish_at: DateTime`：计划执行时间
  - `status: Text`：业务状态

```dever
type ScheduleView {
  id: model.id
  article_id: news.article.model.id
  publish_at: DateTime
  status: Text
}
```

## 创建计划

业务记录、Job 和返回视图在同一事务内完成。

- 函数：`create`
- 输入：
  - `article_id: news.article.model.id`：文章标识
  - `publish_at: DateTime`：计划执行时间
- 输出：
  - `schedule: ScheduleView`：该声明的业务值

```dever
transaction create(article_id: news.article.model.id, publish_at: DateTime) (
  schedule: ScheduleView
) {
  news.article.require_schedulable(article_id)
  stored = model.create(
    {
      article_id = article_id
      publish_at = publish_at
    }
  )
  queued = dever.job.enqueue_at(
    job.publish,
    PublishRequest {
      schedule_id = stored.id
    },
    uuid.to_text(stored.uuid),
    publish_at
  )
  discarded = queued
  schedule = ScheduleView {
    id = stored.id
    article_id = stored.article_id
    publish_at = stored.publish_at
    status = "pending"
  }
}
```

## 执行计划

Worker 首先检查计划状态，再调用文章领域的定时发布入口。

- 函数：`publish`
- 输入：
  - `input: PublishRequest`：经过类型检查的输入
- 输出：无

```dever
transaction publish(input: PublishRequest) () {
  scheduled = model.get(input.schedule_id)
  complete(scheduled.status, scheduled.id)
}
```

## complete

发布计划应用在事务中写入计划并提交持久化 Job。

- 函数：`complete`
- 输入：
  - `status: publishing.schedule.model.ScheduleStatus`：业务状态
  - `schedule_id: publishing.schedule.model.id`：持久计划标识
- 输出：无

```dever
complete(status: model.ScheduleStatus.Completed, schedule_id: model.id) () {}

complete(status: model.ScheduleStatus.Cancelled, schedule_id: model.id) () {}

complete(status: model.ScheduleStatus.Pending, schedule_id: model.id) () {
  scheduled = model.get(schedule_id)
  domain.require_due(
    model.exists(
      {
        where = id == schedule_id and publish_at <= dever.time.now()
      }
    )
  )
  changed = model.update(
    {
      where = id == scheduled.id and status == model.ScheduleStatus.Pending
    },
    {
      status = model.ScheduleStatus.Completed
    }
  )
  publish_claimed(changed, scheduled.article_id)
}
```

## publish_claimed

发布计划应用在事务中写入计划并提交持久化 Job。

- 函数：`publish_claimed`
- 输入：
  - `changed: Int`：条件更新影响行数
  - `article_id: news.article.model.id`：文章标识
- 输出：无

```dever
publish_claimed(changed: 0, article_id: news.article.model.id) () {}

publish_claimed(changed: 1, article_id: news.article.model.id) () {
  publication = news.article.publish_scheduled(article_id)
}

publish_claimed(changed: other, article_id: news.article.model.id) () {
  fail(dever.api.Error.Conflict)
}
```
