# 文章公开读模型

发布动作把可公开内容写入无私密字段的租户内读模型。

- 包：`news.article.model.publication`
- 公开类型：
  - `Publication`
- 公开方法：无
- 使用：无

## 发布记录

本节定义对应的源码合同。

- 类型：`Publication`
- 字段：
  - `slug: Text(1, 180)`：文章标识
  - `title: Text(1, 160)`：标题
  - `body: Text`：正文
  - `published_at: DateTime`：发布时间

```dever
type Publication {
  slug: Text(1, 180) unique
  title: Text(1, 160)
  body: Text
  published_at: DateTime index
}
```
