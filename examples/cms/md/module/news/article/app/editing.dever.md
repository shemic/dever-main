# news.article.app.editing

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 包：`news.article.app.editing`
- 公开类型：
  - `ArticleView`
  - `ArticlePage`
- 公开方法：
  - `create`
  - `list`
  - `detail`
  - `remove`
  - `edit`
  - `require_schedulable`
- 使用：
  - `news.article.create(slug, title, body, category_id, asset_id)`
  - `news.article.list(size)`
  - `news.article.detail(id)`
  - `news.article.remove(id)`
  - `news.article.edit(id, expected_version, title, body, category_id, asset_id)`
  - `news.article.require_schedulable(article_id)`

## ArticleView

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 类型：`ArticleView`
- 字段：
  - `id: model.id`：记录标识
  - `slug: Text`：规范化标识
  - `title: Text`：文章标题
  - `body: Text`：正文
  - `version: Int`：当前编辑版本
  - `category_id: content.category.model.id?`：可选分类引用
  - `asset_id: media.asset.model.id?`：可选媒体引用

```dever
type ArticleView {
  id: model.id
  slug: Text
  title: Text
  body: Text
  version: Int
  category_id: content.category.model.id?
  asset_id: media.asset.model.id?
}
```

## ArticlePage

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 类型：`ArticlePage`
- 字段：
  - `rows: List<ArticleView>`：页面记录
  - `total: Int`：所有权范围内总数

```dever
type ArticlePage {
  rows: List<ArticleView>
  total: Int
}
```

## create

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 函数：`create`
- 输入：
  - `slug: Text`：规范化标识
  - `title: Text`：文章标题
  - `body: Text`：正文
  - `category_id: content.category.model.id?`：可选分类引用
  - `asset_id: media.asset.model.id?`：可选媒体引用
- 输出：
  - `article: ArticleView`：该声明的业务值

```dever
create(
  slug: Text,
  title: Text,
  body: Text,
  category_id: content.category.model.id?,
  asset_id: media.asset.model.id?
) (article: ArticleView) {
  normalized_slug = domain.normalize_slug(slug)
  normalized_title = domain.normalize_title(title)
  domain.require_content(normalized_slug, normalized_title, body)
  content.category.require_active(category_id)
  media.asset.require_ready(asset_id)
  stored = model.create(
    {
      author_id = user.account.current_user()
      slug = normalized_slug
      title = normalized_title
      body = body
      category_id = category_id
      asset_id = asset_id
    }
  )
  article = detail(stored.id)
}
```

## list

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 函数：`list`
- 输入：
  - `size: Int`：受限页大小
- 输出：
  - `articles: ArticlePage`：该声明的业务值

```dever
list(size: Int) (articles: ArticlePage) {
  author = user.account.current_user()
  articles = ArticlePage {
    rows = model.owned_articles(author, domain.page_size(size))
    total = model.count(
      {
        where = author_id == author
      }
    )
  }
}
```

## detail

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 函数：`detail`
- 输入：
  - `id: news.article.model.id`：记录标识
- 输出：
  - `article: ArticleView`：该声明的业务值

```dever
detail(id: model.id) (article: ArticleView) {
  stored = model.get(id)
  domain.require_owner(dever.auth.owns_user(stored.author_id))
  article = ArticleView {
    id = stored.id
    slug = stored.slug
    title = stored.title
    body = stored.body
    version = stored.version
    category_id = stored.category_id
    asset_id = stored.asset_id
  }
}
```

## remove

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 函数：`remove`
- 输入：
  - `id: news.article.model.id`：记录标识
- 输出：
  - `removed: Int`：该声明的业务值

```dever
remove(id: model.id) (removed: Int) {
  stored = model.get(id)
  domain.require_owner(dever.auth.owns_user(stored.author_id))
  domain.require_draft(stored.status == model.ArticleStatus.Draft)
  removed = model.delete(
    {
      where = id == stored.id and version == stored.version
    }
  )
  domain.require_updated(removed)
}
```

## edit

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 函数：`edit`
- 输入：
  - `id: news.article.model.id`：记录标识
  - `expected_version: Int`：调用者读取的编辑版本
  - `title: Text`：文章标题
  - `body: Text`：正文
  - `category_id: content.category.model.id?`：可选分类引用
  - `asset_id: media.asset.model.id?`：可选媒体引用
- 输出：
  - `article: ArticleView`：该声明的业务值

```dever
edit(
  id: model.id,
  expected_version: Int,
  title: Text,
  body: Text,
  category_id: content.category.model.id?,
  asset_id: media.asset.model.id?
) (article: ArticleView) {
  stored = model.get(id)
  domain.require_owner(dever.auth.owns_user(stored.author_id))
  domain.require_draft(stored.status == model.ArticleStatus.Draft)
  normalized_title = domain.normalize_title(title)
  domain.require_content(stored.slug, normalized_title, body)
  content.category.require_active(category_id)
  media.asset.require_ready(asset_id)
  changed = model.update(
    {
      where = id == stored.id and version == expected_version and status == model.ArticleStatus.Draft
    },
    {
      title = normalized_title
      body = body
      category_id = category_id
      asset_id = asset_id
      version = expected_version + 1
    }
  )
  domain.require_updated(changed)
  article = detail(stored.id)
}
```

## require_schedulable

文章创建、所有权查询与带版本的编辑；所有写入校验分类和媒体引用。

- 函数：`require_schedulable`
- 输入：
  - `article_id: news.article.model.id`：文章标识
- 输出：无

```dever
require_schedulable(article_id: model.id) () {
  stored = model.get(article_id)
  domain.require_owner(dever.auth.owns_user(stored.author_id))
  domain.require_draft(stored.status == model.ArticleStatus.Draft)
}
```
