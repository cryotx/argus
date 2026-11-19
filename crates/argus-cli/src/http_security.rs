//! HTTP identities come from server-configured credentials, never role headers.
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::MethodRouter,
};

use argus_core::types::{AuditOutcome, Role};
use argus_security::{
    audit::{AuditEvent, AuditStore, PostgresAuditStore},
    rbac::{Permission, RbacPolicyEngine},
};

pub fn webhook_router<S: Clone + Send + Sync + 'static>(
    telegram_enabled: bool,
    config: &argus_core::config::SocialConfig,
    telegram: MethodRouter<S>,
    qq: MethodRouter<S>,
) -> Router<S> {
    let telegram_mounted = telegram_enabled && !config.telegram_webhook_secret.trim().is_empty();
    let qq_mounted = config.qq.enabled && !config.qq.bot_secret.trim().is_empty();
    for (platform, mounted) in [
        ("telegram", telegram_mounted),
        ("qq", qq_mounted),
        ("feishu", false),
        ("googlechat", false),
        ("whatsapp", false),
    ] {
        tracing::info!(platform, mounted, "Chat webhook mounting status");
    }
    let mut router = Router::new();
    if telegram_mounted {
        router = router.route(
            "/webhook/telegram",
            telegram.layer(axum::middleware::from_fn_with_state(
                Arc::new(config.telegram_webhook_secret.clone()),
                verify_telegram,
            )),
        );
    }
    if qq_mounted {
        router = router.route("/webhook/qq", qq);
    }
    router
}

async fn verify_telegram(
    State(secret): State<Arc<String>>,
    request: Request,
    next: Next,
) -> Response {
    let actual = request
        .headers()
        .get("x-telegram-bot-api-secret-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !token_eq(&secret, actual) {
        return reject(
            StatusCode::UNAUTHORIZED,
            "Telegram webhook authentication failed",
        );
    }
    next.run(request).await
}

#[derive(Clone)]
pub struct HttpActor {
    pub name: String,
    pub role: Role,
}

pub struct HttpSecurity {
    credentials: Vec<(String, Role)>,
    rbac: RbacPolicyEngine,
    audit: tokio::sync::Mutex<Option<AuditEvent>>,
    store: Arc<dyn AuditStore>,
}

impl HttpSecurity {
    pub fn from_env(pool: sqlx::PgPool) -> anyhow::Result<Self> {
        let mut credentials = Vec::new();
        for (key, role) in [
            ("ARGUS_HTTP_ADMIN_TOKEN", Role::Admin),
            ("ARGUS_HTTP_DEVELOPER_TOKEN", Role::Developer),
            ("ARGUS_HTTP_VIEWER_TOKEN", Role::Viewer),
        ] {
            if let Ok(value) = std::env::var(key) {
                if value.trim().is_empty() {
                    continue;
                }
                anyhow::ensure!(
                    value.len() >= 32 && value.bytes().all(|b| b.is_ascii_graphic()),
                    "{key} must contain at least 32 non-whitespace ASCII characters"
                );
                anyhow::ensure!(
                    !credentials.iter().any(|(token, _)| token == &value),
                    "HTTP roles must use distinct credentials"
                );
                credentials.push((value, role));
            }
        }
        Ok(Self {
            credentials,
            rbac: RbacPolicyEngine::new(),
            audit: tokio::sync::Mutex::new(None),
            store: Arc::new(PostgresAuditStore::new(pool)),
        })
    }

    fn authenticate(&self, raw: Option<&str>) -> Option<HttpActor> {
        let token = raw?.strip_prefix("Bearer ")?;
        self.credentials
            .iter()
            .find(|(expected, _)| token_eq(expected, token))
            .map(|(_, role)| HttpActor {
                name: format!("http:{}", role_name(role)),
                role: role.clone(),
            })
    }

    async fn record(
        &self,
        actor: &HttpActor,
        path: &str,
        outcome: AuditOutcome,
        phase: &str,
    ) -> bool {
        let mut head = self.audit.lock().await;
        let event = match AuditEvent::new(
            head.as_ref(),
            &actor.name,
            "http_request",
            outcome,
            Some(path.to_owned()),
            serde_json::json!({"role":role_name(&actor.role), "phase":phase}),
        ) {
            Ok(event) => event,
            Err(_) => return false,
        };
        if self.store.append_event(&event).await.is_err() {
            return false;
        }
        *head = Some(event);
        true
    }
}

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Developer => "developer",
        Role::Viewer => "viewer",
    }
}

fn token_eq(expected: &str, actual: &str) -> bool {
    let mut difference = expected.len() ^ actual.len();
    for (index, byte) in expected.bytes().enumerate() {
        difference |= usize::from(byte ^ actual.as_bytes().get(index).copied().unwrap_or(0));
    }
    difference == 0
}

fn required_permissions(method: &Method, path: &str) -> Vec<Permission> {
    if *method == Method::GET || *method == Method::HEAD {
        return vec![if path == "/api/logs" {
            Permission::ReadAudit
        } else {
            Permission::ReadProject
        }];
    }
    if path.contains("/auth-profiles") {
        return vec![Permission::ManageSecrets];
    }
    if path == "/api/config"
        || path.ends_with("/policy")
        || ["/approve", "/reject", "/apply", "/accept"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
    {
        return vec![Permission::ConfigureSystem];
    }
    if path == "/api/tasks" || path.ends_with("/evaluate") || path.contains("/tasks/") {
        return vec![Permission::WriteCode, Permission::ExecuteCommand];
    }
    vec![Permission::WriteCode]
}

fn reject(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error":message}))).into_response()
}

pub async fn authorize(
    State(security): State<Arc<HttpSecurity>>,
    mut request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let read_only = method == Method::GET || method == Method::HEAD;
    let raw = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    let actor = match security.authenticate(raw) {
        Some(actor) => actor,
        // The local dashboard can read safe operational data without a token.
        None if read_only && !request.headers().contains_key("authorization") => HttpActor {
            name: "anonymous-viewer".into(),
            role: Role::Viewer,
        },
        None => {
            let anonymous = HttpActor {
                name: "anonymous".into(),
                role: Role::Viewer,
            };
            security
                .record(&anonymous, &path, AuditOutcome::Blocked, "authentication")
                .await;
            return reject(
                StatusCode::UNAUTHORIZED,
                "A valid HTTP bearer token is required",
            );
        }
    };
    for permission in required_permissions(&method, &path) {
        if security
            .rbac
            .require_roles(std::slice::from_ref(&actor.role), permission)
            .is_err()
        {
            security
                .record(&actor, &path, AuditOutcome::Blocked, "authorization")
                .await;
            return reject(StatusCode::FORBIDDEN, "Permission denied");
        }
    }
    if !read_only
        && !security
            .record(&actor, &path, AuditOutcome::Success, "authorized")
            .await
    {
        return reject(
            StatusCode::SERVICE_UNAVAILABLE,
            "Audit persistence unavailable; action was not executed",
        );
    }
    request.extensions_mut().insert(actor.clone());
    let response = next.run(request).await;
    if !read_only {
        let outcome = if response.status().is_success() {
            AuditOutcome::Success
        } else {
            AuditOutcome::Failure
        };
        if !security.record(&actor, &path, outcome, "completed").await {
            tracing::error!(path, "HTTP completion audit could not be persisted");
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_security::audit::MemoryAuditStore;
    use axum::{Router, body::Body, middleware::from_fn_with_state, routing::post};
    use tower::ServiceExt;

    fn webhook_app(enabled: bool, secret: &str) -> (Router, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler_calls = Arc::clone(&calls);
        let mut config = argus_core::config::SocialConfig {
            telegram_webhook_secret: secret.into(),
            ..Default::default()
        };
        config.qq.enabled = enabled;
        config.qq.bot_secret = secret.into();
        let app = webhook_router(
            enabled,
            &config,
            post(move || async move {
                handler_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                StatusCode::NO_CONTENT
            }),
            post(|| async { StatusCode::NO_CONTENT }),
        );
        (app, calls)
    }

    async fn webhook_status(app: Router, path: &str, token: Option<&str>) -> StatusCode {
        let mut request = axum::http::Request::builder().method("POST").uri(path);
        if let Some(token) = token {
            request = request.header("X-Telegram-Bot-Api-Secret-Token", token);
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn webhooks_without_secrets_are_not_mounted() {
        for secret in ["", "   "] {
            let (app, calls) = webhook_app(true, secret);
            for platform in ["telegram", "qq", "feishu", "googlechat", "whatsapp"] {
                assert_eq!(
                    webhook_status(app.clone(), &format!("/webhook/{platform}"), None).await,
                    StatusCode::NOT_FOUND
                );
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn disabled_webhooks_with_secrets_are_not_mounted() {
        let (app, calls) = webhook_app(false, "test-webhook-secret");
        for platform in ["telegram", "qq", "feishu", "googlechat", "whatsapp"] {
            assert_eq!(
                webhook_status(app.clone(), &format!("/webhook/{platform}"), None).await,
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn telegram_wrong_or_missing_secret_never_calls_business_handler() {
        let (app, calls) = webhook_app(true, "test-webhook-secret");
        for token in [
            None,
            Some("wrong"),
            Some("test-webhook-secreu"),
            Some("test-webhook-secret-extra"),
        ] {
            assert_eq!(
                webhook_status(app.clone(), "/webhook/telegram", token).await,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn telegram_correct_secret_calls_business_handler() {
        let (app, calls) = webhook_app(true, "test-webhook-secret");
        assert_eq!(
            webhook_status(
                app.clone(),
                "/webhook/telegram",
                Some("test-webhook-secret")
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            webhook_status(app.clone(), "/webhook/qq", None).await,
            StatusCode::NO_CONTENT
        );
        for platform in ["feishu", "googlechat", "whatsapp"] {
            assert_eq!(
                webhook_status(app.clone(), &format!("/webhook/{platform}"), None).await,
                StatusCode::NOT_FOUND
            );
        }
    }

    async fn status(token: Option<&str>, security: HttpSecurity, path: &str) -> StatusCode {
        let app = Router::new()
            .route("/api/tasks", post(|| async { "executed" }))
            .route("/api/work-items/1/accept", post(|| async { "accepted" }))
            .layer(from_fn_with_state(Arc::new(security), authorize));
        let mut request = axum::http::Request::builder().method("POST").uri(path);
        // A caller-supplied role header cannot upgrade its credential's role.
        request = request.header("x-argus-role", "admin");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    fn security() -> HttpSecurity {
        HttpSecurity {
            credentials: vec![
                ("viewer".into(), Role::Viewer),
                ("developer".into(), Role::Developer),
                ("admin".into(), Role::Admin),
            ],
            rbac: RbacPolicyEngine::new(),
            audit: tokio::sync::Mutex::new(None),
            store: Arc::new(MemoryAuditStore::new()),
        }
    }

    #[tokio::test]
    async fn writes_require_auth_and_role_and_acceptance_requires_admin() {
        assert_eq!(
            status(None, security(), "/api/tasks").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(Some("wrong"), security(), "/api/tasks").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(Some("viewer"), security(), "/api/tasks").await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(Some("developer"), security(), "/api/tasks").await,
            StatusCode::OK
        );
        assert_eq!(
            status(Some("developer"), security(), "/api/work-items/1/accept").await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(Some("admin"), security(), "/api/work-items/1/accept").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn audit_failure_blocks_execution() {
        struct Unavailable;
        #[async_trait::async_trait]
        impl AuditStore for Unavailable {
            async fn append_event(
                &self,
                _: &argus_security::audit::AuditEvent,
            ) -> argus_core::types::ArgusResult<()> {
                Err(argus_core::types::ArgusError::Security("offline".into()))
            }
            async fn load_events(
                &self,
                _: usize,
            ) -> argus_core::types::ArgusResult<Vec<argus_security::audit::AuditEvent>>
            {
                Ok(vec![])
            }
        }
        let mut guard = security();
        guard.store = Arc::new(Unavailable);
        let actor = HttpActor {
            name: "http:admin".into(),
            role: Role::Admin,
        };
        assert!(
            !guard
                .record(&actor, "/api/tasks", AuditOutcome::Success, "authorized")
                .await
        );
        assert!(
            guard.audit.lock().await.is_none(),
            "Failed persistence must not advance the audit head"
        );
        assert_eq!(
            status(Some("developer"), guard, "/api/tasks").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
