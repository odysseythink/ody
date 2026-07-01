# 多供应商支持：Kimi / DeepSeek / GLM

除了默认的 Responses API 供应商之外，本 CLI 内置支持三家 OpenAI 兼容的
**Chat Completions** 供应商。它们走 `/chat/completions` 协议（而非 Responses
API），并在内部翻译为统一的事件模型，因此工具调用、流式输出、思考（reasoning）
等能力与默认供应商保持一致。

| 供应商 | provider id | 默认 Base URL | API Key 环境变量 |
|--------|-------------|---------------|------------------|
| Kimi (Moonshot) | `kimi` | `https://api.moonshot.ai/v1` | `KIMI_API_KEY` |
| DeepSeek | `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| GLM (智谱) | `glm` | `https://open.bigmodel.cn/api/paas/v4` | `GLM_API_KEY` |

## 快速开始

1. 设置对应供应商的 API Key 环境变量，例如：

   ```bash
   export KIMI_API_KEY="sk-..."
   ```

2. 在 `~/.ody-code/config.toml` 中选择供应商与模型：

   ```toml
   model_provider = "kimi"
   model = "kimi-k2-0711"
   ```

   或在命令行临时指定（若 CLI 支持对应参数）。

## 内置模型

这些供应商不提供 ody 的 `/models` 目录，CLI 因此内置了一份精简的静态模型列表：

- **Kimi**：`kimi-k2-0711`、`kimi-k2-1024`、`kimi-for-coding`
- **DeepSeek**：`deepseek-chat`、`deepseek-reasoner`（带思考）
- **GLM**：`glm-4.6`、`glm-4.5`、`glm-4.5-air`

也可以在 config 中直接指定任意 `model = "..."`，未列出的 slug 会使用回退元数据。

## 各供应商差异说明

- **思考 / reasoning**：三家均通过响应中的 `reasoning_content` 字段返回思考内容。
  请求侧，Kimi 与 DeepSeek 接受顶层 `reasoning_effort`（`low`/`medium`/`high`）；
  GLM 不发送该字段（思考由模型侧控制）。
- **Kimi 专属**：
  - 工具 JSON Schema 会自动归一化（解引用本地 `$ref`、补全缺失的 `type`），以满足
    Kimi 工具校验器的要求。
  - 以 `$` 前缀命名的工具（如 `$web_search`）会转换为 Kimi 的 `builtin_function`
    线格式。
  - 工具调用 id 会被裁剪到 64 字符以内。
  - 请求附带最小化的 `X-Msh-*` 设备标识头（不含 OAuth 设备登录流程）。
- **GLM**：输出 token 上限使用 `max_tokens` 字段（其余供应商使用
  `max_completion_tokens`）。

## 认证

仅支持通过环境变量提供 API Key（复用内置的 Bearer 认证）。未实现 Kimi 的 OAuth
设备登录流程。

## 自定义 Chat 供应商

若需指向其它 OpenAI 兼容端点（或自建代理），可在 config 中定义一个**自定义**供应商
并将 `wire_api` 设为 `"chat"`：

```toml
[model_providers.my-chat]
name = "My Chat Provider"
base_url = "https://example.com/v1"
env_key = "MY_API_KEY"
wire_api = "chat"
```

> 注意：`kimi`、`deepseek`、`glm` 与其它内置 id 一样是保留 id，不可覆盖；如需自定义请
> 使用不同的 id。

## 暂未支持

- Kimi 视频上传（`files.create` → `ms://<file-id>`）。
- Kimi OAuth 设备登录。
