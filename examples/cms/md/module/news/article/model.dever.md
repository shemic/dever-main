# 文章模型

草稿位于租户数据库，REST 由可信用户 owner 限定到作者范围。

- 包：`news.article.model`
- 公开类型：
  - `ArticleStatus`
  - `Article`
- 公开方法：无
- 使用：无

## 文章状态

本节定义对应的源码合同。

- 类型：`ArticleStatus`
- 分支：
  - `Draft`：该声明的业务值
  - `Published`：该声明的业务值

```dever
type ArticleStatus {
  Draft = "草稿"
  Published = "已发布"
}
```

## 文章记录

本节定义对应的源码合同。

- 类型：`Article`
- 字段：
  - `private author_id: user.account.model.principal.id`：可信作者标识
  - `slug: Text(1, 180)`：规范化标识
  - `title: Text(1, 160)`：文章标题
  - `body: Text`：正文
  - `category_id: content.category.model.id?`：可选分类引用
  - `asset_id: media.asset.model.id?`：可选媒体引用
  - `private version: Int`：当前编辑版本
  - `private status: ArticleStatus`：业务状态
  - `private published_at: DateTime?`：实际发布时间

```dever
type Article {
  private owner author_id: user.account.model.principal.id = dever.auth.user_id()
  slug: Text(1, 180) unique
  title: Text(1, 160)
  body: Text
  category_id: content.category.model.id?
  asset_id: media.asset.model.id?
  private version: Int default 1
  private status: ArticleStatus default ArticleStatus.Draft index
  private published_at: DateTime?
}
```

## 作者索引

本节定义对应的源码合同。

- 声明：`index(author_id, created_at)`

```dever
index(author_id, created_at)
```

## sql owned_articles

草稿位于租户数据库，REST 由可信用户 owner 限定到作者范围。

- 声明：`sql owned_articles`
- 输入：
  - `author: user.account.model.principal.id`：该声明的业务值
  - `size: Int`：受限页大小
- 输出：
  - `articles: List<app.ArticleView>`：该声明的业务值

```dever
sql owned_articles(author: user.account.model.principal.id, size: Int) (
  articles: List<app.ArticleView>
) {
  sqlite = "SELECT id, slug, title, body, version, category_id, asset_id FROM article WHERE author_id = ?1 ORDER BY id DESC LIMIT ?2"
  postgres = "SELECT id, slug, title, body, version, category_id, asset_id FROM article WHERE author_id = $1 ORDER BY id DESC LIMIT $2"
}
```
