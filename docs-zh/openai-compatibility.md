# OpenAI API 兼容性

本文档描述了提供有限 OpenAI API 兼容性的 `/v1/chat/completions` 端点。

> **注意**：此代理的主要用例是通过 `/v1/messages` 让 Claude Code（Anthropic 客户端）连接到各种后端。`/v1/chat/completions` 端点是次要的，功能有限。

## `/v1/chat/completions` 端点

**方法：** POST

**支持的功能：**
- 文本消息补全（仅非流式）
- 系统消息
- 多轮对话
- 图片输入（base64 和 URL）
- 基本参数：temperature、top_p、stop、max_tokens

**不支持的功能：**
- 流式输出（`stream: true` 会返回错误）
- 工具/函数调用
- 使用统计中的缓存 token

## 请求格式

```json
{
  "model": "gpt-4",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "Hello!"}
  ],
  "max_tokens": 1024
}
```

## 响应格式

```json
{
  "id": "chatcmpl-xxx",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "gpt-4",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Hello! How can I help you today?"
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 20,
    "completion_tokens": 10,
    "total_tokens": 30
  }
}
```

## 不支持的参数

| 参数 | 状态 | 说明 |
|-----------|--------|-------|
| `stream` | 不支持 | 返回错误；请改用 `/v1/messages` |
| `tools` | 不支持 | 使用 `/v1/messages` 配合 OpenAI 后端 |
| `tool_choice` | 不支持 | 使用 `/v1/messages` 配合 OpenAI 后端 |
| `response_format` | 不支持 | JSON 模式不可用 |
| `seed` | 不支持 | 不保证可重现性 |
| `logprobs` | 不支持 | - |
| `n` | 不支持 | 仅返回单个补全结果 |
| `presence_penalty` | 不支持 | - |
| `frequency_penalty` | 不支持 | - |

## 结束原因映射

| Anthropic `stop_reason` | OpenAI `finish_reason` |
|-------------------------|------------------------|
| `end_turn` | `stop` |
| `max_tokens` | `length` |
| `stop_sequence` | `stop` |
| `tool_use` | `tool_calls` |

## 错误处理

错误以 OpenAI 兼容格式返回：

```json
{
  "error": {
    "message": "Error description",
    "type": "error_type",
    "code": "error_code"
  }
}
```

## 后端提供商

`/v1/chat/completions` 端点适用于所有配置的后端提供商：

- Anthropic
- OpenAI 兼容提供商（OpenRouter、Groq、DeepInfra 等）
- Gemini
