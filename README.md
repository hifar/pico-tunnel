# pico-tunnel

![Powerby hifar](https://img.shields.io/badge/powerby-hifar-ff6b6b?style=for-the-badge)
![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange?style=for-the-badge&logo=rust)
![Tokio](https://img.shields.io/badge/Tokio-Async%20Runtime-0f766e?style=for-the-badge)
![Clap](https://img.shields.io/badge/CLI-clap-3b82f6?style=for-the-badge)
![MIT License](https://img.shields.io/badge/License-MIT-22c55e?style=for-the-badge)
![Version](https://img.shields.io/badge/version-0.1.0-a855f7?style=for-the-badge)

pico-tunnel 是一个极简反向隧道 CLI，使用 Rust + Tokio 实现。

它的目标是把公网 Server 收到的 HTTP 请求，转发到 Client 所在机器的本地端口。

## 功能

- Server 模式: 监听一个控制端口，接收 Tunnel Client 连接
- 按 Client 上报的映射端口，动态创建公网业务监听端口
- 可选开启服务端 Basic Auth，保护未带登录能力的后端服务
- Client 模式: 连接到 Server，建立反向隧道，将流量转发到本地服务
- 基于 serv-key 的简单握手认证
- 支持并发: 一个 Client 可维持多条连接，Server 可并发处理多个 HTTP 请求

## 当前实现要点

- 入口命令:
  - `pico-tunnel server --serv-port <PORT> --serv-key <KEY> [--debug] [--auth-enabled --auth-user <USER> --auth-pass <PASS>]`
  - `pico-tunnel client --port <LOCAL_PORT[:REMOTE_PORT]> --serv-host <HOST> --serv-port <PORT> --serv-key <KEY> [--debug] [--connections <N>]`
- `--port` 映射规则:
  - `--port 3000` 等价于 `3000:3000`
  - `--port 3000:3002` 表示客户端 `3000` 映射到服务端 `3002`
- Server 监听模型:
  - `serv-port` 仅用于 Client 控制连接与认证
  - 业务请求走映射后的服务端端口（如 `3002`）
- Client 默认维护 16 条工作连接 (`--connections` 可调整)
- 每条工作连接一次处理一个转发会话，结束后自动重连

## 安装与构建

要求:

- Rust stable (建议 1.80+)

构建:

```bash
cargo build --release
```

检查:

```bash
cargo check
```

## 基本用法

### 1. 启动 Server

```bash
cargo run -- server --serv-port 8080 --serv-key 123456
```

如果要开启 Basic Auth:

```bash
cargo run -- server --serv-port 8080 --serv-key 123456 --auth-enabled --auth-user admin --auth-pass 123456
```

如果要开启服务端调试日志（建议排障时开启）:

```bash
cargo run -- server --serv-port 8080 --serv-key 123456 --debug --auth-enabled --auth-user admin --auth-pass 123456
```

### 2. 启动 Client

假设 Client 机器本地已有 HTTP 服务监听 3000 端口:

```bash
cargo run -- client --port 3000 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456
```

如果希望访问 Server 的 3002 时转发到 Client 本地 3000:

```bash
cargo run -- client --port 3000:3002 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456
```

如果要开启客户端调试日志:

```bash
cargo run -- client --port 3000:3002 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456 --debug --connections 64
```

### 3. 从外部访问 Server

当使用 `--port 3000` 时，访问 `11.22.33.11:3000` 会被转发到 Client 本地 `127.0.0.1:3000`。

当使用 `--port 3000:3002` 时，访问 `11.22.33.11:3002` 会被转发到 Client 本地 `127.0.0.1:3000`。

## 并发说明

- Server 通过 Tokio 任务并发处理连接。
- Client 通过 `--connections` 预建多个隧道 worker，提高并发能力。
- 当没有可用 Client 隧道时，Server 会先排队等待（默认 5 秒），超时后返回 `503 Service Unavailable`。

## Basic Auth 说明

- 默认关闭。
- 开启方式: `--auth-enabled --auth-user <USER> --auth-pass <PASS>`。
- 开启后，访问映射端口必须携带 `Authorization: Basic <token>`，否则返回 `401 Unauthorized`。
- 认证仅保护 HTTP 请求头层面，不等价于 TLS 加密。

## 失败场景与返回

- Client key 不匹配: 认证失败，连接被拒绝。
- Client 本地端口不可用: 返回 `502 Bad Gateway`。
- 没有可用隧道连接: 返回 `503 Service Unavailable`。

## 常见日志排障

- `invalid tunnel handshake magic`
  - 常见原因: 公网扫描器或非 pico-tunnel 客户端访问了 server 控制端口。
  - 当前行为: 已降级为 debug 日志，不会中断正常转发。
- `os error 10053 / 10054`
  - 常见原因: 浏览器主动中断、网络抖动、客户端本地服务关闭连接、隧道连接被中间网络设备回收。
  - 建议: server/client 都开启 `--debug`，对照转发开始/结束和字节数，定位是请求方中断还是后端中断。

## 限制

- 当前为极简 TCP 字节流转发实现，主要面向 HTTP 场景。
- 握手认证仅为明文 key，不等价于安全加密通道。
- 未实现多租户隔离、流量审计、TLS 终止、复杂路由。

## 后续可扩展方向

- 增加 TLS 加密与证书校验
- 增加服务名路由（一个 Server 暴露多个本地服务）
- 增加心跳、健康检查和会话可观测性
- 增加配置文件和 systemd/service 部署支持
