use std::{
    error::Error,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{interval, sleep, timeout, MissedTickBehavior},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Parser)]
#[command(
    version,
    about = "Portable SSH-over-WSS bridge; never resumes an interrupted SSH session"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// OpenSSH ProxyCommand: stdin/stdout <-> WSS.
    Stdio {
        #[arg(long)]
        url: String,
        #[command(flatten)]
        options: Options,
    },
    /// Local TCP listener for MobaXterm; one WSS connection per TCP client.
    Listen {
        #[arg(long, default_value_t = 2222)]
        port: u16,
        #[arg(long)]
        url: String,
        #[command(flatten)]
        options: Options,
    },
}

#[derive(clap::Args, Clone)]
struct Options {
    /// Maximum WSS connection attempts (including first). 0 = unlimited.
    #[arg(long, default_value_t = 10)]
    connect_attempts: u32,
    /// Timeout for each connection attempt in seconds.
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,
    /// WebSocket ping interval in seconds. 0 disables heartbeat.
    #[arg(long, default_value_t = 20)]
    ping_interval: u64,
    /// Maximum time without receiving any WebSocket frame in seconds.
    /// 0 disables inactivity detection. Should exceed ping_interval.
    #[arg(long, default_value_t = 65)]
    idle_timeout: u64,
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to initialize TLS crypto provider");

    if let Err(err) = run().await {
        eprintln!("sshbridge: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Stdio { url, options } => {
            validate(&url, &options)?;
            bridge(tokio::io::stdin(), tokio::io::stdout(), &url, &options).await
        }
        Command::Listen { port, url, options } => {
            validate(&url, &options)?;
            let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
            let listener = TcpListener::bind(address).await?;
            eprintln!("sshbridge: listening on {address} -> {url}");
            loop {
                let (socket, peer) = listener.accept().await?;
                let url = url.clone();
                let options = options.clone();
                tokio::spawn(async move {
                    if let Err(err) = bridge_tcp(socket, &url, &options).await {
                        eprintln!("sshbridge: client {peer}: {err}");
                    }
                });
            }
        }
    }
}

fn validate(url: &str, options: &Options) -> Result<()> {
    if !url.starts_with("wss://") {
        return Err("only wss:// URLs are accepted".into());
    }
    if options.connect_timeout == 0 {
        return Err("--connect-timeout must be positive".into());
    }
    if options.idle_timeout != 0
        && options.ping_interval != 0
        && options.idle_timeout <= options.ping_interval
    {
        return Err("--idle-timeout must exceed --ping-interval".into());
    }
    Ok(())
}

async fn bridge_tcp(socket: TcpStream, url: &str, options: &Options) -> Result<()> {
    socket.set_nodelay(true)?;
    let (reader, writer) = socket.into_split();
    bridge(reader, writer, url, options).await
}

async fn bridge<R, W>(mut input: R, mut output: W, url: &str, options: &Options) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Retry only BEFORE any SSH bytes have been transported. A new WebSocket
    // cannot resume an existing SSH cryptographic session after a drop.
    let mut attempt = 0u32;
    let ws = loop {
        attempt = attempt.saturating_add(1);
        match timeout(
            Duration::from_secs(options.connect_timeout),
            connect_async(url),
        )
        .await
        {
            Ok(Ok((ws, _response))) => break ws,
            outcome => {
                let reason = match outcome {
                    Ok(Err(err)) => err.to_string(),
                    Err(_) => format!("connect timeout ({}s)", options.connect_timeout),
                    Ok(Ok(_)) => unreachable!(),
                };
                if options.connect_attempts != 0 && attempt >= options.connect_attempts {
                    return Err(format!(
                        "WSS connection failed after {attempt} attempt(s): {reason}"
                    )
                    .into());
                }
                let backoff = 1u64 << (attempt.saturating_sub(1).min(5));
                eprintln!("sshbridge: WSS attempt {attempt} failed: {reason}; retry in {backoff}s");
                sleep(Duration::from_secs(backoff)).await;
            }
        }
    };

    let (mut ws_tx, mut ws_rx) = ws.split();
    let mut buffer = [0u8; 16 * 1024];
    let mut ticker = interval(Duration::from_secs(options.ping_interval.max(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await; // skip immediate first tick
    let mut last_received = Instant::now();

    let outcome: Result<()> = loop {
        tokio::select! {
            read_result = input.read(&mut buffer) => {
                match read_result {
                    Ok(0) => {
                        let _ = ws_tx.send(Message::Close(None)).await;
                        break Ok(());
                    }
                    Ok(count) => {
                        if let Err(err) = ws_tx.send(Message::Binary(buffer[..count].to_vec().into())).await {
                            break Err(err.into());
                        }
                    }
                    Err(err) => break Err(err.into()),
                }
            }
            message = ws_rx.next() => {
                last_received = Instant::now();
                match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Err(err) = output.write_all(&bytes).await {
                            break Err(err.into());
                        }
                        if let Err(err) = output.flush().await {
                            break Err(err.into());
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // tungstenite automatically queues Pong; flush the queue.
                        if let Err(err) = ws_tx.flush().await {
                            break Err(err.into());
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => {
                        break Err(io::Error::new(io::ErrorKind::ConnectionAborted, "WSS connection closed; SSH session cannot be resumed").into());
                    }
                    Some(Ok(Message::Text(_))) => {
                        break Err("unexpected text WebSocket frame".into());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => break Err(err.into()),
                }
            }
            _ = ticker.tick() => {
                if options.idle_timeout > 0 && last_received.elapsed() > Duration::from_secs(options.idle_timeout) {
                    break Err(io::Error::new(io::ErrorKind::TimedOut, "WSS heartbeat timeout; reconnect SSH").into());
                }
                if options.ping_interval > 0 {
                    if let Err(err) = ws_tx.send(Message::Ping(Vec::new().into())).await {
                        break Err(err.into());
                    }
                }
            }
        }
    };
    // Close the local TCP stream / stdout (stdout shutdown may be a no-op).
    let _ = output.shutdown().await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> Options {
        Options {
            connect_attempts: 10,
            connect_timeout: 10,
            ping_interval: 20,
            idle_timeout: 65,
        }
    }
    #[test]
    fn rejects_unencrypted_websocket() {
        assert!(validate("ws://localhost:8022/ssh", &options()).is_err());
    }
    #[test]
    fn accepts_secure_websocket() {
        assert!(validate("wss://con.lli.be/ssh", &options()).is_ok());
    }
    #[test]
    fn rejects_invalid_timeouts() {
        let mut o = options();
        o.idle_timeout = 10;
        assert!(validate("wss://con.lli.be/ssh", &o).is_err());
    }
}
