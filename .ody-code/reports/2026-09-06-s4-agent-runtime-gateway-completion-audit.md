# S4：统一 Agent Runtime 网关完成审计

日期：2026-09-06  
范围：`ody` app-server（既有协议）与 `odyBox-base`（S4 Work Runtime 默认候选）

## 结论

S4 已从只读 Gateway 推进到 **Work Runtime 默认候选**：只要桌面端处于 Work Mode 且配置了 `ODYBOX_ODY_RUNTIME_COMMAND`，新回合就自动进入 Ody thread/turn，而不再运行 odyBox 的模型工具循环。Runtime 负责模型编排、Skills 注入、MCP、命令、文件修改、权限与子 Agent；odyBox 负责消息、执行卡片、停止按钮和用户交互。

工程迁移已具备产品验证条件，但 S4 的“删除旧实现”退出门槛仍处于稳定观察期。旧 Agent 代码暂时保留为无 Runtime 发行包的兼容回退；在真实 Work 回合、审批、崩溃恢复和升级窗口通过前删除它，会违反本战略规定的“默认切流 → 稳定窗口 → 删除旧实现”顺序。

## 已交付

| 边界 | 实现 | 验证点 |
| --- | --- | --- |
| 子进程与初始化 | `src/main/ody-runtime-visual.ts` 从 Visual 专用进程提升为共享 Runtime Gateway；保持 stdio、超时、退出清理和下次请求恢复 | 仍由 Electron main 独占进程生命周期；在发行包携带 Ody 前，必须显式设置 `ODYBOX_ODY_RUNTIME_COMMAND`，不回退调用 PATH 上不可信的同名 `ody` |
| 权限边界 | `src/main/ody-runtime-policy.ts` 为 renderer RPC 与 Runtime server request 分别设置正向白名单 | 只开放受控 thread/turn 生命周期；renderer 不能借网关直接执行任意命令或写文件 |
| Catalog 协议 | `OdyRuntimeCatalogClient` 直接使用 Ody 生成的 `ClientRequest` / `v2` 类型 | Skills、Hooks、Plugins、MCP 状态请求和递增 JSON-RPC id 受测试覆盖 |
| 事件 | Runtime 的无 id server notification 通过 preload 转给 renderer；adapter 只订阅 catalog 失效事件 | Visual 事件不会被 Catalog listener 混入 |
| 兼容性 | `VITE_ODYBOX_AGENT_CATALOG_RUNTIME=runtime` 是独立 opt-in；缺省为 `legacy` | 不会因 S3 Visual 灰度开关误启用 Agent Catalog |
| 双向请求 | main/preload/renderer 支持 JSON-RPC server request 与 response；请求绑定 thread 所属窗口，5 分钟超时 | 命令、文件、权限、MCP elicitation、用户输入可交互；未知请求 fail closed；`currentTime/read` 由主进程处理 |
| Work 会话 | `work-generation.ts` 创建/恢复持久 Ody thread，启动/停止 turn，保存 `odyRuntimeThreadId` | 应用重启后可续接；首次迁移会将已有对话作为首回合上下文；失效 thread 自动新建 |
| 模型配置 | 每个 Runtime thread 显式携带 odyBox 会话当前的 provider、model、endpoint 和凭据覆盖 | 不依赖 Ody 的全局默认模型；切换 DeepSeek 或其它兼容 provider 后，新 thread 使用该会话选择；Chatbox AI/Azure/Bedrock 不做不安全的错误映射 |
| 流与工具 | 按 thread/turn 路由 delta、reasoning、error、turn completion 与 item lifecycle | 命令、文件、MCP、动态工具和子 Agent 映射到现有消息工具卡片 |
| Skills 注入 | Work 回合通过 Ody `skills/list` 解析用户已启用的 Skill，并以协议 `UserInput::Skill` 传入 | SKILL.md 加载与注入发生在 Ody，不由 odyBox Work 工具重新解析 |
| 单一编排 | Work Mode 禁止启动 odyBox declarative workflow runner | 一个用户回合不会同时运行 Ody 与 odyBox 两套 Agent loop |

## 迁移判断

没有开放任意 JSON-RPC 隧道。Work 只能调用白名单中的生命周期与只读 catalog 方法；命令和文件写入必须由 Ody turn 内部发起并经过 Ody 权限系统。多个 renderer adapter 的局部 request id 会在 main 进程重映射，避免 Canvas、Catalog 与 Agent 并发请求相撞。

普通 Chat 继续使用 odyBox 会话中选择的 provider/model。这是刻意保留的产品边界：S4 先消除昂贵的 Agent 基础设施，不改变纯聊天的多供应商体验。Work 也读取同一个会话选择，但将其转换为 thread-scoped Runtime provider 配置，由 Ody 发起实际模型调用。Canvas 的确定性 Artifact 生成继续读取会话默认模型；只有其工具执行、视觉协议与工作区状态进入 Runtime。

## 已运行验证

在 `odyBox-base`：

```text
pnpm exec vitest run src/main/ody-runtime-policy.test.ts \
  src/renderer/services/ody-runtime/visual-workspace-client.test.ts \
  src/renderer/services/ody-runtime/catalog-client.test.ts \
  src/renderer/packages/model-calls/toolsets/design.test.ts
# Runtime/Canvas/Settings targeted suite: 6 files, 46 tests passed
# Generation/locking/queue regression: 5 files, 90 tests passed

pnpm exec biome check <S4 changed source subset>
# passed

pnpm exec tsc --noEmit --pretty false
# passed

pnpm run build
# main + preload + renderer production bundles passed

# 使用实际 /Users/ranwei/workspace/rust_work/ody_runtime/ody：
# initialize -> model/list: 14 models
# thread/start(model=deepseek-v4-flash, modelProvider=odybox_session,
#              thread-scoped provider config): returned the requested model/provider
```

S3 的协议生成和 app-server Visual 持久化测试仍是该网关的前置已验证基础；本次没有修改 Ody 的 Rust protocol 或处理器。

## S4 退出观察项

1. 用真实模型验证 Work 的连续多回合、命令/文件拒绝与允许、MCP elicitation、停止和应用重启续接；
2. 将当前 `window.prompt` 用户问题适配器替换为 odyBox 原生多问题表单（不影响 Runtime 权威边界，但影响产品体验）；
3. Runtime 随发行包交付后，移除 `VITE_ODYBOX_AGENT_RUNTIME=legacy` 逃生开关以及旧 Work Agent、workflow、命令权限和 Skill 注入代码；
4. Skills / Plugins / MCP 设置页的安装与配置仍需改为 Ody 管理 API；当前 Work 执行以 Ody 为准，但管理界面仍含 legacy 管理能力；
5. 普通 Chat provider/model 按产品边界保留在 odyBox，不作为删除旧 Agent loop 的阻塞项。
