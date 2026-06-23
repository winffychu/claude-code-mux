# OAuth 测试指南

本指南介绍如何测试 Claude Pro/Max 的 OAuth 认证流程。

## 快速测试（CLI 示例）

### 1. 构建项目

```bash
cargo build --examples
```

### 2. 运行 OAuth 登录示例

```bash
cargo run --example oauth_login
```

此操作将：
1. 生成授权 URL
2. 提示你访问 URL 并授权
3. 要求输入授权码
4. 用授权码换取访问/刷新令牌
5. 将令牌保存到 `~/.claude-code-mux/oauth_tokens.json`

### 示例输出

```
🔐 Claude Max OAuth 认证

此操作将认证你的 Claude Pro/Max 账户，
并将 OAuth 令牌保存以供 claude-code-mux 使用。

步骤 1：在浏览器中访问以下 URL：

  https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e...

步骤 2：授权后，你将收到一个授权码。

在此输入授权码：abc123def456#state789

正在用授权码换取令牌...

✅ 认证成功！

令牌详情：
  提供商 ID：anthropic-max
  过期时间： 2025-11-18T16:30:00+00:00

你的 OAuth 令牌已保存到：
  ~/.claude-code-mux/oauth_tokens.json
```

## 通过 API 端点测试

### 1. 启动服务器

```bash
cargo run -- start
```

### 2. 获取授权 URL

```bash
curl -X POST http://localhost:13456/api/oauth/authorize \
  -H "Content-Type: application/json" \
  -d '{"oauth_type": "max"}'
```

响应：
```json
{
  "url": "https://claude.ai/oauth/authorize?...",
  "verifier": "xxxxxxxxxxx",
  "instructions": "访问上述 URL 进行授权..."
}
```

### 3. 用授权码换取令牌

访问 URL，进行授权，获取授权码。然后：

```bash
curl -X POST http://localhost:13456/api/oauth/exchange \
  -H "Content-Type: application/json" \
  -d '{
    "code": "your-code-here#state",
    "verifier": "verifier-from-step-2",
    "provider_id": "anthropic-max"
  }'
```

响应：
```json
{
  "success": true,
  "message": "OAuth 认证成功！令牌已保存。",
  "provider_id": "anthropic-max",
  "expires_at": "2025-11-18T16:30:00+00:00"
}
```

### 4. 列出令牌

```bash
curl http://localhost:13456/api/oauth/tokens
```

响应：
```json
[
  {
    "provider_id": "anthropic-max",
    "expires_at": "2025-11-18T16:30:00+00:00",
    "is_expired": false,
    "needs_refresh": false
  }
]
```

### 5. 刷新令牌

```bash
curl -X POST http://localhost:13456/api/oauth/tokens/refresh \
  -H "Content-Type: application/json" \
  -d '{"provider_id": "anthropic-max"}'
```

### 6. 删除令牌

```bash
curl -X POST http://localhost:13456/api/oauth/tokens/delete \
  -H "Content-Type: application/json" \
  -d '{"provider_id": "anthropic-max"}'
```

## 在提供商中使用 OAuth

### 1. 配置提供商

编辑 `config/default.toml`：

```toml
[[providers]]
name = "claude-max"
provider_type = "anthropic"
auth_type = "oauth"  # 使用 OAuth 而非 API 密钥
oauth_provider = "anthropic-max"  # 必须与 exchange 返回的 provider_id 一致
enabled = true
models = []

[[models]]
name = "claude-sonnet-4.5"

[[models.mappings]]
actual_model = "claude-sonnet-4-5-20250929"
priority = 1
provider = "claude-max"
```

### 2. 重启服务器

```bash
cargo run -- restart
```

### 3. 使用 Claude Code 测试

提供商将自动使用 TokenStore 中的 OAuth 令牌，并以 Bearer 令牌进行认证！

**✅ 阶段 3 完成**：OAuth 提供商现已自动使用 Bearer 令牌认证。当你通过配置了 OAuth 的提供商向 Claude 发起请求时，系统将：
1. 从 TokenStore 加载令牌
2. 检查是否需要刷新（过期前 5 分钟）
3. 如有需要则自动刷新
4. 在 Authorization 请求头中使用 Bearer 令牌
5. 包含 OAuth beta 请求头以确保完全兼容

## 故障排除

### 找不到令牌

检查令牌是否存在：
```bash
cat ~/.claude-code-mux/oauth_tokens.json
```

应显示：
```json
{
  "anthropic-max": {
    "provider_id": "anthropic-max",
    "access_token": "ey...",
    "refresh_token": "rt_...",
    "expires_at": "2025-11-18T16:30:00+00:00",
    "enterprise_url": null
  }
}
```

### 令牌已过期

令牌会在过期前 5 分钟自动刷新。
如需手动刷新：
```bash
curl -X POST http://localhost:13456/api/oauth/tokens/refresh \
  -H "Content-Type: application/json" \
  -d '{"provider_id": "anthropic-max"}'
```

### 授权失败

常见问题：
1. **客户端 ID 错误**：我们使用 OpenCode 的客户端 ID（`9d1c250a-e61b-44d9-88ed-5944d1962f5e`）
2. **重定向 URI 无效**：必须为 `https://console.anthropic.com/oauth/code/callback`
3. **授权码已被使用**：授权码只能使用一次
4. **PKCE 不匹配**：确保使用与 authorize 步骤中相同的 verifier

## 后续步骤

认证成功后：

1. ✅ 令牌已保存到 `~/.claude-code-mux/oauth_tokens.json`
2. ✅ 使用 `auth_type = "oauth"` 配置提供商
3. ✅ **阶段 3 完成**：Bearer 令牌注入已自动生效！
4. 🚧 **阶段 4**：在管理面板中添加 OAuth 用户界面（进行中）

## 安全注意事项

- 令牌以 `0600` 权限存储（仅所有者可读写）
- 切勿将 `oauth_tokens.json` 提交到版本控制
- 令牌将在过期前自动刷新
- PKCE 确保安全的授权流程

## API 端点汇总

| 端点 | 方法 | 用途 |
|----------|--------|---------|
| `/api/oauth/authorize` | POST | 获取授权 URL |
| `/api/oauth/exchange` | POST | 用授权码换取令牌 |
| `/api/oauth/tokens` | GET | 列出所有令牌 |
| `/api/oauth/tokens/refresh` | POST | 刷新令牌 |
| `/api/oauth/tokens/delete` | POST | 删除令牌 |
