# Memory System — Detailed Design

The Memory System is Argus's persistent knowledge store. Unlike typical chatbots that lose context when switching models or sessions, Argus stores all knowledge in PostgreSQL with pgvector, enabling perfect recall across model switches, restarts, and even architecture changes.

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Memory Engine                                │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                  Context Builder                            │ │
│  │  Assembles optimal LLM context from all memory layers       │ │
│  │  Budget: ≤ 2000 tokens (configurable)                       │ │
│  └────────────────────────┬───────────────────────────────────┘ │
│                           │                                      │
│  ┌────────┐ ┌──────────┐ │ ┌───────────┐ ┌──────────────────┐  │
│  │Working │ │Short-Term│ │ │Long-Term  │ │Permanent         │  │
│  │Memory  │ │Memory    │ │ │Memory     │ │Memory            │  │
│  │        │ │          │ │ │           │ │                   │  │
│  │In-RAM  │ │PostgreSQL│ │ │pgvector   │ │PostgreSQL        │  │
│  │Per-task│ │24-72h TTL│ │ │Semantic   │ │Never expires     │  │
│  │        │ │          │ │ │Search     │ │                   │  │
│  └────────┘ └──────────┘   └───────────┘ └──────────────────┘  │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                  Embedding Engine                           │ │
│  │  Fixed model: BGE-M3 via fastembed-rs (local, 1024-dim)     │ │
│  │  NEVER changes when LLM provider changes                    │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Memory Lifecycle Manager                       │ │
│  │  Decay │ Promotion │ Consolidation │ Expiration             │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## 2. Four-Layer Memory Model

### 2.1 Working Memory (Layer 0)

The active context of the current task. Exists only in memory (HashMap), never persisted.

```rust
struct WorkingMemory {
    task_id: Uuid,
    
    // Current task context
    task_description: String,
    acceptance_criteria: Vec<String>,
    
    // Accumulated during task execution
    code_changes: Vec<CodeChange>,
    command_outputs: Vec<CommandOutput>,
    llm_interactions: Vec<LlmInteraction>,
    
    // From other layers (pre-loaded)
    relevant_memories: Vec<RetrievedMemory>,
    
    // Running state
    scratchpad: HashMap<String, serde_json::Value>,
    
    // Token tracking
    total_tokens_used: u64,
    context_window_usage: usize,
}

impl WorkingMemory {
    /// Create working memory for a new task
    async fn for_task(task: &Task, memory_engine: &MemoryEngine) -> Self {
        let relevant = memory_engine.recall(
            &task.description,
            RecallOptions {
                max_results: 10,
                min_relevance: 0.6,
                include_layers: vec![
                    MemoryLayer::Permanent,
                    MemoryLayer::LongTerm,
                    MemoryLayer::ShortTerm,
                ],
                project_filter: Some(task.project_id),
            },
        ).await;

        Self {
            task_id: task.id,
            task_description: task.description.clone(),
            acceptance_criteria: task.acceptance_criteria.clone(),
            relevant_memories: relevant,
            ..Default::default()
        }
    }

    /// When task completes, extract valuable knowledge to store
    fn extract_knowledge(&self) -> Vec<NewMemory> {
        let mut memories = Vec::new();

        // Store the outcome
        memories.push(NewMemory {
            layer: MemoryLayer::ShortTerm,
            memory_type: MemoryType::TaskOutcome,
            content: format!(
                "Task '{}' completed. Changes: {:?}",
                self.task_description,
                self.code_changes.iter().map(|c| &c.file_path).collect::<Vec<_>>()
            ),
            importance: 0.5,
            project_id: Some(self.task_id), // linked via task
            tags: vec!["task_outcome".into()],
            ..Default::default()
        });

        // Store any errors encountered (higher importance)
        for interaction in &self.llm_interactions {
            if interaction.had_error {
                memories.push(NewMemory {
                    layer: MemoryLayer::ShortTerm,
                    memory_type: MemoryType::ErrorPattern,
                    content: format!(
                        "Error during '{}': {} → Fixed by: {}",
                        self.task_description, interaction.error_message, interaction.fix_applied
                    ),
                    importance: 0.8,
                    tags: vec!["error".into(), "lesson".into()],
                    ..Default::default()
                });
            }
        }

        memories
    }
}
```

### 2.2 Short-Term Memory (Layer 1)

Recent events and interactions, stored in PostgreSQL with a time-to-live. Decays naturally after 24-72 hours but can be promoted to long-term if accessed frequently.

```rust
struct ShortTermMemory {
    id: Uuid,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,          // Default: created_at + 48h
    
    content: String,
    memory_type: MemoryType,
    importance: f32,                     // 0.0 ~ 1.0
    
    // Relationships
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    
    // Access tracking (for promotion decisions)
    access_count: u32,
    last_accessed_at: DateTime<Utc>,
    
    // Vector embedding (for semantic search)
    embedding: Option<Vec<f32>>,         // 1024-dim BGE-M3
    
    // Metadata
    tags: Vec<String>,
    source: MemorySource,               // UserMessage, TaskOutput, SystemEvent, etc.
}
```

**What goes into short-term memory:**
- Recent conversation messages and responses
- Task outputs and results
- Error messages and their fixes
- Recent config changes
- Deployment results
- Git operations

### 2.3 Long-Term Memory (Layer 2)

Project knowledge, technical decisions, and accumulated experience. Stored with pgvector embeddings for semantic search. Decays slowly based on importance and access frequency.

```rust
struct LongTermMemory {
    id: Uuid,
    created_at: DateTime<Utc>,
    
    content: String,
    memory_type: MemoryType,
    importance: f32,                     // 0.0 ~ 1.0, decays over time
    
    // Decay management
    initial_importance: f32,             // Original importance when created
    decay_rate: f32,                     // How fast importance decays (default 0.01/day)
    last_decay_at: DateTime<Utc>,
    
    // Relationships
    project_id: Option<Uuid>,
    related_memory_ids: Vec<Uuid>,       // Cross-references to other memories
    
    // Access tracking
    access_count: u32,
    last_accessed_at: DateTime<Utc>,
    
    // Vector embedding
    embedding: Vec<f32>,                 // 1024-dim BGE-M3 (required for long-term)
    
    // Metadata
    tags: Vec<String>,
    source: MemorySource,
    consolidated_from: Vec<Uuid>,        // If this memory was created by consolidating others
}
```

**What goes into long-term memory:**
- Project architecture decisions and rationale
- Successful problem-solving patterns
- Error patterns and their solutions
- Technology-specific knowledge (e.g., "This project uses Taro 4.x, which requires...")
- Cross-project observations
- Owner feedback patterns

### 2.4 Permanent Memory (Layer 3)

Core knowledge that must never be forgotten. No decay, no expiration. Only the owner can explicitly delete permanent memories.

```rust
struct PermanentMemory {
    id: Uuid,
    created_at: DateTime<Utc>,
    
    content: String,
    memory_type: PermanentMemoryType,
    
    // Permanent memories have fixed importance
    importance: f32,                     // Always 1.0 for safety rules, varies for others
    
    // Vector embedding
    embedding: Vec<f32>,
    
    // Metadata
    tags: Vec<String>,
    source: MemorySource,
    
    // Protection
    deletable_by: Vec<Role>,            // Default: [Owner] only
}

enum PermanentMemoryType {
    /// Owner preferences and behavioral patterns
    OwnerPreference,
    
    /// Safety rules and boundaries (e.g., "never force push main")
    SafetyRule,
    
    /// Critical decisions (e.g., "project X uses MIT license")
    CriticalDecision,
    
    /// System identity (e.g., "I am Argus, I serve owner X")
    SystemIdentity,
    
    /// Learned universal truths (e.g., "always run tests before deploy")
    UniversalLesson,
}
```

**What goes into permanent memory:**
- Owner preferences: "I prefer concise responses", "Don't wake me between 11pm and 8am"
- Safety rules: "Never deploy on Friday", "Always backup before migration"
- Critical decisions: "Project X is MIT licensed", "Owner's main language is Chinese"
- System identity and configuration anchors
- Universally learned lessons from failures

## 3. Embedding Engine

The embedding engine converts text into vector representations for semantic search. **The embedding model is fixed and independent of LLM provider selection** — this is a critical design decision to ensure memory vector consistency.

```rust
struct EmbeddingEngine {
    /// The embedding model — NEVER changes when LLM provider changes
    model: FastEmbedModel,   // fastembed-rs with BGE-M3
    dimension: usize,        // 1024 for BGE-M3
    
    /// Cache to avoid re-embedding identical texts
    cache: LruCache<String, Vec<f32>>,
}

impl EmbeddingEngine {
    fn new() -> Self {
        // Initialize fastembed with BGE-M3
        // This model runs locally — zero cost, no API calls
        let model = FastEmbedModel::new(
            "BAAI/bge-m3",
            EmbeddingOptions {
                max_length: 8192,   // BGE-M3 supports up to 8192 tokens
                normalize: true,
            },
        );
        
        Self {
            model,
            dimension: 1024,
            cache: LruCache::new(1000),
        }
    }

    /// Generate embedding for a text
    async fn embed(&self, text: &str) -> Vec<f32> {
        // Check cache first
        if let Some(cached) = self.cache.get(text) {
            return cached.clone();
        }
        
        let embedding = self.model.embed(&[text]).await[0].clone();
        self.cache.put(text.to_string(), embedding.clone());
        embedding
    }

    /// Batch embed for efficiency
    async fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        self.model.embed(texts).await
    }
}
```

### Why BGE-M3?

| Property | Value |
|----------|-------|
| Multilingual | Yes (100+ languages, excellent Chinese + English) |
| Dimension | 1024 |
| Max Tokens | 8192 |
| Runs Locally | Yes (via fastembed-rs, no GPU required) |
| Cost | Free (local inference) |
| Quality | Competitive with OpenAI text-embedding-3-small |

### Why Fixed Embedding Model?

If Argus used different embedding models depending on which LLM provider is active, switching providers would make all existing memory vectors incomparable. By fixing the embedding model:

1. **Memory vectors are always comparable** — cosine similarity works across all memories regardless of when they were created
2. **No re-embedding needed** when switching LLM providers
3. **Consistent recall quality** — search results don't change just because the LLM changed
4. **Zero cost** — local model, no API calls for embedding

## 4. Memory Operations

### 4.1 Store

```rust
impl MemoryEngine {
    async fn store(&self, memory: NewMemory) -> MemoryId {
        // 1. Generate embedding
        let embedding = self.embedding_engine.embed(&memory.content).await;
        
        // 2. Check for duplicates (near-duplicate detection)
        let similar = self.search_similar(&embedding, 0.95, 3).await;
        if let Some(existing) = similar.first() {
            if existing.similarity > 0.98 {
                // Near-exact duplicate — increment access count instead
                self.increment_access(existing.id).await;
                return existing.id;
            }
            if existing.similarity > 0.95 {
                // Very similar — consolidate
                return self.consolidate(existing.id, &memory).await;
            }
        }
        
        // 3. Insert into database
        let id = sqlx::query_scalar!(
            r#"
            INSERT INTO memories (id, layer, memory_type, content, importance, 
                                  embedding, project_id, task_id, tags, source,
                                  created_at, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING id
            "#,
            Uuid::new_v4(), memory.layer, memory.memory_type, memory.content,
            memory.importance, &embedding, memory.project_id, memory.task_id,
            &memory.tags, memory.source, Utc::now(), memory.expires_at,
        )
        .fetch_one(&self.pool)
        .await?;
        
        // 4. Emit event
        self.event_bus.emit(ArgusEvent::MemoryStored(id, memory.layer)).await;
        
        id
    }
}
```

### 4.2 Recall (Semantic Search)

```rust
impl MemoryEngine {
    /// Retrieve relevant memories for a query
    async fn recall(&self, query: &str, options: RecallOptions) -> Vec<RetrievedMemory> {
        // 1. Embed the query
        let query_embedding = self.embedding_engine.embed(query).await;
        
        // 2. Search across specified layers using pgvector
        let results = sqlx::query_as!(
            MemoryRow,
            r#"
            SELECT id, layer, memory_type, content, importance, tags, source,
                   created_at, access_count,
                   1 - (embedding <=> $1::vector) AS similarity
            FROM memories
            WHERE layer = ANY($2)
              AND ($3::uuid IS NULL OR project_id = $3)
              AND (expires_at IS NULL OR expires_at > NOW())
              AND importance > 0.1
            ORDER BY similarity DESC
            LIMIT $4
            "#,
            &query_embedding as &[f32],
            &options.include_layers,
            options.project_filter,
            options.max_results as i64,
        )
        .fetch_all(&self.pool)
        .await?;
        
        // 3. Filter by minimum relevance threshold
        let filtered: Vec<_> = results
            .into_iter()
            .filter(|r| r.similarity >= options.min_relevance)
            .collect();
        
        // 4. Update access counts (async, non-blocking)
        let ids: Vec<Uuid> = filtered.iter().map(|r| r.id).collect();
        let this = self.clone();
        tokio::spawn(async move {
            this.update_access_counts(&ids).await;
        });
        
        // 5. Convert to RetrievedMemory
        filtered.into_iter().map(|r| RetrievedMemory {
            id: r.id,
            content: r.content,
            layer: r.layer,
            similarity: r.similarity,
            importance: r.importance,
            source: r.source,
        }).collect()
    }
}
```

### 4.3 Recall Options

```rust
struct RecallOptions {
    /// Maximum number of results to return
    max_results: usize,        // Default: 10
    
    /// Minimum cosine similarity threshold
    min_relevance: f32,        // Default: 0.6
    
    /// Which memory layers to search
    include_layers: Vec<MemoryLayer>,  // Default: all layers
    
    /// Filter by project
    project_filter: Option<Uuid>,
    
    /// Filter by memory type
    type_filter: Option<Vec<MemoryType>>,
    
    /// Filter by tags
    tag_filter: Option<Vec<String>>,
    
    /// Time range filter
    time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
}
```

## 5. Memory Lifecycle

### 5.1 Decay

Importance of long-term memories decays over time. This prevents the memory system from growing unboundedly and ensures the most relevant memories surface first.

```rust
impl MemoryLifecycleManager {
    /// Run decay cycle (scheduled, e.g., every 6 hours)
    async fn run_decay_cycle(&self) {
        // Decay long-term memories
        sqlx::query!(
            r#"
            UPDATE memories
            SET importance = GREATEST(
                importance - (decay_rate * EXTRACT(EPOCH FROM (NOW() - last_decay_at)) / 86400.0),
                0.0
            ),
            last_decay_at = NOW()
            WHERE layer = 'long_term'
              AND importance > 0.0
            "#
        )
        .execute(&self.pool)
        .await?;
        
        // Remove memories that decayed below threshold
        let expired = sqlx::query!(
            "DELETE FROM memories WHERE layer = 'long_term' AND importance < 0.05 RETURNING id"
        )
        .fetch_all(&self.pool)
        .await?;
        
        for row in &expired {
            self.event_bus.emit(ArgusEvent::MemoryExpired(row.id)).await;
        }
        
        tracing::info!("Decay cycle: {} memories expired", expired.len());
    }
}
```

### 5.2 Promotion

Memories that are accessed frequently get promoted to a higher layer:

```
Short-Term → Long-Term:
  Condition: access_count >= 3 within TTL period
  OR: importance >= 0.8

Long-Term → Permanent:
  Condition: access_count >= 10 AND importance >= 0.9
  AND: manually confirmed by owner OR tagged as critical
```

```rust
impl MemoryLifecycleManager {
    /// Run promotion cycle (scheduled, e.g., every hour)
    async fn run_promotion_cycle(&self) {
        // Promote frequently accessed short-term → long-term
        let promoted = sqlx::query!(
            r#"
            UPDATE memories
            SET layer = 'long_term',
                expires_at = NULL,
                decay_rate = 0.01,
                initial_importance = importance,
                last_decay_at = NOW()
            WHERE layer = 'short_term'
              AND (access_count >= 3 OR importance >= 0.8)
              AND expires_at > NOW()
            RETURNING id
            "#
        )
        .fetch_all(&self.pool)
        .await?;
        
        for row in &promoted {
            self.event_bus.emit(
                ArgusEvent::MemoryPromoted(row.id, MemoryLayer::ShortTerm, MemoryLayer::LongTerm)
            ).await;
        }
        
        tracing::info!("Promotion cycle: {} memories promoted to long-term", promoted.len());
    }
}
```

### 5.3 Consolidation

Over time, similar memories accumulate. Consolidation merges them into a single, richer memory:

```rust
impl MemoryLifecycleManager {
    /// Run consolidation cycle (scheduled, e.g., daily)
    async fn run_consolidation_cycle(&self) {
        // 1. Find clusters of similar memories
        let clusters = self.find_similar_clusters(
            MemoryLayer::LongTerm,
            0.85,  // similarity_threshold
            3,     // min_cluster_size
        ).await;
        
        for cluster in clusters {
            // 2. Use LLM to summarize the cluster into one consolidated memory
            let contents: Vec<&str> = cluster.memories.iter().map(|m| m.content.as_str()).collect();
            
            let summary = self.llm.chat(
                ModelClass::Fast,
                &[Message::system(
                    "Consolidate these related memories into a single, concise memory. \
                     Preserve all unique information. Remove redundancy."
                ), Message::user(&contents.join("\n---\n"))],
            ).await?;
            
            // 3. Create consolidated memory
            let consolidated_id = self.store(NewMemory {
                layer: MemoryLayer::LongTerm,
                content: summary,
                importance: cluster.max_importance(),
                tags: cluster.merged_tags(),
                consolidated_from: cluster.memory_ids(),
                ..Default::default()
            }).await;
            
            // 4. Remove original memories
            for memory in &cluster.memories {
                sqlx::query!("DELETE FROM memories WHERE id = $1", memory.id)
                    .execute(&self.pool)
                    .await?;
            }
            
            self.event_bus.emit(
                ArgusEvent::MemoryConsolidated(cluster.memory_ids(), consolidated_id)
            ).await;
        }
    }
    
    /// Find clusters of similar memories using pgvector
    async fn find_similar_clusters(
        &self,
        layer: MemoryLayer,
        similarity_threshold: f32,
        min_cluster_size: usize,
    ) -> Vec<MemoryCluster> {
        // Use a simple greedy clustering approach:
        // 1. Get all memories in the layer
        // 2. For each memory, find others with similarity > threshold
        // 3. Group them into clusters
        // 4. Return clusters with size >= min_cluster_size
        
        let all_memories = sqlx::query_as!(
            MemoryRow,
            "SELECT * FROM memories WHERE layer = $1 AND importance > 0.1 ORDER BY created_at",
            layer,
        )
        .fetch_all(&self.pool)
        .await?;
        
        // TODO: Implement clustering using pgvector cosine distance
        // Algorithm: Group memories by embedding similarity using pgvector's
        // `<=>` (cosine distance) operator. Iterate through all_memories,
        // assigning each to the nearest existing cluster (if distance < threshold)
        // or creating a new cluster. Return cluster centroids with member counts.
        Vec::new() // placeholder
    }
}
```

### 5.4 Expiration

Short-term memories have a TTL. A periodic job cleans them up:

```rust
impl MemoryLifecycleManager {
    /// Run expiration cycle (scheduled, e.g., every 30 minutes)
    async fn run_expiration_cycle(&self) {
        let expired = sqlx::query!(
            "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < NOW() RETURNING id"
        )
        .fetch_all(&self.pool)
        .await?;
        
        tracing::info!("Expiration cycle: {} memories expired", expired.len());
    }
}
```

## 6. Context Builder

The Context Builder assembles the optimal LLM context from memory layers, respecting token budgets.

```rust
struct ContextBuilder {
    memory_engine: Arc<MemoryEngine>,
    max_tokens: usize,              // Default: 2000 tokens for memory context
    token_counter: TokenCounter,
}

impl ContextBuilder {
    /// Build context for a task
    async fn build_context(&self, task: &Task) -> TaskContext {
        let mut context = TaskContext::new();
        let mut remaining_tokens = self.max_tokens;
        
        // === Priority 1: Permanent memories (always included) ===
        let permanent = self.memory_engine.recall(
            &task.description,
            RecallOptions {
                include_layers: vec![MemoryLayer::Permanent],
                max_results: 5,
                min_relevance: 0.3,  // Lower threshold for permanent — they're important
                ..Default::default()
            },
        ).await;
        
        for memory in &permanent {
            let tokens = self.token_counter.count(&memory.content);
            if tokens <= remaining_tokens {
                context.add_permanent(memory);
                remaining_tokens -= tokens;
            }
        }
        
        // === Priority 2: Project-specific long-term memories ===
        let project_memories = self.memory_engine.recall(
            &task.description,
            RecallOptions {
                include_layers: vec![MemoryLayer::LongTerm],
                project_filter: Some(task.project_id),
                max_results: 10,
                min_relevance: 0.6,
                ..Default::default()
            },
        ).await;
        
        for memory in &project_memories {
            let tokens = self.token_counter.count(&memory.content);
            if tokens <= remaining_tokens {
                context.add_project_knowledge(memory);
                remaining_tokens -= tokens;
            }
        }
        
        // === Priority 3: Cross-project relevant long-term memories ===
        let cross_project = self.memory_engine.recall(
            &task.description,
            RecallOptions {
                include_layers: vec![MemoryLayer::LongTerm],
                project_filter: None,
                max_results: 5,
                min_relevance: 0.75,  // Higher threshold for cross-project
                ..Default::default()
            },
        ).await;
        
        for memory in &cross_project {
            let tokens = self.token_counter.count(&memory.content);
            if tokens <= remaining_tokens {
                context.add_general_knowledge(memory);
                remaining_tokens -= tokens;
            }
        }
        
        // === Priority 4: Recent short-term context ===
        let recent = self.memory_engine.recall(
            &task.description,
            RecallOptions {
                include_layers: vec![MemoryLayer::ShortTerm],
                project_filter: Some(task.project_id),
                max_results: 5,
                min_relevance: 0.5,
                ..Default::default()
            },
        ).await;
        
        for memory in &recent {
            let tokens = self.token_counter.count(&memory.content);
            if tokens <= remaining_tokens {
                context.add_recent(memory);
                remaining_tokens -= tokens;
            }
        }
        
        context
    }
    
    /// Format context for inclusion in LLM prompt
    fn format_for_prompt(&self, context: &TaskContext) -> String {
        let mut sections = Vec::new();
        
        if !context.permanent.is_empty() {
            sections.push(format!(
                "## Core Rules & Preferences\n{}",
                context.permanent.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n")
            ));
        }
        
        if !context.project_knowledge.is_empty() {
            sections.push(format!(
                "## Project Knowledge\n{}",
                context.project_knowledge.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n")
            ));
        }
        
        if !context.general_knowledge.is_empty() {
            sections.push(format!(
                "## Relevant Experience\n{}",
                context.general_knowledge.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n")
            ));
        }
        
        if !context.recent.is_empty() {
            sections.push(format!(
                "## Recent Context\n{}",
                context.recent.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n")
            ));
        }
        
        sections.join("\n\n")
    }
}
```

## 7. Cross-Model Persistence

When Argus switches LLM providers (e.g., from Claude to DeepSeek), memory is not lost because:

1. **Memory is stored in PostgreSQL**, not in any LLM's context window
2. **Embeddings use a fixed local model** (BGE-M3), so vector search works identically regardless of which LLM is active
3. **Context is rebuilt from database** for each new LLM call

```
Model Switch Flow:
                                                                    
  Claude Session                     DeepSeek Session
  ┌──────────────┐                   ┌──────────────┐
  │ Working on   │                   │ Continue     │
  │ Task T1      │ ──── Switch ────→ │ Task T2      │
  └──────┬───────┘                   └──────┬───────┘
         │                                  │
         │ Store memories                   │ Recall memories
         │ to PostgreSQL                    │ from PostgreSQL
         ▼                                  ▼
  ┌──────────────────────────────────────────────────┐
  │                PostgreSQL + pgvector              │
  │                                                   │
  │  memories table:                                   │
  │  ┌────┬─────────┬────────────┬──────────────────┐ │
  │  │ id │ content │ embedding  │ importance       │ │
  │  │    │         │ (BGE-M3)   │                  │ │
  │  └────┴─────────┴────────────┴──────────────────┘ │
  │                                                   │
  │  Embeddings generated by BGE-M3 (local)           │
  │  Same vectors regardless of which LLM is active   │
  └──────────────────────────────────────────────────┘
```

### Context Rebuild on Model Switch

When the Brain Engine switches to a different LLM mid-session:

```rust
impl BrainEngine {
    async fn switch_model_and_continue(&self, new_provider: &str, task: &Task) -> Result<()> {
        // 1. Save any unsaved working memory to short-term
        self.flush_working_memory().await;
        
        // 2. Switch provider in LLM engine
        self.llm_engine.set_preferred_provider(new_provider).await;
        
        // 3. Rebuild context from database (not from LLM history)
        let context = self.context_builder.build_context(task).await;
        
        // 4. Create a fresh conversation with the new model,
        //    injecting database-retrieved context
        let system_prompt = format!(
            "{}\n\n## Memory Context\n{}",
            self.base_system_prompt,
            self.context_builder.format_for_prompt(&context)
        );
        
        // 5. Continue execution with new model
        self.execute_with_context(task, &system_prompt).await
    }
}
```

## 8. Conversation Summarization

Long conversations are periodically summarized and stored, rather than keeping full transcripts:

```rust
struct ConversationSummarizer {
    llm: Arc<LlmEngine>,
}

impl ConversationSummarizer {
    /// Summarize a conversation segment
    async fn summarize(&self, messages: &[Message]) -> String {
        let prompt = format!(
            "Summarize this conversation concisely, preserving:\n\
             1. Key decisions made\n\
             2. Important information learned\n\
             3. Action items and their outcomes\n\
             4. Any preferences or rules established\n\n\
             Conversation:\n{}",
            messages.iter().map(|m| format!("{}: {}", m.role, m.content)).collect::<Vec<_>>().join("\n")
        );
        
        self.llm.chat(ModelClass::Fast, &[Message::user(&prompt)]).await
    }
}
```

## 9. Memory Types

```rust
enum MemoryType {
    // Task-related
    TaskOutcome,           // Result of a completed task
    TaskPlan,              // How a project/milestone was decomposed
    ErrorPattern,          // An error and how it was fixed
    
    // Project-related
    ProjectArchitecture,   // Tech stack, structure decisions
    ProjectConstraint,     // Limitations, requirements
    ProjectDecision,       // Why a specific choice was made
    
    // Technical knowledge
    CodePattern,           // Reusable code patterns
    ToolUsage,            // How to use a specific tool/library
    EnvironmentSetup,      // Server/environment configuration
    
    // Social/interaction
    ConversationSummary,   // Summarized conversation
    OwnerFeedback,         // Owner's feedback on results
    OwnerPreference,       // Learned owner preference
    
    // System
    SystemEvent,           // Important system events
    SecurityIncident,      // Security-related events
    
    // Meta
    ConsolidatedMemory,    // Created by consolidation
}
```

## 10. Lifecycle Schedule

| Cycle | Frequency | Description |
|-------|-----------|-------------|
| Expiration | Every 30 min | Remove expired short-term memories |
| Promotion | Every 1 hour | Promote frequently accessed memories |
| Decay | Every 6 hours | Reduce importance of long-term memories |
| Consolidation | Every 24 hours | Merge similar memories |
| Backup | Every 24 hours | pg_dump of memories table |
| Statistics | Every 24 hours | Log memory count, avg importance, layer distribution |

## 11. Conversational Memory Commands

```
User: "记住：这个项目永远不要在周五部署"
Argus: "已记录为永久记忆：'项目X不在周五部署'。
        分类: 安全规则
        我以后在周五收到部署请求时会拒绝执行。"

User: "我之前处理那个CORS错误是怎么解决的？"
Argus: "根据记忆 (2天前, 项目Y):
        遇到CORS错误时，你在Nginx配置中添加了以下header:
        add_header 'Access-Control-Allow-Origin' '$http_origin';
        add_header 'Access-Control-Allow-Methods' 'GET, POST, OPTIONS';
        同时后端也添加了cors中间件。"

User: "忘掉关于旧支付系统的所有记忆"
Argus: "找到3条相关记忆:
        1. [长期] 旧支付系统使用Stripe v2 API
        2. [长期] 支付回调需要验证签名
        3. [短期] 昨天修复了支付超时问题
        确认要删除这3条记忆吗？(永久记忆不会被删除)"
```

## 12. Performance Considerations

| Operation | Target Latency | Implementation |
|-----------|---------------|----------------|
| Store memory | < 50ms | Async insert + background embedding |
| Recall (semantic search) | < 500ms | pgvector HNSW index |
| Embedding generation | < 100ms | Local BGE-M3 via fastembed-rs |
| Decay cycle | < 5s | Batch SQL UPDATE |
| Consolidation cycle | < 60s | Depends on cluster count |
| Context build | < 200ms | Parallel layer queries |

### pgvector Index Configuration

```sql
-- HNSW index for fast approximate nearest neighbor search
CREATE INDEX idx_memories_embedding ON memories
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Partial indexes for layer-specific queries
CREATE INDEX idx_memories_short_term ON memories (created_at)
    WHERE layer = 'short_term';
CREATE INDEX idx_memories_long_term ON memories (importance DESC)
    WHERE layer = 'long_term';
CREATE INDEX idx_memories_permanent ON memories (memory_type)
    WHERE layer = 'permanent';
CREATE INDEX idx_memories_project ON memories (project_id)
    WHERE project_id IS NOT NULL;
```
