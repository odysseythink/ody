# Ody / odyBox / Canvas 长期产品与技术战略

**日期：** 2026-09-06  
**状态：** 战略基线（供后续 Design / Plan 阶段拆解）  
**涉及代码库：**

- Ody：`/Users/ranwei/workspace/rust_work/ody/`
- odyBox：`/Users/ranwei/workspace/go_work/odyBox-base/`
- OpenDesign 参考实现：`/Users/ranwei/Downloads/open-design-v0.20.1/`

---

## 1. 执行摘要

长期战略采用“**一个运行时、两个主要产品界面、一个内嵌视觉工作区**”的边界：

```text
Ody Runtime（统一能力底座）
├── Ody TUI                         面向开发者与终端编程
└── odyBox                          面向桌面 AI 助手与知识工作
    ├── Chat                        对话、写作、问答
    ├── Work                        文件、工具、知识库、长任务
    └── Canvas / Design Workspace   UI、网页与视觉产物的闭环创作
```

核心决策如下：

1. **不创建第三个独立 UI 生成桌面产品。** Canvas 是 odyBox 的工作区，不是新的安装包、品牌、账户体系或独立 Agent 平台。
2. **Ody TUI 和 odyBox 保持不同产品定位。** 二者共享运行时，但不追求界面和交互能力完全相同。
3. **Ody Runtime 成为 Agent 能力的长期单一事实来源。** 会话编排、工具调用、权限、Skills、Plugins、浏览器控制和工作区文件操作逐步由 Ody 承担。
4. **odyBox 保留产品壳职责。** 它负责图形界面、导航、桌面/移动端集成、消费者级设置、产物呈现与视觉交互。
5. **OpenDesign 作为参考实现和可选择移植的能力来源。** 不整体复制其 Web 应用；优先吸收设计知识、预览桥、元素检查、评论、截图、修改协议和质量检查机制。
6. **迁移禁止“大爆炸重写”。** 先建立协议和适配层，逐项切流，达到质量门槛后再删除 odyBox 中的重复能力。

这一路线的战略目的不是让两个客户端“功能完全一致”，而是让同一种 Agent 能力只建设一次，同时让每个客户端服务清晰、不同的人群。

---

## 2. 产品组合与定位

### 2.1 Ody TUI

**核心用户：** 开发者、重度终端用户、需要在真实代码库内执行复杂任务的人。  
**核心任务：** 从代码问题到经过验证的代码变更。  
**产品承诺：** 在终端内安全、透明、可控地完成工程任务。

Ody TUI 的差异化应集中在：

- 代码库理解与跨文件修改；
- 命令执行、沙箱、权限审批；
- Design / Plan / Default 等结构化协作模式；
- 长任务、工具编排、可追踪的执行过程；
- Skills、Plugins、MCP 与浏览器控制；
- 对键盘、终端和脚本工作流的高效率支持。

Ody TUI 不需要承载完整的视觉画布。对于视觉任务，它可以创建、修改和验证项目，并输出可由 odyBox Canvas 打开的 Artifact 或深链。

### 2.2 odyBox

**核心用户：** 不以终端为主要工作环境的知识工作者、创作者和轻技术用户。  
**核心任务：** 从自然语言、文件与资料到可使用的答案和产物。  
**产品承诺：** 在一个桌面工作台内完成对话、资料处理、任务执行和产物创作。

odyBox 不应继续只以“多模型聊天客户端”定义自己。更稳固的长期定位是：

> **个人 AI 工作台：既能聊天，也能对用户的文件和产物采取行动。**

为了避免功能臃肿，能力通过渐进式披露呈现：

- 默认停留在 Chat，不暴露复杂工具；
- 需要文件、代码执行、MCP 或多步任务时进入 Work；
- 产生 UI、网页或视觉设计意图时打开 Canvas；
- Canvas 初期从 Artifact 卡片进入，不急于成为一级导航；
- 只有当真实使用证明其高频且独立时，才提升为一级工作区。

### 2.3 Canvas / Design Workspace

Canvas 的定位是 **odyBox 中针对视觉产物的专业交互面**，而不是另一个通用 AI 助手。

它必须解决完整闭环：

```text
需求 → 设计方向 → 生成 → 实时预览 → 选择/评论 → 修改
     → 截图与状态检查 → 比较版本 → 验收 → 导出/写回项目
```

Canvas 不拥有独立的模型供应商、账户、Skills、权限、会话或文件系统实现；这些全部复用 Ody Runtime 与 odyBox 已有产品能力。

---

## 3. 为什么不做第三个桌面产品

一个独立 UI 产品会与 odyBox 在以下层面重复：

- Electron/Web 桌面壳、更新、签名和多端发布；
- 模型配置、流式对话与消息历史；
- Agent Mode、工具调用、审批和暂停恢复；
- Skills、MCP、文件上传与知识库；
- 项目、Artifact、HTML 预览和导出；
- 用户教育、品牌传播、获客与付费关系。

更严重的是，三个产品会迫使有限团队同时维护三套路线图和用户心智。视觉能力尚未证明拥有独立获客和留存曲线时，拆产品会先产生组织成本，而不是创造市场边界。

未来只有同时满足下列条件，才重新评估 Canvas 独立产品化：

1. 用户画像、购买者和获客渠道明显不同于 odyBox；
2. 大多数核心工作流无需 Chat / Work 也能独立成立；
3. Canvas 具有独立且稳定的留存和付费证据；
4. 专业设计协作需求迫使其信息架构与 odyBox 明显分叉；
5. 独立后获得的增长收益显著高于重复基础设施和品牌成本。

在此之前，Canvas 始终作为 odyBox 的能力面存在。

---

## 4. 当前基础与关键缺口

### 4.1 Ody 已有的底座

Ody 已具备成为统一 Runtime 的主要基础：

- `app-server` 与独立的 protocol / client / transport crates；
- 线程、Turn、事件、审批和配置协议；
- Skills 发现、选择、注入与统一扩展面；
- Plugins / MCP 扩展机制；
- 文件编辑、命令执行、沙箱和权限体系；
- Browser Control 的导航、截图、DOM、日志和 CDP 能力；
- Design / Plan 协作模式及其文档交接机制。

缺口不是“没有 Agent Runtime”，而是 app-server 协议尚未被定义为 odyBox 的正式长期后端，且缺少视觉 Artifact、Preview、Node Selection 和 Revision 等领域协议。

### 4.2 odyBox 已经存在的视觉骨架

odyBox 不是从零开始。当前源码已经包含：

- Project / Artifact Schema，且注释明确标记为 OpenDesign native integration；
- `/design/$projectId` 路由；
- `design_generate_artifact` 模型工具；
- UI / Graphic / Code / Webpage Artifact Renderer；
- HTML 本地 preview server；
- `frontend-design` 内置 Skill；
- Electron、Web、iOS、Android 多端产品壳。

但当前能力仍是 Phase 1 骨架：

- 生成器只使用泛化的 `You are a UI generator` 提示；
- 生成模型调用与完整会话、设计方向和审查流程脱节；
- 仅支持自包含 HTML；
- iframe 预览基本只读；
- Artifact 卡片的“继续迭代”入口仍禁用；
- Design 项目页只是 Artifact 网格，不是创作工作台；
- 没有元素身份、源码映射、评论、可逆修改和视觉验收闭环。

因此，战略上应当扩展现有骨架，而不是另起应用。

### 4.3 当前最大的技术战略风险

odyBox 已拥有独立的模型调用、Agent Mode、工具构建、Skills、MCP、文件系统和审批链路。若 Ody 与 odyBox 继续分别建设这些能力，长期成本会高于 Canvas 本身。

统一 Runtime 必须成为主线目标，但不能直接删除 odyBox 的现有链路。正确路径是：

```text
定义协议 → 接入单一能力 → 双路对比 → 默认切流 → 稳定观察 → 删除旧实现
```

---

## 5. 长期目标架构

### 5.1 责任边界

| 层 | 长期职责 | 不应拥有 |
|---|---|---|
| Ody Runtime | 模型编排、线程/Turn、工具、权限、Skills、Plugins、MCP、Browser、文件与任务执行 | Electron 导航、React 组件、消费者 UI 状态 |
| Ody TUI | 终端交互、代码任务呈现、审批、日志、Artifact 链接 | 独立 Agent 实现、完整视觉画布 |
| odyBox Shell | 桌面/Web/移动 UI、账户与本地产品设置、导航、通知、输入和产物呈现 | 第二套 Agent/工具/权限事实来源 |
| Canvas | Preview、Viewport、元素选择、评论、视觉对比、版本与导出 | 独立登录、模型供应商、Skills、文件权限 |
| OpenDesign Adapter | 复用或翻译 OpenDesign 的设计和预览能力 | 侵入核心会话与权限模型 |

### 5.2 状态所有权

长期必须明确状态权威，防止“双数据库、双真相”：

- **Runtime 权威：** Thread、Turn、工具调用、审批、任务状态、工作区绑定、Artifact 元数据、Revision 关系。
- **Artifact Store 权威：** HTML、图片、bundle、截图等大对象；以稳定 ID 和内容 hash 引用。
- **odyBox 权威：** 窗口、布局、主题、面板开关、设备特有设置和未提交的本地 UI 草稿。
- **Canvas 临时状态：** hover、当前选择、viewport、缩放；需要跨设备或审计的评论与版本必须写回 Runtime。

迁移期间禁止无版本的双写。需要兼容时，应使用带 schemaVersion、idempotency key 和来源标识的同步事件。

### 5.3 Visual Workspace 协议

在 app-server 之上新增独立、可演进的视觉领域协议。第一版至少覆盖：

**核心实体：**

- `VisualProject`
- `Artifact`
- `ArtifactRevision`
- `PreviewSession`
- `Viewport`
- `ElementRef`
- `SourceRef`
- `Snapshot`
- `Comment`
- `PatchSet`
- `ValidationResult`

**核心能力：**

- 创建/打开/关闭视觉项目；
- 生成 Artifact 和增量 Revision；
- 启动隔离预览并报告 ready/error；
- 截图、DOM 摘要、computed style、bounds 与可访问性信息；
- 通过稳定元素 ID 选择元素；
- `ElementRef ↔ SourceRef` 映射；
- 创建评论、将评论交给 Agent、应用 Patch；
- 切换桌面/平板/移动 viewport 和交互状态；
- 对比版本、撤销、重做、接受与导出；
- 视觉 lint、运行错误和验收结果回传。

协议应与 OpenDesign 的具体 React 组件和 daemon API 解耦，使 TUI、odyBox、测试工具和未来 Web 客户端都能消费同一状态。

---

## 6. OpenDesign 吸收策略

### 6.1 应优先吸收

- 设计方向库、模板和 Design System 约束；
- iframe 与宿主之间的消息桥；
- 稳定元素 ID 注入和目标选择；
- inspect、comment、free-pin、CSS override；
- screenshot / snapshot 回传；
- 视觉 lint、响应式检查和设计审查清单；
- 适合当前用户群的预览工作台交互。

### 6.2 不应直接搬运

- 整个 `apps/web` 应用壳；
- 与 OpenDesign daemon 强绑定的 API 客户端；
- OpenDesign 专属的项目、插件和 sidecar 领域模型；
- 与 odyBox 重复的登录、会话、设置、模型和消息系统；
- 仅提供外观但没有闭环价值的组件复制。

### 6.3 复用方式

优先顺序如下：

1. **行为和协议复刻：** 按 Ody Visual Workspace 协议重新实现，边界最干净；
2. **小模块移植：** 对隔离良好的 bridge、lint、模板模块进行移植；
3. **适配器运行：** 短期将 OpenDesign 作为 sidecar 验证用户体验；
4. **整站嵌入：** 仅用于内部原型，不作为长期架构。

OpenDesign 使用 Apache-2.0，但每次移植仍需保留许可证与 NOTICE、标记修改，并单独检查字体、图片、模板和第三方依赖授权。

---

## 7. 分阶段路线图

时间只表示战略顺序，不是未经团队容量估算的交付承诺。

### 阶段 S0：冻结边界与建立基线（0–1 个月）— 已完成

**完成记录：** [S0 完成审计](./2026-09-06-s0-completion-audit.md)。正式决策、权责矩阵、Canvas 基线和 OpenDesign 许可证台账均已落盘；odyBox Canvas 已显示 Preview 状态，并建立固定评测集与汇总命令。

**目标：** 停止新增重复产品和运行时能力。

行动：

- 正式记录“不创建第三个桌面 UI 产品”的产品决策；
- 将 Canvas 标记为 odyBox 实验性工作区；
- 盘点 Ody 与 odyBox 的模型、会话、工具、权限、Skills、MCP、Artifact 重复矩阵；
- 定义统一 Runtime 的 capability parity 表；
- 为现有 `design_generate_artifact` 建立质量和成功率基线；
- 建立 OpenDesign 代码、素材和依赖许可证清单。

**退出条件：** 每项能力都有长期权威层、迁移状态和负责人；不再新增第三套实现。

### 阶段 S1：让 odyBox 现有 Design 骨架形成最小闭环（1–3 个月）— 工程完成

**完成记录：** [S1 完成审计](./2026-09-06-s1-canvas-minimum-loop-completion-audit.md)。生成上下文、`frontend-design` 注入、Artifact Revision、Chat + Preview 工作台、viewport、运行错误、截图、导出和隐私安全指标均已落地；真实用户指标进入观察期。

**目标：** 在不等待 Runtime 全面统一的情况下，验证 Canvas 是否解决真实问题。

行动：

- 让内层生成器获得完整 brief、会话摘要、设计方向和用户约束；
- 将 `frontend-design` 的选择结果真正注入 Artifact 生成链路；
- 启用“继续迭代”，建立 Artifact Revision，而不是每次创建孤立文件；
- 将项目页改为 Chat + Preview 的分栏工作区；
- 增加 desktop/mobile viewport、刷新、运行错误和截图；
- 保留 HTML 自包含限制，先不扩张到完整框架工程；
- 采集失败类型、首个可用结果时间、迭代次数和导出行为。

**退出条件：** 用户可以完成“生成 → 看到 → 提修改 → 得到新版本 → 导出”的闭环；失败可诊断；版本可回退。

**停止条件：** 若真实用户没有稳定完成闭环，暂停重资产 Canvas 开发，先解决任务价值与入口问题。

### 阶段 S2：双向视觉交互（3–6 个月）— 工程完成

**完成记录：** [S2 完成审计](./2026-09-06-s2-bidirectional-visual-interaction-completion-audit.md)。稳定元素身份、选择/评论/free-pin、结构化 Agent 反馈、持久化 PatchSet、三视口检查、截图留档和跨 Revision 像素对比均已落地；真实浏览器与用户验收仍进入观察期。

**目标：** 从“聊天旁边放 iframe”升级为真正的视觉工作台。

行动：

- 引入稳定元素 ID 和 ElementRef；
- 支持点击选择、hover、bounds、computed styles；
- 支持元素评论和 free-pin；
- 将选择和评论结构化地发送给 Agent；
- 建立 CSS override → PatchSet → 源文件修改的通道；
- 加入截图回看、响应式矩阵、基本 a11y 与运行错误检查；
- 对生成前后截图做可重复的视觉评测。

**退出条件：** 用户不必用文字描述“左上角第二个按钮”，并且视觉修改能够可靠映射到持久化版本或源码。

### 阶段 S3：Visual Workspace 协议进入 Ody app-server（4–8 个月，可与 S2 部分并行）— 工程基础完成，灰度观察期

**完成记录：** [S3 Runtime 接入审计](./2026-09-06-s3-visual-workspace-runtime-completion-audit.md)。协议、持久化、事件、生成 TypeScript client 和 odyBox 的 runtime/legacy 切换已落地；默认切换与发行包内 Runtime 尚未开启。

**目标：** 让视觉能力开始使用统一 Runtime，而不是继续加深 odyBox 私有编排。

行动：

- 在 app-server protocol 中定义版本化 Visual Project / Artifact / Revision / Preview 事件；
- 生成或维护类型安全的 TypeScript client；
- 先将截图、Browser、Artifact 生成和 Patch 应用路由到 Ody；
- odyBox 使用 feature flag 在旧链路和 Ody Runtime 间切换；
- 建立同一输入的行为对比和回归数据；
- 明确进程生命周期、崩溃恢复、升级兼容和离线降级。

**退出条件：** 至少一条完整视觉生成链路默认通过 Ody Runtime；协议错误、恢复能力和性能达到现有链路标准。

### 阶段 S4：统一 Agent Runtime（6–12 个月）— Work 默认候选完成，进入退出观察期

**完成记录：** [S4 Agent Runtime 网关审计](./2026-09-06-s4-agent-runtime-gateway-completion-audit.md)。配置发行 Runtime 后，odyBox Work 回合自动通过类型化 Ody thread/turn 执行，支持流、工具卡片、审批、停止与跨启动恢复；普通 Chat 仍使用会话 provider。旧 Work 实现待真实稳定窗口后删除。

**目标：** 消除 odyBox 中最昂贵的重复 Agent 基础设施。

建议迁移顺序：

1. Browser 与视觉验证；
2. 文件系统、命令执行和审批；
3. Skills / Plugins / MCP；
4. Work Mode 的多步工具编排；
5. 会话、流式事件、暂停与恢复；
6. 普通 Chat 模式的模型调用与供应商配置。

每一项都执行：适配 → 双路测试 → 小流量默认 → 全量默认 → 稳定窗口 → 删除旧实现。

**退出条件：** odyBox 不再拥有第二套 Agent loop、工具权限和 Skills 注入事实来源；残留代码仅限 UI adapter 与设备集成。当前不满足该产品退出条件。


 距离“S4 真正完成、ody 与 odyBox 只共享一套 Agent Runtime”，还剩以下工作，按优先级排序：

  1. 完成真实 Work Runtime 验收

  需要实际验证：

  - 连续多轮对话与上下文保持
  - 命令执行允许、拒绝和超时
  - 文件修改允许、拒绝及结果展示
  - MCP 调用与 elicitation
  - Skills 注入
  - Stop/Interrupt
  - odyBox 重启后的 thread 恢复
  - Runtime 崩溃后的恢复
  - 多窗口与并发会话隔离

  目前自动测试通过，但还不能代替真实模型和真实工具回合。

  2. 补齐模型供应商适配

  目前 DeepSeek、OpenAI、Anthropic、Gemini及多数 OpenAI-compatible provider 已有映射，但仍需处理：

  - Azure OpenAI
  - AWS Bedrock
  - Chatbox AI
  - 特殊 OAuth provider
  - 自定义 Header、代理和企业 endpoint
  - 会话切换模型后 Runtime thread 的迁移策略

  不支持的供应商目前会明确报错，不会错误回退到 Chatbox AI。

  3. 统一 Skills、Plugins、MCP 管理界面

  当前 Work 执行时以 Ody 为准，但 odyBox 的部分设置页面还在操作旧系统。需要把：

  - Skills 安装、删除、升级、启停
  - Plugin 安装、卸载、配置
  - MCP 添加、OAuth、启停、状态
  - Skills 目录和额外根目录

  全部改成调用 Ody app-server。否则仍然存在“界面配置了一份，Runtime 使用另一份”的双重事实来源。

  4. 完善 Runtime 原生交互 UI

  当前命令和文件审批已有 odyBox 弹窗，但 requestUserInput 暂时使用系统 window.prompt。还需要：

  - 原生多问题表单
  - 单选、多选、自由输入
  - 密码字段
  - MCP 表单和 URL elicitation
  - 权限范围选择：单次、当前 Turn、当前 Session
  - 审批等待状态及超时提示

  这不影响运行时统一，但影响产品完成度。

  5. 完善状态映射

  需要把更多 Ody 事件映射到 odyBox UI：

  - Plan 和进度状态
  - Token usage
  - Context compaction
  - Guardian 审查过程
  - Browser 操作与截图
  - 后台终端
  - 子 Agent 状态树
  - Hook 执行
  - 更完整的错误信息和重试入口

  6. 消除兼容回退

  当前仍保留：

  - VITE_ODYBOX_AGENT_RUNTIME=legacy
  - Runtime 未配置时的旧 Work Agent 回退
  - odyBox 旧 workflow runner
  - 旧工具构建和 Skills 注入链路
  - 旧命令、文件审批实现

  真实验证稳定后，需要让“Runtime 不可用”变成明确的启动/修复提示，而不是静默运行第二套 Agent。

  7. 删除旧 Agent 基础设施

  最后才能物理删除不再使用的：

  - 旧 Agent loop
  - Work Mode 模型工具循环
  - declarative workflow 执行器
  - 重复的 Skills loader/injector
  - 重复的 MCP tool builder
  - 重复的命令、文件和权限判断

  普通 Chat 的纯模型调用、Canvas UI、Electron 集成可以保留，它们不属于第二套 Agent Runtime。

  8. 完成发行闭环

  虽然你已手动放置 Runtime，还需要正式产品化：

  - Runtime 二进制随 odyBox 打包
  - 不依赖用户设置环境变量
  - 版本兼容检查
  - 校验和与签名
  - macOS/Windows/Linux 路径
  - 自动升级的一致性
  - Runtime 健康检查和诊断页面

  结论：现在最先做的不是继续删代码，而是完成第 1 项真实验收；随后做第 3、4、6、7 项。完成这些后，才能客观地把 S4 标
  为“完成”，届时 odyBox 和 Ody 才真正只有一套 Agent Runtime。

### 阶段 S5：产品化与平台化（12 个月以后）

**目标：** 在闭环和 Runtime 已稳定后扩大产品价值，而不是提前扩表面积。

候选能力：

- 品牌包、设计系统和组织模板；
- 组件库感知与真实代码库 round-trip；
- 多页面流程、状态机和交互录制；
- 可分享预览与评论协作；
- 自动视觉回归和质量评分；
- Canvas 深链，可从 Ody TUI 打开同一 Artifact；
- 面向插件的 Visual Workspace SDK。

这些能力必须由留存、付费或生态证据排序，不作为前置承诺。

---

## 8. 产品体验原则

### 8.1 不让用户理解内部架构

用户不应选择“调用 Ody”还是“调用 OpenDesign”。用户只选择任务：聊天、工作或创作。Runtime、Skill 和工具路由由系统完成。

### 8.2 模式是界面，不是新的能力孤岛

Chat、Work、Canvas 共享同一会话身份、文件、记忆和权限语义。模式只改变工具暴露、界面密度和交互方式。

### 8.3 Artifact 是跨界面的共同语言

一次工作不应只留下聊天文本。代码、HTML、图片、报告和设计都以可版本化 Artifact 存在：

- TUI 可以生成和修改；
- odyBox 可以预览和继续；
- Canvas 可以选择、评论和比较；
- Runtime 可以验证、追踪来源和应用补丁。

### 8.4 视觉任务必须闭环

“模型输出 HTML”不等于完成设计。完成的最低定义是：已渲染、已检查关键 viewport、无阻断错误、用户能指向具体对象继续修改、结果可保存或导出。

---

## 9. 指标体系

### 9.1 产品指标

- **首次价值时间：** 从请求到首个成功渲染且可交互的 Artifact；
- **闭环完成率：** 开始视觉任务后完成生成、至少一次反馈并导出/接受的比例；
- **指向式修改成功率：** 元素选择或评论后，修改正确落到目标的比例；
- **版本接受率：** 用户保留而非立即废弃的 Revision 比例；
- **周复用率：** 创建过 Canvas 项目的用户在后续周期再次使用的比例；
- **跨界面延续率：** 在 TUI 创建、到 odyBox 继续，或反向延续的比例。

### 9.2 质量指标

- Preview 启动成功率和中位耗时；
- 运行错误、空白页和资源加载失败率；
- desktop/mobile 基础检查通过率；
- ElementRef → SourceRef 定位准确率；
- Patch 应用成功率与可回滚率；
- 同一任务无效重试次数；
- 人工视觉评审与自动评分的相关性。

### 9.3 平台收敛指标

- odyBox Agent/Work/Canvas turns 中经 Ody Runtime 执行的比例；
- odyBox 私有 Agent 基础设施的剩余模块数；
- 两端共享协议的兼容性失败率；
- 重复工具和重复权限规则删除量；
- Runtime 崩溃恢复和版本升级成功率。

禁止把“生成 Artifact 数量”作为主要成功指标；大量无人采用的生成只能说明调用量，不能说明产品价值。

---

## 10. 商业与品牌边界

在未验证定价前，不锁定具体收费方案，但应遵守以下原则：

- Ody 的品牌锚点是“开发者 Agent / AI 编程”；
- odyBox 的品牌锚点是“个人 AI 工作台”；
- Canvas 使用 odyBox 子品牌，例如 `Canvas` 或 `Design Workspace`，不建立第三个主品牌；
- 模型额度、云同步和高级工作区能力可以跨客户端共享权益；
- Runtime 和协议的开放范围，与托管、协作、模板市场等商业服务分开决策；
- 不用“支持更多模型”作为长期护城河，护城河应来自任务闭环、用户上下文、Artifact 图谱、验证数据和扩展生态。

---

## 11. 主要风险与应对

| 风险 | 表现 | 应对 |
|---|---|---|
| odyBox 定位失焦 | Chat、Work、Canvas 同时堆在首页 | 渐进式披露；Canvas 从 Artifact 进入；按任务自动建议模式 |
| Runtime 统一变成多年重写 | 新旧两套长期并存 | 以能力为单位切流；每阶段必须删除已替代实现 |
| Ody 协议被某个 UI 绑死 | app-server 字段直接映射 React 状态 | 协议描述领域事件，不描述组件；做 TUI/测试客户端验证 |
| 只移植 OpenDesign 外观 | 页面变漂亮但无法反馈迭代 | 优先实现 ElementRef、Snapshot、Comment、Patch 和 Revision |
| 生成 HTML 安全风险 | iframe 越权、网络泄漏、恶意脚本 | CSP、sandbox、隔离 profile、资源 allowlist、敏感操作审批 |
| 视觉质量不可度量 | 靠主观演示推动开发 | 固定任务集、截图回归、人工盲评、用户接受行为结合 |
| 多端能力拖累桌面主线 | 移动端被迫实现完整 Canvas | 桌面优先；移动端先只读预览与评论，编辑后置 |
| OpenDesign 上游耦合/许可 | 升级困难、素材授权不明 | 适配层隔离；许可证台账；只吸收有明确收益的模块 |
| Artifact 与真实代码脱节 | 设计能看但无法进入产品 | 中期建立 SourceRef 与 PatchSet；明确 prototype 和 code-backed 两种 Artifact |

---

## 12. 明确不做的事情

在 S0–S3 阶段内不做：

- 第三个桌面安装包或独立账户体系；
- 整体复制 OpenDesign `apps/web`；
- 为追求“架构纯净”一次性重写 odyBox；
- 在视觉闭环验证前建设模板商城或大型协作平台；
- 桌面、Web、移动端同时达到功能对等；
- 把 Design Mode 文档规划能力误当作视觉 Canvas；
- 仅靠更长提示词宣称视觉质量问题已经解决；
- 在没有行为数据时承诺 Canvas 独立商业化。

---

## 13. 决策治理

以下变更必须写 ADR 或设计文档：

- Runtime 与 odyBox 之间状态所有权变化；
- app-server 新增或破坏性修改公共协议；
- Artifact/Revision schemaVersion 变化；
- 将某项 odyBox 工具链切换为 Ody 权威；
- 引入 OpenDesign 源码或资源；
- Canvas 从上下文入口升级为一级导航；
- Canvas 独立产品化。

每项迁移必须同时回答：

1. 谁是单一事实来源？
2. 旧数据如何读取和迁移？
3. 进程退出或版本不匹配如何降级？
4. 如何比较新旧链路行为？
5. 在什么条件下删除旧实现？

---

## 14. 接下来 30 天的建议动作

按优先级排列：

1. 建立 Ody × odyBox capability parity 矩阵，标记 `Ody authority / odyBox temporary / UI-only`；
2. 为 Visual Workspace 写第一版领域协议草案，只定义实体、事件和所有权，不立即编码；
3. 给 odyBox 当前设计生成链路建立 10–20 个固定任务与截图基线；
4. 修复内层 Artifact 生成器的上下文断裂，使设计 Skill 和用户完整约束真正生效；
5. 启用 Artifact 的继续迭代和 Revision 数据结构；
6. 将现有 Design 项目页做成最小 Chat + Preview 分栏；
7. 用独立 spike 验证 OpenDesign iframe bridge 能否映射到 odyBox UIRenderer；
8. 确定 Ody app-server TypeScript client 的生成或维护方案；
9. 对生成 HTML 的 CSP、iframe sandbox、文件访问和网络策略做安全审查；
10. 设立月度战略复盘：闭环数据不足时缩小范围，不以新增功能掩盖核心失败。

---

## 15. 战略结论

最合理的长期结构不是三个相互竞争的应用，而是：

> **Ody 是统一的 Agent Runtime 与开发者 TUI；odyBox 是面向大众的图形化 AI 工作台；Canvas 是 odyBox 中由 Ody 驱动的视觉创作工作区；OpenDesign 是被选择性吸收的设计能力来源。**

这套边界同时避免两种错误：

- 不会为了视觉能力再造一个与 odyBox 重叠的桌面产品；
- 也不会把 Canvas 降格为几个提示词和一个只读 iframe。

战略执行的关键不在于尽快补齐所有功能，而在于持续守住三个约束：

1. 每项 Agent 基础能力最终只有一个权威实现；
2. 每个产品界面服务清晰、不同的核心用户任务；
3. 每个视觉任务都能形成可观察、可修改、可验证的闭环。

---

## 16. 源码依据

### Ody

- `Cargo.toml`：app-server、protocol、client、skills、plugins、browser-control 等 workspace 边界；
- `app-server/README.md`：app-server 通信与应用集成接口；
- `app-server-protocol/src/protocol/`：线程、Turn、配置与审批协议；
- `app-server/src/extensions.rs`：统一扩展和 SkillProvider 接入；
- `docs/chs/browser-control.md`：浏览器控制、安全与审批；
- `docs/en/design_mode.md`：Design Mode 的职责和只读边界；
- `core-skills/`、`ext/skills/`、`skills/`：Skills 的加载、扩展和内置安装边界。

### odyBox

- `src/shared/types/design.ts`：Project、Artifact 和生成 schema；
- `src/renderer/routes/design/$projectId.tsx`：当前 Design 项目页；
- `src/renderer/packages/model-calls/toolsets/design.ts`：`design_generate_artifact`；
- `src/renderer/packages/model-calls/design-generator.ts`：当前孤立 HTML 生成提示；
- `src/renderer/components/artifacts/UIRenderer.tsx`：iframe UI 预览；
- `src/renderer/components/artifacts/ArtifactCard.tsx`：Artifact 操作与禁用的继续迭代入口；
- `docs/technical/code-execution.md`：Agent 工具构建、沙箱和 HTML preview server；
- `docs/technical/agent-skills.md`：odyBox 当前独立 Skills 链路；
- `src/main/skills/builtin/frontend-design.ts`：现有前端设计 Skill。

### OpenDesign

- `LICENSE`：Apache-2.0；
- `apps/web/package.json`：Web 应用与内部 workspace 包的依赖；
- `apps/web/next.config.ts`：对 daemon 的 API、Artifact 和 Frame 代理；
- `apps/web/src/runtime/srcdoc.ts`：iframe bridge、元素身份、inspect/comment、CSS override 和 snapshot；
- `apps/web/src/providers/daemon.ts`：Web UI 与 daemon 的耦合边界。
