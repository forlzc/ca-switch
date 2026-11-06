//! HTTP代理服务器
//!
//! 基于Axum的HTTP服务器，处理代理请求
//!
//! Uses a manual hyper HTTP/1.1 accept loop with `preserve_header_case(true)` so
//! that the original header-name casing from the CLI client is captured in a
//! `HeaderCaseMap` extension.  This map is later forwarded to the upstream via
//! the hyper-based HTTP client, producing wire-level header casing identical to
//! a direct (non-proxied) CLI request.

use super::{
    browser_guard,
    failover_switch::FailoverSwitchManager,
    handlers,
    log_codes::srv as log_srv,
    provider_router::ProviderRouter,
    providers::{codex_chat_history::CodexChatHistoryStore, gemini_shadow::GeminiShadowStore},
    types::*,
    ProxyError,
};
use crate::database::Database;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{any, get, post},
    Router,
};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};
use tokio::task::JoinHandle;

/// 代理服务器状态（共享）
#[derive(Clone)]
pub struct ProxyState {
    pub db: Arc<Database>,
    pub config: Arc<RwLock<ProxyConfig>>,
    pub status: Arc<RwLock<ProxyStatus>>,
    pub start_time: Arc<RwLock<Option<std::time::Instant>>>,
    /// 每个应用类型当前使用的 provider (app_type -> (provider_id, provider_name))
    pub current_providers: Arc<RwLock<std::collections::HashMap<String, (String, String)>>>,
    /// 共享的 ProviderRouter（持有熔断器状态，跨请求保持）
    pub provider_router: Arc<ProviderRouter>,
    /// Gemini Native shadow state，用于 thoughtSignature / tool call 回放
    pub gemini_shadow: Arc<GeminiShadowStore>,
    /// Codex Chat bridge history，用于恢复 previous_response_id 指向的 tool call
    pub codex_chat_history: Arc<CodexChatHistoryStore>,
    /// AppHandle，用于发射事件和更新托盘菜单
    pub app_handle: Option<tauri::AppHandle>,
    /// 故障转移切换管理器
    pub failover_manager: Arc<FailoverSwitchManager>,
}

/// 代理HTTP服务器
pub struct ProxyServer {
    config: ProxyConfig,
    state: ProxyState,
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    /// 服务器任务句柄，用于等待服务器实际关闭
    server_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

#[cfg(test)]
impl ProxyState {
    /// 测试用：用这个数据库、其余都是默认值的状态。
    pub(crate) fn for_test(db: Arc<Database>) -> Self {
        ProxyServer::new(ProxyConfig::default(), db, None).state
    }
}

impl ProxyServer {
    pub fn new(
        config: ProxyConfig,
        db: Arc<Database>,
        app_handle: Option<tauri::AppHandle>,
    ) -> Self {
        // 创建共享的 ProviderRouter（熔断器状态将跨所有请求保持）
        let provider_router = Arc::new(ProviderRouter::new(db.clone()));
        // 创建故障转移切换管理器
        let failover_manager = Arc::new(FailoverSwitchManager::new());

        let state = ProxyState {
            db,
            config: Arc::new(RwLock::new(config.clone())),
            status: Arc::new(RwLock::new(ProxyStatus::default())),
            start_time: Arc::new(RwLock::new(None)),
            current_providers: Arc::new(RwLock::new(std::collections::HashMap::new())),
            provider_router,
            gemini_shadow: Arc::new(GeminiShadowStore::default()),
            codex_chat_history: Arc::new(CodexChatHistoryStore::default()),
            app_handle,
            failover_manager,
        };

        Self {
            config,
            state,
            shutdown_tx: Arc::new(RwLock::new(None)),
            server_handle: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn start(&self) -> Result<ProxyServerInfo, ProxyError> {
        // 检查是否已在运行
        if self.shutdown_tx.read().await.is_some() {
            return Err(ProxyError::AlreadyRunning);
        }

        let addr: SocketAddr =
            format!("{}:{}", self.config.listen_address, self.config.listen_port)
                .parse()
                .map_err(|e| ProxyError::BindFailed(format!("无效的地址: {e}")))?;

        // 创建关闭通道
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // 构建路由
        let app = self.build_router();

        // 绑定监听器
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| ProxyError::BindFailed(e.to_string()))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| ProxyError::BindFailed(e.to_string()))?;
        let actual_port = local_addr.port();

        log::info!("[{}] 代理服务器启动于 {local_addr}", log_srv::STARTED);

        // 更新全局代理端口，用于系统代理检测
        crate::proxy::http_client::set_proxy_port(actual_port);

        // 保存关闭句柄
        *self.shutdown_tx.write().await = Some(shutdown_tx);

        // 更新状态
        let mut status = self.state.status.write().await;
        status.running = true;
        status.address = self.config.listen_address.clone();
        status.port = actual_port;
        drop(status);

        // 记录启动时间
        *self.state.start_time.write().await = Some(std::time::Instant::now());

        // 启动服务器 — 使用手动 hyper HTTP/1.1 accept loop
        // 开启 preserve_header_case 以捕获客户端请求头的原始大小写
        let state = self.state.clone();
        let handle = tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        let (stream, _remote_addr) = match result {
                            Ok(v) => v,
                            Err(e) => {
                                log::error!("[{SRV}] accept 失败: {e}", SRV = log_srv::ACCEPT_ERR);
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                continue;
                            }
                        };

                        let app = app.clone();
                        tokio::spawn(async move {
                            // Peek raw TCP bytes to capture original header casing
                            // before hyper parses (and lowercases) the header names.
                            let original_cases = {
                                let mut peek_buf = vec![0u8; 8192];
                                match stream.peek(&mut peek_buf).await {
                                    Ok(n) => {
                                        let cases = super::hyper_client::OriginalHeaderCases::from_raw_bytes(&peek_buf[..n]);
                                        log::debug!(
                                            "[ProxyServer] Peeked {} bytes, captured {} header casings",
                                            n, cases.cases.len()
                                        );
                                        cases
                                    }
                                    Err(e) => {
                                        log::debug!("[ProxyServer] peek failed (non-fatal): {e}");
                                        super::hyper_client::OriginalHeaderCases::default()
                                    }
                                }
                            };

                            // service_fn 将 axum Router（tower::Service）桥接到 hyper
                            let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                                let mut router = app.clone();
                                let cases = original_cases.clone();
                                async move {
                                    // 将 hyper::body::Incoming 转为 axum::body::Body，保留 extensions
                                    let (mut parts, body) = req.into_parts();

                                    // Insert our own header case map alongside hyper's internal one
                                    parts.extensions.insert(cases);

                                    let body = axum::body::Body::new(body);
                                    let axum_req = http::Request::from_parts(parts, body);
                                    <Router as tower::Service<http::Request<axum::body::Body>>>::call(&mut router, axum_req).await
                                }
                            });

                            if let Err(e) = hyper::server::conn::http1::Builder::new()
                                .preserve_header_case(true)
                                .serve_connection(TokioIo::new(stream), service)
                                .await
                            {
                                // Connection reset / broken pipe 等在代理场景下很常见，debug 级别
                                log::debug!("[{SRV}] connection error: {e}", SRV = log_srv::CONN_ERR);
                            }
                        });
                    }
                    _ = &mut shutdown_rx => {
                        break;
                    }
                }
            }

            // 服务器停止后更新状态
            state.status.write().await.running = false;
            *state.start_time.write().await = None;
        });

        // 保存服务器任务句柄
        *self.server_handle.write().await = Some(handle);

        Ok(ProxyServerInfo {
            address: self.config.listen_address.clone(),
            port: actual_port,
            started_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    pub async fn stop(&self) -> Result<(), ProxyError> {
        // 1. 发送关闭信号
        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send(());
        } else {
            return Err(ProxyError::NotRunning);
        }

        // 2. 等待服务器任务结束（带 5 秒超时保护）
        if let Some(handle) = self.server_handle.write().await.take() {
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {
                    log::info!("[{}] 代理服务器已完全停止", log_srv::STOPPED);
                    Ok(())
                }
                Ok(Err(e)) => {
                    log::warn!("[{}] 代理服务器任务异常终止: {e}", log_srv::TASK_ERROR);
                    Err(ProxyError::StopFailed(e.to_string()))
                }
                Err(_) => {
                    log::warn!(
                        "[{}] 代理服务器停止超时（5秒），强制继续",
                        log_srv::STOP_TIMEOUT
                    );
                    Err(ProxyError::StopTimeout)
                }
            }
        } else {
            Ok(())
        }
    }

    pub async fn get_status(&self) -> ProxyStatus {
        let mut status = self.state.status.read().await.clone();

        // 计算运行时间
        if let Some(start) = *self.state.start_time.read().await {
            status.uptime_seconds = start.elapsed().as_secs();
        }

        // 从 current_providers HashMap 获取每个应用类型当前正在使用的 provider
        let current_providers = self.state.current_providers.read().await;
        status.active_targets = current_providers
            .iter()
            .map(|(app_type, (provider_id, provider_name))| ActiveTarget {
                app_type: app_type.clone(),
                provider_id: provider_id.clone(),
                provider_name: provider_name.clone(),
            })
            .collect();

        status
    }

    /// 更新某个应用类型当前“目标供应商”（用于 UI 展示 active_targets）
    ///
    /// 注意：这不代表该供应商一定已经处理过请求，而是用于“热切换/启用故障转移立即切 P1”
    /// 等场景下，让 UI 能立刻反映最新目标。
    pub async fn set_active_target(&self, app_type: &str, provider_id: &str, provider_name: &str) {
        let mut current_providers = self.state.current_providers.write().await;
        current_providers.insert(
            app_type.to_string(),
            (provider_id.to_string(), provider_name.to_string()),
        );
    }

    fn build_router(&self) -> Router {
        Router::new()
            // 健康检查
            .route("/health", get(handlers::health_check))
            .route("/status", get(handlers::get_status))
            // Claude API (支持带前缀和不带前缀两种格式)
            .route("/v1/messages", post(handlers::handle_messages))
            .route("/claude/v1/messages", post(handlers::handle_messages))
            // OpenAI Chat Completions API (Codex CLI，支持带前缀和不带前缀)
            .route("/chat/completions", post(handlers::handle_chat_completions))
            .route(
                "/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/v1/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            .route(
                "/codex/v1/chat/completions",
                post(handlers::handle_chat_completions),
            )
            // OpenAI Models API (Codex CLI reachability check)
            .route("/models", get(handlers::handle_models))
            .route("/v1/models", get(handlers::handle_models))
            // OpenAI Responses API (Codex CLI，支持带前缀和不带前缀)。GET 是 WebSocket
            // 握手：Codex 内置的 openai（代理的官方路由）会先试 WebSocket。
            .route(
                "/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            .route(
                "/v1/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            .route(
                "/v1/v1/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            .route(
                "/codex/v1/responses",
                post(handlers::handle_responses).get(handlers::handle_responses_websocket),
            )
            // Grok Build uses the Responses protocol but has an independent
            // provider namespace and failover queue.

            // OpenAI Responses Compact API (Codex CLI 远程压缩，透传)
            .route(
                "/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/v1/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            .route(
                "/codex/v1/responses/compact",
                post(handlers::handle_responses_compact),
            )
            // Codex standalone Alpha Search API. All local aliases normalize to
            // the selected provider's canonical sibling `/alpha/search` route.
            .route("/alpha/search", post(handlers::handle_alpha_search))
            .route("/v1/alpha/search", post(handlers::handle_alpha_search))
            .route("/v1/v1/alpha/search", post(handlers::handle_alpha_search))
            .route(
                "/codex/v1/alpha/search",
                post(handlers::handle_alpha_search),
            )
            // Codex built-in ImageGen still calls the legacy OpenAI Images API.
            .route(
                "/images/generations",
                post(handlers::handle_images_generations),
            )
            .route(
                "/v1/images/generations",
                post(handlers::handle_images_generations),
            )
            .route(
                "/v1/v1/images/generations",
                post(handlers::handle_images_generations),
            )
            .route(
                "/codex/v1/images/generations",
                post(handlers::handle_images_generations),
            )
            // Codex ImageGen posts to `/images/edits` when it references existing images.
            .route("/images/edits", post(handlers::handle_images_edits))
            .route("/v1/images/edits", post(handlers::handle_images_edits))
            .route("/v1/v1/images/edits", post(handlers::handle_images_edits))
            .route(
                "/codex/v1/images/edits",
                post(handlers::handle_images_edits),
            )
            // Gemini API (支持带前缀和不带前缀)
            //
            // 用 `any(..)` 覆盖所有 HTTP 方法：除了 POST `:generateContent` /
            // `:streamGenerateContent` / `:countTokens` 之外，Gemini SDK / CLI 还会发
            // GET `/models`、GET `/models/<id>` 等只读端点。如果只挂 POST，这些 GET
            // 请求会在路由层 404，绕过本地代理的统计、整流和故障转移。
            .route("/v1beta/*path", any(handlers::handle_gemini))
            .route("/gemini/v1beta/*path", any(handlers::handle_gemini))
            // Gemini 的 GA 版本也叫 /v1，给原 SDK 留一条出口
            .route("/gemini/v1/*path", any(handlers::handle_gemini))
            // 提高默认请求体大小限制（避免 413 Payload Too Large）
            .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
            // 拒绝网页发来的请求（最外层，读请求体之前）：见 browser_guard
            .layer(middleware::from_fn_with_state(
                self.state.clone(),
                browser_guard::reject_browser_requests,
            ))
            .with_state(self.state.clone())
    }

    /// 在不重启服务的情况下更新运行时配置
    pub async fn apply_runtime_config(&self, config: &ProxyConfig) {
        *self.state.config.write().await = config.clone();
    }

    /// 热更新熔断器配置
    ///
    /// 将新配置应用到所有已创建的熔断器实例
    pub async fn update_circuit_breaker_configs(
        &self,
        config: super::circuit_breaker::CircuitBreakerConfig,
    ) {
        self.state.provider_router.update_all_configs(config).await;
    }

    pub async fn update_circuit_breaker_config_for_app(
        &self,
        app_type: &str,
        config: super::circuit_breaker::CircuitBreakerConfig,
    ) {
        self.state
            .provider_router
            .update_app_configs(app_type, config)
            .await;
    }

    /// 重置指定 Provider 的熔断器
    pub async fn reset_provider_circuit_breaker(&self, provider_id: &str, app_type: &str) {
        self.state
            .provider_router
            .reset_provider_breaker(provider_id, app_type)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, ProviderMeta};
    use axum::http::{header, StatusCode};
    use serde_json::{json, Value};

    /// Windows 上监听 socket 不能被子进程继承：从 CC Switch 拉起的终端、CLI 一旦继承它，
    /// 退出后端口仍被占着，下次启动报 os error 10048（#8552）。mio 1.2.1 起才带
    /// WSA_FLAG_NO_HANDLE_INHERIT，这里防止 Cargo.lock 退回旧版本。
    #[cfg(windows)]
    #[tokio::test]
    async fn listener_socket_is_not_inheritable_on_windows() {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind listener");
        let mut flags = 0u32;
        let ok = unsafe { GetHandleInformation(listener.as_raw_socket() as HANDLE, &mut flags) };
        assert_ne!(ok, 0, "GetHandleInformation failed");
        assert_eq!(
            flags & HANDLE_FLAG_INHERIT,
            0,
            "listener socket is inheritable"
        );
    }

    /// Codex 内置的 openai 先用 WebSocket 连 Responses：握手回 426 它才立刻改走 HTTP。
    #[tokio::test]
    async fn responses_websocket_handshake_is_answered_with_upgrade_required() {
        let db = Arc::new(Database::memory().expect("memory database"));
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db,
            None,
        );
        let proxy_info = proxy.start().await.expect("start test proxy");
        let client = reqwest::Client::new();
        for path in [
            "/responses",
            "/v1/responses",
            "/v1/v1/responses",
            "/codex/v1/responses",
        ] {
            let response = client
                .get(format!("http://127.0.0.1:{}{path}", proxy_info.port))
                .header(header::CONNECTION, "Upgrade")
                .header(header::UPGRADE, "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .send()
                .await
                .expect("websocket handshake");
            assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED, "{path}");
        }
        proxy.stop().await.expect("stop test proxy");
    }

    /// Regression for #3787: a provider accidentally pointing at the proxy must
    /// fail before it creates another request, even with a dynamically bound port.
    #[tokio::test]
    async fn codex_upstream_self_forwarding_is_rejected() {
        let db = Arc::new(Database::memory().expect("memory database"));
        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                ..ProxyConfig::default()
            },
            db.clone(),
            None,
        );
        let info = proxy.start().await.expect("start proxy");
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let loopback = std::net::Ipv4Addr::LOCALHOST.to_string();
        for (host, full_url, stream) in
            [(loopback.as_str(), false, true), ("localhost", true, false)]
        {
            let mut provider = Provider::with_id(
                "self-forwarding".to_string(),
                "Self forwarding".to_string(),
                json!({
                    "base_url": format!("http://{host}:{}/v1{}", info.port, if full_url { "/responses" } else { "" }),
                    "auth": {"OPENAI_API_KEY": "upstream-secret"},
                }),
                None,
            );
            provider.meta = Some(ProviderMeta {
                is_full_url: Some(full_url),
                ..ProviderMeta::default()
            });
            db.save_provider("codex", &provider).unwrap();
            db.set_current_provider("codex", &provider.id).unwrap();
            let before = proxy.get_status().await.total_requests;
            let request = client
                .post(format!(
                    "http://{}:{}/v1/responses",
                    info.address, info.port
                ))
                .json(&json!({
                    "model": "gpt-5",
                    "stream": stream,
                    "input": [{"role": "user", "content": "x".repeat(256 * 1024)}],
                }));
            // Bound the regression itself if the guard is ever removed: stop
            // after eight internal requests instead of letting the loop grow.
            let response = tokio::select! {
                response = request.send() => response.expect("send request"),
                _ = async {
                    while proxy.get_status().await.total_requests < before + 8 {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                } => {
                    proxy.stop().await.unwrap();
                    panic!("provider request looped back into the proxy");
                }
            };
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let error: Value = response.json().await.unwrap();
            assert!(error.to_string().contains("own proxy listener"), "{error}");
            assert_eq!(proxy.get_status().await.total_requests, before + 1);
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while proxy.get_status().await.active_connections != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("rejected request releases its connection");
        }
        proxy.stop().await.unwrap();
    }

    /// 真实上游协议转换验收（默认跳过，避免 CI 花钱）。
    ///
    /// ```bash
    /// E2E_UPSTREAM_API_KEY=... \
    /// E2E_UPSTREAM_BASE_URL=https://bailucode.com/openapi/v1 \
    /// E2E_UPSTREAM_MODEL=bailu-2.7 \
    /// cargo test --lib real_upstream_protocol_matrix -- --ignored --nocapture
    /// ```
    ///
    /// 覆盖：同一入口下 Anthropic / OpenAI Chat / OpenAI Responses 三种上游；
    /// 以及入口协议与上游协议不一致时的转换和明确返回 501 的格子。
    #[tokio::test]
    #[ignore = "需要真实上游 API Key，手动运行"]
    async fn real_upstream_protocol_matrix() {
        // 应用启动时会安装 rustls provider；单测进程没有 Tauri 启动流程，需手动安装。
        let _ = rustls::crypto::ring::default_provider().install_default();
        let Ok(api_key) = std::env::var("E2E_UPSTREAM_API_KEY") else {
            eprintln!("跳过：未设置 E2E_UPSTREAM_API_KEY");
            return;
        };
        let base_url = std::env::var("E2E_UPSTREAM_BASE_URL")
            .unwrap_or_else(|_| "https://bailucode.com/openapi/v1".to_string());
        let model = std::env::var("E2E_UPSTREAM_MODEL")
            .unwrap_or_else(|_| "bailu-2.7".to_string());
        // 沙箱/CI 里直连外网可能被拦，显式给上游客户端配一个出口代理。
        if let Ok(proxy) = std::env::var("E2E_UPSTREAM_PROXY") {
            crate::proxy::http_client::apply_proxy(Some(&proxy))
                .expect("apply upstream proxy");
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap();

        for (upstream, cells) in [
            (
                "anthropic",
                vec![("anthropic", 200), ("openai_responses", 200), ("openai_chat", 200)],
            ),
            (
                "openai_responses",
                vec![("anthropic", 200), ("openai_responses", 200), ("openai_chat", 200)],
            ),
            (
                "openai_chat",
                vec![("anthropic", 200), ("openai_responses", 200), ("openai_chat", 200)],
            ),
        ] {
            let db = Arc::new(Database::memory().expect("memory database"));
            let mut provider = Provider::with_id(
                format!("real-{upstream}"),
                format!("Real upstream ({upstream})"),
                json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": base_url.clone(),
                        "ANTHROPIC_AUTH_TOKEN": api_key.clone(),
                    },
                }),
                None,
            );
            // 协议由 meta.apiFormat 落库（与生产路径一致），
            // 不能只写 settings_config，否则推断会退化成家族默认的 Anthropic。
            provider.meta = Some(ProviderMeta {
                api_format: Some(upstream.to_string()),
                ..ProviderMeta::default()
            });
            db.save_provider("claude", &provider).expect("save provider");
            db.set_current_provider("claude", &provider.id).expect("set current");

            let proxy = ProxyServer::new(
                ProxyConfig {
                    listen_port: 0,
                    enable_logging: false,
                    non_streaming_timeout: 120,
                    ..ProxyConfig::default()
                },
                db,
                None,
            );
            let info = proxy.start().await.expect("start proxy");
            let proxy_url = format!("http://127.0.0.1:{}", info.port);

            let build_request = |inbound: &str, stream: bool| -> (&'static str, Value) {
                match inbound {
                    "anthropic" => (
                        "/v1/messages",
                        json!({
                            "model": model.clone(),
                            "max_tokens": 128,
                            "stream": stream,
                            "messages": [{"role": "user", "content": "只回复 OK"}],
                        }),
                    ),
                    "openai_chat" => (
                        "/v1/chat/completions",
                        json!({
                            "model": model.clone(),
                            "max_tokens": 128,
                            "stream": stream,
                            "messages": [{"role": "user", "content": "只回复 OK"}],
                        }),
                    ),
                    _ => (
                        "/v1/responses",
                        json!({
                            "model": model.clone(),
                            "input": "只回复 OK",
                            "max_output_tokens": 128,
                            "stream": stream,
                        }),
                    ),
                }
            };

            // 非流式验证响应形状；流式验证 SSE 事件也是入口协议的形状。
            for stream in [false, true] {
                for (inbound, expected) in &cells {
                    let expected = *expected as u16;
                    let (path, body) = build_request(inbound, stream);
                    let response = client
                        .post(format!("{proxy_url}{path}"))
                        .header("x-api-key", "client-placeholder")
                        .header("anthropic-version", "2023-06-01")
                        .json(&body)
                        .send()
                        .await
                        .expect("real upstream request");
                    let status = response.status().as_u16();
                    let text = response.text().await.unwrap_or_default();
                    let snippet: String = text.chars().take(160).collect();
                    println!("[{upstream}] {inbound} (stream={stream}) -> {status}: {snippet}");
                    if status != expected {
                        println!("[{upstream}] {inbound} 完整响应: {text}");
                    }
                    assert_eq!(status, expected, "[{upstream}] {inbound} 状态码异常");
                    if expected != 200 {
                        continue;
                    }
                    assert!(
                        text.contains("OK"),
                        "[{upstream}] {inbound} 响应未包含 OK: {text}"
                    );
                    if stream {
                        // 流式：事件名必须属于入口协议。
                        let marker_ok = match *inbound {
                            "anthropic" => {
                                text.contains("message_start")
                                    || text.contains("content_block_delta")
                                    || text.contains("\"type\":\"message\"")
                            }
                            "openai_chat" => {
                                text.contains("chat.completion.chunk")
                                    || text.contains("[DONE]")
                                    || text.contains("\"object\":\"chat.completion\"")
                            }
                            _ => {
                                text.contains("response.created")
                                    || text.contains("response.completed")
                                    || text.contains("response.output_text.delta")
                                    || text.contains("\"object\":\"response\"")
                            }
                        };
                        assert!(
                            marker_ok,
                            "[{upstream}] {inbound} 流式事件不是 {inbound} 协议: {text}"
                        );
                    } else {
                        // 只校验状态码会把「透传了错误协议的响应体」也算通过：
                        // 入口是什么协议，回来的响应必须也是那个形状。
                        let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|e| {
                            panic!("[{upstream}] {inbound} 响应不是 JSON: {e}; body={text}")
                        });
                        let shape_ok = match *inbound {
                            "anthropic" => parsed["type"].as_str() == Some("message"),
                            "openai_chat" => {
                                parsed["object"].as_str() == Some("chat.completion")
                                    || parsed["object"].as_str() == Some("chat.completion.chunk")
                            }
                            _ => parsed["object"].as_str() == Some("response"),
                        };
                        assert!(
                            shape_ok,
                            "[{upstream}] {inbound} 响应形状不是 {inbound} 协议: {text}"
                        );
                    }
                }
            }

            proxy.stop().await.expect("stop proxy");
        }
    }

    /// 起一个指向真实上游的本地代理，`api_format` 决定上游协议。
    ///
    /// 与 `real_upstream_protocol_matrix` 共用同一套落库方式：协议写在
    /// `meta.apiFormat`，与生产路径一致。
    async fn start_real_upstream_proxy(
        api_format: &str,
        base_url: &str,
        api_key: &str,
    ) -> (ProxyServer, String) {
        let db = Arc::new(Database::memory().expect("memory database"));
        let mut provider = Provider::with_id(
            format!("real-{api_format}"),
            format!("Real upstream ({api_format})"),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": base_url,
                    "ANTHROPIC_AUTH_TOKEN": api_key,
                },
            }),
            None,
        );
        provider.meta = Some(ProviderMeta {
            api_format: Some(api_format.to_string()),
            ..ProviderMeta::default()
        });
        db.save_provider("claude", &provider).expect("save provider");
        db.set_current_provider("claude", &provider.id).expect("set current");

        let proxy = ProxyServer::new(
            ProxyConfig {
                listen_port: 0,
                enable_logging: false,
                non_streaming_timeout: 120,
                ..ProxyConfig::default()
            },
            db,
            None,
        );
        let info = proxy.start().await.expect("start proxy");
        let url = format!("http://127.0.0.1:{}", info.port);
        (proxy, url)
    }

    /// Chat 客户端 × Responses 上游（原 CC Switch 不支持的一格）专项验证。
    ///
    /// 矩阵测试只发纯文本，这里补上这条链路真正容易出问题的部分：
    /// 工具定义 / 工具调用响应 / 多轮 tool 消息回传 / 流式工具分块与 usage。
    ///
    /// ```bash
    /// E2E_UPSTREAM_API_KEY=... E2E_UPSTREAM_BASE_URL=https://bailucode.com/openapi/v1 \
    /// E2E_UPSTREAM_MODEL=bailu-2.7 \
    /// cargo test --lib real_upstream_chat_client_responses_provider -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "需要真实上游 API Key，手动运行"]
    async fn real_upstream_chat_client_responses_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let Ok(api_key) = std::env::var("E2E_UPSTREAM_API_KEY") else {
            eprintln!("跳过：未设置 E2E_UPSTREAM_API_KEY");
            return;
        };
        let base_url = std::env::var("E2E_UPSTREAM_BASE_URL")
            .unwrap_or_else(|_| "https://bailucode.com/openapi/v1".to_string());
        let model = std::env::var("E2E_UPSTREAM_MODEL")
            .unwrap_or_else(|_| "bailu-2.7".to_string());
        if let Ok(proxy) = std::env::var("E2E_UPSTREAM_PROXY") {
            crate::proxy::http_client::apply_proxy(Some(&proxy))
                .expect("apply upstream proxy");
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap();

        let (proxy, proxy_url) =
            start_real_upstream_proxy("openai_responses", &base_url, &api_key).await;

        let tools = json!([{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "查询指定城市的当前天气",
                "parameters": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"]
                }
            }
        }]);
        let system = json!({"role": "system", "content": "你是天气助手，需要天气数据时必须调用 get_weather。"});
        let user = json!({"role": "user", "content": "上海现在天气怎么样？必须调用 get_weather 查询，不要凭记忆回答。"});

        // ---- 1. 非流式 + 工具调用 ----
        let body = json!({
            "model": model,
            "max_tokens": 512,
            "stream": false,
            "tools": tools,
            "tool_choice": "auto",
            "messages": [system, user],
        });
        let response = client
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .expect("chat request");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        println!("[chat×responses] 非流式工具调用 -> {status}: {text}");
        assert_eq!(status, 200, "非流式工具调用失败: {text}");
        let parsed: Value = serde_json::from_str(&text).expect("chat completion JSON");
        assert_eq!(parsed["object"], "chat.completion");
        let message = &parsed["choices"][0]["message"];
        let tool_calls = message["tool_calls"]
            .as_array()
            .unwrap_or_else(|| panic!("响应里没有 tool_calls: {text}"));
        assert!(!tool_calls.is_empty(), "tool_calls 为空: {text}");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
        let arguments: Value = serde_json::from_str(
            tool_calls[0]["function"]["arguments"]
                .as_str()
                .unwrap_or("{}"),
        )
        .unwrap_or_else(|e| panic!("工具参数不是合法 JSON: {e}; {text}"));
        assert!(
            arguments.get("city").is_some(),
            "工具参数缺少 city: {text}"
        );
        assert_eq!(parsed["choices"][0]["finish_reason"], "tool_calls");
        assert!(
            parsed["usage"]["prompt_tokens"].as_u64().unwrap_or(0) > 0,
            "usage 未映射: {text}"
        );

        // ---- 2. 多轮：把 tool 结果回传，验证 function_call / function_call_output ----
        let call = tool_calls[0].clone();
        let call_id = call["id"].as_str().unwrap_or_default().to_string();
        println!("[chat×responses] 第一轮 call_id={call_id}");
        let second = json!({
            "model": model,
            "max_tokens": 512,
            "stream": false,
            "tools": tools,
            "messages": [
                system,
                user,
                {"role": "assistant", "content": null, "tool_calls": [call]},
                {"role": "tool", "tool_call_id": call_id, "content": "上海：晴，24℃，东南风 2 级"},
            ],
        });
        let response = client
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&second)
            .send()
            .await
            .expect("second turn");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        println!("[chat×responses] 多轮回传 -> {status}: {text}");
        assert_eq!(status, 200, "多轮回传失败: {text}");
        let parsed: Value = serde_json::from_str(&text).expect("second completion JSON");
        let content = parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !content.is_empty(),
            "多轮后回复为空（tool 结果可能没送到上游）: {text}"
        );
        assert!(
            content.contains("24") || content.contains("晴"),
            "回复没有用上 tool 结果: {text}"
        );

        // ---- 3. 流式 + 工具调用：分块必须是 chat.completion.chunk 形状 ----
        let body = json!({
            "model": model,
            "max_tokens": 512,
            "stream": true,
            "tools": tools,
            "tool_choice": "auto",
            "messages": [system, user],
        });
        let response = client
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .expect("streaming chat request");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        println!("[chat×responses] 流式工具调用 -> {status}: {text}");
        assert_eq!(status, 200, "流式工具调用失败: {text}");
        assert!(
            text.contains("chat.completion.chunk"),
            "流式分块不是 Chat 形状: {text}"
        );
        assert!(
            text.contains("\"tool_calls\""),
            "流式没有工具调用分块: {text}"
        );
        assert!(
            text.contains("\"finish_reason\":\"tool_calls\""),
            "流式 finish_reason 不是 tool_calls: {text}"
        );
        assert!(text.contains("[DONE]"), "流式缺少 [DONE]: {text}");

        proxy.stop().await.expect("stop proxy");
    }

    /// 5 个没有转换器的格子必须返回 501 且文案可读。
    ///
    /// 矩阵是唯一权威（`Matrix::unsupported_cells`），这里按它逐格发真实请求，
    /// 保证「矩阵说不行」与「代理真的拒绝」一致，且拒绝发生在上游请求之前
    /// （不配真实上游也能跑）。
    #[tokio::test]
    async fn unsupported_protocol_cells_return_501_with_readable_message() {
        use crate::protocol::{Matrix, Protocol};

        // 不需要真实上游：501 在发起请求前就返回，base URL 指向不可达地址即可。
        if let Ok(proxy) = std::env::var("E2E_UPSTREAM_PROXY") {
            let _ = crate::proxy::http_client::apply_proxy(Some(&proxy));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();

        let path_of = |protocol: Protocol| match protocol {
            Protocol::Anthropic => "/v1/messages",
            Protocol::OpenAiChat => "/v1/chat/completions",
            Protocol::OpenAiResponses => "/v1/responses",
            Protocol::Gemini => "/v1beta/models/test-model:generateContent",
        };
        let body_of = |protocol: Protocol| match protocol {
            Protocol::Anthropic => json!({
                "model": "test-model",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            }),
            Protocol::OpenAiChat => json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hi"}],
            }),
            Protocol::OpenAiResponses => json!({
                "model": "test-model",
                "input": "hi",
            }),
            Protocol::Gemini => json!({
                "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            }),
        };

        let cells = Matrix::unsupported_cells();
        assert_eq!(cells.len(), 5, "不支持格数量应保持 5: {cells:?}");

        for (inbound, upstream) in cells {
            let db = Arc::new(Database::memory().expect("memory database"));
            let mut provider = Provider::with_id(
                format!("unsupported-{}", upstream.as_str()),
                "Unsupported cell".to_string(),
                json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://upstream.invalid",
                        "ANTHROPIC_AUTH_TOKEN": "test-key",
                    },
                }),
                None,
            );
            provider.meta = Some(ProviderMeta {
                api_format: Some(upstream.as_str().to_string()),
                ..ProviderMeta::default()
            });
            db.save_provider("claude", &provider).expect("save provider");
            db.set_current_provider("claude", &provider.id).expect("set current");

            let proxy = ProxyServer::new(
                ProxyConfig {
                    listen_port: 0,
                    enable_logging: false,
                    ..ProxyConfig::default()
                },
                db,
                None,
            );
            let info = proxy.start().await.expect("start proxy");

            let response = client
                .post(format!(
                    "http://127.0.0.1:{}{}",
                    info.port,
                    path_of(inbound)
                ))
                .json(&body_of(inbound))
                .send()
                .await
                .expect("request to local proxy");
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            assert_eq!(
                status, 501,
                "[{} → {}] 应返回 501: {text}",
                inbound.as_str(),
                upstream.as_str()
            );
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let message = parsed["error"]["message"]
                .as_str()
                .or_else(|| parsed["message"].as_str())
                .unwrap_or(&text);
            assert!(
                message.contains("没有可用的转换器"),
                "[{} → {}] 501 文案不可读: {text}",
                inbound.as_str(),
                upstream.as_str()
            );
            assert!(
                message.contains(inbound.label()) && message.contains(upstream.label()),
                "[{} → {}] 501 文案未标明两种协议: {text}",
                inbound.as_str(),
                upstream.as_str()
            );

            proxy.stop().await.expect("stop proxy");
        }
    }
}
