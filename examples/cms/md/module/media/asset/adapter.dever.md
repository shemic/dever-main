# 媒体本地适配器

本地适配器使用受控 data/upload 路径保存上传内容。

- 包：`media.asset.adapter`
- 公开类型：无
- 公开方法：无
- 使用：无

## 本地存储

适配器实现媒体存储端口。

- 函数：`port.store`
- 输入：
  - `file: Upload`：上传文件
- 输出：
  - `key: Uuid`：存储键

```dever
port.store(file: Upload) (key: Uuid) {
  key = dever.storage.put(file)
}
```
