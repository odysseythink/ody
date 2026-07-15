# Windows Terminal 中 footer 行闪烁竖线 — 根因分析与修复方案

日期：2026-07-16
状态：机制链条已闭合（全部环节有源码/实验证据）；修复未实施
现象来源：用户截图 `C:\Users\Administrator\Pictures\000020.png` — footer 行 `【 Design 】` 标签紧后方规律性闪烁的白色竖线（1px 宽）。仅 Windows Terminal 复现；Mac / cmd / PowerShell 正常。伴随现象：spinner（`◦ Working (7s • esc to interrupt)`）在 WT 中明显更快。

---

## TL;DR

那条竖线**不是 TUI 画出来的字符，而是 Windows Terminal 自己的文本光标（caret）**——它本应在底部输入框（composer）里，但在 WT 中被"困"在了 footer 行文本末尾，按系统光标眨眼节奏明暗交替。这是**应用渲染顺序 + ConPTY 转换 bug + WT 对非法序列的处理**三层因素叠加的结果，只有"应用 → ConPTY → WT"这条渲染路径同时具备全部三个必要条件。

---

## 一、竖线是什么（像素证据）

对用户截图做像素级取证（`.ody-code/user_bar_zoom4.png` 为放大裁剪）：

- 1px 宽 × 19px 高（= 行高），颜色 (243,243,243)，位于 x=2510；
- 位置 = footer 行右对齐内容 `【 Design 】` 末格的**下一格**（~第 295 列）——终端文本光标的标准落位；
- 用户 `settings.json` 无 `cursorShape` 配置 → WT 默认 **bar（1px 竖条）**；
- 对照：复现窗口里 composer 的正常光标同为 1px bar（x=26）。**整条 TUI 只有这一个光标**（全仓库仅 `tui/src/app.rs:1316-1319` 一处 `set_cursor_position`），它在 composer 和在 footer 是同一个东西在不同位置。

## 二、完整因果链

### 第 1 层 · 应用层：每帧的文本批必然写到 footer 行末尾

`tui/src/custom_terminal.rs` 的 `diff_buffers`（576-639 行）第一个循环：对 next_buffer 中**每一行**只要存在尾随空白，就**无条件** push 一条 `ClearToEnd`（604-607 行，不与 previous 比较），且全部排在 Put 之前。后果：即使本帧只有 spinner 动了一下，文本批也会从上扫到最底的 footer 行。

帧尾的光标操作在 `try_draw`（393-429 行）：`Some(pos) => set_cursor_style + show_cursor + set_cursor_position`。**字节顺序是先文本批、后光标归位。**

### 第 2 层 · ConPTY 层：帧界的坏 hide + 归位指令的转发间隙

用 ConPTY harness（`utils/pty/examples/capture_tui.rs`）在 300×70 Working 态抓取原始字节流（`.ody-code/work_cap.bin` + `.times`，1456 帧），帧结构铁证如下：

```
...on my current changes\e[K \e[69;2H\e[K\n kimi_ranweiwei/kimi-for-coding\e[K  ← 文本批末 = footer 行最后内容格
\e[?25h \e[25l \e[?2026l      ← SHOW + 坏 HIDE（无 ?）+ ESU ← 帧界！此刻逻辑光标停在 footer 内容末
\e[68;3H \e[?25h \e[25l \e[?2026h \e[0 q   ← CUP 归位 composer
```

- `\e[25l`（无 `?`）是 **ConPTY 自己注入的**——应用和 crossterm fork（`~/.cargo/git/checkouts/crossterm-*/src/cursor.rs`）全仓库只发 `\e[?25l`。**这是 ConPTY 的上游 bug**：hide 时丢了 `?`。
- 时间戳分析：**1455/1456 帧中，ESU→CUP 落在同一读块、间隔 0ms** → 光标瞬间归位 composer，竖线不可见（这正是最初 11 连拍实机复现失败的原因）。
- **但存在 47ms 离群帧（1/1456）** → 间隙客观存在、可被拉开。用户会话 1.52M tokens、300×70 truecolor 大帧、数小时流式输出，ConPTY 写拆分/调度延迟远比空闲复现机频繁。

### 第 3 层 · WT 层：`\e[25l` 被忽略 → 光标停留在"可见"状态

对照实验（`.ody-code/phaseC.png` / `phaseD.png`）：在真实 WT 窗口分别注入——

- `\e[?25l` → **WT 正确隐藏光标**；
- `\e[25l`（无 `?`）→ **WT 忽略，光标保持可见**。

于是帧界呈现时，最后一个**生效**的可见性指令是 `\e[?25h`，光标位置 = footer 行文本末尾。一旦第 2 层的归位 CUP 被延迟哪怕几十毫秒，WT 就把光标呈现在那里，并按系统眨眼节奏闪烁——即用户看到的"规律性消失出现"。

### 决定性复现（机制演示）

`.ody-code/wt_final_demo.sh` 在真实 WT 窗口严格按上述帧尾序列重放（composer 先写、footer 文本**最后写**使光标停在文本末 → `\e[?25h\e[25l` 停住）：

- 连拍 6 张：`ff_1/4/5.png` 拍到 **1px 竖线紧贴文本末**，`ff_2/3/6.png` 为眨眼灭相；
- `.ody-code/ff_1_zoom.png` 与用户的 `user_bar_zoom4.png` 逐像素同构（文本末+1 格、1×19px 竖条）。

**机制链条就此闭合。**

## 三、为什么 cmd / PowerShell / Mac 都正常

conhost（cmd/PowerShell）没有 ConPTY 重渲染层：应用字节直达 conhost，`\e[?25h` / `\e[?25l` 都被正确执行；既没有坏 hide，也没有"帧界光标滞留"这个概念。Mac（iTerm/Terminal.app）同理，无 ConPTY。**只有"应用 → ConPTY → WT"这条路径同时具备三个必要条件。**

## 四、spinner 为什么在 WT 里明显更快

与光标 bug 无关，是另一条独立机制，源码在 `tui/src/motion.rs:62`：

```rust
if supports_color::on_cached(Stream::Stdout).map(|l| l.has_16m).unwrap_or(false) {
    shimmer_spans("•")...        // truecolor → 32ms 一帧的 shimmer 动画
} else {
    let blink_on = (elapsed.as_millis() / 600).is_multiple_of(2);  // 否则 600ms •/◦ 闪烁
}
```

`tui/src/terminal_palette.rs:58`：检测到 `WT_SESSION` 环境变量即**强制提升为 TrueColor**。所以 WT 走 32ms shimmer 分支，cmd/PowerShell 走 600ms 双色闪烁分支——**动画速度差约 18 倍**，纯属预期行为。

## 五、修复方案（未实施，按优先级）

1. **最直接治症**：`tui/src/custom_terminal.rs` 的 `diff_buffers` 把 ClearToEnd 扫描改为与 previous 比较后再发（只在行尾真的变化时发 EL）。多数帧的文本批末将收缩到 composer 行 = 光标归位点，即使 CUP 被 ConPTY 延迟也无视觉跳变。
2. **最稳兜底**：每帧文本批最后，对光标自身所在格做一次无害的 1-cell 重写 → 批末位置 = 归位位置，ConPTY 延迟多久都无所谓，代价仅每帧 1 格写入。
3. 移植上游 PR #11064（scrollback 注入前 Hide + flex FitContent/FillAssignedExtent 一致性）。
4. Working 期间避免 32ms 全帧重绘（与第 1 条同源）。
5. ConPTY 的 `\e[25l`→`\e[?25l` 是 microsoft/terminal 上游 bug，应用侧无法修，只能绕。

## 六、诚实的保留项

机制本身（间隙一旦拉开 → 竖线出现在文本批末）已端到端实证。唯一无法实锤的是**用户具体会话中**是什么把 0ms 间隙持续撕开——窗口已关闭，合理推断是大帧 truecolor 输出下的 ConPTY 写拆分/调度（47ms 离群帧证明间隙客观存在），但这属于环境触发条件，非机制本身。

## 证据文件索引（均在 `.ody-code/`，未跟踪）

| 文件 | 内容 |
|---|---|
| `user_bar_zoom4.png` | 用户截图中竖线的高清裁剪（1×19px、文本末+1 格） |
| `ff_1_zoom.png` / `ff_1..6.png` | 最终演示：真实 WT 中复刻的闪烁竖线（明暗交替） |
| `work_cap.bin` / `work_cap.bin.times` | Working 态 300×70 原始字节流 + 时间戳（1456 帧） |
| `phaseC.png` / `phaseD.png` | WT 对 `\e[?25l`（隐藏）vs `\e[25l`（忽略）的对照实验 |
| `analyze_work.js` | 按 ESU 切帧、测 ESU→CUP 距离/同块/时差的分析器 |

## 实验遗留说明

- `utils/pty/examples/capture_tui.rs` 的改动（prompt 逐字慢打防 paste-burst + 每读块时间戳记录到 `<out>.times`）对后续 TUI 取证有用，建议保留。
- `.ody-code/` 下的实验脚本和截图未跟踪，可随时清理。
- 排除项备查：focus 不影响 `cursor_pos`（`event_stream.rs:258-266` 仅置标志位）；用户截图中轮换占位符表明 `input_enabled=true`、光标目标为 composer；用户截图 composer 行 (41,41,41) 灰底带 = `style.rs:76 user_message_bg`（OSC11 探针成功的会话指纹），与光标 bug 无因果。
