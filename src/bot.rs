use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context as _;
use teloxide::{
    dispatching::{
        UpdateHandler,
        dialogue::{self, InMemStorage},
    },
    net::Download,
    prelude::*,
    types::{KeyboardButton, KeyboardMarkup, KeyboardRemove},
    utils::command::BotCommands,
};
use tokio::sync::broadcast;

use crate::config::{Config, TorrentFormat};
use crate::format;
use crate::qb::QbClient;
use crate::server::EventPayload;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub qb: Arc<QbClient>,
    pub allowed_users: Arc<HashSet<i64>>,
}

// ---------------------------------------------------------------------------
// Dialogue state
// ---------------------------------------------------------------------------

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum DialogueState {
    #[default]
    Idle,
    AwaitCategory {
        paused: bool,
    },
    AwaitTorrentType {
        paused: bool,
        category: String,
        save_path: String,
    },
    AwaitTorrentInput {
        paused: bool,
        category: String,
        save_path: String,
        url_mode: bool, // true = URL/magnet, false = file
    },
}

type MyDialogue = Dialogue<DialogueState, InMemStorage<DialogueState>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

// ---------------------------------------------------------------------------
// Bot commands
// ---------------------------------------------------------------------------

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "qBittorrent bot commands")]
enum BotCommand {
    #[command(description = "Show available commands")]
    Help,
    #[command(description = "List all torrents")]
    List,
    #[command(description = "List downloading torrents")]
    Down,
    #[command(description = "List seeding torrents")]
    Up,
    #[command(description = "List paused torrents")]
    Paused,
    #[command(description = "Add a new torrent")]
    Add,
    #[command(description = "Add a torrent paused")]
    AddPaused,
    #[command(description = "Cancel the current operation")]
    Cancel,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let bot = Bot::new(&cfg.bot_token);

    let qb = Arc::new(QbClient::new(&cfg).context("Failed to create qBittorrent client")?);
    qb.login().await.context("Initial qBittorrent login failed")?;

    let allowed_users: HashSet<i64> = cfg.bot_allowed_users.iter().cloned().collect();
    let state = AppState {
        config: Arc::new(cfg.clone()),
        qb: qb.clone(),
        allowed_users: Arc::new(allowed_users),
    };

    // Broadcast channel for events from the axum server
    let (tx, _initial_rx) = broadcast::channel::<EventPayload>(32);

    // Spawn event server
    let server_tx = tx.clone();
    let port = cfg.event_server_port;
    tokio::spawn(async move {
        if let Err(e) = crate::server::run(port, server_tx).await {
            tracing::error!("Event server error: {}", e);
        }
    });

    // Spawn notification task
    {
        let bot_clone = bot.clone();
        let state_clone = state.clone();
        let rx = tx.subscribe();
        tokio::spawn(async move {
            notification_task(bot_clone, state_clone, rx).await;
        });
    }

    // Send startup message to all allowed users
    send_startup_message(&bot, &state).await;

    // Register commands with Telegram
    bot.set_my_commands(BotCommand::bot_commands()).await?;

    let storage = InMemStorage::<DialogueState>::new();

    Dispatcher::builder(bot, schema())
        .dependencies(dptree::deps![storage, state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler schema
// ---------------------------------------------------------------------------

fn schema() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    use dptree::case;
    use teloxide::filter_command;

    let command_handler = filter_command::<BotCommand, _>()
        .branch(case![BotCommand::Help].endpoint(cmd_help))
        .branch(case![BotCommand::List].endpoint(cmd_list))
        .branch(case![BotCommand::Down].endpoint(cmd_down))
        .branch(case![BotCommand::Up].endpoint(cmd_up))
        .branch(case![BotCommand::Paused].endpoint(cmd_paused))
        .branch(case![BotCommand::Add].endpoint(cmd_add))
        .branch(case![BotCommand::AddPaused].endpoint(cmd_add_paused))
        .branch(case![BotCommand::Cancel].endpoint(cmd_cancel_noop));

    let message_handler = Update::filter_message()
        .enter_dialogue::<Message, InMemStorage<DialogueState>, DialogueState>()
        .branch(
            // Idle state
            case![DialogueState::Idle]
                .branch(command_handler)
                .branch(
                    Message::filter_text()
                        .filter(|msg: Message| {
                            msg.text()
                                .map(|t| t.trim_start().to_lowercase().starts_with("magnet:?"))
                                .unwrap_or(false)
                        })
                        .endpoint(handle_direct_magnet),
                )
                .branch(Message::filter_document().endpoint(handle_direct_document)),
        )
        .branch(
            // Awaiting category
            case![DialogueState::AwaitCategory { paused }]
                .branch(
                    filter_command::<BotCommand, _>()
                        .branch(case![BotCommand::Cancel].endpoint(cmd_cancel)),
                )
                .branch(Message::filter_text().endpoint(handle_category)),
        )
        .branch(
            // Awaiting torrent type
            case![DialogueState::AwaitTorrentType { paused, category, save_path }]
                .branch(
                    filter_command::<BotCommand, _>()
                        .branch(case![BotCommand::Cancel].endpoint(cmd_cancel)),
                )
                .branch(Message::filter_text().endpoint(handle_torrent_type)),
        )
        .branch(
            // Awaiting torrent input
            case![DialogueState::AwaitTorrentInput { paused, category, save_path, url_mode }]
                .branch(
                    filter_command::<BotCommand, _>()
                        .branch(case![BotCommand::Cancel].endpoint(cmd_cancel)),
                )
                .branch(Message::filter_text().endpoint(handle_torrent_input_text))
                .branch(Message::filter_document().endpoint(handle_torrent_input_file)),
        );

    dialogue::enter::<Update, InMemStorage<DialogueState>, DialogueState, _>()
        .branch(message_handler)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_authorized(msg: &Message, state: &AppState) -> bool {
    msg.from
        .as_ref()
        .map(|u| state.allowed_users.contains(&(u.id.0 as i64)))
        .unwrap_or(false)
}

fn persistent_keyboard() -> KeyboardMarkup {
    KeyboardMarkup::new(vec![
        vec![
            KeyboardButton::new("/add"),
            KeyboardButton::new("/addpaused"),
            KeyboardButton::new("/list"),
        ],
        vec![
            KeyboardButton::new("/down"),
            KeyboardButton::new("/up"),
            KeyboardButton::new("/paused"),
        ],
        vec![KeyboardButton::new("/help")],
    ])
    .resize_keyboard()
    .persistent()
}

async fn send_startup_message(bot: &Bot, state: &AppState) {
    let kb = persistent_keyboard();
    for &uid in state.allowed_users.iter() {
        let result = bot
            .send_message(ChatId(uid), "☠️ qbittorrent bot is online. Send /help to get started.")
            .reply_markup(kb.clone())
            .await;
        if let Err(e) = result {
            tracing::warn!("Could not send startup message to {}: {}", uid, e);
        }
    }
}

async fn notification_task(
    bot: Bot,
    state: AppState,
    mut rx: broadcast::Receiver<EventPayload>,
) {
    loop {
        match rx.recv().await {
            Ok(payload) => {
                for &uid in state.allowed_users.iter() {
                    if let Err(e) = bot.send_message(ChatId(uid), &payload.message).await {
                        tracing::warn!("Notification to {} failed: {}", uid, e);
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Notification receiver lagged, dropped {} events", n);
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::error!("Event broadcast channel closed");
                break;
            }
        }
    }
}

async fn send_torrent_list(
    bot: &Bot,
    msg: &Message,
    state: &AppState,
    filter: Option<&[&str]>,
) -> HandlerResult {
    let torrents = match state.qb.list_torrents(filter).await {
        Ok(t) => t,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("qBittorrent error: {}", e)).await?;
            return Ok(());
        }
    };
    let detailed = state.config.torrent_format == TorrentFormat::Detailed;
    let text = format::format_list(&torrents, state.config.torrent_list_limit, detailed);
    for chunk in format::chunk_text(&text, 4096) {
        bot.send_message(msg.chat.id, chunk).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command handlers — stateless
// ---------------------------------------------------------------------------

async fn cmd_help(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    let text = "Available Commands:\n\n\
        /add — Add a new torrent\n\
        /addpaused — Add a new torrent paused\n\
        /list — List all torrents\n\
        /down — List downloading torrents\n\
        /up — List seeding torrents\n\
        /paused — List paused torrents\n\
        /cancel — Cancel the current operation\n\
        /help — Show this help\n\n\
        You can also send a magnet link or .torrent file directly.";
    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}

async fn cmd_list(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    send_torrent_list(&bot, &msg, &state, None).await
}

async fn cmd_down(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    send_torrent_list(&bot, &msg, &state, Some(&["downloading", "forcedDL"])).await
}

async fn cmd_up(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    send_torrent_list(&bot, &msg, &state, Some(&["uploading", "forcedUP"])).await
}

async fn cmd_paused(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    send_torrent_list(&bot, &msg, &state, Some(&["pausedDL", "pausedUP"])).await
}

async fn cmd_cancel_noop(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    // /cancel outside a conversation — no-op with friendly message
    bot.send_message(msg.chat.id, "No active operation to cancel.")
        .reply_markup(persistent_keyboard())
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Direct magnet / .torrent outside conversations
// ---------------------------------------------------------------------------

async fn handle_direct_magnet(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    let text = msg.text().unwrap_or("").trim().to_string();
    match state.qb.add_torrent_url(&text, None, false, None).await {
        Ok(m) => bot.send_message(msg.chat.id, format!("✅ {}", m)).await?,
        Err(e) => bot.send_message(msg.chat.id, format!("❌ {}", e)).await?,
    };
    Ok(())
}

async fn handle_direct_document(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    let doc = match msg.document() {
        Some(d) if d.file_name.as_deref().unwrap_or("").to_lowercase().ends_with(".torrent") => d,
        _ => return Ok(()), // not a .torrent — ignore silently
    };
    let file = bot.get_file(&doc.file.id).await?;
    let mut buf = Vec::new();
    bot.download_file(&file.path, &mut buf).await?;
    match state.qb.add_torrent_file(buf, None, false, None).await {
        Ok(m) => bot.send_message(msg.chat.id, format!("✅ {}", m)).await?,
        Err(e) => bot.send_message(msg.chat.id, format!("❌ {}", e)).await?,
    };
    Ok(())
}

// ---------------------------------------------------------------------------
// /add and /addpaused — conversation entry
// ---------------------------------------------------------------------------

async fn cmd_add(bot: Bot, dialogue: MyDialogue, msg: Message, state: AppState) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    show_categories(&bot, &dialogue, &msg, &state, false).await
}

async fn cmd_add_paused(
    bot: Bot,
    dialogue: MyDialogue,
    msg: Message,
    state: AppState,
) -> HandlerResult {
    if !is_authorized(&msg, &state) {
        return Ok(());
    }
    show_categories(&bot, &dialogue, &msg, &state, true).await
}

async fn show_categories(
    bot: &Bot,
    dialogue: &MyDialogue,
    msg: &Message,
    state: &AppState,
    paused: bool,
) -> HandlerResult {
    let cats = &state.config.categories;
    let kb_rows: Vec<Vec<KeyboardButton>> = cats
        .iter()
        .map(|c| vec![KeyboardButton::new(&c.name)])
        .collect();
    let kb = KeyboardMarkup::new(if kb_rows.is_empty() {
        vec![vec![KeyboardButton::new("Default")]]
    } else {
        kb_rows
    })
    .one_time_keyboard()
    .resize_keyboard();

    bot.send_message(msg.chat.id, "Choose a save location:")
        .reply_markup(kb)
        .await?;
    dialogue.update(DialogueState::AwaitCategory { paused }).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Conversation: category → type → input
// ---------------------------------------------------------------------------

async fn handle_category(
    bot: Bot,
    dialogue: MyDialogue,
    msg: Message,
    state: AppState,
    paused: bool,
) -> HandlerResult {
    let text = msg.text().unwrap_or("").trim().to_string();
    let (category, save_path) = state
        .config
        .categories
        .iter()
        .find(|c| c.name == text)
        .map(|c| (c.name.clone(), c.save_path.clone()))
        .unwrap_or_else(|| (text.clone(), String::new()));

    let kb = KeyboardMarkup::new(vec![vec![
        KeyboardButton::new("Magnet/URL"),
        KeyboardButton::new(".torrent File"),
    ]])
    .one_time_keyboard()
    .resize_keyboard();

    bot.send_message(msg.chat.id, "Magnet / URL   or   .torrent file?")
        .reply_markup(kb)
        .await?;
    dialogue
        .update(DialogueState::AwaitTorrentType {
            paused,
            category,
            save_path,
        })
        .await?;
    Ok(())
}

async fn handle_torrent_type(
    bot: Bot,
    dialogue: MyDialogue,
    msg: Message,
    // case! returns a tuple for multi-field variants; destructure it here
    (paused, category, save_path): (bool, String, String),
) -> HandlerResult {
    let text = msg.text().unwrap_or("").trim();
    let url_mode = text.starts_with("Magnet") || text.starts_with("magnet");
    let prompt = if url_mode {
        "Paste a magnet link or HTTP(s) URL to a .torrent file."
    } else {
        "Send the .torrent file as a document."
    };
    bot.send_message(msg.chat.id, prompt)
        .reply_markup(KeyboardRemove::new())
        .await?;
    dialogue
        .update(DialogueState::AwaitTorrentInput {
            paused,
            category,
            save_path,
            url_mode,
        })
        .await?;
    Ok(())
}

async fn handle_torrent_input_text(
    bot: Bot,
    dialogue: MyDialogue,
    msg: Message,
    state: AppState,
    // case! returns a tuple for multi-field variants; destructure it here
    (paused, category, save_path, url_mode): (bool, String, String, bool),
) -> HandlerResult {
    if !url_mode {
        bot.send_message(
            msg.chat.id,
            "Please send a .torrent file, or /cancel to abort.",
        )
        .await?;
        return Ok(());
    }
    let source = msg.text().unwrap_or("").trim().to_string();
    let save = if save_path.is_empty() { None } else { Some(save_path.as_str()) };
    let cat = if category.is_empty() { None } else { Some(category.as_str()) };
    match state.qb.add_torrent_url(&source, save, paused, cat).await {
        Ok(m) => {
            bot.send_message(msg.chat.id, format!("✅ {}", m))
                .reply_markup(persistent_keyboard())
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ {}", e))
                .reply_markup(persistent_keyboard())
                .await?;
        }
    }
    dialogue.exit().await?;
    Ok(())
}

async fn handle_torrent_input_file(
    bot: Bot,
    dialogue: MyDialogue,
    msg: Message,
    state: AppState,
    // case! returns a tuple for multi-field variants; destructure it here
    (paused, category, save_path, url_mode): (bool, String, String, bool),
) -> HandlerResult {
    if url_mode {
        bot.send_message(
            msg.chat.id,
            "Please paste a magnet link or URL, or /cancel to abort.",
        )
        .await?;
        return Ok(());
    }
    let doc = match msg.document() {
        Some(d) if d.file_name.as_deref().unwrap_or("").to_lowercase().ends_with(".torrent") => d,
        _ => {
            bot.send_message(
                msg.chat.id,
                "That doesn't look like a .torrent file. Try again or /cancel.",
            )
            .await?;
            return Ok(());
        }
    };
    let file = bot.get_file(&doc.file.id).await?;
    let mut buf = Vec::new();
    bot.download_file(&file.path, &mut buf).await?;

    let save = if save_path.is_empty() { None } else { Some(save_path.as_str()) };
    let cat = if category.is_empty() { None } else { Some(category.as_str()) };
    match state.qb.add_torrent_file(buf, save, paused, cat).await {
        Ok(m) => {
            bot.send_message(msg.chat.id, format!("✅ {}", m))
                .reply_markup(persistent_keyboard())
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("❌ {}", e))
                .reply_markup(persistent_keyboard())
                .await?;
        }
    }
    dialogue.exit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Cancel (in-conversation)
// ---------------------------------------------------------------------------

async fn cmd_cancel(bot: Bot, dialogue: MyDialogue, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Operation cancelled.")
        .reply_markup(KeyboardRemove::new())
        .await?;
    dialogue.exit().await?;
    Ok(())
}
