# 前台会话接口

前台复用账户和会话，但由 front 站点签发独立受众令牌。

- 包：`user.account.api.front.session`
- 公开类型：无
- 公开方法：无
- 使用：无

## 登录

本节定义对应的源码合同。

- 声明：`post login`

```dever
public post login = app.login
```

## 登出

本节定义对应的源码合同。

- 声明：`post logout`

```dever
post logout = app.logout
```
