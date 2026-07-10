use serde::{Deserialize, Serialize};
use crate::models::{AnthropicRequest, MessageContent, ContentBlock, SystemPrompt};
use crate::providers::ProviderResponse;

/// OpenAI Chat Completions request format
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub name: Option<String>,
    /// Assistant tool calls (function calls in assistant messages)
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    /// For role="tool": the tool call ID this result corresponds to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// OpenAI tool call (in assistant messages)
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIToolCall {
    pub id: String,
    pub r#type: String,
    pub function: OpenAIToolCallFunction,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// Content can be string or array of content parts
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OpenAIContent {
    String(String),
    Parts(Vec<OpenAIContentPart>),
}

/// Content part (text or image_url)
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

/// Image URL object
#[derive(Debug, Clone, Deserialize)]
pub struct OpenAIImageUrl {
    pub url: String,
}

/// OpenAI Chat Completions response format
#[derive(Debug, Serialize)]
pub struct OpenAIResponse {
    pub id: String,
    #[serde(rename = "object")]
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

#[derive(Debug, Serialize)]
pub struct OpenAIChoice {
    pub index: u32,
    pub message: OpenAIResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponseMessage {
    pub role: String,
    pub content: Option<String>,
    /// Tool calls in the response (when model invokes tools)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Transform OpenAI request to Anthropic format
pub fn transform_openai_to_anthropic(openai_req: OpenAIRequest) -> Result<AnthropicRequest, String> {
    let mut messages = Vec::new();
    let mut system_prompt: Option<SystemPrompt> = None;

    // Process messages
    for msg in openai_req.messages {
        match msg.role.as_str() {
            "system" => {
                // Extract system message
                if let Some(content) = msg.content {
                    let text = match content {
                        OpenAIContent::String(s) => s,
                        OpenAIContent::Parts(parts) => {
                            parts.iter()
                                .filter_map(|p| {
                                    if let OpenAIContentPart::Text { text } = p {
                                        Some(text.clone())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    };
                    system_prompt = Some(SystemPrompt::Text(text));
                }
            }
            "user" | "assistant" => {
                // Convert user/assistant messages
                let content = if let Some(openai_content) = msg.content {
                    match openai_content {
                        OpenAIContent::String(text) => MessageContent::Text(text),
                        OpenAIContent::Parts(parts) => {
                            let blocks: Vec<ContentBlock> = parts.iter()
                                .filter_map(|part| {
                                    match part {
                                        OpenAIContentPart::Text { text } => {
                                            Some(ContentBlock::text(text.clone(), None))
                                        }
                                        OpenAIContentPart::ImageUrl { image_url } => {
                                            // Parse data URL or external URL
                                            if image_url.url.starts_with("data:") {
                                                // data:image/png;base64,iVBORw0KG...
                                                if let Some(comma_idx) = image_url.url.find(',') {
                                                    let header = &image_url.url[..comma_idx];
                                                    let data = &image_url.url[comma_idx + 1..];

                                                    let media_type = if header.contains("image/jpeg") {
                                                        "image/jpeg"
                                                    } else if header.contains("image/png") {
                                                        "image/png"
                                                    } else if header.contains("image/gif") {
                                                        "image/gif"
                                                    } else if header.contains("image/webp") {
                                                        "image/webp"
                                                    } else {
                                                        "image/png" // default
                                                    };

                                                    Some(ContentBlock::image(crate::models::ImageSource {
                                                        r#type: "base64".to_string(),
                                                        media_type: Some(media_type.to_string()),
                                                        data: Some(data.to_string()),
                                                        url: None,
                                                    }))
                                                } else {
                                                    None
                                                }
                                            } else {
                                                // External URL
                                                Some(ContentBlock::image(crate::models::ImageSource {
                                                    r#type: "url".to_string(),
                                                    media_type: None,
                                                    data: None,
                                                    url: Some(image_url.url.clone()),
                                                }))
                                            }
                                        }
                                    }
                                })
                                .collect();

                            if blocks.is_empty() {
                                MessageContent::Text(String::new())
                            } else {
                                MessageContent::Blocks(blocks)
                            }
                        }
                    }
                } else {
                    MessageContent::Text(String::new())
                };

                // For assistant messages with tool_calls, convert to ToolUse content blocks
                let content = if msg.role == "assistant" {
                    if let Some(ref tool_calls) = msg.tool_calls {
                        let mut blocks: Vec<ContentBlock> = Vec::new();
                        if let MessageContent::Text(t) = &content {
                            if !t.is_empty() {
                                blocks.push(ContentBlock::text(t.clone(), None));
                            }
                        }
                        for tc in tool_calls {
                            let input: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                            blocks.push(ContentBlock::Known(
                                crate::models::KnownContentBlock::ToolUse {
                                    id: tc.id.clone(),
                                    name: tc.function.name.clone(),
                                    input,
                                },
                            ));
                        }
                        MessageContent::Blocks(blocks)
                    } else {
                        content
                    }
                } else {
                    content
                };

                messages.push(crate::models::Message {
                    role: msg.role,
                    content,
                });
            }
            "tool" => {
                // OpenAI "tool" role = Anthropic tool_result in a user message
                let tool_use_id = msg.tool_call_id.unwrap_or_default();
                let result_text = match msg.content {
                    Some(OpenAIContent::String(s)) => s,
                    _ => String::new(),
                };
                let content = MessageContent::Blocks(vec![ContentBlock::Known(
                    crate::models::KnownContentBlock::ToolResult {
                        tool_use_id,
                        content: crate::models::ToolResultContent::Text(result_text),
                        is_error: false,
                        cache_control: None,
                    },
                )]);
                messages.push(crate::models::Message {
                    role: "user".to_string(),
                    content,
                });
            }
            _ => {
                tracing::warn!("Skipping unsupported message role: {}", msg.role);
            }
        }
    }

    Ok(AnthropicRequest {
        model: openai_req.model,
        messages,
        max_tokens: openai_req.max_tokens.unwrap_or(4096),
        thinking: None,
        temperature: openai_req.temperature,
        top_p: openai_req.top_p,
        top_k: None,
        stop_sequences: openai_req.stop,
        stream: openai_req.stream,
        metadata: None,
        system: system_prompt,
        tools: openai_req.tools.map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    // OpenAI tool format: {"type": "function", "function": {"name", "description", "parameters"}}
                    if let Some(func) = t.get("function") {
                        let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let description = func
                            .get("description")
                            .and_then(|d| d.as_str())
                            .map(|s| s.to_string());
                        let input_schema = func.get("parameters").cloned();
                        Some(crate::models::Tool {
                            r#type: Some("custom".to_string()),
                            name: Some(name.to_string()),
                            description,
                            input_schema,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }),
        forward_headers: vec![],
        token_count: None,
    })
}

/// Transform Anthropic response to OpenAI format
pub fn transform_anthropic_to_openai(
    anthropic_resp: ProviderResponse,
    model: String,
) -> OpenAIResponse {
    // Extract text content from content blocks
    let content = anthropic_resp.content.iter()
        .filter_map(|block| block.as_text().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join("\n");

    let content = if content.is_empty() {
        None
    } else {
        Some(content)
    };

    // Extract tool_use content blocks → OpenAI tool_calls
    let tool_calls: Vec<OpenAIToolCall> = anthropic_resp
        .content
        .iter()
        .filter_map(|block| match block {
            crate::models::ContentBlock::Known(crate::models::KnownContentBlock::ToolUse {
                id,
                name,
                input,
            }) => Some(OpenAIToolCall {
                id: id.clone(),
                r#type: "function".to_string(),
                function: OpenAIToolCallFunction {
                    name: name.clone(),
                    arguments: serde_json::to_string(input).unwrap_or_default(),
                },
            }),
            _ => None,
        })
        .collect();

    let tool_calls = if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    };

    // Map finish_reason
    let finish_reason = anthropic_resp.stop_reason.as_ref().map(|reason| {
        match reason.as_str() {
            "end_turn" => "stop",
            "max_tokens" => "length",
            "stop_sequence" => "stop",
            "tool_use" => "tool_calls",
            _ => "stop",
        }
        .to_string()
    });

    OpenAIResponse {
        id: anthropic_resp.id,
        object: "chat.completion".to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        model,
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIResponseMessage {
                role: anthropic_resp.role,
                content,
                tool_calls,
            },
            finish_reason,
        }],
        usage: OpenAIUsage {
            prompt_tokens: anthropic_resp.usage.input_tokens,
            completion_tokens: anthropic_resp.usage.output_tokens,
            total_tokens: anthropic_resp.usage.input_tokens + anthropic_resp.usage.output_tokens,
            },
            }
            }

            #[cfg(test)]
            mod tests {
            use super::*;
            use crate::models::{
            ContentBlock, KnownContentBlock, MessageContent, ToolResultContent,
            };
            use crate::providers::{ProviderResponse, Usage};

            fn make_openai_request(model: &str, messages: Vec<OpenAIMessage>) -> OpenAIRequest {
            OpenAIRequest {
            model: model.to_string(),
            messages,
            max_tokens: Some(4096),
            temperature: None,
            top_p: None,
            stop: None,
            stream: None,
            tools: None,
            tool_choice: None,
            }
            }

            #[test]
            fn test_transform_simple_message() {
            let req = make_openai_request("gpt-4", vec![OpenAIMessage {
            role: "user".to_string(),
            content: Some(OpenAIContent::String("Hello".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            }]);
            let result = transform_openai_to_anthropic(req).unwrap();
            assert_eq!(result.model, "gpt-4");
            assert_eq!(result.messages.len(), 1);
            assert_eq!(result.messages[0].role, "user");
            }

            #[test]
            fn test_transform_system_message() {
            let req = make_openai_request("gpt-4", vec![
            OpenAIMessage {
            role: "system".to_string(),
            content: Some(OpenAIContent::String("You are helpful".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            },
            OpenAIMessage {
            role: "user".to_string(),
            content: Some(OpenAIContent::String("Hi".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            },
            ]);
            let result = transform_openai_to_anthropic(req).unwrap();
            assert!(result.system.is_some());
            assert_eq!(result.messages.len(), 1);
            }

            #[test]
            fn test_transform_tool_definition() {
            let tool_json = serde_json::json!({
            "type": "function",
            "function": {
            "name": "get_weather",
            "description": "Get weather for a city",
            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
            }
            });
            let mut req = make_openai_request("gpt-4", vec![OpenAIMessage {
            role: "user".to_string(),
            content: Some(OpenAIContent::String("What's the weather?".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            }]);
            req.tools = Some(vec![tool_json]);
            let result = transform_openai_to_anthropic(req).unwrap();
            assert!(result.tools.is_some());
            let tools = result.tools.unwrap();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name.as_deref(), Some("get_weather"));
            assert_eq!(tools[0].description.as_deref(), Some("Get weather for a city"));
            }

            #[test]
            fn test_transform_assistant_tool_calls() {
            let req = make_openai_request("gpt-4", vec![OpenAIMessage {
            role: "assistant".to_string(),
            content: None,
            name: None,
            tool_calls: Some(vec![OpenAIToolCall {
            id: "call_1".to_string(),
            r#type: "function".to_string(),
            function: OpenAIToolCallFunction {
                name: "get_weather".to_string(),
                arguments: r#"{"city":"Paris"}"#.to_string(),
            },
            }]),
            tool_call_id: None,
            }]);
            let result = transform_openai_to_anthropic(req).unwrap();
            assert_eq!(result.messages[0].role, "assistant");
            match &result.messages[0].content {
            MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            match &blocks[0] {
                ContentBlock::Known(KnownContentBlock::ToolUse { id, name, input }) => {
                    assert_eq!(id, "call_1");
                    assert_eq!(name, "get_weather");
                    assert_eq!(input["city"], "Paris");
                }
                _ => panic!("Expected ToolUse block"),
            }
            }
            _ => panic!("Expected Blocks"),
            }
            }

            #[test]
            fn test_transform_tool_role_to_tool_result() {
            let req = make_openai_request("gpt-4", vec![OpenAIMessage {
            role: "tool".to_string(),
            content: Some(OpenAIContent::String("Sunny, 25C".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            }]);
            let result = transform_openai_to_anthropic(req).unwrap();
            assert_eq!(result.messages[0].role, "user");
            match &result.messages[0].content {
            MessageContent::Blocks(blocks) => match &blocks[0] {
            ContentBlock::Known(KnownContentBlock::ToolResult {
                tool_use_id, content, ..
            }) => {
                assert_eq!(tool_use_id, "call_1");
                match content {
                    ToolResultContent::Text(s) => assert_eq!(s, "Sunny, 25C"),
                    _ => panic!("Expected Text"),
                }
            }
            _ => panic!("Expected ToolResult"),
            },
            _ => panic!("Expected Blocks"),
            }
            }

            #[test]
            fn test_transform_response_tool_use_to_tool_calls() {
            let anthropic_resp = ProviderResponse {
            id: "msg_1".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::Known(KnownContentBlock::ToolUse {
            id: "tool_1".to_string(),
            name: "get_weather".to_string(),
            input: serde_json::json!({"city": "Tokyo"}),
            })],
            model: "claude-sonnet-4".to_string(),
            stop_reason: Some("tool_use".to_string()),
            stop_sequence: None,
            usage: Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            },
            };
            let result = transform_anthropic_to_openai(anthropic_resp, "gpt-4".to_string());
            let choice = &result.choices[0];
            assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
            assert!(choice.message.tool_calls.is_some());
            let tool_calls = choice.message.tool_calls.as_ref().unwrap();
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].id, "tool_1");
            assert_eq!(tool_calls[0].function.name, "get_weather");
            }

            #[test]
            fn test_finish_reason_mapping() {
            let cases = vec![
            ("end_turn", "stop"),
            ("max_tokens", "length"),
            ("stop_sequence", "stop"),
            ("tool_use", "tool_calls"),
            ];
            for (anthropic_reason, expected_openai) in cases {
            let resp = ProviderResponse {
            id: "msg".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::text("ok".to_string(), None)],
            model: "test".to_string(),
            stop_reason: Some(anthropic_reason.to_string()),
            stop_sequence: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
            };
            let result = transform_anthropic_to_openai(resp, "test".to_string());
            assert_eq!(
            result.choices[0].finish_reason.as_deref(),
            Some(expected_openai),
            "Failed for stop_reason={}",
            anthropic_reason
            );
            }
            }
            }
