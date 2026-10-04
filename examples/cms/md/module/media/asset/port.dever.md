# 媒体存储端口

媒体应用通过端口隔离存储实现。

- 包：`media.asset.port`
- 公开类型：无
- 公开方法：无
- 使用：无

## 保存上传

端口只声明上传内容到存储的契约。

- 函数：`store`
- 输入：
  - `file: Upload`：上传文件
- 输出：
  - `key: Uuid`：存储键
- 允许失败：`dever.storage.PutResult`

```dever
store(file: Upload) (key: Uuid) fails dever.storage.PutResult
```
