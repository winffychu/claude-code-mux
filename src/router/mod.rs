use crate::cli::AppConfig;
use crate::cli::{RouterRule, RouterRuleType, RuleOperator, RouterRuleRewrite, RewriteOperation};
use crate::models::{AnthropicRequest, MessageContent, RouteDecision, RouteType, SystemPrompt};
use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use tracing::{debug, info};

/// Regex to detect capture group references ($1, $name, ${1}, ${name})
static CAPTURE_REF_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$(?:\d+|[a-zA-Z_]\w*|\{[^}]+\})").unwrap());

/// Check if a string contains capture group references
fn contains_capture_reference(s: &str) -> bool {
    s.contains('$') && CAPTURE_REF_PATTERN.is_match(s)
}

/// Compiled prompt rule with pre-compiled regex
#[derive(Clone)]
pub struct CompiledPromptRule {
    pub regex: Regex,
    pub model: String,
    pub strip_match: bool,
    /// True if model contains capture group references ($1, $name, etc.)
    pub is_dynamic: bool,
}

/// Router for intelligently selecting models based on request characteristics
#[derive(Clone)]
pub struct Router {
    config: AppConfig,
    auto_map_regex: Option<Regex>,
    background_regex: Option<Regex>,
    prompt_rules: Vec<CompiledPromptRule>,
    rules: Vec<RouterRule>,
}

/// Extract text from MessageContent for token counting
fn message_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| b.as_text().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Character length of message content (for fallback token estimation)
fn message_text_len(content: &MessageContent) -> usize {
    message_text(content).len()
}

/// Iterate over text segments in a SystemPrompt
fn system_text_iter(system: &SystemPrompt) -> Vec<String> {
    match system {
        SystemPrompt::Text(t) => vec![t.clone()],
        SystemPrompt::Blocks(blocks) => blocks.iter().map(|b| b.text.clone()).collect(),
    }
}

impl Router {
    /// Create a new router with configuration
    pub fn new(config: AppConfig) -> Self {
        // Compile auto-map regex
        let auto_map_regex = config
            .router
            .auto_map_regex
            .as_ref()
            .and_then(|pattern| {
                if pattern.is_empty() {
                    // Empty string: use default Claude pattern
                    Some(Regex::new(r"^claude-").expect("Invalid default Claude regex"))
                } else {
                    // Custom pattern provided
                    match Regex::new(pattern) {
                        Ok(regex) => Some(regex),
                        Err(e) => {
                            eprintln!(
                                "Warning: Invalid auto_map_regex pattern '{}': {}",
                                pattern, e
                            );
                            eprintln!("Falling back to default Claude pattern");
                            Some(Regex::new(r"^claude-").expect("Invalid default Claude regex"))
                        }
                    }
                }
            })
            .or_else(|| {
                // None: use default Claude pattern for backward compatibility
                Some(Regex::new(r"^claude-").expect("Invalid default Claude regex"))
            });

        // Compile background-task regex
        let background_regex = config
            .router
            .background_regex
            .as_ref()
            .and_then(|pattern| {
                if pattern.is_empty() {
                    // Empty string: use default claude-haiku pattern
                    Some(
                        Regex::new(r"(?i)claude.*haiku").expect("Invalid default background regex"),
                    )
                } else {
                    // Custom pattern provided
                    match Regex::new(pattern) {
                        Ok(regex) => Some(regex),
                        Err(e) => {
                            eprintln!(
                                "Warning: Invalid background_regex pattern '{}': {}",
                                pattern, e
                            );
                            eprintln!("Falling back to default claude-haiku pattern");
                            Some(
                                Regex::new(r"(?i)claude.*haiku")
                                    .expect("Invalid default background regex"),
                            )
                        }
                    }
                }
            })
            .or_else(|| {
                // None: use default claude-haiku pattern for backward compatibility
                Some(Regex::new(r"(?i)claude.*haiku").expect("Invalid default background regex"))
            });

        // Compile prompt rules
        let prompt_rules: Vec<CompiledPromptRule> = config
            .router
            .prompt_rules
            .iter()
            .filter_map(|rule| {
                match Regex::new(&rule.pattern) {
                    Ok(regex) => {
                        let is_dynamic = contains_capture_reference(&rule.model);
                        Some(CompiledPromptRule {
                            regex,
                            model: rule.model.clone(),
                            strip_match: rule.strip_match,
                            is_dynamic,
                        })
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Invalid prompt_rule pattern '{}': {}. Skipping.",
                            rule.pattern, e
                        );
                        None
                    }
                }
            })
            .collect();

        if !prompt_rules.is_empty() {
            info!("📝 Loaded {} prompt routing rules", prompt_rules.len());
        }

        let rules: Vec<RouterRule> = config.router.rules.clone();

        Self {
            config,
            auto_map_regex,
            background_regex,
            prompt_rules,
            rules,
        }
    }

    /// Route an incoming request to the appropriate model
    ///
    /// Priority order (highest to lowest):
    /// 1. WebSearch - tool-based detection (web_search tool present)
    /// 2. Background - model name regex match (e.g., haiku) - checked early to save costs
    /// 3. Subagent - CCM-SUBAGENT-MODEL tag in system prompt
    /// 4. Prompt Rules - regex pattern matching on user prompt (after background for cost savings)
    /// 5. Think - Plan Mode / reasoning enabled
    /// 6. Default - auto-mapped or original model name
    pub fn route(&self, request: &mut AnthropicRequest) -> Result<RouteDecision> {
        // Save original model for background task detection
        let original_model = request.model.clone();

        // 0. Auto-mapping (model name transformation FIRST)
        // Transform model name if it matches auto_map_regex
        if let Some(ref regex) = self.auto_map_regex {
            if regex.is_match(&request.model) {
                let old = request.model.clone();
                request.model = self.config.router.default.clone();
                debug!("🔀 Auto-mapped model '{}' → '{}'", old, request.model);
            }
        }

        // 1. WebSearch (HIGHEST PRIORITY - tool-based detection)
        if let Some(ref websearch_model) = self.config.router.websearch {
            if self.has_web_search_tool(request) {
                debug!("🔍 Routing to websearch model (web_search tool detected)");
                return Ok(RouteDecision {
                    model_name: websearch_model.clone(),
                    route_type: RouteType::WebSearch,
                    matched_prompt: None,
                });
            }
        }

        // 2. Background tasks (check against ORIGINAL model name, before auto-mapping)
        // Checked early to prevent expensive models being used for background tasks
        if let Some(ref background_model) = self.config.router.background {
            if self.is_background_task(&original_model) {
                debug!("🔄 Routing to background model");
                return Ok(RouteDecision {
                    model_name: background_model.clone(),
                    route_type: RouteType::Background,
                    matched_prompt: None,
                });
            }
        }

        // 3. Subagent Model (system prompt tag)
        if let Some(model) = self.extract_subagent_model(request) {
            debug!(
                "🤖 Routing to subagent model (CCM-SUBAGENT-MODEL tag): {}",
                model
            );
            return Ok(RouteDecision {
                model_name: model,
                route_type: RouteType::Default,
                matched_prompt: None,
            });
        }

        // 4. Router Rules (condition + model-prefix → rewrite)
        // New: declarative rules with condition matching and request rewriting
        if let Some(model) = self.match_router_rule(request) {
            debug!("📋 Routing to model via router rule: {}", model);
            return Ok(RouteDecision {
                model_name: model,
                route_type: RouteType::PromptRule,
                matched_prompt: None,
            });
        }

        // 5. Prompt Rules (pattern matching on user prompt)
        // NOTE: Checked AFTER background to ensure background tasks use cheaper models
        if let Some((model, matched_text)) = self.match_prompt_rule(request) {
            debug!("📝 Routing to model via prompt rule match: {}", model);
            return Ok(RouteDecision {
                model_name: model,
                route_type: RouteType::PromptRule,
                matched_prompt: Some(matched_text),
            });
        }

        // 5. Think mode (Plan Mode / Reasoning)
        if let Some(ref think_model) = self.config.router.think {
            if self.is_plan_mode(request) {
                debug!("🧠 Routing to think model (Plan Mode detected)");
                return Ok(RouteDecision {
                    model_name: think_model.clone(),
                    route_type: RouteType::Think,
                    matched_prompt: None,
                });
            }
        }

        // 6. Default fallback
        // Use the transformed model name (from auto-mapping) or original if no mapping
        debug!("✅ Using model: {}", request.model);
        Ok(RouteDecision {
            model_name: request.model.clone(),
            route_type: RouteType::Default,
            matched_prompt: None,
        })
    }

    /// Match router rules: iterate rules in order, return model if a rule matches.
    /// Applies rewrites (including model rewrite) to the request in-place.
    fn match_router_rule(&self, request: &mut AnthropicRequest) -> Option<String> {
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }

            let matched = match &rule.rule_type {
                RouterRuleType::Condition { condition } => {
                    self.match_condition(condition, request)
                }
                RouterRuleType::ModelPrefix { prefix } => {
                    request.model.starts_with(prefix)
                }
            };

            // Threshold check: rule only triggers if token_count >= threshold
            if matched {
                if let Some(threshold) = rule.threshold {
                    if request.token_count.is_none() {
                        request.token_count = Some(self.estimate_token_count(request));
                    }
                    let token_count = request.token_count.unwrap_or(0);
                    if token_count < threshold {
                        debug!(
                            "📋 Router rule '{}' matched but token_count {} < threshold {}, skipping",
                            rule.id.as_deref().unwrap_or("?"),
                            token_count, threshold
                        );
                        continue;
                    }
                }
                // Apply rewrites
                for rewrite in &rule.rewrite {
                    self.apply_rewrite(rewrite, request);
                }

                // Convenience: if rule.model is set, it's equivalent to rewriting request.body.model
                if let Some(ref model) = rule.model {
                    debug!("📋 Router rule matched, setting model to '{}'", model);
                    request.model = model.clone();
                    return Some(model.clone());
                }

                // If a rewrite changed the model, return the new model
                // (the rewrite would have modified request.model directly)
                return Some(request.model.clone());
            }
        }
        None
    }

    /// Evaluate a condition against the request
    fn match_condition(&self, condition: &crate::cli::RuleCondition, request: &AnthropicRequest) -> bool {
        // Resolve the left path to a value
        let left_value = self.resolve_path_value(&condition.left, request);

        match left_value {
            Some(val) => {
                let left_str = match &val {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => val.to_string(),
                };

                match condition.operator {
                    RuleOperator::Eq => left_str == condition.right,
                    RuleOperator::Ne => left_str != condition.right,
                    RuleOperator::Gt | RuleOperator::Ge | RuleOperator::Lt | RuleOperator::Le => {
                        // Numeric comparison
                        let left_num: f64 = match left_str.parse() {
                            Ok(n) => n,
                            Err(_) => return false,
                        };
                        let right_num: f64 = match condition.right.parse() {
                            Ok(n) => n,
                            Err(_) => return false,
                        };
                        match condition.operator {
                            RuleOperator::Gt => left_num > right_num,
                            RuleOperator::Ge => left_num >= right_num,
                            RuleOperator::Lt => left_num < right_num,
                            RuleOperator::Le => left_num <= right_num,
                            _ => unreachable!(),
                        }
                    }
                    RuleOperator::Contains => left_str.contains(&condition.right),
                    RuleOperator::ContainsDeep => {
                        self.contains_deep(&val, &condition.right)
                    }
                    RuleOperator::NotContains => !left_str.contains(&condition.right),
                    RuleOperator::StartsWith => left_str.starts_with(&condition.right),
                }
            }
            None => false,
        }
    }

    /// Deep search: recursively check if any string value in a JSON value contains the needle
    fn contains_deep(&self, value: &Value, needle: &str) -> bool {
        match value {
            Value::String(s) => s.contains(needle),
            Value::Array(arr) => arr.iter().any(|v| self.contains_deep(v, needle)),
            Value::Object(obj) => obj.values().any(|v| self.contains_deep(v, needle)),
            Value::Number(n) => n.to_string().contains(needle),
            Value::Bool(b) => b.to_string().contains(needle),
            _ => false,
        }
    }

    /// Estimate token count for threshold routing using tiktoken cl100k_base BPE.
    /// Lazily computed — only called when a router rule has a threshold set.
    fn estimate_token_count(&self, request: &AnthropicRequest) -> u32 {
        use tiktoken_rs::cl100k_base;
        let bpe = match cl100k_base() {
            Ok(bpe) => bpe,
            Err(e) => {
                debug!("Failed to load tiktoken BPE: {}, using char-based fallback", e);
                return request.messages.iter()
                    .map(|m| message_text_len(&m.content))
                    .sum::<usize>() as u32 / 4;
            }
        };
        let mut total: usize = 0;
        // System prompt tokens
        if let Some(ref system) = request.system {
            for t in system_text_iter(system) {
                total += bpe.encode_with_special_tokens(&t).len();
            }
        }
        // Message tokens
        for msg in &request.messages {
            total += bpe.encode_with_special_tokens(&msg.role).len();
            total += bpe.encode_with_special_tokens(&message_text(&msg.content)).len();
        }
        total as u32
    }

    /// Resolve a path like "request.body.model" or "request.body.messages.0.content" to a Value.
    /// Supports: request.body.* (serialized AnthropicRequest fields), request.model (shortcut)
    fn resolve_path_value(&self, path: &str, request: &AnthropicRequest) -> Option<Value> {
        // Normalize: strip "request." prefix
        let path = path.strip_prefix("request.").unwrap_or(path);

        // Handle request.model / request.body.model (same thing in CCM)
        if path == "model" || path == "body.model" {
            return Some(Value::String(request.model.clone()));
        }

        // Handle request.body.messages
        if path == "body.messages" || path == "messages" {
            return serde_json::to_value(&request.messages).ok();
        }

        // Handle request.body.messages.<index>.content
        // e.g. "body.messages.0.content" or "messages.0.content"
        let parts: Vec<&str> = path.split('.').collect();
        if parts.len() >= 3 && (parts[0] == "body" && parts[1] == "messages" || parts[0] == "messages") {
            let idx_offset = if parts[0] == "body" { 2 } else { 1 };
            let msg_idx: usize = parts[idx_offset - 1].parse().ok()?;
            if msg_idx >= request.messages.len() {
                return None;
            }
            let msg = &request.messages[msg_idx];
            // Match content
            let field = parts.get(idx_offset)?;
            if *field == "content" {
                match &msg.content {
                    crate::models::MessageContent::Text(t) => return Some(Value::String(t.clone())),
                    crate::models::MessageContent::Blocks(blocks) => {
                        return serde_json::to_value(blocks).ok();
                    }
                }
            }
        }

        // Handle request.body.system
        if path == "body.system" || path == "system" {
            return serde_json::to_value(&request.system).ok();
        }

        // Handle request.body.tools
        if path == "body.tools" || path == "tools" {
            return serde_json::to_value(&request.tools).ok();
        }

        // Fallback: serialize the whole request and navigate
        let body = serde_json::to_value(request).ok()?;
        let mut current = &body;
        for part in parts {
            // Try as object key first
            if let Ok(part_as_num) = part.parse::<usize>() {
                if let Some(arr) = current.as_array() {
                    if part_as_num < arr.len() {
                        current = &arr[part_as_num];
                        continue;
                    }
                }
            }
            current = current.get(part)?;
        }
        Some(current.clone())
    }

    /// Apply a rewrite operation to the request
    fn apply_rewrite(&self, rewrite: &RouterRuleRewrite, request: &mut AnthropicRequest) {
        let path = rewrite.key.strip_prefix("request.").unwrap_or(&rewrite.key);

        // Handle request.body.model (the main use case)
        if path == "body.model" || path == "model" {
            match rewrite.operation {
                RewriteOperation::Set => {
                    if let Some(ref value) = rewrite.value {
                        debug!("📋 Rewrite: model '{}' → '{}'", request.model, value);
                        request.model = value.clone();
                    }
                }
                RewriteOperation::Delete => {
                    // Can't delete model (required field), but reset to default
                    request.model = self.config.router.default.clone();
                }
                _ => {
                    debug!("📋 Rewrite operation {:?} not supported for model field", rewrite.operation);
                }
            }
            return;
        }

        // Handle request.header.<name> — set or delete a forwarded header
        if let Some(header_name) = path.strip_prefix("header.") {
            match rewrite.operation {
                RewriteOperation::Set => {
                    if let Some(ref value) = rewrite.value {
                        let key = header_name.to_lowercase();
                        // Remove existing entry if present (to update position)
                        request.forward_headers.retain(|(k, _)| k != &key);
                        request.forward_headers.push((key, value.clone()));
                        debug!("📋 Rewrite: header '{}' set to '{}'", header_name, value);
                    }
                }
                RewriteOperation::Delete => {
                    let key = header_name.to_lowercase();
                    let before = request.forward_headers.len();
                    request.forward_headers.retain(|(k, _)| k != &key);
                    if request.forward_headers.len() < before {
                        debug!("📋 Rewrite: header '{}' deleted", header_name);
                    }
                }
                _ => {
                    debug!("📋 Rewrite operation {:?} not supported for header '{}'", rewrite.operation, header_name);
                }
            }
            return;
        }

        // Other rewrite targets not yet implemented (body.* paths, array operations)
        debug!("📋 Rewrite to path '{}' not yet implemented (operation: {:?})", path, rewrite.operation);
    }

    /// Check if request has web_search tool (tool-based detection)
    /// Following claude-code-router pattern: checks if tools array contains web_search type
    fn has_web_search_tool(&self, request: &AnthropicRequest) -> bool {
        if let Some(ref tools) = request.tools {
            tools.iter().any(|tool| {
                tool.r#type
                    .as_ref()
                    .map(|t| t.starts_with("web_search"))
                    .unwrap_or(false)
            })
        } else {
            false
        }
    }

    /// Check if request is Plan Mode by detecting thinking field
    fn is_plan_mode(&self, request: &AnthropicRequest) -> bool {
        request
            .thinking
            .as_ref()
            .map(|t| t.r#type == "enabled")
            .unwrap_or(false)
    }

    /// Detect background tasks using regex pattern
    /// Uses background_regex from config (defaults to claude-haiku pattern)
    fn is_background_task(&self, model: &str) -> bool {
        if let Some(ref regex) = self.background_regex {
            regex.is_match(model)
        } else {
            false
        }
    }

    /// Match prompt rules against the turn-starting user message content
    /// Returns (model_name, matched_text) if a rule matches, None otherwise
    /// Strips the matched phrase from the prompt if strip_match is true
    /// For dynamic rules (model contains $refs), expands capture groups in the model name
    ///
    /// NOTE: We check the turn-starting message (not just the last user message) so that
    /// prompt phrases like "OPUS" persist for the entire turn, even through tool calls.
    fn match_prompt_rule(&self, request: &mut AnthropicRequest) -> Option<(String, String)> {
        if self.prompt_rules.is_empty() {
            return None;
        }

        // Debug: dump message structure for troubleshooting
        if tracing::enabled!(tracing::Level::DEBUG) {
            use crate::models::ContentBlock;
            for (idx, msg) in request.messages.iter().enumerate() {
                let content_desc = match &msg.content {
                    MessageContent::Text(t) => {
                        let preview: String = t.chars().take(60).collect();
                        format!("Text({:?}{})", preview, if t.len() > 60 { "..." } else { "" })
                    }
                    MessageContent::Blocks(blocks) => {
                        let types: Vec<&str> = blocks
                            .iter()
                            .map(|b| match b {
                                ContentBlock::Known(k) => match k {
                                    crate::models::KnownContentBlock::Text { .. } => "text",
                                    crate::models::KnownContentBlock::Image { .. } => "image",
                                    crate::models::KnownContentBlock::ToolUse { .. } => "tool_use",
                                    crate::models::KnownContentBlock::ToolResult { .. } => "tool_result",
                                    crate::models::KnownContentBlock::Thinking { .. } => "thinking",
                                },
                                ContentBlock::Unknown(_) => "unknown",
                            })
                            .collect();
                        format!("Blocks({:?})", types)
                    }
                };
                debug!(
                    "🔍 msg[{}] role={}: {}",
                    idx, msg.role, content_desc
                );
            }
        }

        // Extract turn-starting user message content (persists through tool calls)
        let user_content = self.extract_turn_starting_user_message(request)?;

        // Check each rule in order (first match wins)
        for rule in &self.prompt_rules {
            if let Some(captures) = rule.regex.captures(&user_content) {
                let matched_text = captures
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();

                // Resolve the model name (expand capture refs if dynamic)
                let model_name = if rule.is_dynamic {
                    Self::expand_model_template(&rule.model, &captures)
                } else {
                    rule.model.clone()
                };

                debug!(
                    "📝 Prompt rule matched: pattern='{}' → model='{}' (strip_match={})",
                    rule.regex.as_str(),
                    model_name,
                    rule.strip_match
                );

                // Strip the matched phrase from the turn-starting message if requested
                if rule.strip_match {
                    self.strip_match_from_turn_starting_message(request, &rule.regex);
                }

                return Some((model_name, matched_text));
            }
        }

        None
    }

    /// Expand capture group references in a model template string
    /// Supports $1, $name, ${1}, ${name} syntax via regex crate's Captures::expand
    fn expand_model_template(template: &str, captures: &regex::Captures) -> String {
        let mut expanded = String::new();
        captures.expand(template, &mut expanded);
        expanded
    }

    /// Extract the text content from the last user message
    fn extract_last_user_message(&self, request: &AnthropicRequest) -> Option<String> {
        // Find the last user message
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")?;

        // Extract text content (excluding system-reminder blocks)
        match &last_user.content {
            MessageContent::Text(text) => {
                if text.trim().starts_with("<system-reminder>") {
                    None
                } else {
                    Some(text.clone())
                }
            }
            MessageContent::Blocks(blocks) => {
                // Concatenate text blocks, excluding system-reminder blocks
                let text: String = blocks
                    .iter()
                    .filter_map(|block| block.as_text())
                    .filter(|s| !s.trim().starts_with("<system-reminder>"))
                    .collect::<Vec<_>>()
                    .join(" ");
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
        }
    }

    /// Extract the text content from the turn-starting user message
    ///
    /// A "turn" starts when:
    /// 1. The conversation begins, OR
    /// 2. After an assistant message that has no tool_use (i.e., the previous turn ended)
    ///
    /// This allows prompt phrases like "OPUS" to persist throughout a turn,
    /// even when the model makes tool calls and the last user message is just tool results.
    fn find_turn_start_index(&self, request: &AnthropicRequest) -> usize {
        use crate::models::ContentBlock;

        // Debug: log message structure for prompt rule detection
        debug!(
            "🔍 find_turn_start_index: {} messages in request",
            request.messages.len()
        );

        for (idx, msg) in request.messages.iter().enumerate().rev() {
            if msg.role == "assistant" {
                // Check if this assistant message has any tool_use blocks
                let has_tool_use = match &msg.content {
                    MessageContent::Text(_) => false,
                    MessageContent::Blocks(blocks) => blocks.iter().any(|block| {
                        matches!(
                            block,
                            ContentBlock::Known(crate::models::KnownContentBlock::ToolUse { .. })
                        )
                    }),
                };

                debug!(
                    "🔍 Assistant msg at idx={}: has_tool_use={}",
                    idx, has_tool_use
                );

                if !has_tool_use {
                    // This assistant message ends the previous turn
                    // Current turn starts after this message
                    debug!("🔍 Turn starts at idx={} (after assistant without tool_use)", idx + 1);
                    return idx + 1;
                }
            }
        }

        debug!("🔍 No turn boundary found, starting from idx=0");
        0 // No assistant message found, start from beginning
    }

    fn extract_turn_starting_user_message(&self, request: &AnthropicRequest) -> Option<String> {
        let turn_start_idx = self.find_turn_start_index(request);

        // Find the first user message with text content from turn_start_idx onwards
        for (offset, msg) in request.messages.iter().skip(turn_start_idx).enumerate() {
            if msg.role != "user" {
                continue;
            }

            // Check if this user message has text content (not just tool_result)
            let text_content = match &msg.content {
                MessageContent::Text(text) => {
                    if !text.trim().is_empty() && !text.trim().starts_with("<system-reminder>") {
                        Some(text.clone())
                    } else {
                        None
                    }
                }
                MessageContent::Blocks(blocks) => {
                    // Get text blocks, excluding system-reminder blocks (which are generated by client)
                    let text: String = blocks
                        .iter()
                        .filter_map(|block| block.as_text())
                        .filter(|s| !s.trim().starts_with("<system-reminder>"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                }
            };

            if let Some(ref content) = text_content {
                let preview: String = content.chars().take(80).collect();
                debug!(
                    "🔍 Turn-starting user msg at idx={}: {:?}{}",
                    turn_start_idx + offset,
                    preview,
                    if content.len() > 80 { "..." } else { "" }
                );
                return text_content;
            }
        }

        // Fallback to last user message if no turn-starting message found
        debug!("🔍 No turn-starting user message found, falling back to last user message");
        self.extract_last_user_message(request)
    }

    /// Strip the matched phrase from the turn-starting user message
    fn strip_match_from_turn_starting_message(&self, request: &mut AnthropicRequest, regex: &Regex) {
        let turn_start_idx = self.find_turn_start_index(request);

        // Find the first user message with text content from turn_start_idx onwards
        for msg in request.messages.iter_mut().skip(turn_start_idx) {
            if msg.role != "user" {
                continue;
            }

            // Check if this message has non-system-reminder text content
            let has_text = match &msg.content {
                MessageContent::Text(text) => {
                    !text.trim().is_empty() && !text.trim().starts_with("<system-reminder>")
                }
                MessageContent::Blocks(blocks) => blocks.iter().any(|block| {
                    block
                        .as_text()
                        .map(|s| !s.trim().is_empty() && !s.trim().starts_with("<system-reminder>"))
                        .unwrap_or(false)
                }),
            };

            if has_text {
                // This is the turn-starting message, strip from it and return
                match &mut msg.content {
                    MessageContent::Text(text) => {
                        let new_text = regex.replace_all(text, "").to_string();
                        if new_text != *text {
                            debug!("🔪 Stripped matched phrase from turn-starting prompt");
                            *text = new_text;
                        }
                    }
                    MessageContent::Blocks(blocks) => {
                        for block in blocks.iter_mut() {
                            if let Some(text) = block.as_text_mut() {
                                let new_text = regex.replace_all(text, "").to_string();
                                if new_text != *text {
                                    debug!("🔪 Stripped matched phrase from turn-starting prompt block");
                                    *text = new_text;
                                }
                            }
                        }
                    }
                }
                return;
            }
        }

        // Fallback: strip from last user message if no turn-starting message found
        // (matches the fallback behavior in extract_turn_starting_user_message)
        self.strip_match_from_last_user_message(request, regex);
    }

    /// Strip the matched phrase from the last user message (fallback for edge cases)
    fn strip_match_from_last_user_message(&self, request: &mut AnthropicRequest, regex: &Regex) {
        // Find the last user message (mutable)
        let last_user = request.messages.iter_mut().rev().find(|m| m.role == "user");

        if let Some(msg) = last_user {
            match &mut msg.content {
                MessageContent::Text(text) => {
                    let stripped = regex.replace_all(text, "").to_string();
                    if stripped != *text {
                        debug!("🔪 Stripped matched phrase from prompt");
                        *text = stripped;
                    }
                }
                MessageContent::Blocks(blocks) => {
                    // Strip from all text blocks
                    for block in blocks.iter_mut() {
                        if let Some(text) = block.as_text_mut() {
                            let stripped = regex.replace_all(text, "").to_string();
                            if stripped != *text {
                                debug!("🔪 Stripped matched phrase from prompt block");
                                *text = stripped;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Extract subagent model from system prompt tag
    /// Checks for <CCM-SUBAGENT-MODEL>model-name</CCM-SUBAGENT-MODEL> in system[1].text
    /// and removes the tag after extraction.
    ///
    /// First attempts to resolve the tag value as a model name in the models config.
    /// Falls back to treating it as a direct provider model name (deprecated behavior).
    fn extract_subagent_model(&self, request: &mut AnthropicRequest) -> Option<String> {
        // Check if system exists and is Blocks type with at least 2 blocks
        let system = request.system.as_mut()?;

        if let SystemPrompt::Blocks(blocks) = system {
            if blocks.len() < 2 {
                return None;
            }

            // Check second block (index 1) for tag
            let second_block = &mut blocks[1];
            if !second_block.text.contains("<CCM-SUBAGENT-MODEL>") {
                return None;
            }

            // Extract model name using regex
            let re = Regex::new(r"<CCM-SUBAGENT-MODEL>(.*?)</CCM-SUBAGENT-MODEL>")
                .expect("Invalid regex pattern");

            if let Some(captures) = re.captures(&second_block.text) {
                if let Some(model_match) = captures.get(1) {
                    let tag_value = model_match.as_str().to_string();

                    // Remove the tag from the text
                    second_block.text = re.replace_all(&second_block.text, "").to_string();

                    // First, try to find a model with this name in the models config (case-insensitive)
                    if let Some(_model) = self.config.models.iter().find(|m| m.name.eq_ignore_ascii_case(&tag_value)) {
                        // Found a configured model with this name (use the configured case)
                        return Some(_model.name.clone());
                    }

                    // DEPRECATED: Fall back to treating the tag value as a direct provider model name
                    // This behavior is deprecated and should not be relied upon.
                    // Please configure a named model in the [models] section instead.
                    debug!("⚠️  CCM-SUBAGENT-MODEL tag '{}' not found in models config, using as direct provider model name (deprecated)", tag_value);
                    return Some(tag_value);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{RouterConfig, ServerConfig};
    use crate::models::{Message, MessageContent, ThinkingConfig};

    fn create_test_config() -> AppConfig {
        AppConfig {
            server: ServerConfig::default(),
            router: RouterConfig {
                default: "default.model".to_string(),
                background: Some("background.model".to_string()),
                think: Some("think.model".to_string()),
                websearch: Some("websearch.model".to_string()),
                auto_map_regex: None,   // Use default Claude pattern
                background_regex: None, // Use default claude-haiku pattern
                prompt_rules: vec![],   // No prompt rules by default
                rules: vec![],          // No router rules by default
            },
            providers: vec![],
            models: vec![],
        }
    }

    fn create_simple_request(text: &str) -> AnthropicRequest {
        AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text(text.to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        }
    }

    #[test]
    fn test_plan_mode_detection() {
        let config = create_test_config();
        let router = Router::new(config);

        let mut request = create_simple_request("Explain quantum computing");
        request.thinking = Some(ThinkingConfig {
            r#type: "enabled".to_string(),
            budget_tokens: Some(10_000),
        });

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Think);
        assert_eq!(decision.model_name, "think.model");
    }

    #[test]
    fn test_background_task_detection() {
        let config = create_test_config();
        let router = Router::new(config);

        // Create request with haiku model
        let mut request = create_simple_request("Hello");
        request.model = "claude-3-5-haiku-20241022".to_string();

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Background);
        assert_eq!(decision.model_name, "background.model");
    }

    #[test]
    fn test_default_routing() {
        let mut config = create_test_config();
        config.router.background = None; // Disable background routing
        let router = Router::new(config);

        let mut request = create_simple_request("Write a function to sort an array");

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Default);
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_routing_priority() {
        let config = create_test_config();
        let router = Router::new(config);

        // Think has highest priority
        let mut request = create_simple_request("Explain complex topic");
        request.thinking = Some(ThinkingConfig {
            r#type: "enabled".to_string(),
            budget_tokens: Some(10_000),
        });

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Think); // Think wins
    }

    #[test]
    fn test_websearch_tool_detection() {
        let config = create_test_config();
        let router = Router::new(config);

        let mut request = create_simple_request("Search the web for latest news");
        request.tools = Some(vec![crate::models::Tool {
            r#type: Some("web_search_2025_04".to_string()),
            name: Some("web_search".to_string()),
            description: Some("Search the web".to_string()),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {}
            })),
        }]);

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::WebSearch);
        assert_eq!(decision.model_name, "websearch.model");
    }

    #[test]
    fn test_websearch_has_highest_priority() {
        let config = create_test_config();
        let router = Router::new(config);

        // WebSearch should win even if thinking is enabled
        let mut request = create_simple_request("Search and explain");
        request.thinking = Some(ThinkingConfig {
            r#type: "enabled".to_string(),
            budget_tokens: Some(10_000),
        });
        request.tools = Some(vec![crate::models::Tool {
            r#type: Some("web_search".to_string()),
            name: None,
            description: None,
            input_schema: None,
        }]);

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::WebSearch); // WebSearch wins over Think
        assert_eq!(decision.model_name, "websearch.model");
    }

    #[test]
    fn test_auto_map_claude_models() {
        let config = create_test_config();
        let router = Router::new(config);

        // Test Claude model auto-mapping (default pattern)
        let mut request = create_simple_request("Hello");
        request.model = "claude-3-5-sonnet-20241022".to_string();

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Default);
        assert_eq!(decision.model_name, "default.model"); // Auto-mapped to default
    }

    #[test]
    fn test_auto_map_custom_regex() {
        let mut config = create_test_config();
        config.router.auto_map_regex = Some("^(claude-|gpt-)".to_string());
        let router = Router::new(config);

        // Test GPT model auto-mapping with custom regex
        let mut request = create_simple_request("Hello");
        request.model = "gpt-4".to_string();

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Default);
        assert_eq!(decision.model_name, "default.model"); // Auto-mapped to default
    }

    #[test]
    fn test_no_auto_map_non_matching() {
        let config = create_test_config();
        let router = Router::new(config);

        // Test non-Claude model (should not auto-map, use model name as-is)
        let mut request = create_simple_request("Hello");
        request.model = "glm-4.6".to_string();

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::Default);
        assert_eq!(decision.model_name, "glm-4.6"); // Uses original model name (no auto-mapping)
    }

    #[test]
    fn test_prompt_rule_matching() {
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: "(?i)commit.*changes".to_string(),
            model: "fast-model".to_string(),
            strip_match: false,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("Please commit these changes");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "fast-model");
    }

    #[test]
    fn test_prompt_rule_strip_match() {
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"\[fast\]".to_string(),
            model: "fast-model".to_string(),
            strip_match: true,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("[fast] Write a function to sort an array");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "fast-model");

        // Verify the matched phrase was stripped from the prompt
        if let MessageContent::Text(text) = &request.messages[0].content {
            assert_eq!(text, " Write a function to sort an array");
            assert!(!text.contains("[fast]"));
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_prompt_rule_no_strip_match() {
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"\[fast\]".to_string(),
            model: "fast-model".to_string(),
            strip_match: false,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("[fast] Write a function to sort an array");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "fast-model");

        // Verify the matched phrase was NOT stripped (strip_match = false)
        if let MessageContent::Text(text) = &request.messages[0].content {
            assert!(text.contains("[fast]"));
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_prompt_rule_dynamic_model_numeric() {
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"(?i)CCM-MODEL:([a-zA-Z0-9._-]+)".to_string(),
            model: "$1".to_string(),
            strip_match: true,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("CCM-MODEL:deepseek-v3 Write a function");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "deepseek-v3");

        // Verify strip worked
        if let MessageContent::Text(text) = &request.messages[0].content {
            assert!(!text.contains("CCM-MODEL"));
            assert!(text.contains("Write a function"));
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_prompt_rule_dynamic_model_named() {
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"(?i)USE-MODEL:(?P<model>[a-zA-Z0-9._-]+)".to_string(),
            model: "$model".to_string(),
            strip_match: true,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("USE-MODEL:gpt-4o please help");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "gpt-4o");
    }

    #[test]
    fn test_prompt_rule_dynamic_model_with_prefix() {
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"@(\w+)-mode".to_string(),
            model: "provider-$1".to_string(),
            strip_match: false,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("@fast-mode explain this");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "provider-fast");
    }

    #[test]
    fn test_prompt_rule_static_model_unchanged() {
        // Ensure existing static behavior is preserved (no $ references)
        use crate::cli::PromptRule;
        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"\[static\]".to_string(),
            model: "static-model".to_string(), // No $ references
            strip_match: true,
        }];
        let router = Router::new(config);

        let mut request = create_simple_request("[static] do something");
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "static-model");
    }

    #[test]
    fn test_contains_capture_reference() {
        assert!(super::contains_capture_reference("$1"));
        assert!(super::contains_capture_reference("$model"));
        assert!(super::contains_capture_reference("${1}"));
        assert!(super::contains_capture_reference("${name}"));
        assert!(super::contains_capture_reference("prefix-$1-suffix"));
        assert!(!super::contains_capture_reference("static-model"));
        assert!(!super::contains_capture_reference("no-refs-here"));
    }

    #[test]
    fn test_prompt_rule_persists_through_tool_calls() {
        // Test that prompt phrases "stick" for the entire turn, even after tool calls
        use crate::cli::PromptRule;
        use crate::models::{ContentBlock, KnownContentBlock, ToolResultContent};

        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"(?i)OPUS".to_string(),
            model: "opus-model".to_string(),
            strip_match: false,
        }];
        let router = Router::new(config);

        // Simulate a turn with tool calls:
        // 1. User: "OPUS write me a test suite"
        // 2. Assistant: [tool_use: Read]
        // 3. User: [tool_result: file contents]
        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![
                // Turn-starting user message with prompt phrase
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Text("OPUS write me a test suite".to_string()),
                },
                // Assistant response with tool_use
                Message {
                    role: "assistant".to_string(),
                    content: MessageContent::Blocks(vec![
                        ContentBlock::Known(KnownContentBlock::ToolUse {
                            id: "tool_1".to_string(),
                            name: "Read".to_string(),
                            input: serde_json::json!({"file_path": "/src/main.rs"}),
                        }),
                    ]),
                },
                // User message with only tool_result (no text)
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Blocks(vec![
                        ContentBlock::Known(KnownContentBlock::ToolResult {
                            tool_use_id: "tool_1".to_string(),
                            content: ToolResultContent::Text("fn main() {}".to_string()),
                            is_error: false,
                            cache_control: None,
                        }),
                    ]),
                },
            ],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Should match the "OPUS" from the turn-starting message, not the tool_result
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "opus-model");
    }

    #[test]
    fn test_prompt_rule_resets_after_turn_ends() {
        // Test that prompt phrases reset when a new turn starts
        // (after an assistant message without tool_use)
        use crate::cli::PromptRule;

        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"(?i)OPUS".to_string(),
            model: "opus-model".to_string(),
            strip_match: false,
        }];
        let router = Router::new(config);

        // Simulate two turns:
        // Turn 1: User: "OPUS write me tests" → Assistant: "Here are the tests..."
        // Turn 2: User: "Now add documentation" (no OPUS)
        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![
                // Turn 1: User with OPUS
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Text("OPUS write me tests".to_string()),
                },
                // Turn 1: Assistant response (text only, no tool_use - ends the turn)
                Message {
                    role: "assistant".to_string(),
                    content: MessageContent::Text("Here are the tests...".to_string()),
                },
                // Turn 2: User without OPUS (new turn)
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Text("Now add documentation".to_string()),
                },
            ],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Should NOT match "OPUS" because it was in the previous turn
        // The current turn started with "Now add documentation"
        assert_eq!(decision.route_type, RouteType::Default);
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_prompt_rule_strip_match_in_multi_turn() {
        // Test that strip_match works on the turn-starting message in a multi-message turn
        use crate::cli::PromptRule;
        use crate::models::{ContentBlock, KnownContentBlock, ToolResultContent};

        let mut config = create_test_config();
        config.router.prompt_rules = vec![PromptRule {
            pattern: r"\[OPUS\]".to_string(),
            model: "opus-model".to_string(),
            strip_match: true,
        }];
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![
                // Turn-starting message with [OPUS] tag
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Text("[OPUS] write me tests".to_string()),
                },
                // Assistant with tool_use
                Message {
                    role: "assistant".to_string(),
                    content: MessageContent::Blocks(vec![
                        ContentBlock::Known(KnownContentBlock::ToolUse {
                            id: "tool_1".to_string(),
                            name: "Read".to_string(),
                            input: serde_json::json!({}),
                        }),
                    ]),
                },
                // User with tool_result
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Blocks(vec![
                        ContentBlock::Known(KnownContentBlock::ToolResult {
                            tool_use_id: "tool_1".to_string(),
                            content: ToolResultContent::Text("content".to_string()),
                            is_error: false,
                            cache_control: None,
                        }),
                    ]),
                },
            ],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.route_type, RouteType::PromptRule);
        assert_eq!(decision.model_name, "opus-model");

        // Verify [OPUS] was stripped from the first (turn-starting) message
        if let MessageContent::Text(text) = &request.messages[0].content {
            assert!(!text.contains("[OPUS]"));
            assert!(text.contains("write me tests"));
        } else {
            panic!("Expected text content in first message");
        }
    }

    #[test]
    fn test_router_rule_condition_eq_model() {
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("test-eq".to_string()),
            name: Some("Equal model test".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: "request.body.model".to_string(),
                    operator: RuleOperator::Eq,
                    right: "gpt-4".to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some("claude-sonnet-4".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "gpt-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "claude-sonnet-4");
    }

    #[test]
    fn test_router_rule_model_prefix() {
        let config = create_test_config();
        // Use auto_map_regex = Some("".to_string()) to disable auto-mapping
        let rules = vec![RouterRule {
            id: Some("test-prefix".to_string()),
            name: Some("Prefix test".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "claude-opus".to_string(),
            },
            enabled: true,
            rewrite: vec![],
            model: Some("think.model".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()), // Match nothing (disable auto-map)
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "claude-opus-4-20250514".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "think.model");
    }

    #[test]
    fn test_router_rule_contains_deep() {
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("test-contains-deep".to_string()),
            name: Some("Deep search test".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: "request.body.messages".to_string(),
                    operator: RuleOperator::ContainsDeep,
                    right: "large_file".to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some("think.model".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "default.model".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("Please read the large_file content".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "think.model");
    }

    #[test]
    fn test_router_rule_disabled() {
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("test-disabled".to_string()),
            name: Some("Disabled rule".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "claude".to_string(),
            },
            enabled: false, // Disabled
            rewrite: vec![],
            model: Some("think.model".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some(String::new()), // Disable auto-map
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        // Disabled rule should NOT match → falls through to default
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_router_rule_no_match_falls_through() {
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("test-no-match".to_string()),
            name: Some("No match test".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: "request.body.model".to_string(),
                    operator: RuleOperator::Eq,
                    right: "nonexistent-model".to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some("think.model".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "claude-sonnet-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        // Rule doesn't match → falls through to default
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "default.model");
    }

    // Integration-style tests using real model names from the production config:
    // deepseek-v4-flash, deepseek-v4-pro, claude-haiku-4-5

    #[test]
    fn test_real_router_rule_eq_deepseek_flash() {
        // Simulate: rule matches model == "deepseek-v4-flash" → route to "claude-haiku-4-5"
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("deepseek-flash-rule".to_string()),
            name: Some("Route deepseek-v4-flash to haiku".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: "request.body.model".to_string(),
                    operator: RuleOperator::Eq,
                    right: "deepseek-v4-flash".to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some("claude-haiku-4-5".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()), // Disable auto-map for non-claude models
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "claude-haiku-4-5");
    }

    #[test]
    fn test_real_router_rule_prefix_deepseek() {
        // Simulate: any model starting with "deepseek" → route to "deepseek-v4-pro"
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("deepseek-prefix".to_string()),
            name: Some("Route all deepseek models".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "deepseek".to_string(),
            },
            enabled: true,
            rewrite: vec![],
            model: Some("deepseek-v4-pro".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()), // Disable auto-map
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "deepseek-v4-pro");
    }

    #[test]
    fn test_real_router_rule_rewrite_model() {
        // Simulate: rewrite model from deepseek-v4-flash → claude-haiku-4-5 via set operation
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("rewrite-test".to_string()),
            name: Some("Rewrite model name".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "deepseek".to_string(),
            },
            enabled: true,
            rewrite: vec![RouterRuleRewrite {
                key: "request.body.model".to_string(),
                operation: RewriteOperation::Set,
                value: Some("claude-haiku-4-5".to_string()),
                r#match: None,
            }],
            model: None, // No model override, rely on rewrite
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // After rewrite, request.model should be "claude-haiku-4-5"
        assert_eq!(request.model, "claude-haiku-4-5");
        // Decision should use the rewritten model name (not a rule model override)
        assert_eq!(decision.model_name, "claude-haiku-4-5");
    }

    #[test]
    fn test_real_router_rule_contains_deep_with_deepseek() {
        // Simulate: deep search in messages for "deepseek" keyword → route to think model
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("keyword-route".to_string()),
            name: Some("Route based on keyword".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: "request.body.messages".to_string(),
                    operator: RuleOperator::ContainsDeep,
                    right: "deepseek".to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some("deepseek-v4-pro".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("Please use deepseek for this task".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "deepseek-v4-pro");
    }

    #[test]
    fn test_real_router_rule_no_match_keeps_original() {
        // Simulate: rule doesn't match deepseek-v4-flash → falls through to default
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("no-match-real".to_string()),
            name: Some("No match real test".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: "request.body.model".to_string(),
                    operator: RuleOperator::Eq,
                    right: "claude-opus-4-5".to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some("claude-haiku-4-5".to_string()),
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()), // Disable auto-map
                ..config.router
            },
            ..config
        });

        let mut request = AnthropicRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        // Rule expects claude-opus-4-5, request has deepseek-v4-flash → no match
        // auto_map disabled (^$), so model stays as original deepseek-v4-flash
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "deepseek-v4-flash");
    }

    // ===== P0: Operator coverage tests =====

    fn make_condition_rule(
        operator: RuleOperator,
        left: &str,
        right: &str,
        target_model: &str,
    ) -> RouterRule {
        RouterRule {
            id: Some("op-test".to_string()),
            name: Some("Operator test".to_string()),
            rule_type: RouterRuleType::Condition {
                condition: crate::cli::RuleCondition {
                    left: left.to_string(),
                    operator,
                    right: right.to_string(),
                },
            },
            enabled: true,
            rewrite: vec![],
            model: Some(target_model.to_string()),
            threshold: None,
        }
    }

    fn make_request_with_model(model: &str) -> AnthropicRequest {
        AnthropicRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: None,
            tools: None,
            forward_headers: vec![],
            token_count: None,
        }
    }

    fn route_with_rule(rule: RouterRule, request: &mut AnthropicRequest) -> String {
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        router.route(request).unwrap().model_name
    }

    #[test]
    fn test_operator_ne_match() {
        // model != "gpt-4" → "claude-opus-4" should match and route to target
        let rule = make_condition_rule(RuleOperator::Ne, "request.body.model", "gpt-4", "think.model");
        let mut request = make_request_with_model("claude-opus-4");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_ne_no_match() {
        // model != "claude-opus-4" but model IS "claude-opus-4" → false → no rule match
        // auto_map disabled (^$), so model stays original → default fallback returns original
        let rule = make_condition_rule(RuleOperator::Ne, "request.body.model", "claude-opus-4", "think.model");
        let mut request = make_request_with_model("claude-opus-4");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // With auto_map disabled (^$), model is not auto-mapped to default.model,
        // so the default fallback returns the original model name unchanged.
        assert_eq!(decision.model_name, "claude-opus-4");
    }

    #[test]
    fn test_operator_gt_match() {
        // "100" > "50" → match
        let rule = make_condition_rule(RuleOperator::Gt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("100");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_gt_no_match() {
        // "10" > "50" → false → default
        let rule = make_condition_rule(RuleOperator::Gt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("10");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // With auto_map disabled (^$), "10" is not auto-mapped, so default fallback
        // returns the original model name "10", not "default.model".
        assert_eq!(decision.model_name, "10");
    }

    #[test]
    fn test_operator_ge_match_equal() {
        // "50" >= "50" → true (boundary: equal case)
        let rule = make_condition_rule(RuleOperator::Ge, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("50");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_ge_match_greater() {
        // "51" >= "50" → true
        let rule = make_condition_rule(RuleOperator::Ge, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("51");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_ge_no_match() {
        // "49" >= "50" → false → no rule match
        let rule = make_condition_rule(RuleOperator::Ge, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("49");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled, "49" not mapped → returns original "49"
        assert_eq!(decision.model_name, "49");
    }

    #[test]
    fn test_operator_gt_equal_boundary() {
        // "50" > "50" → false (strict greater, equal should NOT match)
        let rule = make_condition_rule(RuleOperator::Gt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("50");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // Gt is strict: 50 > 50 is false → no match → returns original "50"
        assert_eq!(decision.model_name, "50");
    }

    #[test]
    fn test_operator_lt_match() {
        // "10" < "50" → true
        let rule = make_condition_rule(RuleOperator::Lt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("10");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_lt_no_match() {
        // "100" < "50" → false → default
        let rule = make_condition_rule(RuleOperator::Lt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("100");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled (^$), "100" not auto-mapped → returns original "100"
        assert_eq!(decision.model_name, "100");
    }

    #[test]
    fn test_operator_le_match_equal() {
        // "50" <= "50" → true (boundary: equal case)
        let rule = make_condition_rule(RuleOperator::Le, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("50");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_le_match_less() {
        // "49" <= "50" → true
        let rule = make_condition_rule(RuleOperator::Le, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("49");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_le_no_match() {
        // "51" <= "50" → false → no rule match
        let rule = make_condition_rule(RuleOperator::Le, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("51");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled, "51" not mapped → returns original "51"
        assert_eq!(decision.model_name, "51");
    }

    #[test]
    fn test_operator_lt_equal_boundary() {
        // "50" < "50" → false (strict less, equal should NOT match)
        let rule = make_condition_rule(RuleOperator::Lt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("50");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // Lt is strict: 50 < 50 is false → no match → returns original "50"
        assert_eq!(decision.model_name, "50");
    }

    #[test]
    fn test_operator_numeric_parse_failure_left() {
        // left = "abc" can't parse to f64 → condition returns false → default
        let rule = make_condition_rule(RuleOperator::Gt, "request.body.model", "50", "think.model");
        let mut request = make_request_with_model("abc");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled (^$), "abc" not auto-mapped → returns original "abc"
        assert_eq!(decision.model_name, "abc");
    }

    #[test]
    fn test_operator_numeric_parse_failure_right() {
        // right = "xyz" can't parse to f64 → condition returns false → default
        let rule = make_condition_rule(RuleOperator::Lt, "request.body.model", "xyz", "think.model");
        let mut request = make_request_with_model("10");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled (^$), "10" not auto-mapped → returns original "10"
        assert_eq!(decision.model_name, "10");
    }

    #[test]
    fn test_operator_numeric_decimal_values() {
        // 3.14 > 3.0 → true (float comparison)
        let rule = make_condition_rule(RuleOperator::Gt, "request.body.model", "3.0", "think.model");
        let mut request = make_request_with_model("3.14");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_numeric_negative_values() {
        // -5 < -1 → true
        let rule = make_condition_rule(RuleOperator::Lt, "request.body.model", "-1", "think.model");
        let mut request = make_request_with_model("-5");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_contains_match() {
        // model contains "sonnet" → match
        let rule = make_condition_rule(RuleOperator::Contains, "request.body.model", "sonnet", "think.model");
        let mut request = make_request_with_model("claude-sonnet-4");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_contains_no_match() {
        // model contains "sonnet" but model is "gpt-4" → false → default
        let rule = make_condition_rule(RuleOperator::Contains, "request.body.model", "sonnet", "think.model");
        let mut request = make_request_with_model("gpt-4");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled (^$), "gpt-4" not auto-mapped → returns original "gpt-4"
        assert_eq!(decision.model_name, "gpt-4");
    }

    #[test]
    fn test_operator_not_contains_match() {
        // model NOT contains "sonnet" → "gpt-4" doesn't contain "sonnet" → true
        let rule = make_condition_rule(RuleOperator::NotContains, "request.body.model", "sonnet", "think.model");
        let mut request = make_request_with_model("gpt-4");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_not_contains_no_match() {
        // model NOT contains "sonnet" but model IS "claude-sonnet-4" → false → default
        let rule = make_condition_rule(RuleOperator::NotContains, "request.body.model", "sonnet", "think.model");
        let mut request = make_request_with_model("claude-sonnet-4");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled (^$), "claude-sonnet-4" not auto-mapped → returns original
        assert_eq!(decision.model_name, "claude-sonnet-4");
    }

    #[test]
    fn test_operator_starts_with_match() {
        // model starts with "claude" → "claude-opus-4" matches → think.model
        let rule = make_condition_rule(RuleOperator::StartsWith, "request.body.model", "claude", "think.model");
        let mut request = make_request_with_model("claude-opus-4");
        assert_eq!(route_with_rule(rule, &mut request), "think.model");
    }

    #[test]
    fn test_operator_starts_with_no_match() {
        // model starts with "claude" but model is "gpt-4" → false → default
        let rule = make_condition_rule(RuleOperator::StartsWith, "request.body.model", "claude", "think.model");
        let mut request = make_request_with_model("gpt-4");
        let config = create_test_config();
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let decision = router.route(&mut request).unwrap();
        // auto_map disabled (^$), "gpt-4" not auto-mapped → returns original "gpt-4"
        assert_eq!(decision.model_name, "gpt-4");
    }

    // ===== P1: Rewrite Delete operation =====

    #[test]
    fn test_rewrite_delete_resets_to_default() {
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("delete-test".to_string()),
            name: Some("Delete rewrite test".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "deepseek".to_string(),
            },
            enabled: true,
            rewrite: vec![RouterRuleRewrite {
                key: "request.body.model".to_string(),
                operation: RewriteOperation::Delete,
                value: None,
                r#match: None,
            }],
            model: None,
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = make_request_with_model("deepseek-v4-flash");
        let decision = router.route(&mut request).unwrap();
        // Delete resets model to default.model
        assert_eq!(decision.model_name, "default.model");
        assert_eq!(request.model, "default.model");
    }

    #[test]
    fn test_rewrite_header_set() {
        // Simulate: set a custom header via rewrite
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("header-set-test".to_string()),
            name: Some("Set custom header".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "deepseek".to_string(),
            },
            enabled: true,
            rewrite: vec![RouterRuleRewrite {
                key: "request.header.x-target-provider".to_string(),
                operation: RewriteOperation::Set,
                value: Some("openai".to_string()),
                r#match: None,
            }],
            model: None,
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = make_request_with_model("deepseek-v4-flash");
        let _decision = router.route(&mut request).unwrap();
        // Verify header was set in forward_headers
        assert!(request.forward_headers.iter().any(|(k, v)| k == "x-target-provider" && v == "openai"));
    }

    #[test]
    fn test_rewrite_header_delete() {
        // Simulate: delete a header from forward_headers
        let config = create_test_config();
        let rules = vec![RouterRule {
            id: Some("header-del-test".to_string()),
            name: Some("Delete header".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "deepseek".to_string(),
            },
            enabled: true,
            rewrite: vec![RouterRuleRewrite {
                key: "request.header.x-api-key".to_string(),
                operation: RewriteOperation::Delete,
                value: None,
                r#match: None,
            }],
            model: None,
            threshold: None,
        }];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = make_request_with_model("deepseek-v4-flash");
        // Add a test header that should be deleted
        request.forward_headers.push(("x-api-key".to_string(), "test-value".to_string()));
        let _decision = router.route(&mut request).unwrap();
        // Verify header was removed
        assert!(!request.forward_headers.iter().any(|(k, _)| k == "x-api-key"));
    }

    // ===== P1: extract_subagent_model =====

    #[test]
    fn test_subagent_model_extraction_with_config() {
        use crate::cli::{ModelConfig, ModelMapping};
        use crate::models::{SystemBlock, SystemPrompt};

        let config = create_test_config();
        let config = AppConfig {
            models: vec![ModelConfig {
                name: "custom-agent-model".to_string(),
                mappings: vec![ModelMapping {
                    priority: 1,
                    provider: "test-provider".to_string(),
                    actual_model: "provider-actual-model".to_string(),
                    inject_continuation_prompt: false,
                }],
            }],
            ..config
        };
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: Some(SystemPrompt::Blocks(vec![
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "System prompt block 1".to_string(),
                    cache_control: None,
                },
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "Context here <CCM-SUBAGENT-MODEL>custom-agent-model</CCM-SUBAGENT-MODEL> end".to_string(),
                    cache_control: None,
                },
            ])),
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Should route to the configured model name
        assert_eq!(decision.model_name, "custom-agent-model");

        // Verify the tag was removed from the system prompt
        if let Some(SystemPrompt::Blocks(blocks)) = &request.system {
            assert!(!blocks[1].text.contains("<CCM-SUBAGENT-MODEL>"));
        } else {
            panic!("Expected SystemPrompt::Blocks");
        }
    }

    #[test]
    fn test_subagent_model_extraction_fallback_deprecated() {
        // Tag value not in models config → falls back to direct provider model name
        use crate::models::{SystemBlock, SystemPrompt};

        let config = create_test_config();
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: Some(SystemPrompt::Blocks(vec![
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "Block 1".to_string(),
                    cache_control: None,
                },
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "<CCM-SUBAGENT-MODEL>unregistered-model</CCM-SUBAGENT-MODEL>".to_string(),
                    cache_control: None,
                },
            ])),
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Falls back to direct provider model name (deprecated behavior)
        assert_eq!(decision.model_name, "unregistered-model");
    }

    #[test]
    fn test_subagent_model_no_system_prompt() {
        let config = create_test_config();
        let router = Router::new(config);

        let mut request = make_request_with_model("claude-opus-4");
        // system is None → no subagent model extraction
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_subagent_model_text_system_prompt() {
        // System prompt as Text (not Blocks) → no subagent extraction
        use crate::models::SystemPrompt;

        let config = create_test_config();
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: Some(SystemPrompt::Text("Just text".to_string())),
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Text system prompt → no extraction → default
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_subagent_model_single_block() {
        // Only 1 block → needs at least 2 → no extraction
        use crate::models::{SystemBlock, SystemPrompt};

        let config = create_test_config();
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: Some(SystemPrompt::Blocks(vec![
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "<CCM-SUBAGENT-MODEL>some-model</CCM-SUBAGENT-MODEL>".to_string(),
                    cache_control: None,
                },
            ])),
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Only 1 block (needs >= 2) → no extraction → auto-map to default
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_subagent_model_no_tag_in_second_block() {
        // 2 blocks but second block has no tag → no extraction
        use crate::models::{SystemBlock, SystemPrompt};

        let config = create_test_config();
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: Some(SystemPrompt::Blocks(vec![
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "Block 1".to_string(),
                    cache_control: None,
                },
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "No tag here".to_string(),
                    cache_control: None,
                },
            ])),
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "default.model");
    }

    #[test]
    fn test_subagent_model_case_insensitive_config_lookup() {
        // Tag value "Custom-Model" should match config model "custom-model" (case-insensitive)
        use crate::cli::{ModelConfig, ModelMapping};
        use crate::models::{SystemBlock, SystemPrompt};

        let config = create_test_config();
        let config = AppConfig {
            models: vec![ModelConfig {
                name: "custom-model".to_string(),
                mappings: vec![ModelMapping {
                    priority: 1,
                    provider: "test".to_string(),
                    actual_model: "actual".to_string(),
                    inject_continuation_prompt: false,
                }],
            }],
            ..config
        };
        let router = Router::new(config);

        let mut request = AnthropicRequest {
            model: "claude-opus-4".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::Text("hello".to_string()),
            }],
            max_tokens: 1024,
            thinking: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            metadata: None,
            system: Some(SystemPrompt::Blocks(vec![
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "Block 1".to_string(),
                    cache_control: None,
                },
                SystemBlock {
                    r#type: "text".to_string(),
                    text: "<CCM-SUBAGENT-MODEL>Custom-Model</CCM-SUBAGENT-MODEL>".to_string(),
                    cache_control: None,
                },
            ])),
            tools: None,
            forward_headers: vec![],
            token_count: None,
        };

        let decision = router.route(&mut request).unwrap();
        // Should resolve to the configured model name (lowercase)
        assert_eq!(decision.model_name, "custom-model");
    }

    // ===== P1: Multi-rule priority =====

    #[test]
    fn test_multiple_rules_first_match_wins() {
        let config = create_test_config();
        let rules = vec![
            RouterRule {
                id: Some("rule-1".to_string()),
                name: Some("First rule".to_string()),
                rule_type: RouterRuleType::ModelPrefix {
                    prefix: "claude".to_string(),
                },
                enabled: true,
                rewrite: vec![],
                model: Some("first-model".to_string()),
                threshold: None,
            },
            RouterRule {
                id: Some("rule-2".to_string()),
                name: Some("Second rule".to_string()),
                rule_type: RouterRuleType::ModelPrefix {
                    prefix: "claude".to_string(),
                },
                enabled: true,
                rewrite: vec![],
                model: Some("second-model".to_string()),
                threshold: None,
            },
        ];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = make_request_with_model("claude-opus-4");
        let decision = router.route(&mut request).unwrap();
        // First matching rule wins
        assert_eq!(decision.model_name, "first-model");
    }

    #[test]
    fn test_multiple_rules_skip_disabled_match_second() {
        let config = create_test_config();
        let rules = vec![
            RouterRule {
                id: Some("rule-1".to_string()),
                name: Some("First rule (disabled)".to_string()),
                rule_type: RouterRuleType::ModelPrefix {
                    prefix: "claude".to_string(),
                },
                enabled: false,
                rewrite: vec![],
                model: Some("first-model".to_string()),
                threshold: None,
            },
            RouterRule {
                id: Some("rule-2".to_string()),
                name: Some("Second rule".to_string()),
                rule_type: RouterRuleType::ModelPrefix {
                    prefix: "claude".to_string(),
                },
                enabled: true,
                rewrite: vec![],
                model: Some("second-model".to_string()),
                threshold: None,
            },
        ];
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules,
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });

        let mut request = make_request_with_model("claude-opus-4");
        let decision = router.route(&mut request).unwrap();
        // First rule disabled → second rule matches
        assert_eq!(decision.model_name, "second-model");
    }

    // ===== P0.2: Token threshold routing =====

    #[test]
    fn test_threshold_blocks_when_token_count_below() {
        // Rule with threshold=100000, but token_count < 100000 → rule skipped, falls through
        let config = create_test_config();
        let rule = RouterRule {
            id: Some("threshold-test".to_string()),
            name: Some("threshold test".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "claude".to_string(),
            },
            enabled: true,
            rewrite: vec![],
            model: Some("big-model".to_string()),
            threshold: Some(100000),
        };
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let mut request = make_request_with_model("claude-opus-4");
        request.token_count = Some(50); // Far below threshold
        let decision = router.route(&mut request).unwrap();
        // Rule matches prefix "claude" but token_count=50 < 100000 → skip → default
        assert_eq!(decision.model_name, "claude-opus-4"); // auto_map disabled ^$, returns original
    }

    #[test]
    fn test_threshold_passes_when_token_count_above() {
        // Rule with threshold=100, token_count=200 >= 100 → triggers
        let config = create_test_config();
        let rule = RouterRule {
            id: Some("threshold-test".to_string()),
            name: Some("threshold test".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "claude".to_string(),
            },
            enabled: true,
            rewrite: vec![],
            model: Some("big-model".to_string()),
            threshold: Some(100),
        };
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let mut request = make_request_with_model("claude-opus-4");
        request.token_count = Some(200); // Above threshold
        let decision = router.route(&mut request).unwrap();
        // Rule matches prefix "claude" and token_count=200 >= 100 → triggers
        assert_eq!(decision.model_name, "big-model");
    }

    #[test]
    fn test_threshold_none_always_triggers() {
        // Rule with threshold=None → always triggers (no token count check)
        let config = create_test_config();
        let rule = RouterRule {
            id: Some("no-threshold".to_string()),
            name: Some("no threshold".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "claude".to_string(),
            },
            enabled: true,
            rewrite: vec![],
            model: Some("any-model".to_string()),
            threshold: None,
        };
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let mut request = make_request_with_model("claude-opus-4");
        // token_count=None (not set) — rule without threshold should still match
        let decision = router.route(&mut request).unwrap();
        assert_eq!(decision.model_name, "any-model");
    }

    #[test]
    fn test_threshold_lazy_computation() {
        // When threshold is set and token_count is None, it should be lazily computed.
        // We can't predict the exact token count, but we verify the field gets populated.
        let config = create_test_config();
        let rule = RouterRule {
            id: Some("lazy".to_string()),
            name: Some("lazy computation".to_string()),
            rule_type: RouterRuleType::ModelPrefix {
                prefix: "claude".to_string(),
            },
            enabled: true,
            rewrite: vec![],
            model: Some("target".to_string()),
            threshold: Some(1), // Very low threshold — will match
        };
        let router = Router::new(AppConfig {
            router: RouterConfig {
                rules: vec![rule],
                auto_map_regex: Some("^$".to_string()),
                ..config.router
            },
            ..config
        });
        let mut request = make_request_with_model("claude-opus-4");
        // token_count starts as None
        assert!(request.token_count.is_none());
        let decision = router.route(&mut request).unwrap();
        // Rule matched, threshold check happened, token_count should be populated
        assert!(request.token_count.is_some());
        assert_eq!(decision.model_name, "target");
    }
}
