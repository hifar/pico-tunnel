# Pico Tunnel Overview

## 1. 项目定位

pico-tunnel 是一个最小可用的反向隧道工具。

核心目标:

- 在公网部署一个 Server
- 在内网或本地部署一个 Client
- 把访问 Server 的请求，反向转发到 Client 本地服务

## 2. 架构角色

- Server
  - 监听 `serv-port` 作为控制端口
  - 校验 Client 的 `serv-key`
  - 按映射的服务端端口维护空闲 Tunnel 连接池
  - 为映射端口动态创建公网监听
  - 对外请求到来时分配隧道并双向转发

- Client
  - 主动连接 Server 并认证
  - 维护多个长连接 worker
  - 接到请求后连接本地 `127.0.0.1:<port>`
  - 与 Server 间做字节流双向转发

## 3. 数据流简述

1. Client worker 连接 Server，并发送握手标识 + key
2. Client 同时上报映射端口（remote_port）
3. Server 验证通过后，把连接放入对应端口的空闲池
4. Server 为该映射端口创建业务监听（若尚未创建）
5. 外部请求访问映射端口
6. Server 从该端口对应池中拿一个空闲隧道连接
7. 请求数据经隧道到达 Client
8. Client 把数据转发到本地服务端口
9. 本地服务响应原路返回给外部请求方

## 4. 并发模型

- Server: 每个新连接由独立 Tokio 任务处理
- Client: 默认 16 个 worker 并发待命
- 会话级别: 每条 worker 连接一次处理一个转发会话，完成后重连

## 5. 协议与鉴权

- 使用极简私有握手头用于识别 Tunnel Client
- 握手阶段校验 `serv-key` 并携带映射端口
- 业务请求只进入映射端口监听，不走控制端口

说明:

- 当前鉴权是简单明文 key，不提供传输层加密
- 生产环境建议叠加 TLS/VPN/内网专线

## 6. 错误语义

- 无可用隧道: `503 Service Unavailable`
- Client 无法连接本地服务: `502 Bad Gateway`
- key 不匹配: 连接拒绝并断开

## 7. 适用场景

- 本地开发服务临时公网暴露
- 测试环境联调
- 小规模内网服务映射

## 8. 不覆盖范围

- 高安全要求场景（未内建 TLS/mTLS）
- 多租户生产网关场景
- 大规模连接治理与复杂流量策略
