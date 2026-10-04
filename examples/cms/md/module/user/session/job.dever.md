# 会话清理任务

周期任务清理撤销或过期会话。

- 包：`user.session.job`
- 公开类型：无
- 公开方法：无
- 使用：无

## 队列连接

使用默认数据库连接。

- 声明：`database default`

```dever
database default
```

## 清理任务

任务委托给会话应用。

- 函数：`cleanup`
- 输入：无
- 输出：无

```dever
job cleanup() () retry(3) timeout(30000) {
  app.cleanup()
}
```

## 每小时执行

按 UTC 每小时清理。

- 声明：`schedule cleanup`

```dever
schedule cleanup = "0 * * * *"
```
