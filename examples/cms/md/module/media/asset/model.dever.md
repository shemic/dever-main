# 媒体资源模型

媒体模型只保存受控存储键和文件元数据。

- 包：`media.asset.model`
- 公开类型：
  - `AssetStatus`
  - `Asset`
- 公开方法：无
- 使用：无

## 资源状态

资源生命周期状态。

- 类型：`AssetStatus`
- 分支：
  - `Ready`：就绪
  - `Removed`：已删除

```dever
type AssetStatus {
  Ready = "就绪"
  Removed = "已删除"
}
```

## 资源记录

资源元数据不包含上传内容本身。

- 类型：`Asset`
- 字段：
  - `storage_key: Uuid`：存储键
  - `filename: Text(1, 255)`：文件名
  - `content_type: Text(1, 160)`：内容类型
  - `size: Int`：大小
  - `status: AssetStatus`：状态

```dever
type Asset {
  storage_key: Uuid unique
  filename: Text(1, 255)
  content_type: Text(1, 160)
  size: Int
  status: AssetStatus default AssetStatus.Ready index
}
```
