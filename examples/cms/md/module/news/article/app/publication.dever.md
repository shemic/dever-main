# news.article.app.publication

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 包：`news.article.app.publication`
- 公开类型：无
- 公开方法：
  - `publish`
  - `published`
  - `published_detail`
  - `publish_scheduled`
  - `scheduled_publication`
  - `publish_draft`
  - `existing_publication`
- 使用：
  - `news.article.publish(article_id)`
  - `news.article.published(size)`
  - `news.article.published_detail(id)`
  - `news.article.publish_scheduled(article_id)`
  - `news.article.scheduled_publication(status, article_id)`
  - `news.article.publish_draft(article_id)`
  - `news.article.existing_publication(stored)`

## publish

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`publish`
- 输入：
  - `article_id: news.article.model.id`：文章标识
- 输出：
  - `publication: news.article.model.publication.Publication`：该声明的业务值

```dever
transaction publish(article_id: model.id) (publication: model.publication.Publication) {
  stored = model.get(article_id)
  domain.require_owner(dever.auth.owns_user(stored.author_id))
  domain.require_draft(stored.status == model.ArticleStatus.Draft)
  publication = publish_draft(article_id)
}
```

## published

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`published`
- 输入：
  - `size: Int`：受限页大小
- 输出：
  - `articles: news.article.model.publication.Page`：该声明的业务值

```dever
published(size: Int) (articles: model.publication.Page) {
  articles = model.publication.list(
    {
      order = [published_at.desc, id.desc]
      size = domain.page_size(size)
    }
  )
}
```

## published_detail

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`published_detail`
- 输入：
  - `id: news.article.model.publication.id`：记录标识
- 输出：
  - `article: news.article.model.publication.Publication`：该声明的业务值

```dever
published_detail(id: model.publication.id) (article: model.publication.Publication) {
  article = model.publication.get(id)
  domain.require_published(text.trim(article.body) != "")
}
```

## publish_scheduled

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`publish_scheduled`
- 输入：
  - `article_id: news.article.model.id`：文章标识
- 输出：
  - `publication: news.article.model.publication.Publication`：该声明的业务值

```dever
transaction publish_scheduled(article_id: model.id) (
  publication: model.publication.Publication
) {
  stored = model.get(article_id)
  publication = scheduled_publication(stored.status, article_id)
}
```

## scheduled_publication

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`scheduled_publication`
- 输入：
  - `status: news.article.model.ArticleStatus`：业务状态
  - `article_id: news.article.model.id`：文章标识
- 输出：
  - `publication: news.article.model.publication.Publication`：该声明的业务值

```dever
scheduled_publication(status: model.ArticleStatus.Published, article_id: model.id) (
  publication: model.publication.Publication
) {
  stored = model.get(article_id)
  publication = existing_publication(
    model.publication.first(
      {
        where = slug == stored.slug
      }
    )
  )
}

scheduled_publication(status: model.ArticleStatus.Draft, article_id: model.id) (
  publication: model.publication.Publication
) {
  publication = publish_draft(article_id)
}
```

## publish_draft

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`publish_draft`
- 输入：
  - `article_id: news.article.model.id`：文章标识
- 输出：
  - `publication: news.article.model.publication.Publication`：该声明的业务值

```dever
publish_draft(article_id: model.id) (publication: model.publication.Publication) {
  stored = model.get(article_id)
  domain.require_draft(stored.status == model.ArticleStatus.Draft)
  content.category.require_active(stored.category_id)
  media.asset.require_ready(stored.asset_id)
  published_at = dever.time.now()
  changed = model.update(
    {
      where = id == stored.id and version == stored.version and status == model.ArticleStatus.Draft
    },
    {
      status = model.ArticleStatus.Published
      published_at = published_at
      version = stored.version + 1
    }
  )
  domain.require_updated(changed)
  audit.operation.record(
    audit.operation.model.OperationKind.Publish,
    "article",
    stored.slug
  )
  publication = model.publication.upsert(
    {
      slug = stored.slug
    },
    {
      title = stored.title
      body = stored.body
      published_at = published_at
    },
    {
      title = stored.title
      body = stored.body
      published_at = published_at
    }
  )
}
```

## existing_publication

手动发布和定时任务共用原子发布流程，发布快照与审计同步提交。

- 函数：`existing_publication`
- 输入：
  - `stored: news.article.model.publication.Publication?`：该声明的业务值
- 输出：
  - `publication: news.article.model.publication.Publication`：该声明的业务值

```dever
existing_publication(stored: model.publication.Publication) (
  publication: model.publication.Publication
) {
  publication = stored
}

existing_publication(stored: null) (publication: model.publication.Publication) {
  fail(dever.api.Error.NotFound)
}
```
