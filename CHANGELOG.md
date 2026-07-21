# Changelog

All notable changes to Claude Code Mux will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.3-eli.1] - 2026-07-14

### Added
- **Auth gateways** (opt-in, backward compatible):
  - `server.api_key`: when set, LLM proxy endpoints (`/v1/messages`,
    `/v1/chat/completions`, `/v1/models`, `/v1/messages/count_tokens`)
    require `x-api-key: <key>` OR `Authorization: Bearer <key>` (standard
    Anthropic/OpenAI conventions — existing clients work unmodified).
    Unset/empty → endpoints open (backward compatible for local use).
  - `server.admin_key`: when set, admin routes (`/api/config/json`,
    `/api/reload`, `/api/logs`, `/api/logs/stream`, `/api/i18n`, `/`)
    require `x-ccm-admin-key: <key>`. Unset/empty → admin open.
    The SSE stream endpoint `/api/logs/stream` also accepts the key via
    `?key=` query param (EventSource cannot set custom headers), header
    still takes precedence; query fallback is restricted to `*/stream`
    paths only (see admin UI live-logs viewer).
  - Both keys support `$ENV_VAR` syntax (resolved at config load).
  - Admin UI Settings tab now has an "Admin Access Key" card — the key is
    stored in the browser's localStorage and auto-attached to every admin
    fetch via a `window.fetch` monkey-patch (no per-call changes).
  - Auth headers `x-api-key`, `authorization`, `x-ccm-admin-key` added to
    `headers::BLOCK_LIST` — they are **never forwarded to upstream providers**,
    preventing client CCM-auth credentials from leaking to or conflicting with
    provider credentials.
  - Constant-time key comparison mitigates timing side-channels (length leaked
    via early-return; acceptable for high-entropy keys — documented).
  - OAuth token **management** endpoints (`/api/oauth/tokens`,
    `/api/oauth/tokens/delete`, `/api/oauth/tokens/refresh`) moved behind the
    admin_key gate (they can enumerate/delete/refresh tokens). The OAuth
    **flow** endpoints (authorize/exchange/callback, `/auth/callback`) stay
    public as they come from browser redirects.
  - `reload_config` now re-runs `resolve_env_vars()` after disk reparse, so
    `$ENV_VAR` keys stay resolved post-reload (previously left as literal
    `$VAR` → auth breakage).
- Fork-specific: OpenAI-compatible `/v1/chat/completions` tool/function calling support
- OpenAI-compatible `/v1/models` endpoint (aggregates provider models, BTreeSet dedup)
- Message tracing with BufWriter (buffered I/O, lazy flush on /api/logs)
- Live logs viewer: `/api/logs` (paginated) and `/api/logs/stream` (real-time SSE)
- Admin UI i18n (zh-CN + en, 173 keys each; P5 后续 cost_first / P6 commit 又加 4 keys → 现 177 keys each)
- Admin UI dark theme (system prefers-color-scheme + manual toggle)
- Admin UI URL-based tab navigation (`?tab=router`, `?tab=logs`, etc.)
- Router rules: `[[router.rules]]` with model-prefix and condition type, request rewrites
- Long context routing (`long_context` + `long_context_threshold`)
- Subagent model routing via `<CCM-SUBAGENT-MODEL>` tags
- `config.example.toml` with fully commented all-options example
- Docker image: `ghcr.io/winffychu/claude-code-mux:latest` (distroless, musl static)
- `write_routing_info` moved to `tokio::task::spawn_blocking` (non-blocking)
- Cached regex for subagent model extraction (`Lazy<Regex>`)

### Changed
- Router autoSave: localStorage only; 💾 Save button syncs to server + hot-reload
- Routing order now switchable via `cost_first` (default `false`):
  - `cost_first = false` (default — think-first, matches upstream 9j): auto-map → websearch → subagent → think → background → router.rules → prompt_rules → long_context → default
  - `cost_first = true` (cost-first, matches elidickinson fork): auto-map → websearch → background → subagent → router.rules → prompt_rules → think → long_context → default
- Admin UI: new "Cost-First Routing" toggle in Settings tab exposing `cost_first` (sync + reload on save); zh-CN + en i18n
- New `docs/think-routing.md §11.14` concurrent stress test against real LLM (NVIDIA/Llama-3.1-8B): 280 req / 0 failures, 38.5 req/s peak, route distribution matches both modes' theory precisely
- New `docs/think-routing.md §11.15` full 9 routing-branch e2e coverage (web-search/subagent/think/background/router-rule/prompt-rule/long-context/auto_map/default) × `cost_first` dual mode = 20 req / 0 failures, `[:sync]` tags match theory per branch
- New `docs/think-routing.md §11.16` edge / fallback real-LLM coverage (empty rules pass-through to default, route-target=unknown-model → HTTP 502 fail-fast, 1:N mapping retry with bad primary → successful fallback, all-N-mappings-failed → HTTP 502 fail-fast) × `cost_first` dual mode = 10 req (4 base + 4 base + 2 all-fail), all behaviors verified by server-log traces including `[2/2]` retry indicator and `❌ All 2 provider mappings failed`
- Docs split: §11.10 / §11.11 / §11.14-11.16 真机实证内容 (408 行) 拆出到独立 `docs/routing-e2e-evidence.md` (421 行)，`think-routing.md` 保留设计主干 + anchor 引用，总长度 1009→614 行（↓ 39%），原 §11.X 编号保留以提高双文档互引稳定性
- P6 (max_entries) audit: P6 内存日志架构 (commit `6b877af`, 2026-07-16) post-R2 阶段 R7 真机复验补修。`get_config_json` / `update_config_json` / `update_tracing_field` 三处补上 `tracing.max_entries` 字段同步（原先 P6 commit 只在 cli/message_tracing/logs 三处加 max_entries，admin UI 调出/保存时该字段缺失）。真机 e2e 验证：3 真实 LLM 请求 → `/api/logs` 即时返 8 entries / tracing_enabled:true；`/api/logs/stream` SSE backlog + live broadcast 实测 10 data event；POST `/api/config/json` + `/api/reload` 把 max_entries 2000→500 e2e 通过
- `update_config_json`: `tokio::fs::write` (async, non-blocking) instead of `std::fs::write`
- RwLock poison recovery: `unwrap_or_else(|e| e.into_inner())` on all lock sites
- `openai.rs:769`: `.expect()` → `match` returning error response (no panic on empty choices)
- Stream state mutex: poison recovery instead of `.unwrap()` cascade
- `SystemTime::now().duration_since()`: `.unwrap_or(0)` (NTP clock rollback safe)
- `response_builder.body()`: `.unwrap()` → `match` with error fallback

### Fixed
- `update_config_json`: `router.rules` / `router.prompt_rules` Vec arrays now persisted on PATCH (previously silent no-op — `update_field` helper only handled scalar string fields, so a JSON POST with rules/prompt_rules returned 200 success but wrote nothing to disk). Same replace-or-preserve semantics as `providers`/`models` sections: array present replaces the on-disk Vec, array absent preserves the existing Vec (so toggling `cost_first` etc. cannot wipe rules), empty array `[]` clears them. **Type-checked via strong `Vec<RouterRule>` / `Vec<PromptRule>` deserialise first** — malformed payloads (non-array string, partial rule missing required `prefix`) return HTTP 500 ParseError before touching disk, preventing legal-but-broken TOML from crashing the next `ccm start`. 7 `#[tokio::test]` guards added (`src/server/mod.rs::config_json_tests`) covering persist / replace / preserve / clear / non-array-reject / partial-rule-reject / **condition-variant round-trip** (both `RouterRuleType` variants — `model-prefix` and `condition` — exercised). The `serde_json::to_string` step uses `map_err` instead of `unwrap` so a serialise failure surfaces as a 500, not a panic.
- `config.example.toml`: `auth_type = "api_key"` → `"apikey"` to match `ProviderAuthType::ApiKey` serde tag (mismatch caused fail-fast on `ccm start` with the example config)
- Performance: tracing sync I/O + std::Mutex on every request → BufWriter + lazy flush
- Stability: RwLock unwrap → poison propagation → server permanent crash on single panic
- Stability: `.expect()` on empty OpenAI choices → panic → connection abort
- Stability: autoSave syncToServer storm → POST entire config on every keystroke → reload contention
- Stability: Gemini Vertex AI `.unwrap()` on missing project_id/location → ok_or_else
- Stability: OpenAI stream finalizer `.lock().unwrap()` → poison recovery
- Stability: `write_routing_info` sync file I/O in async handler → spawn_blocking
- Performance: regex compiled per-request → cached `Lazy<Regex>`
- localStorage guard: try/catch on all access points (prevents total JS crash)
- Logs tab i18n: all strings now use translation keys
- Dark theme: CSS variables + `html.dark` class (robust against Tailwind overrides)
- Container startup: PID file moved from `~/.claude-code-mux/ccm.pid` (persistent volume) → `/tmp/ccm.pid` (ephemeral), preventing stale-PID blocking on container restart
- Container PID 1: `is_process_running()` now returns false for PID ≤ 1 (init/container process), fixing "already running" false positive when stale PID file contains PID 1
- Header passthrough: OpenAI provider `existing_keys` now conditional on `is_oauth()` — non-OAuth (API key) path no longer blocks client User-Agent and browser fingerprint headers from passing through to upstream
- Logs visibility: BufWriter now flushes after every trace write (was 8KB lazy flush), ensuring `/api/logs` can read entries immediately without waiting for buffer fill or tracer drop

## [0.6.0] - 2025-11-19

### Added
- Google Gemini provider with OAuth 2.0 support (Google AI Pro/Ultra via Code Assist API)
- Separate Vertex AI provider for GCP platform with multi-model support
- Three authentication methods for Gemini: OAuth, API Key (AI Studio), and Vertex AI (ADC)
- Anthropic to Gemini API format transformation
  - System prompts to systemInstruction
  - Message conversion (user/assistant to user/model)
  - Content blocks (text, image, thinking)
  - Tools/functions to functionDeclarations
  - Generation config mapping (temperature, top_p, top_k, max_tokens)
- Gemini to Anthropic response transformation
- OAuth token refresh logic for Gemini provider
- Admin UI support for Gemini and Vertex AI providers
- Comprehensive Gemini/Vertex AI integration documentation
- Project ID and location configuration for Vertex AI
- OAuth token store with project_id field for Gemini

### Changed
- Separated Vertex AI as distinct provider type from Gemini
- Enhanced OAuth flow to support Google's standard OAuth 2.0 parameters
- Updated OAuth handlers with loadCodeAssist API integration for project_id retrieval

## [0.5.0] - 2025-11-19

### Added
- OpenAI ChatGPT Plus/Pro OAuth 2.0 authentication support
- GPT-5.1 and GPT-5.1 Codex model support via OpenAI OAuth
- OpenAI Codex Responses API integration (`/codex/responses` endpoint)
- Reasoning block to thinking block conversion for Codex models
- Separate OAuth callback server on port 1455 for OpenAI OAuth
- Official OpenAI Codex instructions from rust-v0.58.0
- Browser-like headers for Cloudflare bypass (native-tls)
- SSE (Server-Sent Events) response parsing for streaming
- JWT token decoding to extract ChatGPT account_id
- Admin UI support for OpenAI OAuth flow (similar to Anthropic OAuth)

### Changed
- Switched from rustls-tls to native-tls for better compatibility
- Enhanced OpenAI provider to support both API Key and OAuth authentication
- Updated OAuth handlers to support "openai-codex" type
- Improved SSE parsing to extract both reasoning and message content blocks

### Fixed
- OpenAI Codex model streaming with proper endpoint routing
- PKCE state/verifier separation for OpenAI OAuth compatibility
- Reasoning block handling in gpt-5.1-codex responses

## [0.4.3] - 2025-11-17

### Added
- CI and Latest Release badges to README
- FAQ section with 6 common questions
- CHANGELOG.md with full version history
- Collapsible screenshots with descriptive captions
- Collapsible provider details section

### Changed
- Restructured README for better onboarding flow (moved comparison section to bottom)
- Compressed Supported Providers section with summary
- Updated performance metrics with actual measurements (6MB vs 156MB)
- Improved OAuth description to focus on Claude Pro/Max compatibility

### Fixed
- Memory usage comparison (updated from 10x to accurate 25x difference)

## [0.4.2] - 2025-11-17

### Fixed
- Use rustls instead of native-tls for better cross-compilation support

### Changed
- Added automated release workflow for GitHub releases

## [0.4.1] - 2025-11-17

### Fixed
- Use `/v1/responses` endpoint for Codex model streaming requests

## [0.4.0] - 2025-11-17

### Added
- OpenAI Responses API support for Codex models (gpt-5-turbo, etc.)
- Automatic endpoint routing based on model type

## [0.3.0] - 2025-11-17

### Added
- OpenAI-compatible `/v1/chat/completions` endpoint
- Support for OpenAI format requests alongside Anthropic format

### Fixed
- Router tab auto-save logging improvements

## [0.2.0] - 2025-11-17

### Added
- Documentation improvements
- Engaging intro tagline in README

## [0.1.0] - 2025-11-17

### Added
- Initial release of Claude Code Mux
- High-performance AI routing proxy built in Rust
- Anthropic Messages API compatibility (`/v1/messages`)
- Intelligent model routing (default, think, background, websearch)
- Provider failover with priority-based routing
- Streaming support (SSE)
- Web-based admin UI with auto-save
- OAuth 2.0 authentication for Anthropic
- Multi-provider support (16+ providers)
- Auto-mapping with regex patterns
- TOML-based configuration
- Token counting endpoint (`/v1/messages/count_tokens`)

[0.6.3-eli.1]: https://github.com/winffychu/claude-code-mux/commits/main
[0.6.0]: https://github.com/9j/claude-code-mux/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/9j/claude-code-mux/compare/v0.4.3...v0.5.0
[0.4.3]: https://github.com/9j/claude-code-mux/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/9j/claude-code-mux/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/9j/claude-code-mux/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/9j/claude-code-mux/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/9j/claude-code-mux/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/9j/claude-code-mux/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/9j/claude-code-mux/releases/tag/v0.1.0
