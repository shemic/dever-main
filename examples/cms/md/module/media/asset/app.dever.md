# 媒体资源应用

媒体应用通过 Storage Port 接收上传并保存元数据。

- 包：`media.asset.app`
- 公开类型：
  - `StoreResult`
- 公开方法：
  - `store`
  - `require_ready`
  - `ready`
- 使用：
  - `media.asset.store(file)`
  - `media.asset.require_ready(id)`
  - `media.asset.ready(available)`

## 保存结果

返回资源公开元数据。

- 类型：`StoreResult`
- 字段：
  - `id: model.id`：记录标识
  - `storage_key: Uuid`：该声明的业务值
  - `filename: Text`：该声明的业务值
  - `content_type: Text`：该声明的业务值
  - `size: Int`：受限页大小

```dever
type StoreResult {
  id: model.id
  storage_key: Uuid
  filename: Text
  content_type: Text
  size: Int
}
```

## 保存上传

文件内容只能通过 Port 交给 Adapter。

- 函数：`store`
- 输入：
  - `file: Upload`：该声明的业务值
- 输出：
  - `result: StoreResult`：该声明的业务值

```dever
store(file: Upload) (result: StoreResult) {
  filename = dever.api.upload_filename(file)
  content_type = dever.api.upload_content_type(file)
  size = dever.api.upload_size(file)
  key = port.store(file)
  asset = model.create(
    {
      storage_key = key
      filename = filename
      content_type = content_type
      size = size
    }
  )
  audit.operation.record(audit.operation.model.OperationKind.Upload, "asset", filename)
  result = StoreResult {
    id = asset.id
    storage_key = asset.storage_key
    filename = asset.filename
    content_type = asset.content_type
    size = asset.size
  }
}
```

## require_ready

媒体应用通过 Storage Port 接收上传并保存元数据。

- 函数：`require_ready`
- 输入：
  - `id: media.asset.model.id?`：记录标识
- 输出：无

```dever
require_ready(id: model.id) () {
  stored = model.get(id)
  ready(stored.status == model.AssetStatus.Ready)
}

require_ready(id: null) () {}
```

## ready

媒体应用通过 Storage Port 接收上传并保存元数据。

- 函数：`ready`
- 输入：
  - `available: Bool`：该声明的业务值
- 输出：无

```dever
ready(available: true) () {}

ready(available: false) () {
  fail(dever.api.Error.Conflict)
}
```
