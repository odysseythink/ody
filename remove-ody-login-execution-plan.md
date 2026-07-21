# 删除 ody-login crate 执行计划（roadmap-architect 标注版）

> 源 roadmap: `.ody-code/roadmaps/2026-07-06-remove-ody-login-migration-plan.md`  
> 标注时间: 2026-07-09  
> 当前 commit: `684180f`

## 当前状态

- **已执行完成**：`ody-login` crate 已删除（git commit `25a996e`），`default_client` 和 `AuthRouteConfig`/`outbound_proxy` 已迁移到 `ody-client`。
- **剩余工作**：残留的用户面字符串、测试文件、`secrets` 命名空间、`otel` telemetry 字段、以及 `config`/`core` 中的 auth 相关字段需要清理。

---

## Execution Rubric

### A. 拆分粒度原则

- 当子任务涉及 >~8 个文件/模块、混合共享基础设施与叶子改动、或把可独立交付的内容打包在一起时，必须拆分。
- 每个子任务要小到能在一次工作会话内完成，保持上下文余量（以 ~256k token 为上限，实际应远低于此）。

### B. 模式判定标准

| Mode | 适用场景 | 原因 |
|---|---|---|
| **normal** | 机械、低风险、答案唯一、改动隔离 | 可直接编辑，无需计划开销 |
| **plan** | 多步骤、有依赖、涉及共享签名/调用扇出、需要先写测试再实现 | 强制先产出依赖图和任务列表 |
| **design** | 架构、数据模型、公共接口或迁移语义存在真实不确定性 | 在用户批准 spec 前禁止实现 |

**Tie-break**：有真实未知时选更谨慎的模式；否则优先更便宜的模式。

---

## Overview

| Phase | 子任务 | Mode | 状态 | 关键文件/说明 |
|---|---|---|---|---|
| 1 | 审计 | normal | **done** | 来源已明确 |
| 2.1 | 迁移 `default_client` 到 `ody-client` | normal | **done** | `ody-client/src/default_client.rs` 已存在 |
| 2.2 | 迁移 `AuthRouteConfig`/`outbound_proxy` | normal | **done** | `ody-client/src/outbound_proxy.rs` 已存在 |
| 2.3 | `ody-client` 自洽验证 | normal | **done** | `cargo check -p ody-client` 通过 |
| 3.1 | 删除 `ody-login` 认证模块 | normal | **done** | `login/` 目录已删除 |
| 3.2 | 清理 config 中 auth 字段 | plan | **pending** | `config/src/types.rs`, `config/src/config_toml.rs`, `core/src/config/mod.rs` |
| 4.1 | HTTP client 引用迁移 | normal | **done** | `use ody_client::...` 已普及 |
| 4.2 | `AuthRouteConfig` 引用迁移 | normal | **done** | 已迁移 |
| 4.3 | Ody auth 逻辑处理 | plan | **pending** | `secrets/src/local.rs`, `cli/src/doctor.rs`, `cli/tests/login.rs`, `app-server/tests/suite/auth.rs` |
| 4.4 | CLI 改造 | normal | **pending** | `cli/src/main.rs`, `cli/src/doctor/output.rs`, `cli/src/plugin_cmd.rs` |
| 4.5 | TUI 改造 | normal | **pending** | `tui/src/app/background_requests.rs`, `tui/src/chatwidget/tests/popups_and_settings.rs` |
| 4.6 | `Cargo.toml` 清理 | normal | **done** | workspace 中已无 `ody-login` 依赖 |
| 4.7 | `BUILD.bazel` 清理 | normal | **done** | 无 `ody_login` 引用 |
| 5 | 遥测处理 | plan | **pending** | `otel/src/events/session_telemetry.rs` 仍含 `ody_api_key_*` 字段 |
| 6 | 验证 | normal | **pending** | `cargo check --workspace`, `cargo test --workspace`, 文档更新 |

---

## Dependencies

```
2.1 → 2.2 → 2.3
2.3 ───────────────→ 3.1 → 3.2
3.2 → 4.3
4.3 → 4.4
4.3 → 4.5
4.4, 4.5 → 6

5 可与其他 phase 并行执行，但逻辑上依赖 3.2（先确定 auth 字段语义）
```

---

## 详细子任务

### Phase 1: 全面审计 **[mode: normal | done]**

来源已明确：
- `ody-login` 的公开 API 已梳理。
- 依赖方已切到 `ody-client`。
- 无需进一步执行。

### Phase 2: 迁移 `default_client` 到 `ody-client` **[mode: normal | done]**

- 2.1 `default_client` 已迁移至 `ody-client/src/default_client.rs`。
- 2.2 `AuthRouteConfig`/`outbound_proxy` 已迁移至 `ody-client/src/outbound_proxy.rs`。
- 2.3 `cargo check -p ody-client` 已通过，无反向依赖 `ody-login`。

### Phase 3: 删除 `ody-login` 的 Ody 认证逻辑

#### 3.1 删除 `login/` 认证模块 **[mode: normal | done]**

- `login/src/auth/*`、`login/src/outbound_proxy.rs`、`login/tests/` 已删除。
- `login/BUILD.bazel` 已不存在。

#### 3.2 清理 config 中 auth 字段 **[mode: plan | pending]**

Depends on: 3.1

- 清理字段：
  - `AuthCredentialsStoreMode` / `cli_auth_credentials_store`
  - `forced_login_method`
  - `auth_keyring_backend_kind`
- 涉及文件：
  - `config/src/types.rs`
  - `config/src/config_toml.rs`
  - `core/src/config/mod.rs`
  - `core/src/config/auth_keyring.rs`
- 验证：`cargo check --workspace`
- **注意**：`OAuthCredentialsStoreMode` / `mcp_oauth_credentials_store` 属于第三方 MCP OAuth，保留，不要误删。

### Phase 4: 更新所有依赖方

#### 4.1 HTTP client 引用迁移 **[mode: normal | done]**

- `use ody_client::...` 已在 `core`, `model-provider`, `ody-api`, `analytics`, `app-server-transport` 等 crate 中普及。

#### 4.2 `AuthRouteConfig` 引用迁移 **[mode: normal | done]**

- 已改为 `ody_client::AuthRouteConfig`。

#### 4.3 Ody auth 逻辑处理 **[mode: plan | pending]**

Depends on: 3.2

- `secrets/src/local.rs`：删除 `LocalSecretsNamespace::OdyAuth` 和 `ODY_AUTH_SECRETS_FILENAME`。
- `cli/src/doctor.rs`：移除 `load_auth_dot_json()` 和 `ody login` 文案。
- `app-server/tests/suite/auth.rs`：删除或改写为使用第三方 API key。
- `app-server/tests/suite/v2/realtime_conversation.rs`：替换 `login_with_api_key` helper。
- `cli/tests/login.rs`：删除或改写为环境变量检测。
- 验证：相关测试可编译并通过。

#### 4.4 CLI 改造 **[mode: normal | pending]**

Depends on: 4.3

- `cli/src/main.rs`：移除 `"remote exec-server registration requires API key authentication; run ody login..."` 文案。
- `cli/src/doctor/output.rs`：移除 7 处 `"Run ody login"` 测试/输出文案。
- `cli/src/plugin_cmd.rs`：清理旧 `ody-login` 注释。
- **注意**：`cli/src/mcp_cmd.rs` 中的 `McpSubcommand::Login` 属于第三方 MCP OAuth，不属于 Ody login，保留。

#### 4.5 TUI 改造 **[mode: normal | pending]**

Depends on: 4.3

- `tui/src/app/background_requests.rs`：替换 `"Sign in with ody login..."` 文案。
- `tui/src/chatwidget/tests/popups_and_settings.rs`：替换测试文案。

#### 4.6 `Cargo.toml` 清理 **[mode: normal | done]**

- workspace 和子 crate 中已无 `ody-login = { ... }` 依赖。

#### 4.7 Bazel 文件清理 **[mode: normal | done]**

- 无 `ody_login` 引用；`login/BUILD.bazel` 已不存在。

### Phase 5: 遥测处理 **[mode: plan | pending]**

Depends on: 3.2

 - 当前 `otel/src/events/session_telemetry.rs` 仍包含 `AuthEnvTelemetryMetadata` 和 `ody_api_key_env_present` 字段；`ody_api_key_env_enabled` 已随 `enable_ody_api_key_env` 死代码清理一并删除。
- 选择：
  - **A（推荐）**：完全删除 `AuthEnvTelemetryMetadata` 中的 Ody auth 字段，同步清理 `otel/tests/suite/otel_export_routing_policy.rs`。
  - **B**：保留精简版，仅记录第三方 key（KIMI/DEEPSEEK/GLM）存在性。
- 验证：`cargo test -p ody-otel`

### Phase 6: 验证 **[mode: normal | pending]**

Depends on: 4.4, 4.5, 5

- `cargo check --workspace`
- `cargo build --workspace`
- `cargo test --workspace`
- 文档清理：检查 `docs/multi_provider.md`、`AGENTS.md` 中是否还有 `ody login` 相关段落（当前 grep 已未发现）。

---

## 关键风险点

1. **第三方 OAuth 不要误删**：`McpServerOAuthConfig`、`OAuthCredentialsStoreMode`、`McpSubcommand::Login` 属于第三方 MCP OAuth，不是 Ody auth。
2. **测试 helper 替换**：`app-server/tests/suite/v2/realtime_conversation.rs` 的 `login_with_api_key` 需要改为第三方模型 key 注入方式。
3. **config schema 兼容性**：删除 `forced_login_method` 等字段可能影响旧配置解析，需确认是否允许破坏性变更。
