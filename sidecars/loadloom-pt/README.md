# loadloom-pt

LoadLoom 的真实 BitTorrent 压测 sidecar。它使用 `anacrolix/torrent` 提供 Tracker、DHT、PEX、TCP、uTP、协议加密和连接调度，stdout 只输出一行一个 JSON 事件，供 Tauri 桥接。

## 存储保证

- torrent payload 不写文件，也不使用 mmap。
- 未完成 piece 只在有上限的 RAM 缓冲中存在。
- BT 引擎完成哈希校验并调用 `MarkComplete` 后，piece 字节立即释放，只保留完成位。
- `NoUpload` 已启用；已经释放的数据不会被当作种子上传。
- `.torrent` 元数据 URL 最大 4 MiB，并直接从 HTTP 响应解析到内存。

## Peer 诊断口径

- `deadPeers`：连接关闭时累计有效 payload 少于 64 KiB。
- `stalledPeers`：连接已超过配置的宽限期，且在同一时长内没有新增有效 payload。
- `wastedBytes`：收到的 BT 数据字节减去有效数据字节。
- `wireBytes`：含握手、加密和协议开销的总接收字节。

## 开发

```bash
go test ./...
go vet ./...
go build -trimpath -o loadloom-pt.exe .
```

示例（只使用你有权下载的内容）：

```bash
loadloom-pt.exe \
  --source "https://releases.ubuntu.com/26.04/ubuntu-26.04.1-desktop-amd64.iso.torrent" \
  --connections 180 \
  --ram-mib 512 \
  --duration-seconds 600
```

通过 stdin 发送 `{"type":"stop"}` 可优雅停止。协议版本由启动事件的 `protocolVersion` 字段声明。
