use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result, bail};
use clap::{Args, Parser, Subcommand};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

const HANDSHAKE_MAGIC: &[u8] = b"PICO-T1";
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_CONNECTIONS: usize = 16;
const TUNNEL_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Parser, Debug)]
#[command(name = "pico-tunnel")]
#[command(about = "A minimal reverse HTTP tunnel CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Server(ServerArgs),
    Client(ClientArgs),
}

#[derive(Args, Debug, Clone)]
struct ServerArgs {
    #[arg(long = "serv-port", help = "Server control port for tunnel clients")]
    serv_port: u16,
    #[arg(long = "serv-key")]
    serv_key: String,
}

#[derive(Args, Debug, Clone)]
struct ClientArgs {
    #[arg(
        long,
        value_parser = parse_port_mapping,
        help = "Port mapping: <local> or <local>:<remote>, e.g. 3000 or 3000:3002"
    )]
    port: PortMapping,
    #[arg(long = "serv-host")]
    serv_host: String,
    #[arg(long = "serv-port")]
    serv_port: u16,
    #[arg(long = "serv-key")]
    serv_key: String,
    #[arg(
        long,
        default_value_t = DEFAULT_CONNECTIONS,
        value_parser = parse_connections,
        help = "Number of concurrent tunnel workers"
    )]
    connections: usize,
}

#[derive(Debug, Clone, Copy)]
struct PortMapping {
    local_port: u16,
    remote_port: u16,
}

fn parse_port_mapping(value: &str) -> Result<PortMapping, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("port cannot be empty".to_string());
    }

    let mut parts = value.split(':');
    let local = parts.next().ok_or_else(|| "missing local port".to_string())?;
    let remote = parts.next();

    if parts.next().is_some() {
        return Err("port format must be <local> or <local>:<remote>".to_string());
    }

    let local_port = local
        .parse::<u16>()
        .map_err(|_| format!("invalid local port: {local}"))?;
    let remote_port = match remote {
        Some(raw) => raw
            .parse::<u16>()
            .map_err(|_| format!("invalid remote port: {raw}"))?,
        None => local_port,
    };

    Ok(PortMapping {
        local_port,
        remote_port,
    })
}

fn parse_connections(value: &str) -> Result<usize, String> {
    let parsed = value
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("invalid connections value: {value}"))?;
    if parsed == 0 {
        return Err("connections must be >= 1".to_string());
    }
    Ok(parsed)
}

struct TunnelPool {
    idle: Mutex<VecDeque<TcpStream>>,
    notify: Notify,
}

impl TunnelPool {
    fn new() -> Self {
        Self {
            idle: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        }
    }

    async fn push(&self, stream: TcpStream) {
        self.idle.lock().await.push_back(stream);
        self.notify.notify_one();
    }

    async fn pop_nowait(&self) -> Option<TcpStream> {
        self.idle.lock().await.pop_front()
    }

    async fn pop_wait(&self, wait_timeout: Duration) -> Option<TcpStream> {
        timeout(wait_timeout, async {
            loop {
                let notified = self.notify.notified();
                if let Some(stream) = self.pop_nowait().await {
                    return stream;
                }
                notified.await;
            }
        })
        .await
        .ok()
    }
}

struct ServerState {
    serv_key: String,
    pools: Mutex<HashMap<u16, Arc<TunnelPool>>>,
    listeners: Mutex<HashSet<u16>>,
}

impl ServerState {
    fn new(serv_key: String) -> Self {
        Self {
            serv_key,
            pools: Mutex::new(HashMap::new()),
            listeners: Mutex::new(HashSet::new()),
        }
    }

    async fn pool_for(&self, remote_port: u16) -> Arc<TunnelPool> {
        let mut pools = self.pools.lock().await;
        pools
            .entry(remote_port)
            .or_insert_with(|| Arc::new(TunnelPool::new()))
            .clone()
    }
}

struct PrefixedStream<S> {
    prefix: Vec<u8>,
    offset: usize,
    inner: S,
}

impl<S> PrefixedStream<S> {
    fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            offset: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.offset < self.prefix.len() {
            let remaining = &self.prefix[self.offset..];
            let chunk_len = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..chunk_len]);
            self.offset += chunk_len;
            return Poll::Ready(Ok(()));
        }

        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server(args) => run_server(args).await,
        Commands::Client(args) => run_client(args).await,
    }
}

async fn run_server(args: ServerArgs) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", args.serv_port))
        .await
        .with_context(|| format!("failed to bind server port {}", args.serv_port))?;
    let state = Arc::new(ServerState::new(args.serv_key));

    println!("server control listening on 0.0.0.0:{}", args.serv_port);

    loop {
        let (socket, peer_addr) = listener.accept().await.context("accept failed")?;
        let state = Arc::clone(&state);

        tokio::spawn(async move {
            if let Err(error) = handle_server_client_connection(socket, state).await {
                eprintln!("server connection error from {peer_addr}: {error:#}");
            }
        });
    }
}

async fn handle_server_client_connection(
    mut socket: TcpStream,
    state: Arc<ServerState>,
) -> Result<()> {
    let (key, remote_port) = read_handshake(&mut socket).await?;
    if key != state.serv_key {
        bail!("client authentication failed");
    }

    let pool = state.pool_for(remote_port).await;
    pool.push(socket).await;
    ensure_public_listener(Arc::clone(&state), remote_port).await?;
    Ok(())
}

async fn ensure_public_listener(state: Arc<ServerState>, remote_port: u16) -> Result<()> {
    {
        let mut listeners = state.listeners.lock().await;
        if !listeners.insert(remote_port) {
            return Ok(());
        }
    }

    let public_listener = match TcpListener::bind(("0.0.0.0", remote_port)).await {
        Ok(listener) => listener,
        Err(error) => {
            state.listeners.lock().await.remove(&remote_port);
            return Err(error)
                .with_context(|| format!("failed to bind mapped server port {remote_port}"));
        }
    };

    println!("public listener ready on 0.0.0.0:{remote_port}");

    tokio::spawn(async move {
        loop {
            let accepted = public_listener.accept().await;
            let (incoming, peer_addr) = match accepted {
                Ok(pair) => pair,
                Err(error) => {
                    eprintln!("accept failed on mapped port {remote_port}: {error}");
                    continue;
                }
            };

            let state = Arc::clone(&state);
            tokio::spawn(async move {
                if let Err(error) = handle_public_connection(incoming, state, remote_port).await {
                    eprintln!(
                        "forwarding failed on mapped port {remote_port} for {peer_addr}: {error:#}"
                    );
                }
            });
        }
    });

    Ok(())
}

async fn handle_public_connection(
    mut incoming: TcpStream,
    state: Arc<ServerState>,
    remote_port: u16,
) -> Result<()> {
    let pool = state.pool_for(remote_port).await;
    let Some(mut tunnel_socket) = pool.pop_wait(TUNNEL_WAIT_TIMEOUT).await else {
        write_http_response(
            &mut incoming,
            503,
            "Service Unavailable",
            "no tunnel client is currently connected or available within wait timeout\n",
        )
        .await?;
        return Ok(());
    };

    let _ = copy_bidirectional(&mut incoming, &mut tunnel_socket).await?;
    Ok(())
}

async fn run_client(args: ClientArgs) -> Result<()> {
    let worker_count = args.connections;
    let shared_args = Arc::new(args);
    let mut workers = JoinSet::new();

    println!(
        "client workers: {}, local {} -> server {}",
        worker_count, shared_args.port.local_port, shared_args.port.remote_port
    );

    for worker_id in 0..worker_count {
        let shared_args = Arc::clone(&shared_args);
        workers.spawn(async move { run_client_worker_loop(worker_id, shared_args).await });
    }

    loop {
        match workers.join_next().await {
            Some(Ok(Ok(()))) => bail!("client worker exited unexpectedly"),
            Some(Ok(Err(error))) => return Err(error),
            Some(Err(error)) => return Err(error.into()),
            None => bail!("all client workers stopped"),
        }
    }
}

async fn run_client_worker_loop(worker_id: usize, args: Arc<ClientArgs>) -> Result<()> {
    loop {
        if let Err(error) = run_client_worker_once(&args).await {
            eprintln!("client worker {worker_id} error: {error:#}");
            sleep(RECONNECT_DELAY).await;
        }
    }
}

async fn run_client_worker_once(args: &ClientArgs) -> Result<()> {
    let server_addr = format!("{}:{}", args.serv_host, args.serv_port);
    let mut tunnel_socket = TcpStream::connect(&server_addr)
        .await
        .with_context(|| format!("failed to connect to server {server_addr}"))?;

    write_handshake(&mut tunnel_socket, &args.serv_key, args.port.remote_port).await?;

    let first_chunk = read_first_chunk(&mut tunnel_socket).await?;
    if first_chunk.is_empty() {
        bail!("server closed the tunnel before forwarding any request")
    }

    let mut local_socket = match TcpStream::connect(("127.0.0.1", args.port.local_port)).await {
        Ok(stream) => stream,
        Err(error) => {
            let body = format!(
                "local service 127.0.0.1:{} is unavailable: {}\n",
                args.port.local_port, error
            );
            write_http_response(&mut tunnel_socket, 502, "Bad Gateway", &body).await?;
            bail!("failed to connect local service on port {}", args.port.local_port);
        }
    };

    let mut tunneled_request = PrefixedStream::new(first_chunk, tunnel_socket);
    let _ = copy_bidirectional(&mut tunneled_request, &mut local_socket).await?;
    Ok(())
}

async fn read_handshake(socket: &mut TcpStream) -> Result<(String, u16)> {
    let mut magic = vec![0_u8; HANDSHAKE_MAGIC.len()];
    socket
        .read_exact(&mut magic)
        .await
        .context("failed to read handshake magic")?;
    if magic != HANDSHAKE_MAGIC {
        bail!("invalid tunnel handshake magic");
    }

    let key_len = socket.read_u16().await.context("failed to read key length")? as usize;
    let mut key_bytes = vec![0_u8; key_len];
    socket
        .read_exact(&mut key_bytes)
        .await
        .context("failed to read key bytes")?;
    let remote_port = socket
        .read_u16()
        .await
        .context("failed to read mapped remote port")?;
    let key = String::from_utf8(key_bytes).context("server key was not valid utf-8")?;
    Ok((key, remote_port))
}

async fn write_handshake(socket: &mut TcpStream, key: &str, remote_port: u16) -> Result<()> {
    let key_bytes = key.as_bytes();
    if key_bytes.len() > u16::MAX as usize {
        bail!("server key is too long")
    }

    socket.write_all(HANDSHAKE_MAGIC).await?;
    socket.write_u16(key_bytes.len() as u16).await?;
    socket.write_all(key_bytes).await?;
    socket.write_u16(remote_port).await?;
    socket.flush().await?;
    Ok(())
}

async fn read_first_chunk(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buffer = vec![0_u8; 16 * 1024];
    let bytes_read = socket.read(&mut buffer).await?;
    buffer.truncate(bytes_read);
    Ok(buffer)
}

async fn write_http_response(
    socket: &mut TcpStream,
    status_code: u16,
    reason: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status_code} {reason}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;
    Ok(())
}
