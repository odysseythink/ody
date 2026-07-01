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
