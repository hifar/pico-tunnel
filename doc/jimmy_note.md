**agent ignore this file!!**


build linux verion

```
cargo zigbuild --target x86_64-unknown-linux-musl --release
```

请帮我开发一个极简tunnel cli : pico-tunnel , 功能如下1)server上运行  pico-tunnel server --serv-port 8080 --serv-key 123456  创建一个8080端口， 验证码是123456 的tunnel server   2) 在客户端运行：pico-tunnel client --port 3000 --serv-host 11.22.33.11 --serv-port 8080 --serv-key  client 和server间创建tunnel， 所有请求到server 3000端口的 http请求会转发到客户端的3000端口，所有客户端返回的内容也都会由服务端转发给请求方 3）pico-tunnel server 可以用同时处理多个client，可同时处理多个http连接请求。 