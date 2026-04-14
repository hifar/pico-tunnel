use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result, bail};
use clap::{Args, Parser, Subcommand};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

const HANDSHAKE_MAGIC: &[u8] = b"PICO-T1";
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_CONNECTIONS: usize = 16;

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
    #[arg(long = "serv-port")]
    serv_port: u16,
    #[arg(long = "serv-key")]
    serv_key: String,
}

#[derive(Args, Debug, Clone)]
struct ClientArgs {
    #[arg(long)]
    port: u16,
    #[arg(long = "serv-host")]
    serv_host: String,
    #[arg(long = "serv-port")]
    serv_port: u16,
    #[arg(long = "serv-key")]
    serv_key: String,
    #[arg(long, default_value_t = DEFAULT_CONNECTIONS)]
    connections: usize,
}

struct TunnelPool {
    idle: Mutex<VecDeque<TcpStream>>,
}

impl TunnelPool {
    fn new() -> Self {
        Self {
            idle: Mutex::new(VecDeque::new()),
        }
    }

    async fn push(&self, stream: TcpStream) {
        self.idle.lock().await.push_back(stream);
    }

    async fn pop(&self) -> Option<TcpStream> {
        self.idle.lock().await.pop_front()
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
    let pool = Arc::new(TunnelPool::new());
    let server_key = Arc::new(args.serv_key);

    println!("server listening on 0.0.0.0:{}", args.serv_port);

    loop {
        let (socket, peer_addr) = listener.accept().await.context("accept failed")?;
        let pool = Arc::clone(&pool);
        let server_key = Arc::clone(&server_key);

        tokio::spawn(async move {
            if let Err(error) = handle_server_connection(socket, pool, server_key).await {
                eprintln!("server connection error from {peer_addr}: {error:#}");
            }
        });
    }
}

async fn handle_server_connection(
    mut socket: TcpStream,
    pool: Arc<TunnelPool>,
    server_key: Arc<String>,
) -> Result<()> {
    let probe = read_probe_bytes(&mut socket).await?;

    if probe == HANDSHAKE_MAGIC {
        let key = read_handshake_key(&mut socket).await?;
        if key != server_key.as_str() {
            bail!("client authentication failed");
        }

        pool.push(socket).await;
        return Ok(());
    }

    let Some(mut tunnel_socket) = pool.pop().await else {
        write_http_response(
            &mut socket,
            503,
            "Service Unavailable",
            "no tunnel client is currently connected\n",
        )
        .await?;
        return Ok(());
    };

    let mut incoming = PrefixedStream::new(probe, socket);
    let _ = copy_bidirectional(&mut incoming, &mut tunnel_socket).await?;
    Ok(())
}

async fn run_client(args: ClientArgs) -> Result<()> {
    let worker_count = args.connections.max(1);
    let shared_args = Arc::new(args);
    let mut workers = JoinSet::new();

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

    write_handshake(&mut tunnel_socket, &args.serv_key).await?;

    let first_chunk = read_first_chunk(&mut tunnel_socket).await?;
    if first_chunk.is_empty() {
        bail!("server closed the tunnel before forwarding any request")
    }

    let mut local_socket = match TcpStream::connect(("127.0.0.1", args.port)).await {
        Ok(stream) => stream,
        Err(error) => {
            let body = format!(
                "local service 127.0.0.1:{} is unavailable: {}\n",
                args.port, error
            );
            write_http_response(&mut tunnel_socket, 502, "Bad Gateway", &body).await?;
            bail!("failed to connect local service on port {}", args.port);
        }
    };

    let mut tunneled_request = PrefixedStream::new(first_chunk, tunnel_socket);
    let _ = copy_bidirectional(&mut tunneled_request, &mut local_socket).await?;
    Ok(())
}

async fn read_probe_bytes(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut probe = vec![0_u8; HANDSHAKE_MAGIC.len()];
    let mut read_len = 0;

    while read_len < probe.len() {
        let read_result = timeout(PROBE_TIMEOUT, socket.read(&mut probe[read_len..])).await;
        match read_result {
            Ok(Ok(0)) => break,
            Ok(Ok(bytes_read)) => read_len += bytes_read,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => break,
        }
    }

    probe.truncate(read_len);
    Ok(probe)
}

async fn read_handshake_key(socket: &mut TcpStream) -> Result<String> {
    let key_len = socket.read_u16().await.context("failed to read key length")? as usize;
    let mut key_bytes = vec![0_u8; key_len];
    socket
        .read_exact(&mut key_bytes)
        .await
        .context("failed to read key bytes")?;
    String::from_utf8(key_bytes).context("server key was not valid utf-8")
}

async fn write_handshake(socket: &mut TcpStream, key: &str) -> Result<()> {
    let key_bytes = key.as_bytes();
    if key_bytes.len() > u16::MAX as usize {
        bail!("server key is too long")
    }

    socket.write_all(HANDSHAKE_MAGIC).await?;
    socket.write_u16(key_bytes.len() as u16).await?;
    socket.write_all(key_bytes).await?;
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
