# Ody CLI

[**Ody CLI Documentation**](https://developers.odysseythink.com/ody/cli)

## 多供应商支持

除默认供应商外，现已内置支持 Kimi、DeepSeek、GLM 三家 OpenAI 兼容（Chat
Completions）供应商。配置与差异说明见 [docs/multi_provider.md](docs/multi_provider.md)。

## 构建

本仓库使用 Cargo 作为构建工具。

### 常用编译命令

```bash
# 编译整个 workspace（debug）
cargo build

# 只编译主 CLI 二进制（debug）
cargo build -p ody-cli

# 发布编译主 CLI 二进制
cargo build --release -p ody-cli
```

### 产物位置

主二进制名为 `ody`，由 `cli/Cargo.toml` 中的 `[[bin]] name = "ody"` 定义：

```bash
# debug 产物
./target/debug/ody

# release 产物
./target/release/ody
```

### 测试

```bash
# 整个 workspace
cargo test

# 单个 crate，例如 ody-core
cargo test -p ody-core
```

### 本地安装

```bash
cargo install --path cli
```

这会编译并把 `ody` 安装到 `~/.cargo/bin/`。

### 发布打包

目前仓库中没有 `cargo dist` 之类的分发配置。最实际的做法是：

```bash
cargo build --release -p ody-cli
```

然后使用产物 `target/release/ody` 进行打包。


## 默认状态栏开启说明
默认状态:statusline 是关的

status_surfaces.rs:177 → enabled = !status_line_items.is_empty(),而 tui.status_line 配置默认为空(core/src/config/mod.rs:763 Option<Vec<String>>)。所以开箱默认不显示 statusline 行,你现在看到的是 footer 右侧那条环境 "% context left"。

怎么开(二选一,都是配置)

- 命令:/statusline(slash_command.rs:109 "configure which items appear in the status line")——交互式勾选。
- 配置文件 ~/.ody/config.toml:
[tui]
status_line = ["model", "context-remaining", "used-tokens"]

context 相关的可选 item(strum 配置名,已核实):

┌──────────────────────────────────┬─────────────────┐
│              配置名              │      显示       │
├──────────────────────────────────┼─────────────────┤
│ context-remaining                │ Context X% left │
├──────────────────────────────────┼─────────────────┤
│ context-used(旧名 context-usage) │ Context X% used │
├──────────────────────────────────┼─────────────────┤
│ context-window-size              │ N window        │
├──────────────────────────────────┼─────────────────┤
│ used-tokens                      │ N used          │
└──────────────────────────────────┴─────────────────┘

组合 ["model", "context-remaining", "used-tokens"] 就能得到接近甚至超过 ody-code context: X% (used/max) 的效果。

关键:开启后是"替换单行",不是"加一行"

这是渲染逻辑 chat_composer.rs:4208-4265 的实际行为——status_line_active 为真时:

- 左侧 = 你配置的 statusline items(model / branch / Context X% left / N used …);
- 右侧 = collaboration mode 指示器;
- 默认的环境 right_footer_line_with_context()(那条 "% context left")不再渲染——因为它只在 !status_line_active 分支里跑。

默认(statusline 关):
  ⇧Tab to cycle  …轮播快捷键提示…                    75% context left
开启后(statusline 开):
  model · Context 75% left · 12.3k used            [Plan]

所以不会重复、不会多占一行。代价是:那行原本轮播的快捷键提示会被 statusline 内容取代,mode 指示挪到右侧。这就是主要 tradeoff——用"常驻的自选状态项"换掉"轮播的操作提示"。

同一个 caveat 依然在

context-remaining / context-used / context-window-size 都依赖 model_context_window 已知(chatwidget.rs:1143 context_remaining_percent)。kimi/deepseek/glm 若 provider 配置没带 max_context_tokens,这几个 item 会返回 None → 直接不显示(不报错,就是空白)。这跟上一轮说的第 2 点缺口是同一个根因——所以如果你开了 statusline 却发现 context% 不出来,先去补这三家的上下文窗口配置。

---
一句话:开启只需 /statusline 或配 tui.status_line,零代码;它替换而非新增 footer 行,所以不会跟现有 "% context left" 重复;但要真显示出百分比,前提是对应 provider 配了上下文窗口大小。

要我帮你确认 kimi/deepseek/glm 三家现在的 provider 配置到底有没有带 max_context_tokens 吗?这是决定"开了能不能看到 %"的实际前提。