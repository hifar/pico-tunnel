# pico-tunnel

pico-tunnel 是一个极简反向隧道 CLI，使用 Rust + Tokio 实现。

它的目标是把公网 Server 收到的 HTTP 请求，转发到 Client 所在机器的本地端口。

## 功能

- Server 模式: 监听一个控制端口，接收 Tunnel Client 连接
- 按 Client 上报的映射端口，动态创建公网业务监听端口
- Client 模式: 连接到 Server，建立反向隧道，将流量转发到本地服务
- 基于 serv-key 的简单握手认证
- 支持并发: 一个 Client 可维持多条连接，Server 可并发处理多个 HTTP 请求

## 当前实现要点

- 入口命令:
  - `pico-tunnel server --serv-port <PORT> --serv-key <KEY>`
  - `pico-tunnel client --port <LOCAL_PORT[:REMOTE_PORT]> --serv-host <HOST> --serv-port <PORT> --serv-key <KEY> [--connections <N>]`
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

### 2. 启动 Client

假设 Client 机器本地已有 HTTP 服务监听 3000 端口:

```bash
cargo run -- client --port 3000 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456
```

如果希望访问 Server 的 3002 时转发到 Client 本地 3000:

```bash
cargo run -- client --port 3000:3002 --serv-host 11.22.33.11 --serv-port 8080 --serv-key 123456
```

### 3. 从外部访问 Server

当使用 `--port 3000` 时，访问 `11.22.33.11:3000` 会被转发到 Client 本地 `127.0.0.1:3000`。

当使用 `--port 3000:3002` 时，访问 `11.22.33.11:3002` 会被转发到 Client 本地 `127.0.0.1:3000`。

## 并发说明

- Server 通过 Tokio 任务并发处理连接。
- Client 通过 `--connections` 预建多个隧道 worker，提高并发能力。
- 当没有可用 Client 隧道时，Server 返回 `503 Service Unavailable`。

## 失败场景与返回

- Client key 不匹配: 认证失败，连接被拒绝。
- Client 本地端口不可用: 返回 `502 Bad Gateway`。
- 没有可用隧道连接: 返回 `503 Service Unavailable`。

## 限制

- 当前为极简 TCP 字节流转发实现，主要面向 HTTP 场景。
- 握手认证仅为明文 key，不等价于安全加密通道。
- 未实现多租户隔离、流量审计、TLS 终止、复杂路由。

## 后续可扩展方向

- 增加 TLS 加密与证书校验
- 增加服务名路由（一个 Server 暴露多个本地服务）
- 增加心跳、健康检查和会话可观测性
- 增加配置文件和 systemd/service 部署支持
