# Security Policy

Argus is currently an experimental alpha. Treat it as a local development tool,
not as a production automation platform.

## Current Security Boundary

Argus can run shell commands and external coding agents. That means a malicious
repository, prompt, dependency, or tool output may attempt to:

- read files outside the intended workspace
- exfiltrate credentials from the environment
- modify local files
- run network calls
- consume CPU, memory, disk, or API budget

The planned worker sandbox is container or Linux namespace isolation with
resource limits and network policy. This work is not complete yet. Until it is,
use a disposable machine or container when testing untrusted workloads.

## Do Not Commit Secrets

Never commit:

- `.env`
- `config/providers.toml` with real API keys
- LLM provider keys
- database passwords
- SSH keys
- OAuth tokens
- local logs containing credentials
- `.claude/`, `.codex/`, or other local agent state

Run this before opening a PR:

```bash
./scripts/check_open_source_ready.sh
```

## Recommended Local Setup

- Use a throwaway PostgreSQL database
- Use low-budget LLM API keys
- Use a repository copy, not your primary working tree
- Disable network access for worker experiments where possible
- Do not expose the API server to the public internet

## Reporting Vulnerabilities

Use [GitHub private vulnerability reporting](https://github.com/cryotx/argus/security/advisories/new)
to report a vulnerability to the maintainers. Do not open a public issue
containing secrets, exploit payloads, or working exfiltration steps.
