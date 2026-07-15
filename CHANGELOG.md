# Changelog

All notable changes to Claude Code Mux will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.3-eli.1] - 2026-07-14

### Added
- Fork-specific: OpenAI-compatible `/v1/chat/completions` tool/function calling support
- OpenAI-compatible `/v1/models` endpoint (aggregates provider models, BTreeSet dedup)
- Message tracing with BufWriter (buffered I/O, lazy flush on /api/logs)
- Live logs viewer: `/api/logs` (paginated) and `/api/logs/stream` (real-time SSE)
- Admin UI i18n (zh-CN + en, 173 keys each)
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
- Routing order: auto-map → websearch → background → subagent → router.rules → prompt_rules → think → long_context → default
- `update_config_json`: `tokio::fs::write` (async, non-blocking) instead of `std::fs::write`
- RwLock poison recovery: `unwrap_or_else(|e| e.into_inner())` on all lock sites
- `openai.rs:769`: `.expect()` → `match` returning error response (no panic on empty choices)
- Stream state mutex: poison recovery instead of `.unwrap()` cascade
- `SystemTime::now().duration_since()`: `.unwrap_or(0)` (NTP clock rollback safe)
- `response_builder.body()`: `.unwrap()` → `match` with error fallback

### Fixed
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
