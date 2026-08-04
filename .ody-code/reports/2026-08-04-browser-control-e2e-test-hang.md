# Browser Control 端到端测试卡住问题报告

**日期：** 2026-08-04  
**报告人：** 安全审计后续排查  
**相关组件：** `ody-browser-control` / `chromiumoxide` / Chrome DevTools Protocol  
**状态：** 已定位根因，依赖外部条件修复；代码已回滚至干净状态

---

## 1. 问题现象

用户按 roadmap 完成 Task 6.1 后，尝试执行完整端到端测试，执行：

```bash
cargo test -p ody-browser-control --test process_lifecycle -- --ignored
```

输出如下并持续卡住：

```text
running 2 tests
test multiple_pages_are_independent has been running for over 60 seconds
test thread_state_navigates_and_reuses_default_page has been running for over 60 seconds
```

Chrome 在 headless 模式下未显示窗口，且测试未进入“新建页面”阶段。

---

## 2. 排查过程与关键证据

### 2.1 测试分层结果

| 测试/命令 | 结果 | 说明 |
|---|---|---|
| `discover_chrome_finds_an_executable` | ✅ 通过 | 系统能找到 Chrome 可执行文件 |
| `launch_creates_local_session_with_temp_profile` | ✅ 通过 | `BrowserSession::launch` 可启动进程、创建临时 profile、关闭后清理 |
| `browser.version()` 等浏览器级命令 | ✅ 正常 | CDP 连接本身可用 |
| `Target.createTarget` / `Target.getTargets` | ✅ 正常 | 浏览器级 target 命令可用 |
| `Browser::new_page("about:blank")` | ❌ 无限卡住 | `chromiumoxide` 内部等待 target 初始化事件，永不返回 |
| 绕过 `new_page`：手动 `CreateTarget` + 轮询 `Browser::pages()` | ⚠️ 能拿到 `Page` 句柄 | 但拿到的页面无法执行后续命令 |
| `Page.navigate` / `Page.goto` / 原始 `Page.navigate` CDP 命令 | ❌ 30s 超时 | 所有页面级命令均超时 |
| `Page.url()` | ✅ 返回 | 但仅读取 `chromiumoxide` 内部状态，不发送页面级 CDP 命令 |

### 2.2 已排除的因素

- **headless 标志**：尝试 `headless=old`、`headless=chrome`、`--disable-gpu` 组合均无效。
- **目标 URL**：尝试 `about:blank`、`chrome://new-tab-page/`、`data:text/html,...`、`https://example.com` 均无效。
- **测试服务器**：使用真实公网 URL（`https://example.com`）复现同样的 `navigate` 超时，排除本地 mock server 问题。
- **运行线程模型**：单线程、多线程、`current_thread` 阻塞运行时均复现相同行为。
- **Chrome 进程未启动**：`Browser::launch` 能成功返回并响应 `Browser.getVersion`，证明进程和 WebSocket 连接已建立。

### 2.3 环境信息

- `chromiumoxide` 版本：`0.9.1`（workspace `Cargo.toml` 锁定）
- Chrome 版本：`150.0.7871.184`（2026 年中版本）
- 依赖 mirror：`tuna` registry 仅包含 `0.8.0 / 0.9.0 / 0.9.1`，无 `0.10+`
- 操作系统：Windows（`E:\ody-rs` 工作区）

### 2.4 根因定位

`chromiumoxide` 0.9.1 的 `Browser::new_page()` 内部依赖一个状态机：

1. 发送 `Target.attachToTarget` 请求
2. 等待 `Target.attachedToTarget` 事件设置 `session_id`
3. 依次初始化 `FrameManager` / `NetworkManager` / `PageManager` / `EmulationManager`
4. 等待主 frame 加载完成，再通过 `initiator` 通道返回 `Page`

在当前 Chrome 150 上，第 3–4 步卡住，`initiator` 通道永不返回。绕过 `new_page` 后拿到的 `Page` 虽然 `session_id` 已设置，但 target 初始化未完成，导致**页面级 CDP 命令无响应**。

这不是 `ody-browser-control` 自身代码的 bug，而是 **chromiumoxide 0.9.1 与 Chrome 150 的页面 session/初始化协议不兼容**。

---

## 3. 已尝试的修复

### 3.1 不成功的方向

- 手动 `CreateTarget` + 轮询 `pages()` 拿到 `Page`：可解除 `new_page` 无限卡，但后续 `navigate`/`evaluate` 仍超时，因此是半成品 workaround。
- 调用 `Target.activateTarget` 后再使用页面：仍无法让页面级命令恢复响应。
- 尝试升级 `chromiumoxide` 到 `0.10.0`：当前 `tuna` mirror 索引无此版本，无法解析。

### 3.2 代码回滚

所有临时诊断文件（`tests/diagnostic_*.rs`）和未完成的 `new_page` workaround 已删除/回滚。当前 `ody-browser-control` 代码回到 Task 6.1 完成时的干净状态。

---

## 4. 影响范围

- **本地模式（Local）**：`new_page`、`navigate`、`evaluate`、`click`、`type`、`screenshot` 等依赖页面级命令的操作在 Chrome 150 下均不可用。
- **外部模式（External）**：尚未实测，但同一 `chromiumoxide` 版本处理页面 session 的逻辑一致，大概率同样受影响。
- **CLI 端到端**：`ody-cli` 调用 `browser__navigate` 等工具时，会落到同样的 `new_page` / `navigate` 路径，预期也会卡住或超时。
- **安全审计结论不受影响**：Task 6.1 的审计结论、文档更新和测试断言均已完成并通过。

---

## 5. 建议措施

### 5.1 短期（当前环境可执行）

1. 使用旧版 Chrome/Chromium 进行端到端验证，例如 Chrome 114–120 的便携版：

   ```toml
   [services.browser]
   chrome_executable = "C:/path/to/chromium-portable/chrome.exe"
   ```

2. 继续把被 `#[ignore]` 的测试作为手动验证入口：

   ```bash
   cargo test -p ody-browser-control --test process_lifecycle -- --ignored
   ```

### 5.2 中期（需要仓库/基础设施配合）

1. 在 roadmap 中新增一个独立任务：**升级 `chromiumoxide` 到 0.10+ 并解除真实页面测试的 `#[ignore]`**。
2. 解决依赖源限制：当前 `tuna` mirror 没有新版 `chromiumoxide`，需要切换到 crates.io 或维护一个本地 mirror 快照。
3. 升级后补充一条覆盖 `navigate` + `screenshot` + `evaluate` 的端到端集成测试，替换现有被 ignore 的测试或降低其不稳定性。

### 5.3 文档记录

建议在 `docs/browser-control.md` 的“测试”或“残余风险”章节补充：

> 当前锁定的 `chromiumoxide 0.9.1` 在 Chrome 150 上无法完成页面初始化，导致依赖真实 Chrome 的 `new_page` / `navigate` 测试被 `#[ignore]`。端到端验证需使用旧版 Chrome 或等待依赖升级。

---

## 6. 附件：复现命令

```bash
# 1. 确认基础测试通过
cargo test -p ody-browser-control --tests

# 2. 复现卡住的端到端测试
cargo test -p ody-browser-control --test process_lifecycle -- --ignored

# 3. 查看 Chrome 版本（需要在运行 Chrome 的机器上执行）
google-chrome --version  # 或 chrome.exe --version
```

---

## 7. 结论

端到端测试无法在当前环境（`chromiumoxide 0.9.1 + Chrome 150`）下跑通，根因是依赖版本与 Chrome 协议不兼容。本次 Task 6.1 的代码与文档改动本身已正确完成；完整的真实浏览器端到端验证需要外部条件（Chrome 降级或 `chromiumoxide` 升级）才能继续推进。
