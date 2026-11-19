use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use argus_core::types::{ArgusError, ArgusResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Sse,
    StreamableHttp,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub endpoint: Option<String>,
    pub transport: McpTransport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub content: Value,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct McpClient {
    config: McpServerConfig,
}

impl McpClient {
    pub fn connect(config: McpServerConfig) -> ArgusResult<Self> {
        if config.id.trim().is_empty() {
            return Err(ArgusError::Mcp("server id cannot be empty".to_string()));
        }

        if matches!(config.transport, McpTransport::Stdio) {
            let command = config
                .command
                .as_ref()
                .ok_or_else(|| ArgusError::Mcp("stdio transport requires command".to_string()))?;
            if command.trim().is_empty() {
                return Err(ArgusError::Mcp(
                    "stdio transport command cannot be empty".to_string(),
                ));
            }
        }

        Ok(Self { config })
    }

    pub fn server_id(&self) -> &str {
        &self.config.id
    }

    pub async fn call_tool(&self, tool_name: &str, args: Value) -> ArgusResult<ToolResult> {
        match self.config.transport {
            McpTransport::Stdio => self.call_stdio(tool_name, args).await,
            McpTransport::Sse | McpTransport::StreamableHttp => Err(ArgusError::Mcp(format!(
                "transport not implemented yet: {:?}",
                self.config.transport
            ))),
        }
    }

    async fn call_stdio(&self, tool_name: &str, args: Value) -> ArgusResult<ToolResult> {
        let command = self
            .config
            .command
            .as_deref()
            .ok_or_else(|| ArgusError::Mcp("stdio transport requires command".to_string()))?;

        let mut child = Command::new(command)
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                ArgusError::Mcp(format!(
                    "failed to spawn MCP server '{}' for '{}': {e}",
                    self.config.id, command
                ))
            })?;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": args,
            }
        });

        let request_line = serde_json::to_string(&request)
            .map_err(|e| ArgusError::Mcp(format!("failed to serialize MCP request: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ArgusError::Mcp("MCP child stdin unavailable".to_string()))?;
        stdin
            .write_all(request_line.as_bytes())
            .await
            .map_err(|e| ArgusError::Mcp(format!("failed to write MCP request: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| ArgusError::Mcp(format!("failed to flush MCP request line: {e}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| ArgusError::Mcp(format!("failed to close MCP stdin: {e}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ArgusError::Mcp("MCP child stdout unavailable".to_string()))?;

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        timeout(Duration::from_secs(30), reader.read_line(&mut line))
            .await
            .map_err(|_| ArgusError::Mcp("MCP server response timed out".to_string()))?
            .map_err(|e| ArgusError::Mcp(format!("failed to read MCP response: {e}")))?;

        if line.trim().is_empty() {
            let stderr_output = child
                .wait_with_output()
                .await
                .map_err(|e| ArgusError::Mcp(format!("failed waiting MCP process: {e}")))?;
            let stderr_text = String::from_utf8_lossy(&stderr_output.stderr);
            return Err(ArgusError::Mcp(format!(
                "empty MCP response from '{}', stderr: {}",
                self.config.id,
                stderr_text.trim()
            )));
        }

        let parsed: Value = serde_json::from_str(line.trim()).map_err(|e| {
            ArgusError::Mcp(format!(
                "invalid JSON response from MCP server '{}': {e}",
                self.config.id
            ))
        })?;

        parse_tool_result(parsed)
    }
}

fn parse_tool_result(value: Value) -> ArgusResult<ToolResult> {
    if let Ok(result) = serde_json::from_value::<ToolResult>(value.clone()) {
        return Ok(result);
    }

    if let Some(error_obj) = value.get("error") {
        let message = error_obj
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown MCP error");
        return Ok(ToolResult {
            success: false,
            content: Value::Null,
            error: Some(message.to_string()),
        });
    }

    if let Some(result_obj) = value.get("result") {
        if let Ok(result) = serde_json::from_value::<ToolResult>(result_obj.clone()) {
            return Ok(result);
        }

        return Ok(ToolResult {
            success: true,
            content: result_obj.clone(),
            error: None,
        });
    }

    Err(ArgusError::Mcp(
        "unable to parse MCP tool response payload".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_stdio_requires_command() {
        let result = McpClient::connect(McpServerConfig {
            id: "s1".to_string(),
            name: "server".to_string(),
            command: None,
            args: vec![],
            endpoint: None,
            transport: McpTransport::Stdio,
        });

        let err = match result {
            Ok(_) => panic!("expected stdio validation to fail"),
            Err(err) => err,
        };

        assert!(format!("{err}").contains("requires command"));
    }

    #[tokio::test]
    async fn test_call_tool_stdio_roundtrip() {
        let client = McpClient::connect(McpServerConfig {
            id: "s1".to_string(),
            name: "test-stdio".to_string(),
            command: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "read line; printf '{\"success\":true,\"content\":{\"echo\":%s},\"error\":null}\\n' \"$line\"".to_string(),
            ],
            endpoint: None,
            transport: McpTransport::Stdio,
        })
        .unwrap();

        let result = client
            .call_tool("echo", serde_json::json!({"x": 1}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content.get("echo").is_some());
    }

    #[tokio::test]
    async fn test_call_tool_sse_not_implemented() {
        let client = McpClient::connect(McpServerConfig {
            id: "s2".to_string(),
            name: "test-sse".to_string(),
            command: None,
            args: vec![],
            endpoint: Some("https://example.com/sse".to_string()),
            transport: McpTransport::Sse,
        })
        .unwrap();

        let err = client
            .call_tool("echo", serde_json::json!({"x": 1}))
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("not implemented"));
    }

    #[test]
    fn test_parse_tool_result_jsonrpc_error() {
        let parsed = parse_tool_result(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32000, "message": "boom"}
        }))
        .unwrap();

        assert!(!parsed.success);
        assert_eq!(parsed.error.as_deref(), Some("boom"));
    }
}
