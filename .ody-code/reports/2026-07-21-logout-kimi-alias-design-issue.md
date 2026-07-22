# `/logout` 无法删除 `kimi`：问题分析与设计改进

**日期**: 2026-07-21  
**状态**: 已定位根因，待修复  
**相关模块**: `tui`, `core/src/config`, `model-provider-info`, `ody-config`

---

## 1. 问题现象

在 TUI 中执行 `/logout` → 选择 Kimi → 选择 `kimi` 删除，提示：

```text
Logged out of kimi alias 'kimi'.
```

但下次进入 `/logout` 菜单时，`kimi` 仍然出现在可删除账号列表中，无论删除多少次都无法消失。

菜单显示如下：

```text
Select Kimi account to log out
Choose an account to remove

› 1. kimi_ranweiwei    Remove kimi account 'kimi_ranweiwei'
  2. kimi_xi           Remove kimi account 'kimi_xi'
  3. managed:ody-code  Remove kimi account 'managed:ody-code'
  4. kimi              Remove kimi account 'kimi'
```

---

## 2. 关键证据

### 2.1 用户配置文件中没有 `kimi` 这个 alias

读取 `C:\Users\hkb819\.ody-code\config.toml`，其中包含：

```toml
[providers.kimi_ranweiwei]
type = "kimi"
...

[providers.kimi_xi]
type = "kimi"
...

[providers."managed:ody-code"]
type = "kimi"
...
```

没有 `[providers.kimi]`，也没有 `[model_providers.kimi]`。

### 2.2 登录流程明确禁止把 `kimi` 当作用户 alias

`tui/src/login/validation.rs:22-38`：

```rust
pub(crate) fn validate_custom_alias(alias: &str) -> Result<(), String> {
    ...
    for reserved in [
        LoginProvider::Kimi.id(),      // "kimi"
        LoginProvider::Deepseek.id(),  // "deepseek"
        LoginProvider::Glm.id(),       // "glm"
    ] {
        if trimmed.eq_ignore_ascii_case(reserved) {
            return Err(format!("'{trimmed}' is a reserved provider alias"));
        }
    }
    ...
}
```

测试也确认：

```rust
assert!(validate_custom_alias("kimi").is_err());
assert!(validate_custom_alias("DEEPSEEK").is_err());
assert!(validate_custom_alias("glm").is_err());
```

这说明系统设计层面已经知道 `kimi`/`deepseek`/`glm` 是**保留的 provider type ID**，不是用户可创建的 alias。

### 2.3 `/logout` 菜单读取的是合并后的 provider map

`tui/src/chatwidget/slash_dispatch.rs:1269-1280`：

```rust
fn configured_aliases_for_provider(&self, provider: LoginProvider) -> Vec<String> {
    self.config
        .model_providers  // ← 合并了内置 + 用户配置的 map
        .iter()
        .filter(|(_, p)| match provider {
            LoginProvider::Kimi => p.is_kimi(),
            LoginProvider::Deepseek => p.is_deepseek(),
            LoginProvider::Glm => p.is_glm(),
        })
        .map(|(alias, _)| alias.clone())
        .collect()
}
```

### 2.4 合并后的 map 始终包含内置 `kimi`

`model-provider-info/src/lib.rs:479-493`：

```rust
pub fn built_in_model_providers() -> HashMap<String, ModelProviderInfo> {
    [
        (KIMI_PROVIDER_ID, create_kimi_provider()),
        (DEEPSEEK_PROVIDER_ID, create_deepseek_provider()),
        (GLM_PROVIDER_ID, create_glm_provider()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}
```

`KIMI_PROVIDER_ID` 定义为 `"kimi"`（`model-provider-info/src/lib.rs:40`）。

`core/src/config/mod.rs:3580-3582`：

```rust
let model_providers =
    merge_configured_model_providers(built_in_model_providers(), configured_model_providers)
        .map_err(...)?;
```

### 2.5 删除逻辑只能清除用户配置的 provider

`tui/src/login/config.rs:74-112`：

```rust
pub(crate) fn build_logout_provider_edits(
    aliases_to_remove: &[String],
    configured_models: &HashMap<String, OdyCodeModelConfig>,
    default_model: Option<&str>,
) -> Vec<ConfigEdit> {
    ...
    for alias in aliases_to_remove {
        edits.push(clear_config_value(format!("providers.{alias}")));
        ...
    }
    ...
}
```

它只能生成 `clear_config_value("providers.kimi")` 来删除用户配置，但 `kimi` 是内置的，不在用户配置中，所以删除操作无效。

### 2.6 登出 guard 也基于合并后的 map

`tui/src/app/config_persistence.rs:848-865`：

```rust
let is_matching_alias = self
    .config
    .model_providers
    .get(&alias)
    .map(|p| match provider { ... })
    .unwrap_or(false);
```

因为合并后的 map 包含内置 `kimi`，所以 guard 认为 `kimi` 是合法的、可删除的 alias。

---

## 3. 根因分析

问题的本质不是配置没删干净，而是**概念模型不统一**：

1. **登录流程**知道 `kimi`/`deepseek`/`glm` 是保留的 provider type ID，不能作为用户 alias。
2. **登出流程**却把它们和用户自定义 alias（如 `kimi_ranweiwei`）混在一个 map 里，全部当成可删除账号展示。
3. 删除逻辑只能作用于用户配置，对内置 provider 无效。

因此，用户看到的“可删除 `kimi` 账号”是一个**没有实际意义的操作项**，点击后会执行一次无效的清除，然后刷新时内置 `kimi` 再次出现。

更深一层看，`Config.model_providers` 的 key 同时承载两种语义：

| key 来源 | 语义 | 是否可删除 |
|---|---|---|
| `built_in_model_providers()` | provider type ID（如 `kimi`） | 否 |
| `cfg.normalized_providers()` | 用户自定义 alias（如 `kimi_ranweiwei`） | 是 |

把这两个不同语义的数据结构放进同一个 map，是当前设计问题的根源。

---

## 4. 短期修复（最小改动）

目标：让 `/logout` 只展示用户实际创建的 alias，内置 provider type ID 不再出现在可删除列表中。
目标：让 `/logout` 只展示用户实际创建的 alias，内置 provider type ID 不再出现在可删除列表中。

### 4.1 在 `core` 的 `Config` 中记录用户配置的 alias 集合

在 `core/src/config/mod.rs` 的 `Config` 结构体新增：

```rust
/// Provider aliases explicitly configured by the user, excluding built-in providers.
pub user_configured_provider_aliases: HashSet<String>,
```

在 `load_config_with_layer_stack` 中初始化：

```rust
let configured_model_providers = cfg.normalized_providers();
let user_configured_provider_aliases: HashSet<String> =
    configured_model_providers.keys().cloned().collect();
```

### 4.2 在 TUI 中同步该字段

`tui/src/chatwidget.rs` 的 `sync_provider_config`：

```rust
pub(crate) fn sync_provider_config(&mut self, config: &Config) {
    self.config.model_providers = config.model_providers.clone();
    self.config.configured_models = config.configured_models.clone();
    self.config.model = config.model.clone();
    self.config.user_configured_provider_aliases =
        config.user_configured_provider_aliases.clone();
}
```

### 4.3 修改 `/logout` 菜单数据来源

`tui/src/chatwidget/slash_dispatch.rs` 的 `configured_aliases_for_provider`：

```rust
fn configured_aliases_for_provider(&self, provider: LoginProvider) -> Vec<String> {
    self.config
        .user_configured_provider_aliases
        .iter()
        .filter(|alias| {
            self.config
                .model_providers
                .get(*alias)
                .map(|p| match provider {
                    LoginProvider::Kimi => p.is_kimi(),
                    LoginProvider::Deepseek => p.is_deepseek(),
                    LoginProvider::Glm => p.is_glm(),
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}
```

### 4.4 修改删除 guard

`tui/src/app/config_persistence.rs` 的 `logout_provider_alias`：

```rust
let is_matching_alias = self.config.user_configured_provider_aliases.contains(&alias)
    && self
        .config
        .model_providers
        .get(&alias)
        .map(|p| match provider { ... })
        .unwrap_or(false);
```

如果 `alias` 不在用户配置集合中，直接拒绝并提示“该 alias 不是用户配置，无法删除”。

### 4.5 补充测试

- 测试 `configured_aliases_for_provider` 不返回内置 `kimi`/`deepseek`/`glm`。
- 测试 `logout_provider_alias` 对内置 `kimi` 返回错误或不生成任何编辑。

---

## 5. 长期设计改进

短期修复是在现有架构上打补丁。长期应该把 **provider type catalog** 和 **user provider aliases** 在数据模型上彻底分开。

### 5.1 数据结构拆分

```rust
/// 内置 provider 类型，如 kimi/deepseek/glm。
/// 这些不是用户账号，而是系统支持的 provider 后端。
pub built_in_providers: HashMap<String, ProviderTypeInfo>,

/// 用户创建的 provider alias，如 kimi_ranweiwei。
/// key 是用户自定义名称，value 包含 type 引用、api_key 等。
pub user_configured_aliases: HashMap<String, ProviderAliasConfig>,

/// 运行时合并后的 effective provider map，仅供请求模型时使用。
/// 不应作为“可删除账号”的来源。
pub effective_providers: HashMap<String, ModelProviderInfo>,
```

### 5.2 明确各模块使用哪个来源

| 场景 | 应该使用的来源 | 原因 |
|---|---|---|
| `/login` 选择 provider 类型 | `built_in_providers` | 用户选择要登录哪种 provider |
| 输入自定义 alias | `user_configured_aliases` | 检查是否冲突、是否合法 |
| `/logout` 展示可删除账号 | `user_configured_aliases` | 只能删用户创建的 |
| 发送模型请求 | `effective_providers` | 合并后的最终配置 |
| 模型选择器 | `effective_providers` | 展示所有可用 provider |

### 5.3 保留 ID 的校验统一

目前 `tui/src/login/validation.rs` 单独维护了一份保留 ID 列表。应该把这份规则提升到 `model-provider-info` 或 `ody-config` 中，作为单一事实来源，供登录、登出、配置校验共同使用。例如：

```rust
pub fn is_reserved_provider_alias(alias: &str) -> bool {
    matches!(alias.to_ascii_lowercase().as_str(), "kimi" | "deepseek" | "glm")
}
```

### 5.4 去除 `Config.model_providers` 的语义过载

`Config.model_providers` 当前同时包含内置 type 和用户 alias，建议改名或拆分：

- 方案 A：拆成 `built_in_providers` + `user_configured_aliases` + `effective_providers`。
- 方案 B：保留 `model_providers` 作为运行时合并结果，但不再把它当成“账号列表”的来源；新增 `configured_provider_aliases` 字段专门用于 `/logout` 和账号管理。

方案 A 更彻底，但改动面大；方案 B 是较温和的过渡。

---

## 6. 结论

> “provider 和 provider alias 是两个概念，不应该混在一起。”

这个判断是正确的。当前代码在登录侧已经意识到了这一点（`validate_custom_alias` 禁止保留 ID 作为 alias），但在登出侧和 `Config.model_providers` 的数据模型上把它们混成了一个 map，导致：

- `/logout` 菜单把内置 provider type ID 显示为“可删除账号”；
- 删除内置 `kimi` 没有实际效果；
- 用户反复删除仍会复现。

这不仅是实现 bug，更是**概念模型不一致**带来的设计问题。推荐的修复方向是：

1. 短期：在登出流程中区分“用户配置 alias”和“内置 provider type ID”，只展示和删除前者。
2. 长期：在数据模型上拆分 `provider type catalog` 和 `user provider aliases`，避免同一字段承载两种语义。

---

## 7. 相关代码路径汇总

- `tui/src/login/validation.rs` — 自定义 alias 校验，已区分保留 ID
- `tui/src/chatwidget/slash_dispatch.rs:1269-1280` — `/logout` 菜单生成
- `tui/src/app/config_persistence.rs:842-907` — `/logout` 删除逻辑
- `tui/src/login/config.rs:74-112` — 生成删除配置编辑
- `tui/src/chatwidget.rs:1856-1862` — provider 配置同步到 widget
- `core/src/config/mod.rs:3580-3582` — 合并内置与用户 provider
- `model-provider-info/src/lib.rs:40` — `KIMI_PROVIDER_ID = "kimi"`
- `model-provider-info/src/lib.rs:479-512` — 内置 provider 定义与合并逻辑
