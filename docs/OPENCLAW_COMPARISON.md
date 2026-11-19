# OpenClaw vs Argus 对比分析

> 更新时间: 2026-03-02

## 基本信息

| 项目 | OpenClaw | Argus |
|------|----------|-------|
| **Stars** | 246,591 ⭐ | 新项目 |
| **语言** | TypeScript | Rust |
| **运行时** | Node ≥22 | Tokio async |
| **架构** | Gateway + WebSocket | EventBus + 模块化 crates |
| **定位** | 个人 AI 助理 | 自治 AI 操作系统 |
| **仓库** | [github.com/openclaw/openclaw](https://github.com/openclaw/openclaw) | 内部开发 |

## 核心架构对比

| 能力 | OpenClaw | Argus |
|------|----------|-------|
| **控制平面** | Gateway WebSocket (单进程) | EventBus + 多 crate |
| **消息渠道** | 20+ 平台 | 7 平台 |
| **多代理** | 多 agent 路由 (工作空间隔离) | Worker 池 + 协作模式 |
| **持久化** | 本地文件 | PostgreSQL + pgvector |
| **记忆系统** | 会话上下文 | 四层记忆 + 向量搜索 |

### OpenClaw 支持的消息平台
WhatsApp, Telegram, Slack, Discord, Google Chat, Signal, iMessage, BlueBubbles, IRC, Microsoft Teams, Matrix, Feishu, LINE, Mattermost, Nextcloud Talk, Nostr, Synology Chat, Tlon, Twitch, Zalo, Zalo Personal, WebChat

### Argus 支持的消息平台
Telegram, Feishu, QQ, WhatsApp, Discord, Slack, WeChat (规划中)

## 开发能力对比

| 能力 | OpenClaw | Argus |
|------|----------|-------|
| **代码生成** | 通过 Skills | Brain + Worker 驱动 |
| **任务分解** | 无原生支持 | ✅ 原生 Brain 分解器 |
| **Git 操作** | 通过 Skills | ✅ 原生 argus-git |
| **部署** | 无原生支持 | ✅ 原生 argus-deploy |
| **质量检查** | 无原生支持 | ✅ 原生 Quality Check 循环 |
| **项目管理** | 无原生支持 | ✅ 多项目注册表 |
| **依赖解析** | 无原生支持 | ✅ DAG 依赖图 |

## 扩展性对比

| 能力 | OpenClaw | Argus |
|------|----------|-------|
| **插件系统** | Skills (AgentSkills 兼容) | Skills + MCP 宿主 |
| **技能市场** | ClawHub (5400+ 技能) | 规划中 |
| **自定义工具** | 浏览器/Canvas/Nodes | MCP 协议 |
| **安全审计** | 基础 | ✅ RBAC + CommandGuard + 审计日志 |

## 独特功能

### OpenClaw 优势
- 🦞 **语音交互** — Voice Wake + Talk Mode (macOS/iOS/Android)
- 📱 **原生应用** — macOS 菜单栏应用 + iOS/Android 节点
- 🎨 **Canvas** — 实时可视化工作区 (A2UI)
- 🌐 **Tailscale 集成** — 一键远程访问
- 🔧 **成熟度** — 246K stars，生产就绪
- 📦 **ClawHub** — 5400+ 预构建技能

### Argus 优势
- 🧠 **自治架构** — 任务分解 → 执行 → 质量检查 → 部署闭环
- 💾 **四层记忆** — 工作/短期/长期/永久 + 向量搜索
- 🔄 **自进化** — 可以修改自己的代码库
- 📊 **项目管理** — 多项目、里程碑、任务依赖 DAG
- 🔒 **企业安全** — RBAC + 密钥管理 + 审计
- 🚀 **Rust 性能** — 高并发、低资源占用

## 技术栈对比

| 层面 | OpenClaw | Argus |
|------|----------|-------|
| **核心语言** | TypeScript | Rust |
| **运行时** | Node.js/Bun | Tokio |
| **数据库** | 本地文件 | PostgreSQL + pgvector |
| **嵌入模型** | 外部 API | 本地 BGE-M3 (fastembed) |
| **LLM 接入** | OAuth + API Key | 多提供商路由 + 熔断器 |
| **容器化** | Docker | Docker + bollard |

## OpenClaw 架构亮点 (可借鉴)

### 1. Gateway WebSocket 协议
```
消息平台 → Gateway (WebSocket) → Clients/Nodes
                ↓
         单一控制平面
```
- 所有消息平台连接由 Gateway 统一管理
- 客户端通过 WebSocket 与 Gateway 通信
- 节点 (Nodes) 可扩展设备能力

### 2. Skills 系统
```markdown
---
name: skill-name
description: Skill description
metadata:
  {
    "openclaw":
      {
        "requires": { "bins": ["uv"], "env": ["API_KEY"] },
        "primaryEnv": "API_KEY"
      }
  }
---
```
- AgentSkills 兼容格式
- 运行时过滤 (bins/env/config)
- ClawHub 技能市场

### 3. 节点模式
- macOS/iOS/Android 作为节点连接
- 暴露设备能力: camera, screen record, location, notifications
- 通过 `node.invoke` 远程调用

### 4. 安全模型
- DM 配对机制 (pairing)
- 本地信任 + 远程审批
- Tailscale 身份验证

## Argus 可借鉴的功能

| 功能 | 借鉴方式 | 优先级 |
|------|----------|--------|
| Skills 格式 | 采用 AgentSkills 兼容格式 | 高 |
| Gateway 协议 | WebSocket 控制平面 | 中 |
| 节点模式 | 设备能力扩展 | 低 |
| ClawHub | 建立技能市场 | 低 |
| Voice Wake | 语音唤醒 | 低 |

## 决策

**选择方案 B: 继续完善 Argus**

理由：
1. 保持架构独立性，完全自主可控
2. 专注自治开发能力 (任务分解 + 质量检查 + 部署)
3. Rust 性能优势，适合长期运行
4. 可借鉴 OpenClaw 的设计模式，但不依赖它

## 下一步

1. 实现 Worker 驱动 (真正执行 shell 命令)
2. 完善 Git 操作模块
3. 建立 Skills 系统 (AgentSkills 兼容)
4. 质量检查闭环测试