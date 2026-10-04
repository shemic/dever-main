# 内容修订应用

修订应用创建文章历史版本并返回公开视图。

- 包：`content.revision.app`
- 公开类型：
  - `RevisionView`
- 公开方法：
  - `create`
- 使用：
  - `content.revision.create(selected_article_id, expected_version, title, body, category_id, asset_id)`

## 修订视图

不暴露存储实现细节的修订结果。

- 类型：`RevisionView`
- 字段：
  - `id: model.id`：记录标识
  - `article_id: news.article.model.id`：文章标识
  - `version: Int`：当前编辑版本
  - `title: Text`：文章标题
  - `body: Text`：正文
  - `status: Text`：业务状态

```dever
type RevisionView {
  id: model.id
  article_id: news.article.model.id
  version: Int
  title: Text
  body: Text
  status: Text
}
```

## 创建修订

按文章已有版本数生成下一个版本。

- 函数：`create`
- 输入：
  - `selected_article_id: news.article.model.id`：待编辑文章标识
  - `expected_version: Int`：调用者读取的编辑版本
  - `title: Text`：文章标题
  - `body: Text`：正文
  - `category_id: content.category.model.id?`：可选分类引用
  - `asset_id: media.asset.model.id?`：可选媒体引用
- 输出：
  - `revision: RevisionView`：该声明的业务值

```dever
transaction create(
  selected_article_id: news.article.model.id,
  expected_version: Int,
  title: Text,
  body: Text,
  category_id: content.category.model.id?,
  asset_id: media.asset.model.id?
) (revision: RevisionView) {
  edited = news.article.edit(
    selected_article_id,
    expected_version,
    title,
    body,
    category_id,
    asset_id
  )
  stored = model.create(
    {
      article_id = selected_article_id
      version = edited.version
      title = edited.title
      body = edited.body
      category_id = edited.category_id
      asset_id = edited.asset_id
      status = model.RevisionStatus.Draft
    }
  )
  revision = RevisionView {
    id = stored.id
    article_id = stored.article_id
    version = stored.version
    title = stored.title
    body = stored.body
    status = "draft"
  }
}
```
