# 前台文章接口

前台只查询已发布内容，不开放自动 REST 写路由。

- 包：`news.article.api.front.browse`
- 公开类型：无
- 公开方法：无
- 使用：无

## 文章列表

本节定义对应的源码合同。

- 声明：`get list`

```dever
get list = app.published
```

## 文章详情

本节定义对应的源码合同。

- 声明：`get detail`

```dever
get detail = app.published_detail
```
