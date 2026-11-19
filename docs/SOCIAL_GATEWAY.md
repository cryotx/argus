# Social Gateway — Detailed Design

> **Crate**: `argus-social`

The Social Gateway is Argus's communication layer. It provides a unified interface for receiving commands and sending notifications across all supported social platforms (Telegram, Discord, WeChat, Slack, Web Dashboard).

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Social Gateway                               │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Unified Message Protocol                       │ │
│  │  All platforms normalized to UnifiedMessage format           │ │
│  └────────────────────────┬───────────────────────────────────┘ │
│                           │                                      │
│  ┌─────────┐ ┌──────────┐ │ ┌──────────┐ ┌──────┐ ┌──────────┐│
│  │Telegram │ │Discord   │ │ │WeChat    │ │Slack │ │Web       ││
│  │Adapter  │ │Adapter   │ │ │Adapter   │ │Adapt.│ │Dashboard ││
│  │(teloxide│ │(serenity/│ │ │(Enterprise│ │(API) │ │(axum+ws) ││
│  │)        │ │poise)    │   │WeChat)   │ │      │ │          ││
│  └─────────┘ └──────────┘   └──────────┘ └──────┘ └──────────┘│
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Notification Engine                            │ │
│  │  Smart routing based on owner preferences                   │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Multi-Turn Dialog Manager                      │ │
│  │  State machine for complex conversations                    │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Owner Profile Manager                          │ │
│  │  Per-owner preferences, active hours, style                 │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Message Buffer                                 │ │
│  │  Queue messages during inactive hours                       │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## 2. Unified Message Protocol

### 2.1 Message Types

```rust
struct UnifiedMessage {
    id: Uuid,
    
    // Source
    platform: Platform,
    platform_message_id: String,
    sender: MessageSender,
    channel: MessageChannel,
    
    // Content
    content: MessageContent,
    
    // Threading
    reply_to: Option<Uuid>,
    thread_id: Option<String>,
    
    // Metadata
    received_at: DateTime<Utc>,
    raw_payload: serde_json::Value,
}

enum Platform {
    Telegram,
    Discord,
    WeChat,
    Slack,
    WebDashboard,
    Internal,  // System-generated
}

struct MessageSender {
    owner_id: Uuid,
    platform_user_id: String,
    display_name: String,
    role: Role,
}

enum MessageContent {
    Text(String),
    Command { name: String, args: Vec<String> },
    File { name: String, data: Vec<u8>, mime_type: String },
    Image { data: Vec<u8>, caption: Option<String> },
    Button { callback_id: String, data: String },
    Reaction { emoji: String, target_message_id: Uuid },
}

enum MessageChannel {
    DirectMessage,
    ProjectChannel(Uuid),   // Project-specific channel
    AlertsChannel,
    GeneralChannel,
}
```

### 2.2 Outgoing Message

```rust
struct OutgoingMessage {
    /// Target platform (or all)
    target: MessageTarget,
    
    /// Content
    content: OutgoingContent,
    
    /// Priority (affects delivery timing)
    priority: NotificationPriority,
    
    /// Related project
    project_id: Option<Uuid>,
    
    /// Should this message be buffered if owner is inactive?
    bufferable: bool,
}

enum MessageTarget {
    /// Send to specific owner on specific platform
    Owner { owner_id: Uuid, platform: Platform },
    
    /// Send to owner's preferred platform
    OwnerPreferred { owner_id: Uuid },
    
    /// Send to all connected platforms
    Broadcast,
    
    /// Send to project-specific channel
    ProjectChannel { project_id: Uuid },
}

enum OutgoingContent {
    /// Simple text message
    Text(String),
    
    /// Formatted message with sections
    Report {
        title: String,
        sections: Vec<ReportSection>,
    },
    
    /// Message with action buttons
    Interactive {
        text: String,
        buttons: Vec<Button>,
    },
    
    /// Progress update
    Progress {
        task_name: String,
        progress_pct: f32,
        details: Option<String>,
    },
    
    /// Approval request
    ApprovalRequest {
        action: String,
        description: String,
        action_id: Uuid,
        timeout: Duration,
    },
    
    /// File attachment
    File {
        name: String,
        data: Vec<u8>,
        mime_type: String,
        caption: Option<String>,
    },
}

struct Button {
    label: String,
    callback_id: String,
    style: ButtonStyle,
}

enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
}
```

## 3. Platform Adapters

### 3.1 Telegram Adapter

```rust
struct TelegramAdapter {
    bot: teloxide::Bot,
    owner_chat_ids: HashMap<Uuid, i64>,  // owner_id → chat_id
}

impl TelegramAdapter {
    async fn start(&self) {
        let handler = dptree::entry()
            .branch(Update::filter_message().endpoint(self.handle_message))
            .branch(Update::filter_callback_query().endpoint(self.handle_callback));
        
        Dispatcher::builder(self.bot.clone(), handler)
            .build()
            .dispatch()
            .await;
    }
    
    async fn handle_message(&self, msg: teloxide::types::Message) -> Result<()> {
        let unified = UnifiedMessage {
            id: Uuid::new_v4(),
            platform: Platform::Telegram,
            platform_message_id: msg.id.to_string(),
            sender: self.resolve_sender(&msg),
            channel: MessageChannel::DirectMessage,
            content: self.parse_content(&msg),
            received_at: Utc::now(),
            raw_payload: serde_json::to_value(&msg)?,
            ..Default::default()
        };
        
        self.event_bus.emit(ArgusEvent::MessageReceived(unified)).await;
        Ok(())
    }
    
    async fn send(&self, msg: &OutgoingMessage) -> Result<()> {
        let chat_id = self.resolve_chat_id(&msg.target);
        
        match &msg.content {
            OutgoingContent::Text(text) => {
                self.bot.send_message(chat_id, text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await?;
            }
            OutgoingContent::Interactive { text, buttons } => {
                let keyboard = InlineKeyboardMarkup::new(
                    buttons.iter().map(|b| {
                        vec![InlineKeyboardButton::callback(&b.label, &b.callback_id)]
                    }).collect::<Vec<_>>()
                );
                self.bot.send_message(chat_id, text)
                    .reply_markup(keyboard)
                    .await?;
            }
            OutgoingContent::ApprovalRequest { action, description, action_id, .. } => {
                let text = format!("🔔 需要审批\n\n{}\n\n{}", action, description);
                let keyboard = InlineKeyboardMarkup::new(vec![
                    vec![
                        InlineKeyboardButton::callback("✅ 批准", format!("approve:{}", action_id)),
                        InlineKeyboardButton::callback("❌ 拒绝", format!("reject:{}", action_id)),
                    ],
                ]);
                self.bot.send_message(chat_id, text)
                    .reply_markup(keyboard)
                    .await?;
            }
            OutgoingContent::File { name, data, caption, .. } => {
                let file = InputFile::memory(data.clone()).file_name(name.clone());
                self.bot.send_document(chat_id, file)
                    .caption(caption.as_deref().unwrap_or(""))
                    .await?;
            }
            _ => {
                // Format other types as text
                let text = self.format_as_text(&msg.content);
                self.bot.send_message(chat_id, text).await?;
            }
        }
        
        Ok(())
    }
}
```

### 3.2 Discord Adapter

```rust
struct DiscordAdapter {
    client: serenity::Client,
    guild_id: GuildId,
    channels: DiscordChannels,
}

struct DiscordChannels {
    general: ChannelId,
    alerts: ChannelId,
    reports: ChannelId,
    control: ChannelId,
    per_project: HashMap<Uuid, ChannelId>,  // project_id → channel
}

impl DiscordAdapter {
    async fn send(&self, msg: &OutgoingMessage) -> Result<()> {
        let channel_id = self.resolve_channel(&msg);
        
        match &msg.content {
            OutgoingContent::Report { title, sections } => {
                // Use Discord embeds for rich formatting
                channel_id.send_message(&self.client.http, CreateMessage::default()
                    .embed(CreateEmbed::default()
                        .title(title)
                        .fields(sections.iter().map(|s| {
                            (s.title.clone(), s.content.clone(), s.inline)
                        }))
                        .color(self.priority_color(&msg.priority))
                    )
                ).await?;
            }
            OutgoingContent::Interactive { text, buttons } => {
                channel_id.send_message(&self.client.http, CreateMessage::default()
                    .content(text)
                    .components(vec![CreateActionRow::Buttons(
                        buttons.iter().map(|b| {
                            CreateButton::new(&b.callback_id)
                                .label(&b.label)
                                .style(match b.style {
                                    ButtonStyle::Primary => serenity::model::application::ButtonStyle::Primary,
                                    ButtonStyle::Secondary => serenity::model::application::ButtonStyle::Secondary,
                                    ButtonStyle::Danger => serenity::model::application::ButtonStyle::Danger,
                                })
                        }).collect()
                    )])
                ).await?;
            }
            _ => {
                let text = self.format_as_text(&msg.content);
                channel_id.say(&self.client.http, text).await?;
            }
        }
        
        Ok(())
    }
    
    /// Auto-create per-project channels
    async fn ensure_project_channel(&mut self, project_id: Uuid, project_name: &str) -> ChannelId {
        if let Some(channel) = self.channels.per_project.get(&project_id) {
            return *channel;
        }
        
        let channel = self.guild_id.create_channel(&self.client.http, CreateChannel::new(
            &format!("project-{}", project_name.to_lowercase().replace(' ', '-"))
        ).kind(ChannelType::Text)).await.unwrap();
        
        self.channels.per_project.insert(project_id, channel.id);
        channel.id
    }
}
```

### 3.3 WeChat Adapter

```rust
struct WeChatAdapter {
    /// Enterprise WeChat API (企业微信)
    api: WeChatEnterpriseApi,
    
    /// Alternative: WeChatFerry bridge for personal WeChat
    bridge: Option<WeChatFerryBridge>,
}

struct WeChatEnterpriseApi {
    corp_id: String,
    agent_id: String,
    secret: SecretString,
    access_token: RwLock<String>,
    token_expires_at: RwLock<DateTime<Utc>>,
}

impl WeChatAdapter {
    async fn send(&self, msg: &OutgoingMessage) -> Result<()> {
        let user_id = self.resolve_user(&msg.target);
        
        match &msg.content {
            OutgoingContent::Text(text) => {
                self.api.send_text(&user_id, text).await?;
            }
            OutgoingContent::Report { title, sections } => {
                // Use WeChat's markdown card
                let markdown = format!(
                    "# {}\n\n{}",
                    title,
                    sections.iter()
                        .map(|s| format!("**{}**\n{}", s.title, s.content))
                        .collect::<Vec<_>>().join("\n\n")
                );
                self.api.send_markdown(&user_id, &markdown).await?;
            }
            OutgoingContent::Interactive { text, buttons } => {
                // WeChat supports template cards with buttons
                self.api.send_template_card(&user_id, text, buttons).await?;
            }
            _ => {
                let text = self.format_as_text(&msg.content);
                self.api.send_text(&user_id, &text).await?;
            }
        }
        
        Ok(())
    }
}
```

### 3.4 Slack Adapter

```rust
struct SlackAdapter {
    client: slack_morphism::SlackClient,
    token: SecretString,
    channels: SlackChannels,
}

impl SlackAdapter {
    async fn send(&self, msg: &OutgoingMessage) -> Result<()> {
        let channel = self.resolve_channel(&msg);
        
        match &msg.content {
            OutgoingContent::Report { title, sections } => {
                // Use Slack Block Kit for rich formatting
                let blocks = vec![
                    SlackBlock::Header(SlackHeaderBlock::new(title.into())),
                ];
                for section in sections {
                    blocks.push(SlackBlock::Section(
                        SlackSectionBlock::new()
                            .with_text(format!("*{}*\n{}", section.title, section.content).into())
                    ));
                    blocks.push(SlackBlock::Divider(SlackDividerBlock::new()));
                }
                
                self.client.chat_post_message(&SlackApiChatPostMessageRequest::new(
                    channel,
                    SlackMessageContent::new().with_blocks(blocks),
                )).await?;
            }
            OutgoingContent::Interactive { text, buttons } => {
                let actions = SlackBlock::Actions(SlackActionsBlock::new(
                    buttons.iter().map(|b| {
                        SlackBlockElement::Button(
                            SlackBlockButtonElement::new(b.callback_id.clone().into(), b.label.clone().into())
                        )
                    }).collect()
                ));
                
                self.client.chat_post_message(&SlackApiChatPostMessageRequest::new(
                    channel,
                    SlackMessageContent::new()
                        .with_text(text.clone())
                        .with_blocks(vec![
                            SlackBlock::Section(SlackSectionBlock::new().with_text(text.clone().into())),
                            actions,
                        ]),
                )).await?;
            }
            _ => {
                let text = self.format_as_text(&msg.content);
                self.client.chat_post_message(&SlackApiChatPostMessageRequest::new(
                    channel,
                    SlackMessageContent::new().with_text(text),
                )).await?;
            }
        }
        
        Ok(())
    }
}
```

### 3.5 Web Dashboard Adapter

```rust
struct WebDashboardAdapter {
    /// Active WebSocket connections
    connections: Arc<RwLock<HashMap<Uuid, WebSocketSender>>>,
}

impl WebDashboardAdapter {
    /// Setup axum routes
    fn routes(&self) -> Router {
        Router::new()
            .route("/ws", get(self.handle_websocket))
            .route("/api/messages", get(self.get_messages))
            .route("/api/messages", post(self.send_message))
            .route("/api/projects", get(self.get_projects))
            .route("/api/projects/:id", get(self.get_project))
            .route("/api/tasks", get(self.get_tasks))
            .route("/api/workers", get(self.get_workers))
            .route("/api/stats", get(self.get_stats))
    }
    
    async fn handle_websocket(&self, ws: WebSocketUpgrade, user: AuthenticatedUser) -> Response {
        ws.on_upgrade(|socket| async move {
            let (sender, mut receiver) = socket.split();
            
            // Register connection
            self.connections.write().await.insert(user.owner_id, sender);
            
            // Handle incoming messages
            while let Some(Ok(msg)) = receiver.next().await {
                if let Message::Text(text) = msg {
                    let unified = UnifiedMessage {
                        platform: Platform::WebDashboard,
                        content: MessageContent::Text(text),
                        sender: MessageSender {
                            owner_id: user.owner_id,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    self.event_bus.emit(ArgusEvent::MessageReceived(unified)).await;
                }
            }
            
            // Cleanup on disconnect
            self.connections.write().await.remove(&user.owner_id);
        })
    }
    
    async fn send(&self, msg: &OutgoingMessage) -> Result<()> {
        let owner_id = self.resolve_owner(&msg.target);
        
        let ws_message = serde_json::json!({
            "type": "message",
            "content": msg.content,
            "priority": msg.priority,
            "project_id": msg.project_id,
            "timestamp": Utc::now(),
        });
        
        if let Some(sender) = self.connections.read().await.get(&owner_id) {
            sender.send(Message::Text(ws_message.to_string())).await?;
        }
        
        Ok(())
    }
}
```

## 4. Notification Engine

### 4.1 Notification Priority

```rust
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum NotificationPriority {
    /// Background info — only show on dashboard, never push
    Background,
    
    /// Routine status, stats — batch and deliver during report time
    Low,
    
    /// Task completed, progress update — deliver based on preference
    Normal,
    
    /// Deployment failed, task blocked — deliver if owner is active
    Urgent,
    
    /// System down, data loss risk — always deliver immediately
    Critical,
}
```

### 4.2 Notification Router

```rust
struct NotificationEngine {
    adapters: HashMap<Platform, Box<dyn PlatformAdapter>>,
    owner_profiles: Arc<OwnerProfileManager>,
    message_buffer: MessageBuffer,
}

impl NotificationEngine {
    async fn notify(&self, msg: OutgoingMessage) -> Result<()> {
        let owner_id = msg.resolve_owner_id();
        let profile = self.owner_profiles.get(owner_id).await;
        
        // 1. Check if notification should be sent based on disturbance level
        if !self.should_notify(&profile, &msg) {
            if msg.bufferable {
                self.message_buffer.buffer(msg).await;
            }
            return Ok(());
        }
        
        // 2. Check active hours
        if !profile.is_active_now() && msg.priority < NotificationPriority::Urgent {
            if msg.bufferable {
                self.message_buffer.buffer(msg).await;
            }
            return Ok(());
        }
        
        // 3. Select platform
        let platform = match &msg.target {
            MessageTarget::Owner { platform, .. } => platform.clone(),
            MessageTarget::OwnerPreferred { owner_id } => {
                profile.preferred_platform.clone()
            }
            MessageTarget::Broadcast => {
                // Send to all connected platforms
                for (platform, adapter) in &self.adapters {
                    if profile.connected_platforms.contains(platform) {
                        adapter.send(&msg).await?;
                    }
                }
                return Ok(());
            }
            _ => profile.preferred_platform.clone(),
        };
        
        // 4. Send
        if let Some(adapter) = self.adapters.get(&platform) {
            adapter.send(&msg).await?;
        }
        
        Ok(())
    }
    
    fn should_notify(&self, profile: &OwnerProfile, msg: &OutgoingMessage) -> bool {
        match profile.disturbance_level {
            DisturbanceLevel::Minimal => {
                msg.priority >= NotificationPriority::Critical
            }
            DisturbanceLevel::Low => {
                msg.priority >= NotificationPriority::Urgent
            }
            DisturbanceLevel::Medium => {
                msg.priority >= NotificationPriority::Normal
            }
            DisturbanceLevel::High => {
                msg.priority >= NotificationPriority::Low
            }
            DisturbanceLevel::RealTime => {
                true // Everything
            }
        }
    }
}
```

### 4.3 Message Buffer

```rust
struct MessageBuffer {
    buffer: HashMap<Uuid, Vec<BufferedMessage>>,  // owner_id → messages
    pool: PgPool,
}

struct BufferedMessage {
    message: OutgoingMessage,
    buffered_at: DateTime<Utc>,
    deliver_after: Option<DateTime<Utc>>,
}

impl MessageBuffer {
    /// Buffer a message for later delivery
    async fn buffer(&self, msg: OutgoingMessage) {
        let owner_id = msg.resolve_owner_id();
        let buffered = BufferedMessage {
            message: msg,
            buffered_at: Utc::now(),
            deliver_after: None,
        };
        
        self.buffer.entry(owner_id).or_default().push(buffered);
        
        // Also persist to database for crash recovery
        self.persist_to_db(&buffered).await;
    }
    
    /// Flush buffered messages when owner becomes active
    async fn flush(&mut self, owner_id: Uuid, notification_engine: &NotificationEngine) {
        if let Some(messages) = self.buffer.remove(&owner_id) {
            // Group similar messages
            let grouped = self.group_messages(messages);
            
            // Send summary instead of individual messages
            let summary = self.create_summary(&grouped);
            notification_engine.notify(summary).await;
        }
    }
    
    /// Create a digest summary of buffered messages
    fn create_summary(&self, groups: &[MessageGroup]) -> OutgoingMessage {
        let sections: Vec<ReportSection> = groups.iter().map(|g| {
            ReportSection {
                title: g.category.clone(),
                content: g.messages.iter()
                    .map(|m| format!("• {}", m.brief_summary()))
                    .collect::<Vec<_>>().join("\n"),
                inline: false,
            }
        }).collect();
        
        OutgoingMessage {
            content: OutgoingContent::Report {
                title: format!("📋 离线期间有 {} 条消息", groups.iter().map(|g| g.messages.len()).sum::<usize>()),
                sections,
            },
            priority: NotificationPriority::Normal,
            bufferable: false,
            ..Default::default()
        }
    }
}
```

## 5. Owner Profile Manager

```rust
struct OwnerProfile {
    id: Uuid,
    name: String,
    role: Role,
    
    // === Platform Connections ===
    connected_platforms: Vec<Platform>,
    preferred_platform: Platform,
    platform_ids: HashMap<Platform, String>,  // platform → user_id_on_platform
    
    // === Notification Preferences ===
    disturbance_level: DisturbanceLevel,
    
    // === Active Hours ===
    timezone: chrono_tz::Tz,
    active_hours: ActiveHours,
    
    // === Interaction Style ===
    communication_style: CommunicationStyle,
    language: String,                         // "zh-CN", "en-US"
    
    // === Approval Mode ===
    approval_mode: ApprovalMode,
    
    // === Learned Preferences ===
    response_time_avg: Duration,              // How fast they typically respond
    approval_tendency: f32,                   // 0.0 (rejects everything) ~ 1.0 (approves everything)
    active_times: Vec<(u8, u8)>,              // Learned active hour ranges (hour, minute)
    preferred_report_time: Option<NaiveTime>, // When they like to receive daily reports
}

enum DisturbanceLevel {
    /// Only critical alerts (system down, data loss)
    Minimal,
    /// Urgent + critical
    Low,
    /// Normal + above
    Medium,
    /// Most notifications
    High,
    /// Everything in real-time
    RealTime,
}

enum CommunicationStyle {
    /// "部署成功。"
    Minimal,
    /// "项目X已部署到生产环境，所有健康检查通过。"
    Concise,
    /// Full details with context
    Detailed,
    /// Include technical details, logs, metrics
    Technical,
}

enum ApprovalMode {
    /// Argus does everything autonomously
    FullAuto,
    /// Notifies and waits 10 min, proceeds if no objection
    TimedAuto,
    /// Asks for approval on risky operations only
    RiskBased,
    /// Asks for approval on everything
    ManualAll,
}

struct ActiveHours {
    /// Day-of-week active hours
    schedule: HashMap<Weekday, Vec<(NaiveTime, NaiveTime)>>,
    
    /// Override: specific dates (holidays, etc.)
    overrides: HashMap<NaiveDate, Vec<(NaiveTime, NaiveTime)>>,
}

impl OwnerProfile {
    fn is_active_now(&self) -> bool {
        let now = Utc::now().with_timezone(&self.timezone);
        let today = now.weekday();
        let current_time = now.time();
        
        // Check overrides first
        if let Some(hours) = self.active_hours.overrides.get(&now.date_naive()) {
            return hours.iter().any(|(start, end)| current_time >= *start && current_time <= *end);
        }
        
        // Check regular schedule
        if let Some(hours) = self.active_hours.schedule.get(&today) {
            return hours.iter().any(|(start, end)| current_time >= *start && current_time <= *end);
        }
        
        // Default: active 8:00 - 23:00
        current_time >= NaiveTime::from_hms_opt(8, 0, 0).unwrap() && current_time <= NaiveTime::from_hms_opt(23, 0, 0).unwrap()
    }
}
```

### 5.1 Preference Learning

```rust
struct PreferenceLearner {
    memory: Arc<MemoryEngine>,
}

impl PreferenceLearner {
    /// Learn from owner's response patterns
    async fn learn_from_interaction(&self, owner_id: Uuid, interaction: &Interaction) {
        let profile = self.profiles.get_mut(owner_id).await;
        
        // Learn response time
        if let Some(response_time) = interaction.response_time {
            profile.response_time_avg = Duration::from_secs(
                ((profile.response_time_avg.as_secs() as f64 * 0.9)
                 + (response_time.as_secs() as f64 * 0.1)) as u64
            );
        }
        
        // Learn approval tendency
        match interaction.interaction_type {
            InteractionType::Approval => {
                profile.approval_tendency = profile.approval_tendency * 0.95 + 0.05;
            }
            InteractionType::Rejection => {
                profile.approval_tendency = profile.approval_tendency * 0.95;
            }
            _ => {}
        }
        
        // Learn active times
        let now = Utc::now().with_timezone(&profile.timezone);
        profile.active_times.push((now.hour() as u8, now.minute() as u8));
        
        // Store learned preference in permanent memory
        if profile.significant_change_detected() {
            self.memory.store(NewMemory {
                layer: MemoryLayer::Permanent,
                memory_type: MemoryType::OwnerPreference,
                content: format!(
                    "Owner {} preference update: disturbance={:?}, approval_tendency={:.2}, avg_response={:?}",
                    profile.name, profile.disturbance_level, profile.approval_tendency, profile.response_time_avg
                ),
                importance: 1.0,
                ..Default::default()
            }).await;
        }
    }
}
```

## 6. Multi-Turn Dialog Manager

For complex operations that require multiple steps of conversation:

```rust
struct DialogManager {
    active_dialogs: HashMap<(Uuid, Platform), DialogState>,  // (owner_id, platform) → state
}

struct DialogState {
    dialog_type: DialogType,
    current_step: usize,
    collected_data: HashMap<String, serde_json::Value>,
    started_at: DateTime<Utc>,
    timeout: Duration,
}

enum DialogType {
    ProjectCreation,
    McpInstallation,
    ConfigChange,
    Approval,
    SkillCreation,
    BugReport,
    DeploymentSetup,
}

impl DialogManager {
    /// Handle a message in the context of an active dialog
    async fn handle_dialog_message(
        &mut self,
        owner_id: Uuid,
        platform: Platform,
        message: &str,
    ) -> Option<DialogResponse> {
        let key = (owner_id, platform);
        
        let state = self.active_dialogs.get_mut(&key)?;
        
        // Check timeout
        if Utc::now() - state.started_at > state.timeout {
            self.active_dialogs.remove(&key);
            return Some(DialogResponse::Expired);
        }
        
        match state.dialog_type {
            DialogType::ProjectCreation => {
                self.handle_project_creation_step(state, message).await
            }
            DialogType::McpInstallation => {
                self.handle_mcp_installation_step(state, message).await
            }
            DialogType::ConfigChange => {
                self.handle_config_change_step(state, message).await
            }
            _ => None,
        }
    }
    
    /// Example: Multi-turn project creation dialog
    async fn handle_project_creation_step(
        &self,
        state: &mut DialogState,
        message: &str,
    ) -> Option<DialogResponse> {
        match state.current_step {
            0 => {
                // Step 1: Get project description
                state.collected_data.insert("description".into(), message.into());
                state.current_step = 1;
                Some(DialogResponse::Ask(
                    "项目类型是什么？(Web应用/小程序/API/CLI/其他)"
                ))
            }
            1 => {
                // Step 2: Get project type
                state.collected_data.insert("type".into(), message.into());
                state.current_step = 2;
                Some(DialogResponse::Ask(
                    "有偏好的技术栈吗？(如: React+Node.js, 或者让我来决定)"
                ))
            }
            2 => {
                // Step 3: Get tech stack preference
                state.collected_data.insert("tech_stack".into(), message.into());
                state.current_step = 3;
                
                // All data collected — create project
                let summary = format!(
                    "好的，我来确认一下:\n\
                     描述: {}\n\
                     类型: {}\n\
                     技术栈: {}\n\n\
                     确认创建？",
                    state.collected_data["description"],
                    state.collected_data["type"],
                    state.collected_data["tech_stack"],
                );
                Some(DialogResponse::Confirm(summary))
            }
            3 => {
                // Step 4: Confirmation
                if message.contains("确认") || message.contains("是") || message.contains("yes") {
                    Some(DialogResponse::Complete(state.collected_data.clone()))
                } else {
                    Some(DialogResponse::Cancelled)
                }
            }
            _ => None,
        }
    }
}
```

## 7. Communication Style Formatting

```rust
struct MessageFormatter;

impl MessageFormatter {
    fn format_task_completion(
        &self,
        task: &Task,
        output: &TaskOutput,
        style: CommunicationStyle,
    ) -> String {
        match style {
            CommunicationStyle::Minimal => {
                format!("{} ✓", task.title)
            }
            CommunicationStyle::Concise => {
                format!(
                    "任务完成: {}\n修改了{}个文件，测试全部通过。",
                    task.title, output.files_changed.len()
                )
            }
            CommunicationStyle::Detailed => {
                format!(
                    "✅ 任务完成\n\
                     任务: {}\n\
                     修改文件:\n{}\n\
                     测试结果: {} 通过, {} 跳过\n\
                     耗时: {}分钟\n\
                     Token消耗: {} (约${})",
                    task.title,
                    output.files_changed.iter().map(|f| format!("  - {}", f)).collect::<Vec<_>>().join("\n"),
                    output.test_results.as_ref().map_or(0, |t| t.passed),
                    output.test_results.as_ref().map_or(0, |t| t.skipped),
                    task.actual_minutes.unwrap_or(0),
                    output.tokens_used.total(),
                    output.tokens_used.estimated_cost(),
                )
            }
            CommunicationStyle::Technical => {
                format!(
                    "✅ Task Completed: {}\n\
                     ```\n\
                     Files: {:?}\n\
                     Tests: {:?}\n\
                     Commit: {}\n\
                     Worker: {:?} (attempt {}/{})\n\
                     Tokens: {} in / {} out = ${:.4}\n\
                     Duration: {:?}\n\
                     ```",
                    task.title,
                    output.files_changed,
                    output.test_results,
                    output.commit.as_ref().map_or("none", |c| &c.sha),
                    task.worker_type_hint,
                    task.attempt, task.max_attempts,
                    output.tokens_used.input, output.tokens_used.output,
                    output.tokens_used.estimated_cost(),
                    task.actual_minutes.map(|m| Duration::from_secs(m as u64 * 60)),
                )
            }
        }
    }
}
```

## 8. Configuration

> **Note**: Configuration values shown below are system-managed defaults. In production, all configuration is managed through Argus's conversational interface (see `CONVERSATIONAL_CONFIG.md`). Direct TOML file editing is not required.

```toml
# config/social.toml

[telegram]
enabled = true
bot_token = "encrypted:..."
# owner_chat_ids managed via conversation

[discord]
enabled = false
bot_token = "encrypted:..."
guild_id = "..."

[wechat]
enabled = false
corp_id = "..."
agent_id = "..."
secret = "encrypted:..."

[slack]
enabled = false
bot_token = "encrypted:..."

[web_dashboard]
enabled = true
port = 8080
host = "0.0.0.0"
auth_secret = "encrypted:..."

[notification]
default_disturbance = "medium"
buffer_inactive_messages = true
daily_report_time = "09:00"
weekly_report_day = "monday"
```
