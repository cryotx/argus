# Chat Integrations

This document describes practical chat-platform and model integrations for Argus.

## Platform adapters

Implemented in `argus-social`:

- `TelegramAdapter`: real send via Telegram Bot API (`sendMessage`)
- `FeishuAdapter`: send via Feishu incoming webhook
- `GoogleChatAdapter`: send via Google Chat incoming webhook
- `WhatsAppAdapter`: send via WhatsApp Cloud API (Meta Graph)
- `WebDashboardAdapter`: local web message normalization and pass-through
- `QqAdapter`: QQ Bot API v2 webhook + C2C send/receive (minimal)

### Adapter setup cheat sheet

- Telegram
  - Required: `bot_token`
  - Send target: `chat_id`
- Feishu
  - Required: `webhook_url`
  - Send target: ignored for webhook mode
- Google Chat
  - Required: `webhook_url`
  - Send target: ignored for webhook mode
- WhatsApp
  - Required: `access_token`, `phone_number_id`
  - Send target: recipient phone number (E.164)
- QQ
  - Required: `app_id`, `app_secret`, `bot_secret`
  - Optional env switch: `ARGUS_QQ_ENABLED=true`
  - Send target: `user_openid` (C2C)

## LLM providers

- Recommended first wave: ChatGPT + Qwen
- Use `openai_compatible` provider type for both
- Example file: `config/providers.example.toml`

### About Copilot

GitHub Copilot does not currently expose a stable, general-purpose public Chat API equivalent to OpenAI-compatible provider integrations for self-hosted bot routing. Keep Copilot out of the provider chain unless a supported API becomes available.

## Suggested rollout order

1. Telegram + ChatGPT
2. Add Qwen failover
3. Add Feishu/Google Chat
4. Add WhatsApp Cloud API
5. Add QQ webhook + C2C

## Runtime endpoints

When `argus run` is started, these endpoints are exposed on `server.host:server.port`:

- `POST /api/chat`
  - Body: `{ "session_id": "s1", "user_id": "u1", "text": "hello" }`
- `POST /webhook/telegram`
- `POST /webhook/feishu`
- `POST /webhook/googlechat`
- `POST /webhook/whatsapp`
- `POST /webhook/qq`

All inbound routes normalize payloads into a unified social message format, route to LLM, and send replies through the corresponding adapter.
