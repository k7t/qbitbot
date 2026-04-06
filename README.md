# qbitbot

A Telegram bot for managing qBittorrent, written in Rust. Uses qBittorrent's built-in "Run external program" hooks for instant add/complete notifications — no polling required.

## Features

- Add torrents via magnet link, HTTP URL, or `.torrent` file
- Multi-category support with configurable save paths
- List torrents filtered by state (all / downloading / seeding / paused)
- Instant Telegram notifications when torrents are added or finish — triggered directly by qBittorrent, not a polling loop
- User whitelist — only configured Telegram user IDs can interact with the bot
- Persistent quick-access keyboard in chat

## How It Works

qbitbot runs as two things at once:

1. **A long-running Telegram bot** that also opens a small HTTP server on `localhost:9091`
2. **A one-shot CLI trigger** (`qbitbot notify "..."`) that qBittorrent calls when a torrent event fires

When qBittorrent adds or finishes a torrent, it executes `qbitbot notify "..."` with a message you configure. That invocation POSTs to the running bot's local server, which immediately forwards the message to all authorised Telegram users.

```
qBittorrent event
    └─▶ qbitbot notify "🎉 Torrent complete: %N"
            └─▶ POST localhost:9091/event
                    └─▶ Telegram message sent instantly
```

## Requirements

- Rust toolchain (MSVC on Windows, stable on Linux/macOS)
- qBittorrent with Web UI enabled
- A Telegram bot token from [@BotFather](https://t.me/BotFather)
- Your Telegram user ID (get it from [@userinfobot](https://t.me/userinfobot))

## Installation

### 1. Clone and build

```bash
git clone <repo-url>
cd qbitbot
cargo build --release
```

The binary is at `target/release/qbitbot` (or `qbitbot.exe` on Windows).

To install system-wide:

```bash
cargo install --path .
```

### 2. Configure

Copy the example config and fill in your details:

```bash
cp config.json.example config.json
```

Edit `config.json`:

```json
{
    "qb_url": "http://localhost:8080",
    "qb_username": "admin",
    "qb_password": "adminadmin",
    "bot_token": "YOUR_BOT_TOKEN",
    "bot_allowed_users": [123456789],
    "torrent_list_limit": 10,
    "torrent_format": "detailed",
    "categories": [
        {"name": "Movies",   "save_path": "/path/to/movies"},
        {"name": "TV",       "save_path": "/path/to/tv"},
        {"name": "Music",    "save_path": "/path/to/music"},
        {"name": "Default",  "save_path": ""}
    ],
    "event_server_port": 9091
}
```

| Field | Default | Description |
|---|---|---|
| `qb_url` | `http://localhost:8080` | qBittorrent Web UI URL |
| `qb_username` | `admin` | qBittorrent login |
| `qb_password` | `adminadmin` | qBittorrent password |
| `bot_token` | *(required)* | Telegram bot token |
| `bot_allowed_users` | *(required)* | Array of allowed Telegram user IDs |
| `torrent_list_limit` | `10` | Max torrents shown in list commands |
| `torrent_format` | `"detailed"` | `"detailed"` or `"brief"` |
| `categories` | Default only | Named download locations |
| `event_server_port` | `9091` | Internal IPC server port (loopback only) |

### 3. Configure qBittorrent hooks

Open qBittorrent and go to **Tools → Options → Downloads**.

Set the **"Run external program on torrent added"** field to:

```
"/full/path/to/qbitbot" notify "✅ Torrent added: %N"
```

Set the **"Run external program on torrent finished"** field to:

```
"/full/path/to/qbitbot" notify "🎉 Torrent complete: %N (%Z bytes)"
```

Replace `/full/path/to/qbitbot` with the actual path to the binary. On Windows use the full path with `.exe`.

qBittorrent expands `%N` (torrent name), `%Z` (size in bytes), `%I` (info hash), and other tokens before calling the binary. The resulting string is forwarded directly as the Telegram notification message.

> **Note:** The `event_server_port` is a loopback-only HTTP server used for IPC between the qBittorrent hook invocation and the running bot. It is not a qBittorrent port and is not reachable from the network.

### 4. Run the bot

```bash
./target/release/qbitbot --config /path/to/config.json
```

On first start, the bot sends a welcome message to all configured users and registers its commands with Telegram.

## Bot Commands

| Command | Description |
|---|---|
| `/add` | Add a torrent — prompts for category, then magnet/URL or `.torrent` file |
| `/addpaused` | Same as `/add` but torrent starts paused |
| `/list` | List all torrents |
| `/down` | List downloading torrents |
| `/up` | List seeding torrents |
| `/paused` | List paused torrents |
| `/cancel` | Cancel an in-progress `/add` or `/addpaused` |
| `/help` | Show command list |

You can also paste a magnet link or send a `.torrent` file directly in chat without going through `/add`.

## Deployment

### Linux — systemd

Create `/etc/systemd/system/qbitbot.service`:

```ini
[Unit]
Description=qBittorrent Telegram Bot
After=network.target

[Service]
Type=simple
User=YOUR_USER
WorkingDirectory=/opt/qbitbot
ExecStart=/opt/qbitbot/qbitbot --config /opt/qbitbot/config.json
Restart=on-failure
RestartSec=10
Environment=RUST_LOG=qbitbot=info

[Install]
WantedBy=multi-user.target
```

Then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now qbitbot
sudo systemctl status qbitbot
```

View logs:

```bash
journalctl -u qbitbot -f
```

### Windows — Task Scheduler

1. Open **Task Scheduler** and create a new task
2. **General:** Run whether user is logged on or not
3. **Triggers:** At startup
4. **Actions:** Start a program
   - Program: `C:\path\to\qbitbot.exe`
   - Arguments: `--config C:\path\to\config.json`
5. **Settings:** Restart if the task fails, stop the existing instance before starting a new one

Alternatively, install as a Windows Service using [NSSM](https://nssm.cc/):

```cmd
nssm install qbitbot C:\path\to\qbitbot.exe
nssm set qbitbot AppParameters --config C:\path\to\config.json
nssm set qbitbot AppEnvironmentExtra RUST_LOG=qbitbot=info
nssm start qbitbot
```

## Testing

Test the notify IPC manually while the bot is running:

```bash
qbitbot --config config.json notify "🧪 Test message"
```

The message should appear in Telegram within a second.

Test the bot without running qBittorrent hooks:

```bash
RUST_LOG=qbitbot=debug cargo run -- --config config.json
```

## Logging

Set the `RUST_LOG` environment variable to control log verbosity:

```bash
RUST_LOG=qbitbot=info   # normal operation (default)
RUST_LOG=qbitbot=debug  # verbose, includes all handler activity
RUST_LOG=debug          # very verbose, includes teloxide internals
```

## Configuration Notes

- `config.json` is gitignored — keep your credentials out of version control
- Only one instance of the bot can run at a time (the event server binds to a fixed port)
- If the bot is stopped when qBittorrent fires a hook, the notification is silently dropped (the `notify` subcommand exits cleanly on connection refused)
- In-progress `/add` conversations are lost on bot restart (stored in memory only)
