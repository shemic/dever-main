# 内容修订模型

修订记录保存文章历史内容和版本。

- 包：`content.revision.model`
- 公开类型：
  - `RevisionStatus`
  - `Revision`
- 公开方法：无
- 使用：无

## 修订状态

修订状态枚举。

- 类型：`RevisionStatus`
- 分支：
  - `Draft`：该声明的业务值
  - `Published`：该声明的业务值

```dever
type RevisionStatus {
  Draft = "草稿"
  Published = "已发布"
}
```

## 修订记录

文章的一个历史版本。

- 类型：`Revision`
- 字段：
  - `article_id: news.article.model.id`：文章标识
  - `version: Int`：当前编辑版本
  - `title: Text(1, 160)`：文章标题
  - `body: Text`：正文
  - `category_id: content.category.model.id?`：可选分类引用
  - `asset_id: media.asset.model.id?`：可选媒体引用
  - `status: RevisionStatus`：业务状态

```dever
type Revision {
  article_id: news.article.model.id index
  version: Int
  title: Text(1, 160)
  body: Text
  category_id: content.category.model.id?
  asset_id: media.asset.model.id?
  status: RevisionStatus default RevisionStatus.Draft index
}
```

## 版本唯一约束

同一文章的版本号唯一。

- 声明：`unique(article_id, version)`

```dever
unique(article_id, version)
```
