# Quick Start

本文用最短步骤带你跑通 pico-tunnel。

## 0. 前置条件

- 机器 A: 作为 Server（有公网 IP）
- 机器 B: 作为 Client（能访问机器 A）
- 机器 B 本地已有 HTTP 服务，例如监听 `3000`
- 已安装 Rust 与 Cargo

## 1. 获取并构建

在项目根目录执行:

```bash
cargo build --release
```

可执行文件路径:

- Windows: `target/release/pico-tunnel.exe`
- Linux/macOS: `target/release/pico-tunnel`

## 2. 启动 Server（机器 A）

```bash
target/release/pico-tunnel server --serv-port 8080 --serv-key 123456
```

启动后，Server 会监听 `0.0.0.0:8080`。

## 3. 启动 Client（机器 B）

```bash
target/release/pico-tunnel client --port 3000 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456
```

参数说明:

- `--port`: Client 本地服务端口
- `--serv-host`: Server 地址
- `--serv-port`: Server 监听端口
- `--serv-key`: 与 Server 一致的认证 key

可选并发参数:

```bash
target/release/pico-tunnel client --port 3000 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456 --connections 32
```

## 4. 验证转发

在任意可访问 Server 的机器上执行:

```bash
curl http://11.22.33.11:8080/
```

如果 Client 本地 `127.0.0.1:3000` 正常提供 HTTP 服务，应返回对应响应内容。

## 5. 常见问题

- 返回 `503 Service Unavailable`
  - 原因: 当前没有可用 tunnel worker
  - 处理: 检查 Client 是否在线、key 是否匹配

- 返回 `502 Bad Gateway`
  - 原因: Client 无法连接本地 `127.0.0.1:<port>`
  - 处理: 确认本地服务已启动且端口正确

- Client 持续重连
  - 可能原因: Server 地址/端口错误、网络不通、key 不匹配

## 6. 最小排障命令

```bash
cargo check
cargo run -- server --serv-port 8080 --serv-key 123456
cargo run -- client --port 3000 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456
```
