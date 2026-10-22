// SPDX-License-Identifier: Apache-2.0
use crate::*;
use parking_lot::RwLock;
use std::sync::{atomic::AtomicBool, Arc};
pub fn app() -> Arc<app::App> {
    let base = std::env::temp_dir().join(format!("toktail-test-{}", uuid::Uuid::new_v4()));
    let mut cfg = config::Config::default();
    cfg.capture.enabled = true;
    let ledger = ledger::Ledger::open(base.join("data"), &cfg.default_host_id()).unwrap();
    Arc::new(app::App {
        config: RwLock::new(cfg),
        paths: config::Paths {
            config_file: base.join("config.toml"),
            data_dir: base.join("data"),
            sync_state: base.join("s3.json"),
            base: base.clone(),
        },
        ledger,
        status: RwLock::new(Default::default()),
        sync_now: tokio::sync::Notify::new(),
        rt: tokio::runtime::Handle::current(),
        live: live::Live::new(),
        host_live: RwLock::new(Default::default()),
        prices: RwLock::new(pricing::PriceBook::load(&base)),
        demo: false,
        host_tokens: RwLock::new(Default::default()),
        proxy_restart: tokio::sync::Notify::new(),
        check_upstream: tokio::sync::Notify::new(),
        crypto: crypto::Crypto::new(),
        records: records::Records::new(),
        hardware: RwLock::new(Default::default()),
        activity: live::Activity::new(),
        alerts: budgets::Alerts::new(),
        retrieval: retrieval::Index::new(),
        chat_client: reqwest::Client::new(),
        shutting_down: AtomicBool::new(false),
        cache: cache::PromptCache::new(),
        commentary: commentary::State::new(),
        chains: chain::Chains::new(),
        portal_auth: portal_auth::PortalAuth::new(&base),
        api: api::ApiState::new(),
        autopilot: autopilot::Autopilot::new(),
        summaries: summaries::Summaries::new(),
        host_status: hoststatus::HostStatus::new(),
    })
}
pub fn record(model: &str) -> ledger::Record {
    ledger::Record {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp: chrono::Local::now(),
        model: model.into(),
        endpoint: "/v1/chat/completions".into(),
        streamed: false,
        status: 200,
        input_tokens: 100,
        output_tokens: 50,
        duration_ms: 500,
        ttft_ms: None,
        usage_source: "reported",
        prompt_sha256: String::new(),
        host: "laptop".into(),
        record: String::new(),
        outcome: "complete".into(),
        signal: "brief".into(),
        response_chars: 20,
        prompt_blob: String::new(),
        prompt_key: String::new(),
        response_key: String::new(),
        reference_key: String::new(),
        meta: Default::default(),
    }
}
