# 发布计划任务

发布计划任务由持久化 Job 执行。

- 包：`publishing.schedule.job`
- 公开类型：无
- 公开方法：无
- 使用：无

## 队列连接

使用默认数据库连接。

- 声明：`database default`

```dever
database default
```

## 发布任务

任务委托给发布计划应用。

- 函数：`publish`
- 输入：
  - `input: publishing.schedule.PublishRequest`：发布请求
- 输出：无

```dever
job publish(input: app.PublishRequest) () retry(3) timeout(30000) {
  app.publish(input)
}
```
