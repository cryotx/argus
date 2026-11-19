use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    PrivateKey,
    AwsAccessKey,
    GithubToken,
    OpenAiKey,
    SlackToken,
    GenericCredential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretFinding {
    pub kind: SecretKind,
    pub line: usize,
    pub column: usize,
    pub excerpt: String,
}

#[derive(Debug, Clone)]
pub struct SecretScanner {
    credential_keywords: Vec<String>,
}

impl SecretScanner {
    pub fn new() -> Self {
        Self {
            credential_keywords: vec![
                "api_key".to_string(),
                "apikey".to_string(),
                "token".to_string(),
                "secret".to_string(),
                "password".to_string(),
            ],
        }
    }

    pub fn with_additional_keywords<I, S>(additional_keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut scanner = Self::new();
        for keyword in additional_keywords {
            let keyword = keyword.into();
            if !keyword.trim().is_empty() {
                scanner
                    .credential_keywords
                    .push(keyword.to_ascii_lowercase());
            }
        }
        scanner
    }

    pub fn scan_text(&self, text: &str) -> Vec<SecretFinding> {
        let mut findings = Vec::new();

        for (line_idx, line) in text.lines().enumerate() {
            let line_no = line_idx + 1;
            findings.extend(self.scan_line(line_no, line));
        }

        findings
    }

    fn scan_line(&self, line_no: usize, line: &str) -> Vec<SecretFinding> {
        let mut findings = Vec::new();
        let lower = line.to_ascii_lowercase();

        if let Some(column) = find_marker(line, "-----BEGIN PRIVATE KEY-----") {
            findings.push(SecretFinding {
                kind: SecretKind::PrivateKey,
                line: line_no,
                column,
                excerpt: trim_excerpt(line),
            });
        }

        for prefix in ["ghp_", "github_pat_"] {
            if let Some(column) = find_marker(line, prefix) {
                findings.push(SecretFinding {
                    kind: SecretKind::GithubToken,
                    line: line_no,
                    column,
                    excerpt: trim_excerpt(line),
                });
            }
        }

        if let Some(column) = find_marker(line, "sk-") {
            findings.push(SecretFinding {
                kind: SecretKind::OpenAiKey,
                line: line_no,
                column,
                excerpt: trim_excerpt(line),
            });
        }

        if let Some(column) = find_marker(line, "xoxb-") {
            findings.push(SecretFinding {
                kind: SecretKind::SlackToken,
                line: line_no,
                column,
                excerpt: trim_excerpt(line),
            });
        }

        if let Some(column) = find_aws_access_key(line) {
            findings.push(SecretFinding {
                kind: SecretKind::AwsAccessKey,
                line: line_no,
                column,
                excerpt: trim_excerpt(line),
            });
        }

        if looks_like_generic_credential(&lower, line, &self.credential_keywords) {
            findings.push(SecretFinding {
                kind: SecretKind::GenericCredential,
                line: line_no,
                column: 1,
                excerpt: trim_excerpt(line),
            });
        }

        findings
    }
}

impl Default for SecretScanner {
    fn default() -> Self {
        Self::new()
    }
}

fn find_marker(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle).map(|i| i + 1)
}

fn find_aws_access_key(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    if bytes.len() < 20 {
        return None;
    }

    for i in 0..=(bytes.len() - 20) {
        let window = &bytes[i..(i + 20)];
        if window.starts_with(b"AKIA")
            && window
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Some(i + 1);
        }
    }

    None
}

fn looks_like_generic_credential(lower: &str, original: &str, keywords: &[String]) -> bool {
    let has_key_name = keywords.iter().any(|k| lower.contains(k));
    if !has_key_name {
        return false;
    }

    if lower.contains("example") || lower.contains("dummy") || lower.contains("changeme") {
        return false;
    }

    if let Some((_, value)) = original.split_once('=') {
        return significant_value(value);
    }
    if let Some((_, value)) = original.split_once(':') {
        return significant_value(value);
    }

    false
}

fn significant_value(raw: &str) -> bool {
    let v = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`');

    if v.len() < 16 {
        return false;
    }

    v.chars().any(|c| c.is_ascii_digit()) && v.chars().any(|c| c.is_ascii_alphabetic())
}

fn trim_excerpt(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.len() <= 120 {
        trimmed.to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_private_key() {
        let scanner = SecretScanner::new();
        let findings = scanner.scan_text("-----BEGIN PRIVATE KEY-----");
        assert!(findings.iter().any(|f| f.kind == SecretKind::PrivateKey));
    }

    #[test]
    fn test_detect_openai_and_github() {
        let scanner = SecretScanner::new();
        let findings = scanner.scan_text("OPENAI_API_KEY=sk-1234567890abcdefghijklmnop\nGITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz123456");
        assert!(findings.iter().any(|f| f.kind == SecretKind::OpenAiKey));
        assert!(findings.iter().any(|f| f.kind == SecretKind::GithubToken));
    }

    #[test]
    fn test_detect_aws_access_key() {
        let scanner = SecretScanner::new();
        let findings = scanner.scan_text("AWS_ACCESS_KEY_ID=AKIA1234567890ABCD12");
        assert!(findings.iter().any(|f| f.kind == SecretKind::AwsAccessKey));
    }

    #[test]
    fn test_skip_obvious_placeholder() {
        let scanner = SecretScanner::new();
        let findings = scanner.scan_text("API_KEY=example1234567890123456");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_additional_keyword_detection() {
        let scanner = SecretScanner::with_additional_keywords(["credential"]);
        let findings = scanner.scan_text("CREDENTIAL=abcd1234abcd1234");
        assert!(
            findings
                .iter()
                .any(|f| f.kind == SecretKind::GenericCredential)
        );
    }
}
