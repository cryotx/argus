# Setup Guide

This guide is the first-run onboarding for Argus.

## What you need to install

Required:

- Rust toolchain (`cargo`, `rustc`) 1.85+
- PostgreSQL with `pgvector`
- An installed and authenticated Codex CLI (default) or Claude Code CLI
- Node.js 20.19+ or 22.12+ if you want to run the dashboard

Optional but useful:

- `psql` / `pg_isready` for local DB checks
- `curl` for local API testing

Not required to start:

- Docker (only needed for your own deploy flow)
- Frontend toolchains (you can use API/webhooks first)
- All chat platforms at once (start with one)

## One-command setup

From repo root:

```bash
./scripts/setup.sh
```

This will:

- check core tools
- create missing config files
  - `config/argus.toml`
  - `config/providers.toml`
  - `.env`
- print the next commands to migrate/run/test

## Minimum configuration to run

Edit `.env` and fill at least:

- `ARGUS_DATABASE_URL`
- `ARGUS_MASTER_KEY`
- `ARGUS_HTTP_ADMIN_TOKEN` (a random token of at least 32 ASCII characters)
- `ARGUS_DEFAULT_WORKER` (`codex` or `claude_code`)

For local development, set `ARGUS_ENV=development`. The coding Agent uses its
own CLI login. API-backed chat is optional in development; configure the provider
and its corresponding environment key in `config/providers.toml` when needed.
Production startup also validates that provider configuration and credentials
are present.

If you use chat platforms, also fill related envs:

- Telegram: `ARGUS_TELEGRAM_BOT_TOKEN`
- Feishu: `ARGUS_FEISHU_WEBHOOK_URL`
- Google Chat: `ARGUS_GOOGLE_CHAT_WEBHOOK_URL`
- WhatsApp: `ARGUS_WHATSAPP_ACCESS_TOKEN`, `ARGUS_WHATSAPP_PHONE_NUMBER_ID`
- QQ: `ARGUS_QQ_APP_ID`, `ARGUS_QQ_APP_SECRET`, `ARGUS_QQ_BOT_SECRET`

QQ can also be configured in `config/argus.toml`:

```toml
[social.qq]
enabled = true
app_id = "your-qq-app-id"
app_secret = "<qq-app-secret>"
bot_secret = "<qq-bot-secret>"
api_base_url = "https://api.sgroup.qq.com"
token_url = "https://bots.qq.com/app/getAppAccessToken"
```

Environment variables still override file values.

## Start sequence

```bash
cargo run -p argus-cli -- migrate
cargo run -p argus-cli -- run
```

## Quick smoke test

```bash
curl http://127.0.0.1:3100/api/status
```

This read-only check needs no chat provider or HTTP token. HTTP write requests
require the appropriate `Authorization: Bearer ...` token. If you use a token
variable in a terminal command, export it into that shell first; the CLI's
`.env` loading does not change the parent shell environment.
