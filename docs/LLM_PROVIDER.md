# LLM Provider Engine — Detailed Design

The LLM Provider Engine is the "fuel system" of Argus. It manages access to multiple LLM providers, handles automatic failover, optimizes costs, and ensures Argus never runs out of AI capability.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                   LLM Provider Engine                            │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Unified LLM API Layer                         │ │
│  │  fn chat(model_class, messages, options) -> Response        │ │
│  │  fn embed(text) -> Vec<f32>                                │ │
│  │  fn stream_chat(model_class, messages) -> Stream<Token>    │ │
│  └───────────────────────┬────────────────────────────────────┘ │
│                          │                                      │
│  ┌───────────────────────┴────────────────────────────────────┐ │
│  │              Provider Router                                │ │
│  │  ┌──────────────┐ ┌──────────────┐ ┌────────────────────┐ │ │
│  │  │ Model Class  │ │ Availability │ │ Cost Optimizer     │ │ │
│  │  │ Matcher      │ │ Checker      │ │                    │ │ │
│  │  └──────────────┘ └──────────────┘ └────────────────────┘ │ │
│  │  ┌──────────────┐ ┌──────────────┐ ┌────────────────────┐ │ │
│  │  │ Key Rotator  │ │ Rate Limiter │ │ Failover Ctrl      │ │ │
│  │  └──────────────┘ └──────────────┘ └────────────────────┘ │ │
│  └────────┬──────┬──────┬──────┬──────┬──────┬───────────────┘ │
│           │      │      │      │      │      │                  │
│  ┌────────┴──┐ ┌─┴────┐ ┌┴─────┐ ┌──┴───┐ ┌┴─────┐ ┌──────┐ │
│  │ OpenAI   │ │Anthr.│ │Deep- │ │Gemini│ │ Qwen │ │Zhipu │ │
│  │          │ │opic  │ │seek  │ │      │ │      │ │      │ │
│  └──────────┘ └──────┘ └──────┘ └──────┘ └──────┘ └──────┘ │
│  ┌──────────┐ ┌──────┐ ┌───────────────────────────────────┐ │
│  │Moonshot  │ │Local │ │ Web Scrapers (Zero-Cost Fallback) │ │
│  │          │ │Ollama│ │ chat.qwen.ai │ poe.com │ ...      │ │
│  └──────────┘ └──────┘ └───────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Token Budget Tracker                           │ │
│  │  Global │ Per-Provider │ Per-Project │ Per-Task             │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## 2. Provider Configuration

All providers are configured via TOML, modifiable through conversation.

> **Credentials are environment references, never literals.** `config/` is
> tracked, so anything written there is published to the repository and its
> history. Write `api_keys = ["${OPENAI_API_KEY}"]` and keep the value in the
> environment (or point `ARGUS_PROVIDERS_CONFIG_PATH` at an untracked local
> file). Both `${VAR}` and `$VAR` are resolved at startup by
> `apply_provider_env_overrides`; an unresolved placeholder becomes empty, and
> an enabled provider with no key **fails startup in production** rather than
> running without credentials. The examples below show `sk-xxx1` purely to
> illustrate the field's shape. `cargo test -p argus-cli --bin argus --
> tracked_configs_hold_no_literal_credentials` enforces this on every tracked
> config file.

```toml
# config/providers.toml

[global]
default_model_class = "coding"
max_monthly_budget_usd = 200.0
max_daily_budget_usd = 20.0
max_per_task_budget_usd = 0.50

# ============================================================
# API Providers
# ============================================================

[providers.openai]
enabled = true
type = "openai_compatible"
priority = 2
api_keys = ["sk-xxx1", "sk-xxx2", "sk-xxx3"]
base_url = "https://api.openai.com/v1"
models = [
    { id = "gpt-4o", class = "coding", cost_per_1k_input = 0.0025, cost_per_1k_output = 0.01 },
    { id = "gpt-4o-mini", class = "fast", cost_per_1k_input = 0.00015, cost_per_1k_output = 0.0006 },
    { id = "o1", class = "reasoning", cost_per_1k_input = 0.015, cost_per_1k_output = 0.06 },
    { id = "o3-mini", class = "reasoning", cost_per_1k_input = 0.0011, cost_per_1k_output = 0.0044 },
]
rate_limit = { rpm = 500, tpm = 150000 }
monthly_budget_usd = 80.0

[providers.anthropic]
enabled = true
type = "anthropic"
priority = 1
api_keys = ["sk-ant-xxx1", "sk-ant-xxx2"]
base_url = "https://api.anthropic.com"
models = [
    { id = "claude-sonnet-4-20250514", class = "coding", cost_per_1k_input = 0.003, cost_per_1k_output = 0.015 },
    { id = "claude-opus-4-20250514", class = "reasoning", cost_per_1k_input = 0.015, cost_per_1k_output = 0.075 },
    { id = "claude-haiku-3-5", class = "fast", cost_per_1k_input = 0.0008, cost_per_1k_output = 0.004 },
]
rate_limit = { rpm = 300, tpm = 100000 }
monthly_budget_usd = 80.0

[providers.deepseek]
enabled = true
type = "openai_compatible"
priority = 3
api_keys = ["sk-ds-xxx"]
base_url = "https://api.deepseek.com"
models = [
    { id = "deepseek-chat", class = "coding", cost_per_1k_input = 0.00014, cost_per_1k_output = 0.00028 },
    { id = "deepseek-reasoner", class = "reasoning", cost_per_1k_input = 0.00055, cost_per_1k_output = 0.0022 },
]
rate_limit = { rpm = 60, tpm = 60000 }
monthly_budget_usd = 20.0

[providers.gemini]
enabled = true
type = "gemini"
priority = 4
api_keys = ["AIza-xxx1", "AIza-xxx2"]
models = [
    { id = "gemini-2.5-pro", class = "reasoning", cost_per_1k_input = 0.00125, cost_per_1k_output = 0.01 },
    { id = "gemini-2.5-flash", class = "fast", cost_per_1k_input = 0.000075, cost_per_1k_output = 0.0003 },
]
monthly_budget_usd = 50.0

[providers.qwen]
enabled = true
type = "openai_compatible"
priority = 5
api_keys = ["sk-qwen-xxx"]
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
models = [
    { id = "qwen-max", class = "coding", cost_per_1k_input = 0.0016, cost_per_1k_output = 0.007 },
    { id = "qwen-plus", class = "coding", cost_per_1k_input = 0.0004, cost_per_1k_output = 0.0012 },
    { id = "qwen-turbo", class = "fast", cost_per_1k_input = 0.00003, cost_per_1k_output = 0.0006 },
]

[providers.zhipu]
enabled = true
type = "openai_compatible"
priority = 6
api_keys = ["xxx.xxx"]
base_url = "https://open.bigmodel.cn/api/paas/v4"
models = [
    { id = "glm-4-plus", class = "coding", cost_per_1k_input = 0.007, cost_per_1k_output = 0.007 },
    { id = "glm-4-flash", class = "fast", cost_per_1k_input = 0.0001, cost_per_1k_output = 0.0001 },
]

[providers.moonshot]
enabled = true
type = "openai_compatible"
priority = 7
api_keys = ["sk-moon-xxx"]
base_url = "https://api.moonshot.cn/v1"
models = [
    { id = "moonshot-v1-128k", class = "long_context", cost_per_1k_input = 0.0085, cost_per_1k_output = 0.0085 },
]

# ============================================================
# Local Models (Zero Cost)
# ============================================================

[providers.ollama]
enabled = true
type = "ollama"
priority = 50
base_url = "http://localhost:11434"
models = [
    { id = "qwen2.5:72b", class = "coding" },
    { id = "qwen2.5:7b", class = "fast" },
    { id = "deepseek-r1:32b", class = "reasoning" },
    { id = "codellama:34b", class = "coding" },
]
# No rate limits, no budget — it's free

# ============================================================
# Web Scrapers (Zero Cost, Last Resort)
# ============================================================

[providers.web_qwen]
enabled = true
type = "web_scraper"
priority = 99
url = "https://chat.qwen.ai"
session_tokens = ["token1", "token2"]
max_concurrent = 2
models = [
    { id = "qwen-web", class = "coding" },
]

[providers.web_chatgpt]
enabled = false
type = "web_scraper"
priority = 99
url = "https://chatgpt.com"
session_tokens = []
max_concurrent = 1

# ============================================================
# Embedding (Fixed, does not switch with LLM)
# ============================================================

[embedding]
# Fixed embedding model — does NOT change when LLM provider changes
# This ensures memory vectors remain consistent
type = "local"                    # "local" or "api"
model = "bge-m3"                  # Local: fastembed-rs
# model = "text-embedding-3-small" # API alternative (OpenAI)
dimension = 1024                  # BGE-M3 dimension
# dimension = 1536                # OpenAI dimension
```

## 3. Model Class System

```rust
/// Model classes group models by capability, not by provider
enum ModelClass {
    /// Complex reasoning: architecture decisions, debugging, root cause analysis
    /// Models: o1, o3-mini, claude-opus, deepseek-r1, gemini-pro
    Reasoning,

    /// Code generation and modification: the primary workhorse
    /// Models: claude-sonnet, gpt-4o, deepseek-chat, qwen-max, glm-4-plus
    Coding,

    /// Simple tasks: intent classification, formatting, simple edits
    /// Models: gpt-4o-mini, claude-haiku, qwen-turbo, glm-4-flash
    Fast,

    /// Long documents: summarize large codebases, analyze long logs
    /// Models: moonshot-128k, gemini-pro (1M context)
    LongContext,

    /// Generate vector embeddings for memory system
    /// Fixed model, not affected by routing decisions
    Embedding,
}
```

## 4. Routing Algorithm

```rust
impl ProviderRouter {
    /// Select the best provider + model for a given request
    async fn route(&self, request: &LlmRequest) -> Result<RoutingDecision> {
        let class = request.model_class;

        // 1. Get all providers that have models in this class
        let candidates: Vec<_> = self.providers.values()
            .filter(|p| p.enabled)
            .filter(|p| p.has_model_class(class))
            .collect();

        // 2. Sort by priority (lower = higher priority)
        let mut sorted = candidates.clone();
        sorted.sort_by_key(|p| p.priority);

        // 3. Find first available provider
        for provider in &sorted {
            // Check: has available API key?
            let key = match provider.get_available_key().await {
                Some(k) => k,
                None => continue, // All keys exhausted, try next provider
            };

            // Check: within rate limit?
            if provider.is_rate_limited().await {
                continue;
            }

            // Check: within budget?
            if provider.is_over_budget().await {
                continue;
            }

            // Check: provider is healthy? (not down)
            if provider.is_down().await {
                continue;
            }

            // Found a viable provider
            let model = provider.best_model_for_class(class);
            return Ok(RoutingDecision {
                provider_id: provider.id.clone(),
                model_id: model.id.clone(),
                api_key: key,
                estimated_cost: self.estimate_cost(&model, &request),
                fallbacks: self.compute_fallbacks(&sorted, provider),
            });
        }

        // 4. All API providers exhausted — try web scrapers
        for scraper in &self.web_scrapers {
            if scraper.is_available().await {
                return Ok(RoutingDecision {
                    provider_id: scraper.id.clone(),
                    model_id: scraper.model_id.clone(),
                    api_key: "web".into(),
                    estimated_cost: 0.0,
                    fallbacks: vec![],
                });
            }
        }

        // 5. Everything exhausted
        Err(Error::NoAvailableProvider {
            class,
            reason: "All API keys exhausted, all web scrapers unavailable".into(),
        })
    }
}
```

## 5. Key Rotation

```rust
struct KeyRotator {
    keys: Vec<ApiKeyState>,
    current_index: AtomicUsize,
}

struct ApiKeyState {
    key: SecretString,          // Encrypted
    is_exhausted: bool,
    is_rate_limited: bool,
    cooldown_until: Option<DateTime<Utc>>,
    total_tokens_used: u64,
    last_error: Option<String>,
    last_used_at: Option<DateTime<Utc>>,
}

impl KeyRotator {
    /// Get the next available key (round-robin)
    fn get_available_key(&self) -> Option<&SecretString> {
        let len = self.keys.len();
        let start = self.current_index.load(Ordering::Relaxed);

        for i in 0..len {
            let idx = (start + i) % len;
            let key = &self.keys[idx];

            if !key.is_exhausted && !key.is_rate_limited {
                if let Some(cooldown) = key.cooldown_until {
                    if Utc::now() < cooldown {
                        continue; // Still in cooldown
                    }
                }
                self.current_index.store((idx + 1) % len, Ordering::Relaxed);
                return Some(&key.key);
            }
        }

        None // All keys exhausted or rate-limited
    }

    /// Handle API response errors
    fn handle_error(&mut self, key_index: usize, error: &ApiError) {
        match error {
            ApiError::RateLimited { retry_after } => {
                self.keys[key_index].is_rate_limited = true;
                self.keys[key_index].cooldown_until =
                    Some(Utc::now() + Duration::seconds(*retry_after as i64));
            }
            ApiError::Unauthorized | ApiError::InsufficientQuota => {
                self.keys[key_index].is_exhausted = true;
            }
            _ => {
                // Transient error — don't mark key as bad
                self.keys[key_index].last_error = Some(error.to_string());
            }
        }
    }
}
```

## 6. Token Budget Tracking

```rust
struct TokenBudgetTracker {
    global: BudgetState,
    per_provider: HashMap<String, BudgetState>,
    per_project: HashMap<Uuid, BudgetState>,
    per_task: HashMap<Uuid, BudgetState>,
}

struct BudgetState {
    // Limits
    monthly_limit_usd: Option<f64>,
    daily_limit_usd: Option<f64>,

    // Current usage
    month_spent_usd: f64,
    today_spent_usd: f64,

    // Token counts
    total_input_tokens: u64,
    total_output_tokens: u64,

    // Tracking
    last_reset_date: NaiveDate,
    request_count: u64,
}

impl TokenBudgetTracker {
    /// Check if a request is within budget
    fn check_budget(&self, provider: &str, project: Option<Uuid>, estimated_cost: f64) -> BudgetCheck {
        // Check global
        if let Some(limit) = self.global.monthly_limit_usd {
            if self.global.month_spent_usd + estimated_cost > limit {
                return BudgetCheck::OverBudget { scope: "global", remaining: limit - self.global.month_spent_usd };
            }
        }

        // Check provider
        if let Some(state) = self.per_provider.get(provider) {
            if let Some(limit) = state.monthly_limit_usd {
                if state.month_spent_usd + estimated_cost > limit {
                    return BudgetCheck::OverBudget { scope: "provider", remaining: limit - state.month_spent_usd };
                }
            }
        }

        // Check project
        if let Some(pid) = project {
            if let Some(state) = self.per_project.get(&pid) {
                if let Some(limit) = state.daily_limit_usd {
                    if state.today_spent_usd + estimated_cost > limit {
                        return BudgetCheck::OverBudget { scope: "project_daily", remaining: limit - state.today_spent_usd };
                    }
                }
            }
        }

        BudgetCheck::WithinBudget
    }

    /// Record usage after a successful call
    fn record_usage(&mut self, usage: &UsageRecord) {
        let cost = usage.calculate_cost();

        self.global.month_spent_usd += cost;
        self.global.today_spent_usd += cost;
        self.global.total_input_tokens += usage.input_tokens;
        self.global.total_output_tokens += usage.output_tokens;
        self.global.request_count += 1;

        // Update per-provider, per-project, per-task
        // ...

        // Check warning thresholds
        if let Some(limit) = self.global.monthly_limit_usd {
            let pct = self.global.month_spent_usd / limit;
            if pct > 0.9 {
                // Emit event: BudgetCritical — auto-downgrade all models
            } else if pct > 0.8 {
                // Emit event: BudgetWarning — notify owner
            }
        }
    }
}
```

## 7. Web Scraper Fallback

Web scrapers provide zero-cost LLM access by automating web-based AI chat interfaces. They are the **last resort** when all API keys are exhausted.

```rust
#[async_trait]
trait WebScraper: Send + Sync {
    /// Send a message and get a response
    async fn chat(&self, messages: &[Message]) -> Result<String>;

    /// Check if the scraper is currently usable
    async fn is_available(&self) -> bool;

    /// Rotate session (use next account/cookie)
    async fn rotate_session(&mut self);
}

struct QwenWebScraper {
    url: String,
    sessions: Vec<SessionState>,
    current_session: usize,
    browser: Option<BrowserHandle>,  // Headless browser via Playwright MCP
}

// Limitations of web scrapers:
// - Slower than API (5-30 seconds per response)
// - May break if website changes its interface
// - Cannot do structured output reliably
// - Rate limited by the website
// - Should ONLY be used when all API options are exhausted
```

## 8. Conversational Management

All LLM provider configuration can be modified through conversation:

```
User: "给我加一个OpenAI的key: sk-新的key"
Argus: "已添加OpenAI API key (sk-新...y)，现在OpenAI共有4个key可轮转。"

User: "OpenAI太贵了，非必要别用"
Argus: "好的，已将OpenAI优先级从2降到8。
        现在路由优先级: Anthropic(1) > DeepSeek(3) > Gemini(4) > Qwen(5) > ... > OpenAI(8)
        仅在其他provider无法处理时才会使用OpenAI。"

User: "这个月预算控制在100美元以内"
Argus: "已设置全局月预算: $100.00
        当前本月已消耗: $23.45
        预计本月总消耗: $78.50 (在预算内)
        到达80%时我会提醒你。"

User: "本地装了个72B的qwen模型"
Argus: "好的，我来检测一下...
        ✓ 检测到 Ollama 运行在 localhost:11434
        ✓ 模型 qwen2.5:72b 可用 (47GB)
        已添加到路由表。简单任务会优先使用本地模型以节省费用。"
```

## 9. Usage Reporting

```rust
struct UsageReport {
    period: String,  // "2026-02-10" or "2026-02"

    total_cost_usd: f64,
    total_requests: u64,
    total_input_tokens: u64,
    total_output_tokens: u64,

    by_provider: Vec<ProviderUsageSummary>,
    by_project: Vec<ProjectUsageSummary>,
    by_model_class: Vec<ModelClassUsageSummary>,

    savings: SavingsReport,
}

struct SavingsReport {
    saved_by_local_model: f64,      // $ saved by using Ollama
    saved_by_cache: f64,            // $ saved by response caching
    saved_by_compression: f64,      // $ saved by context compression
    saved_by_tiered_routing: f64,   // $ saved by using cheap models for simple tasks
    total_saved: f64,
}
```
