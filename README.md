# sshbridge

Small, portable **client-side** SSH-over-WSS tunnel written in Rust. Compatible with the existing server at `wss://con.lli.be/ssh` (Nginx → Websocat → sshd:23400). **No server changes required.**

## Files

- `Cargo.toml`: Rust dependencies
- `src/main.rs`: complete source code
- `examples/ssh-config-windows.txt`: Windows OpenSSH configuration
- `examples/ssh-config-linux.txt`: Linux/macOS OpenSSH configuration

## Compile on Windows

1. Install Rust via [rustup](https://rustup.rs/) (Windows MSVC toolchain; Microsoft C++ build tools may be required).
2. In PowerShell, from the project folder:

```powershell
cargo build --release
& ".\target\release\sshbridge.exe" --help
Copy-Item ".\target\release\sshbridge.exe" "C:\lli\tools\sshbridge.exe"
```

Only the **build machine** needs Rust; the resulting executable can be copied to other compatible Windows machines. Build for the target CPU/OS architecture. Windows MSVC binaries may depend on the system's Microsoft C/C++ runtime; for maximal portability consider a static CRT build.

## Compile on Arch Linux

```bash
sudo pacman -S --needed rust cargo gcc
cargo build --release
./target/release/sshbridge --help
sudo install -m 755 target/release/sshbridge /usr/local/bin/sshbridge
```

## Mode 1: SSH ProxyCommand (recommended)

Windows PowerShell:

```powershell
sshbridge.exe stdio --url wss://con.lli.be/ssh
```

This command appears idle when run manually because it expects SSH protocol data on stdin. Configure `~/.ssh/config` as shown in `examples/ssh-config-windows.txt`, then connect with:

```powershell
ssh hetzner-https
```

The process starts and stops automatically with the SSH session. **Never write debug messages to stdout in this mode.**

## Mode 2: MobaXterm local port

```powershell
& "C:\lli\tools\sshbridge.exe" listen --port 2222 --url wss://con.lli.be/ssh
```

In MobaXterm: SSH host `127.0.0.1`, port `2222`, user `lli`, your existing private key. Keep the terminal running; stop with Ctrl+C. Each TCP connection creates a separate WSS connection.

## Security

- Only `wss://` URLs are accepted; TLS certificate validation uses standard trusted root certificates.
- Local TCP listening is restricted to `127.0.0.1`.
- SSH authentication and host key verification remain the SSH client's responsibility.
- No `--insecure` / TLS bypass option.
- This does not conceal SSH traffic from a server-side endpoint and does not bypass all enterprise proxy restrictions.
- In `listen` mode, the SSH client sees `127.0.0.1` as the host; verify the expected SSH host key when prompted. Existing host key records for the remote host may need deliberate configuration.
- Existing SSH sessions cannot resume if the WSS connection drops.

## Notes

- This project implements a WSS **client**, not a WebSocket server.
- The code has not been compiled or integration-tested in this environment. Run `cargo build --release` and validate with your own server before relying on it.
- For Windows hosts requiring an explicit corporate HTTP CONNECT proxy, this minimal implementation would need a proxy feature added.

## Connection resilience (v0.2)

The client retries a **failed initial WSS connection**, with exponential delays
(1, 2, 4, 8, 16, 32 seconds max) and a default limit of 10 attempts. It sends
WebSocket Ping frames every 20 seconds and detects a dead tunnel if no frame
arrives for 65 seconds. A network change may still require TCP timeout or
WebSocket ping timeout to be detected.

**A dropped SSH session cannot be resumed by opening a new WebSocket**: SSH
cryptographic/session state is lost. The bridge closes the affected local SSH
connection; the user must reconnect. The `listen` process remains available for
new SSH sessions. For long-running remote shell work, use `tmux` on the server.

```powershell
sshbridge.exe listen --port 2222 --url wss://con.lli.be/ssh `
  --connect-attempts 0 --connect-timeout 8 --ping-interval 15 --idle-timeout 45
```

`--connect-attempts 0` retries indefinitely **before the tunnel is established**.
This is appropriate for `listen` mode if you want new connections to wait for
network recovery, but it does not reconnect an existing SSH session.

Tests (on a machine with Rust installed):

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Network-change test: connect via Wi-Fi, switch to mobile hotspot, observe
heartbeat timeout and SSH disconnect, then reconnect SSH; the new session
should use the restored network. Run `tmux new -As main` on the server to
recover the same remote terminal after reconnecting.

## Cross-compilation Windows depuis Arch Linux

```bash
sudo pacman -S --needed rustup mingw-w64-gcc
rustup default stable
./build-arch.sh
```

Le script compile Linux et Windows x64 (MinGW GNU), puis affiche les chemins et tailles.
La configuration `.cargo/config.toml` indique le linker Windows.
Alternative manuelle : `cargo build --release --target x86_64-pc-windows-gnu`.
Pour le poste Windows, copier `target/x86_64-pc-windows-gnu/release/sshbridge.exe`.

**Limites de validation :** sources non compilées dans l'environnement de génération ;
valider `cargo test`, le démarrage, les connexions SSH, les coupures réseau et
les reconnexions sur vos systèmes. La reconnexion ne restaure pas une session SSH interrompue.
