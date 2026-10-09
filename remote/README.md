# ody-remote — 手机远程查看 ody 实例（只读切片）

架构：`手机浏览器(PWA) ⇄ gateway(VPS) ⇄ agent(每台机器) ⇄ ody app-server(本机 ws)`。

- **gateway**（VPS 上）：Caddy 终结 TLS → `ody-remote-gateway`（默认 `127.0.0.1:8080`）。单用户登录（argon2 + session cookie + 登录限流），agent 用预共享 Bearer token 接入，浏览器侧只放行只读 JSON-RPC 白名单（`thread/list`、`thread/read`、`thread/resume`、`thread/unsubscribe`）。
- **agent**（每台 ody 机器上）：反连 gateway（一条 wss 多路复用本机全部实例），把本机 app-server 的通知事件流转发上行，把白名单 RPC 转发下行。断线自动重连。
- **PWA**：gateway 内嵌静态页（`static/`），手机浏览器打开、添加到主屏幕即可。

## gateway 配置（VPS）

```toml
# gateway.toml
username = "you"
password_hash = "$argon2id$..."   # 由 `ody-remote-gateway hash-password <pw>` 生成
agent_token = "<至少16字符的随机串>"
# session_ttl_hours = 168         # 可选，默认 7 天
```

```bash
# VPS（无需 agent 重依赖）：
cargo build --release -p ody-remote --no-default-features --bin ody-remote-gateway
ody-remote-gateway serve --config gateway.toml          # 听 127.0.0.1:8080
ody-remote-gateway serve --config gateway.toml --listen 0.0.0.0:8080  # 裸跑（不推荐）

# Caddy 终结 TLS：
# your.domain.example {
#     reverse_proxy 127.0.0.1:8080
# }
```

## agent 配置（每台 ody 机器）

```toml
# agent.toml
gateway_url = "wss://your.domain.example/ws/agent"
token = "<与 gateway 的 agent_token 相同>"
machine = "mac-a"
# reconnect_delay_ms = 5000       # 可选

[[instances]]
name = "inst-1"
url = "ws://127.0.0.1:8781"       # ody app-server --listen ws://127.0.0.1:8781
# token = "<capability-token>"    # 该实例配了 --ws-auth 时必填

[[instances]]
name = "inst-2"
url = "ws://127.0.0.1:8782"
```

```bash
cargo build --release -p ody-remote --bin ody-remote-agent   # 本机（默认 feature 含 agent）
ody-remote-agent --config agent.toml
```

## 边界与已知限制

- 只读切片：浏览器端除白名单 4 个方法外一律拒绝；审批/输入控制不在本切片。
- guardian 审批等 server→client 请求会作为事件透传展示，但手机上无法应答（线程会停在等待审批）。
- 会话在内存中：gateway 重启后需重新登录。
- 登录限流按用户名不按 IP（5 次失败锁定）。

## 测试

```bash
cargo nextest run -p ody-remote
cargo check -p ody-remote --no-default-features   # VPS gateway 构建路径
```
