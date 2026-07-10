# P2: 故障重试升级 — 指数退避 + Retry-After + 智能Fallback

> 来源：CCR `service.ts:2041-2133`（fetchUpstreamWithFallback）+ `2507-2560`（shouldFallbackAfterStatus + parseRetryAfterHeaderMs + exponentialRetryBackoffMs）vs CCM `server/mod.rs:610-879`
> 状态：暂缓（2026-07-10 风险审计：3 处遍历点遗漏 + streaming 不可能 fallback + chain 映射不明确，风险 > 收益）
> 审计修正：2026-07-03 初版（2 处修正 + 拆分为 P2a/P2b）/ 2026-07-04 二次修正（7 处文档错误修正，见底部修正日志 #4-#10）/ 2026-07-09 三轮修正（见底部修正日志 #11-#13）

---

## ⚠️ 前置依赖：ProviderError 扩展（P2a）

> **审计发现**：原文档中 `e.retry_after_header().unwrap_or("")` 和 `e.status_code().unwrap_or(500)` 引用了 **不存在的方法**。
>
> CCM `ProviderError` 当前定义（`providers/error.rs:1-23`）：
> ```rust
> pub enum ProviderError {
>     HttpError(#[from] reqwest::Error),
>     SerializationError(#[from] serde_json::Error),
>     ModelNotSupported(String),
>     ApiError { status: u16, message: String },  // 无 retry_after 字段
>     ConfigError(String),
>     AuthError(String),
> }
> ```
> - **没有** `retry_after_header()` 方法
> - **没有** `status_code()` 方法
> - **没有** response headers 存储
>
> 要实现 Retry-After 解析，必须先扩展 `ApiError` 变体 — 这是一个影响所有 provider 实现的前置改动。

### P2a: ProviderError 扩展

```rust
// src/providers/error.rs 修改

#[derive(Error, Debug)]
pub enum ProviderError {
    // ... 其他变体不变 ...

    #[error("Provider API error: {status} - {message}")]
    ApiError {
        status: u16,
        message: String,
        /// Retry-After header 值（从 upstream response 提取）
        #[serde(default)]
        retry_after: Option<String>,
    },
}

impl ProviderError {
    /// 提取 HTTP 状态码
    pub fn status_code(&self) -> Option<u16> {
        match self {
            ProviderError::ApiError { status, .. } => Some(*status),
            ProviderError::HttpError(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }

    /// 提取 Retry-After header 值
    pub fn retry_after_header(&self) -> Option<&str> {
        match self {
            ProviderError::ApiError { retry_after, .. } => retry_after.as_deref(),
            _ => None,
        }
    }
}
```

### 受影响的 provider 构建点（P2a）

修改所有 `ProviderError::ApiError { status, message }` 构造点，补充 `retry_after` 字段：

| 文件 | 行号 | 当前代码 | 修改 |
|------|------|---------|------|
| `openai.rs` | L371, L1291, L1363, L1505, L1568 | `ApiError { status, message }` | `ApiError { status, message, retry_after }` — 从 `response.headers()` 提取 `retry-after`。L371 是 `Err(...)` 表达式返回，非 `return Err` |
| `gemini.rs` | L314, L392, L399, L516, L523, L589, L676, L737 | 同上 | 同上 — Gemini 另有 `parse_retry_delay()` (L952) 可复用。L314 是 `.ok_or_else(\|\| ...)` 闭包返回，非 `return Err` |
| `anthropic_compatible.rs` | L370, L418, L490 | 同上 | 同上 |

> **注意**：`anthropic_compatible.rs:448` 的模式匹配 `ProviderError::ApiError { message, .. }` 和 L567 的 `ProviderError::ApiError { message, .. }` 使用了 `..` 通配符，新增字段**不会破坏**这些匹配。

### P2a 工作量

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/providers/error.rs` | ApiError 增字段 + impl 2 个方法 | ~20 行 |
| `src/providers/openai.rs` | 4 处 ApiError 构造 + retry_after 提取 | ~12 行 |
| `src/providers/gemini.rs` | 7 处 ApiError 构造 + retry_after 提取 | ~18 行 |
| `src/providers/anthropic_compatible.rs` | 3 处 ApiError 构造 + retry_after 提取 | ~10 行 |
| **P2a 合计** | | **~65 行** |

---

## 对比分析

### CCR 实现（`service.ts:2041-2133` + `2507-2560`）

```
fetchUpstreamWithFallback:
  fallback_mode = "off" | "retry" | "model-chain"

  attempt chain 构建（buildUpstreamAttempts）：
  - mode=off → 单次尝试，不重试
  - mode=retry → 同一模型重试 N 次（N = fallback.retryCount，上限 9999）
  - mode=model-chain → [当前模型, ...配置的 fallback 模型列表]（去重）

  每个 attempt：
  1. prepareUpstreamCredentialAttempt — 选 credential + 改 model name + headers
  2. fetch 到 upstream
  3. 状态码判断（shouldFallbackAfterStatus）：
     - model-chain 模式：任何 ≥400 → fallback
     - 所有模式：408/409/429/≥500 → fallback
  4. 429 时解析 Retry-After header（parseRetryAfterHeaderMs）：
     - 支持秒数格式（"120" → 120000ms）
     - 支持 HTTP-date 格式（"Wed, 21 Oct 2025 07:28:00 GMT" → now - date）
     - 上限 60s
  5. 非 429 错误 → 指数退避（exponentialRetryBackoffMs）：
     - base = 1000ms, max = 30000ms
     - delay = min(max, base * 2^attemptIndex)
  6. 记录 failedAttempts（credential, model, statusCode, delayMs, error）
  7. 返回时注入 response headers：
     - x-ccr-fallback-attempts
     - x-ccr-fallback-failures
     - x-ccr-fallback-delays-ms
     - x-ccr-fallback-model
```

### CCM 现状（`server/mod.rs:610-879`）

```
handle_messages:
  1. route() → RouteDecision
  2. 找到 model_config → 获取 sorted_mappings（1:N by priority）
  3. for (idx, mapping) in sorted_mappings:
     - provider.send_message_stream(anthropic_request)
     - 成功 → 返回 response
     - 失败 → info!("⚠️ Provider failed") + continue
  4. 全部失败 → 返回最后一个错误
```

### 差距

| 能力 | CCR | CCM |
|------|-----|-----|
| Fallback mode 选择 | off / retry / model-chain | 仅 1:N mapping 遍历 |
| 429 Retry-After 解析 | ✅ 秒数 + HTTP-date | ❌ |
| 指数退避延迟 | ✅ base 1s, max 30s, 2^n | ❌ 无延迟 |
| 状态码感知 fallback | ✅ model-chain: 任何 ≥400 → fallback；其他模式: 408/409/429/≥500 → fallback | ❌ 任何错误 → continue |

> **审计修正**：原文档对比表仅列出 "408/409/429/≥500"，遗漏了 `model-chain` 模式对 400+ 的特殊行为。CCR `shouldFallbackAfterStatus()` (`service.ts:2507-2515`)：`model-chain` mode 下 **任何 ≥400 状态码都触发 fallback**（含 400/401/403），因为 model-chain 的语义是"换一个模型试试"，而其他模式仅对可重试状态码 (408/409/429/5xx) 触发。
| Per-rule fallback 配置 | ✅ 每个 rule 可独立配置 fallback | ❌ 全局 model mapping |
| Fallback trace headers | ✅ x-ccr-fallback-* | ❌ |

---

## 方案

### 目标

1. 新增 `FallbackMode` 枚举：`Off` / `Retry` / `ModelChain`
2. 构建 attempt chain：retry → 同模型 N 次；model-chain → 模型列表遍历
3. 429 响应解析 `Retry-After` header → 等待指定时间
4. 通用错误指数退避：`base_ms * 2^attempt`，上限 30s
5. 状态码感知：408/409/429/≥500 才触发 fallback，其他错误直接返回
6. Response headers 注入 fallback trace

### 数据结构

```rust
// src/providers/retry.rs（新建）

#[derive(Debug, Clone, Deserialize, Default)]
pub enum FallbackMode {
    /// 不重试，单次尝试
    #[default]
    Off,
    /// 同一模型重试 N 次
    Retry,
    /// 模型链：当前模型 → fallback 模型列表
    ModelChain,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackConfig {
    #[serde(default)]
    pub mode: FallbackMode,
    /// retry 模式：重试次数（不含首次）
    #[serde(default)]
    pub retry_count: u32,
    /// model-chain 模式：fallback 模型列表
    #[serde(default)]
    pub models: Vec<String>,
}

const BACKOFF_BASE_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 30_000;
const RETRY_AFTER_MAX_MS: u64 = 60_000;
const MAX_RETRY_COUNT: u32 = 10;

/// 构建 fallback attempt chain
/// 返回 (model_name, attempt_index) 列表
pub fn build_fallback_chain(
    current_model: &str,
    fallback: &FallbackConfig,
) -> Vec<(String, u32)> {
    match fallback.mode {
        FallbackMode::Off => vec![(current_model.to_string(), 0)],
        FallbackMode::Retry => {
            let count = fallback.retry_count.min(MAX_RETRY_COUNT);
            (0..=count)
                .map(|i| (current_model.to_string(), i))
                .collect()
        }
        FallbackMode::ModelChain => {
            let mut chain = vec![(current_model.to_string(), 0)];
            for model in &fallback.models {
                if !chain.iter().any(|(m, _)| m == model) {
                    chain.push((model.clone(), chain.len() as u32));
                }
            }
            chain
        }
    }
}

/// 状态码是否应触发 fallback
pub fn should_fallback(status: u16, mode: &FallbackMode) -> bool {
    if matches!(mode, FallbackMode::ModelChain) && status >= 400 {
        return true;
    }
    status == 408 || status == 409 || status == 429 || status >= 500
}

/// 解析 Retry-After header → 等待毫秒数
/// 支持：秒数（"120"）和 HTTP-date（"Wed, 21 Oct 2025 07:28:00 GMT"）
///
/// **与 Gemini `parse_retry_delay` 的关系**：
/// Gemini 的 `parse_retry_delay()` (gemini.rs:952) 返回 `Option<Duration>`，本函数返回 `Option<u64>`（毫秒）。
/// 复用方式：`parse_retry_delay(s).map(|d| d.as_millis() as u64)`
pub fn parse_retry_after(header_value: &str) -> Option<u64> {
    let trimmed = header_value.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 尝试秒数
    if let Ok(secs) = trimmed.parse::<f64>() {
        if secs >= 0.0 {
            let ms = (secs * 1000.0) as u64;
            return Some(ms.min(RETRY_AFTER_MAX_MS));
        }
    }

    // 尝试 HTTP-date
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(trimmed) {
        let now = chrono::Utc::now();
        let diff = dt.signed_duration_since(now);
        if diff.num_milliseconds() > 0 {
            return Some(diff.num_milliseconds() as u64);
        }
    }

    None
}

/// 指数退避延迟
pub fn exponential_backoff_ms(attempt_index: u32) -> u64 {
    let exponent = attempt_index.min(10) as u32;
    let delay = BACKOFF_BASE_MS.saturating_mul(2u64.saturating_pow(exponent));
    delay.min(BACKOFF_MAX_MS)
}
```

### 配置格式

```toml
# 全局默认 fallback
[router.fallback]
mode = "model-chain"
retry_count = 2
models = ["claude-sonnet-4-20250514", "gpt-4o"]

# 无 fallback 的 provider
[[models]]
name = "quick-test"
# ... mappings ...
# fallback mode = off

# per-rule fallback（P0 Router Rules 集成时）
[[router.rules]]
id = "important-task"
# ...
[router.rules.fallback]
mode = "retry"
retry_count = 3
```

### `handle_messages` 集成（P2b）

> **前置依赖**：以下代码引用 `e.retry_after_header()` 和 `e.status_code()` — 这两个方法在 P2a 中新增。必须先完成 P2a 才能实施 P2b。
>
> **`build_fallback_chain` 与 `sorted_mappings` 关系**：
> P2b 的 `build_fallback_chain` **替换**现有 `sorted_mappings` 遍历逻辑。现有代码中 `sorted_mappings` 按 priority 1:N 遍历是 model-chain 的简化版；P2b 将其泛化为 FallbackMode 控制——`mode=off` 时仅取首个 mapping，`mode=retry` 时同 mapping 重试 N 次，`mode=model-chain` 时按 `fallback.models` 列表遍历。`sorted_mappings` 仍用于查找 mapping 对应的 provider。

```rust
// src/server/mod.rs — handle_messages 修改

// 现有：遍历 sorted_mappings 硬 continue
// 改为：fallback chain + 退避 + retry-after

let fallback_config = model_config.fallback.as_ref()
    .unwrap_or(&inner.config.router.fallback);
let chain = build_fallback_chain(&decision.model_name, fallback_config);

let mut failed_attempts: Vec<FailedAttempt> = Vec::new();
let mut last_error: Option<ProviderError> = None;

for (idx, (model_name, attempt_idx)) in chain.iter().enumerate() {
    let has_next = idx < chain.len() - 1;

    // 找到 mapping → provider
    let mapping = sorted_mappings.iter()
        .find(|m| m.actual_model == *model_name)
        .or_else(|| sorted_mappings.first());
    let provider = match mapping {
        Some(m) => inner.provider_registry.get_provider(&m.provider),
        None => None,
    };

    match provider {
        Some(p) => {
            let mut req = anthropic_request.clone();
            req.model = mapping.unwrap().actual_model.clone();

            let result = if is_streaming {
                p.send_message_stream(req).await
            } else {
                p.send_message(req).await
            };

            match result {
                Ok(resp) => {
                    // 注入 fallback trace headers
                    let mut resp = resp;
                    if !failed_attempts.is_empty() {
                        inject_fallback_headers(&mut resp, &failed_attempts);
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    let status = e.status_code().unwrap_or(500);
                    if !has_next || !should_fallback(status, &fallback_config.mode) {
                        return Err(e);
                    }

                    let delay_ms = if status == 429 {
                        // 解析 Retry-After（P2a 中 ProviderError.retry_after_header() 方法）
                        parse_retry_after(&e.retry_after_header().unwrap_or(""))
                            .unwrap_or_else(|| exponential_backoff_ms(*attempt_idx))
                    } else {
                        exponential_backoff_ms(*attempt_idx)
                    };

                    failed_attempts.push(FailedAttempt {
                        model: model_name.clone(),
                        status,
                        delay_ms,
                        error: e.to_string(),
                    });

                    last_error = Some(e);

                    if delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    continue;
                }
            }
        }
        None => {
            // provider 未找到（mapping 存在但 provider 已卸载等）
            last_error = Some(ProviderError::ConfigError(format!(
                "provider not found for model '{}'", model_name
            )));
            continue;
        }
    }
}

// 所有 attempt 失败
return Err(last_error.unwrap_or(ProviderError::ConfigError(
    "no providers available for fallback".to_string(),
)));
```

### Response Headers 注入

```rust
fn inject_fallback_headers(response: &mut Response, attempts: &[FailedAttempt]) {
    let count = attempts.len() + 1;
    response.headers_mut().insert(
        "x-ccm-fallback-attempts",
        HeaderValue::from_str(&count.to_string()).unwrap(),
    );

    let failures: Vec<String> = attempts.iter()
        .map(|a| format!("{}:{}", a.model, a.status))
        .collect();
    response.headers_mut().insert(
        "x-ccm-fallback-failures",
        HeaderValue::from_str(&failures.join(",")).unwrap(),
    );

    let delays: Vec<String> = attempts.iter()
        .filter(|a| a.delay_ms > 0)
        .map(|a| a.delay_ms.to_string())
        .collect();
    if !delays.is_empty() {
        response.headers_mut().insert(
            "x-ccm-fallback-delays-ms",
            HeaderValue::from_str(&delays.join(",")).unwrap(),
        );
    }
}
```

---

## 文件改动

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/providers/retry.rs`（新建） | FallbackMode + FallbackConfig + build_fallback_chain + should_fallback + parse_retry_after + exponential_backoff_ms | ~80 行 |
| `src/providers/mod.rs` | pub mod retry; re-export | ~3 行 |
| `src/cli/mod.rs` | ModelConfig 新增 `fallback: Option<FallbackConfig>` 字段 + AppConfig.router.fallback 字段 | ~15 行 |
| `src/server/mod.rs` | handle_messages fallback chain 集成 + headers 注入 | ~25 行 |
| `src/lib.rs` | 无需改动（retry 在 providers 下） | — |
| **P2b 合计** | | **~123 行** |
| **P2a + P2b 总计** | | **~188 行** |

### 新增依赖

```toml
# Cargo.toml — chrono 已存在
# 已确认：Cargo.toml L66 有 chrono = { version = "0.4", features = ["serde"] }
# parse_retry_after 的 HTTP-date 解析使用 chrono::DateTime::parse_from_rfc2822，无需新增依赖
```

---

## 验证

```bash
cargo check
cargo test
cargo clippy --no-deps
```

新增单元测试：
- `test_fallback_chain_off_mode`：off 模式只产生 1 个 attempt
- `test_fallback_chain_retry_mode`：retry 模式产生 retry_count+1 个同模型 attempt
- `test_fallback_chain_model_chain_mode`：model-chain 模式去重并拼接当前模型 + fallback 列表
- `test_should_fallback_status_codes`：验证 408/409/429/500 触发 fallback，400（非 model-chain）不触发
- `test_parse_retry_after_seconds`：`"120"` → 120000ms
- `test_parse_retry_after_http_date`：HTTP-date 格式正确解析
- `test_exponential_backoff`：验证指数增长和上限
- `test_model_chain_400_fallback`：验证 model-chain 模式对 400 状态码也触发 fallback
- `test_gemini_parse_retry_delay_reuse`：验证 Gemini 的 `parse_retry_delay()` 可被 `parse_retry_after` 复用

---

## 审计修正日志（2026-07-03 初版 / 2026-07-04 二次修正）

| # | 原始内容 | 修正为 | 依据 |
|---|---------|--------|------|
| 1 | `e.retry_after_header().unwrap_or("")` 和 `e.status_code().unwrap_or(500)` 可直接使用 | 这两个方法**不存在**。拆分为 P2a（ProviderError 扩展，~60 行）+ P2b（retry 逻辑，~123 行）| `providers/error.rs:1-23`：`ApiError { status, message }` 无 `retry_after` 字段，无 impl 方法。grep 确认 16 处 `ApiError` 构造点需修改（openai 5 + gemini 8 + anthropic 3）|
| 2 | `should_fallback()` 仅检查 408/409/429/≥500 | 补充 `model-chain` 模式对 **400+** 的特殊 fallback 行为 | CCR `shouldFallbackAfterStatus()` (`service.ts:2507-2515`)：`model-chain` mode 下任何 ≥400 触发 fallback |
| 3 | — | 补充 Gemini `parse_retry_delay()` (L952) 可复用 | 代码验证发现 Gemini 已有 retry delay 解析，P2b 的 `parse_retry_after` 可调用 Gemini 特有逻辑 |
| 4 | openai.rs ApiError 构造点仅列 4 处 | 实际 **5 处**：L371, L1291, L1363, L1505, L1568。L371 是 `Err(...)` 表达式返回（非 `return Err`），仍需修改 | `grep -n 'ProviderError::ApiError' openai.rs` = 5 处 |
| 5 | gemini.rs ApiError 构造点仅列 7 处 | 实际 **8 处**：L314, L392, L399, L516, L523, L589, L676, L737。L314 是 `.ok_or_else(\|\| ...)` 闭包返回，仍需修改 | `grep -n 'ProviderError::ApiError' gemini.rs` = 8 处 |
| 6 | ApiError 构造点总计 14 处 | 实际总计 **16 处**（openai 5 + gemini 8 + anthropic 3） | 包含 `Err(...)` 表达式返回和 `.ok_or_else` 闭包返回 |
| 7 | `return Err(last_error)` 但 `last_error` 未定义 | 在 for 循环前加 `let mut last_error: Option<ProviderError> = None;`；错误分支中 `last_error = Some(e);`；None provider 分支中 `last_error = Some(ProviderError::ConfigError(...))`；尾部改为 `return Err(last_error.unwrap_or(...))` | 代码审查发现 `last_error` 变量从未声明，直接使用会编译错误 |
| 8 | `handle_messages` 集成代码段未说明 `build_fallback_chain` 与现有 `sorted_mappings` 遍历的关系 | 补充说明：`build_fallback_chain` **替换** `sorted_mappings` 遍历逻辑，`sorted_mappings` 仍用于查找 mapping→provider | 文档审计发现 P2b 集成代码与 P2a 前置描述存在语义冲突 |
| 9 | `parse_retry_after` 函数注释未说明与 Gemini `parse_retry_delay` 的复用方式 | 补充：Gemini `parse_retry_delay()` (gemini.rs:952) 返回 `Option<Duration>`，本函数返回 `Option<u64>`（毫秒）；复用方式 `parse_retry_delay(s).map(\|d\| d.as_millis() as u64)` | Gemini 已有类似实现，文档应引导复用以减少重复代码（区分返回类型） |
| 10 | 依赖段标注"chrono 已有" | 改为"已确认存在"——Cargo.toml L66 有 `chrono = { version = "0.4", features = ["serde"] }`，无需新增依赖 | 代码验证：`grep -n "chrono" Cargo.toml` = L66 |
| 11 | 三轮自审：CCR 行号引用 `service.ts:2522-3027` | 修正为 `2041-2133`（fetchUpstreamWithFallback）+ `2507-2560`（shouldFallbackAfterStatus + parseRetryAfterHeaderMs + exponentialRetryBackoffMs） | 代码验证：`grep -n "function fetchUpstreamWithFallback" service.ts` = L2041 |
| 12 | 三轮自审：CCM 行号引用 `server/mod.rs:608-807` | 修正为 `610-879`（handle_messages 实际范围） | 代码验证：`grep -n "handle_messages" mod.rs` = L610，结束 `}` = L879 |
| 13 | 三轮自审：openai.rs ApiError 构造点 4 处 | 实际 **5 处**：L371 是 `Err(...)` 表达式返回（非 `return Err`），仍需修改。gemini.rs L314 同理是 `.ok_or_else(\|\| ...)` 闭包返回。总计 14 处修正回 **16 处** | 代码验证：`grep -n 'ProviderError::ApiError' openai.rs` = 5; gemini.rs = 8; anthropic_compatible.rs = 3（构造点） |

### 拆分说明

原文档将 ProviderError 扩展和 retry 逻辑混在一起，风险评估：

- **P2a（ProviderError 扩展）**：涉及 16 处 provider 代码修改（openai 5 + gemini 8 + anthropic 3），每个 provider 文件都有独立的事故风险（遗漏某处会导致编译错误或运行时 panic）
- **P2b（retry 逻辑）**：独立新建模块，不触及现有 provider 代码

拆分为 P2a → P2b 两步后，P2a 可独立编译验证，P2b 在 P2a 完成后实施。
