# 参与 Argus

感谢你参与 Argus。项目目前处于开发者预览阶段，欢迎报告问题、完善文档和提交小范围改进。

## 开始之前

先阅读 [中文 README](README.md)、[当前实现](docs/PROJECT_STATE.md) 和
[路线图](docs/MASTER_ROADMAP.md)。涉及架构或新模块的改动，请先开 Issue 讨论目标和范围。

## 本地开发

```bash
./scripts/setup.sh
# 配置本地 .env；真实凭证和运行配置不能提交。
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```

数据库集成测试需要独立 PostgreSQL 测试库。按 [验收指南](docs/EVALUATION_HARNESS.md)
配置环境后串行执行，避免使用包含业务数据的数据库。真实 Agent 测试会消耗额度，需要单独启用。

前端修改请在 `web/` 下运行 `npm ci`、`npm run lint` 和 `npm run build`。

## 提交 Pull Request

- 保持改动聚焦，说明解决的问题、实际行为和验证结果。
- 修复行为问题时添加相关回归测试，遵循现有 Rust 与 TypeScript 风格。
- 运行 `./scripts/check_open_source_ready.sh --secrets-only`，确认没有提交凭证或本机状态。
- 日志、截图和复现配置中的 token、账号 ID、主机地址与私人路径应先脱敏。
- 保留功能边界说明，不把规划能力描述为已经实现。

发现安全问题请按 [安全政策](SECURITY.md) 私下报告，不在公开 Issue 中提交利用步骤或密钥。

提交贡献即表示你同意该贡献按本项目的 [MIT 许可证](LICENSE) 发布。
