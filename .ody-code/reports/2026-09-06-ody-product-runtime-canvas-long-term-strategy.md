# Ody / odyBox / Canvas 长期产品与技术战略

**首次制定：** 2026-09-06

**最近整理：** 2026-09-13

**状态：** 战略基线；S0–S4 已关闭，Engineering Workspace 为下一条产品主线

**涉及代码库：**

- Ody：`/Users/ranwei/workspace/rust_work/ody/`
- odyBox：`/Users/ranwei/workspace/go_work/odyBox-base/`
- OpenDesign 参考实现：`/Users/ranwei/Downloads/open-design-v0.20.1/`

---

## 1. 执行摘要

### 1.1 最终产品定义

> **Ody 是统一的 Agent、工程执行与多服务联动 Runtime；odyBox 是同时承载 Chat、Work 和 Canvas 的图形化 AI 工作台。Canvas 既能创建独立视觉 Artifact，也能作为真实前端或全栈 Workspace 的可视化操作面。**

产品不以“生成一个漂亮页面”为终点。对于工程任务，完成标准是：变更进入用户的真实工程，能够运行，经过构建、测试和浏览器验证，并形成可审核的 diff。

```text
Ody Runtime                         单一 Agent 与工程能力底座
├── Ody TUI                         面向开发者的终端编程界面
└── odyBox                          面向桌面用户的图形化 AI 工作台
    ├── Chat                        对话、写作、问答
    ├── Work                        文件、工具与多步工程任务
    ├── Canvas                      视觉创建、选择、评论和比较
    └── Services                    前端、后端与依赖服务状态
```

Canvas 不成为第三个安装包、品牌、账户体系或 Agent 平台。Chat、Work、Canvas 和 Services 是同一项目的不同操作面，不是四套能力孤岛。

### 1.2 当前状态

| 成熟度轴 | 当前状态 | 已解决 | 尚未解决 |
|---|---|---|---|
| Agent Runtime 统一 | S4 已关闭 | Work 的 thread/turn、工具、审批、Skills、MCP、恢复与发行 Runtime | 不再是当前主线 |
| Canvas Artifact | S1–S3 工程闭环已建立 | 自包含 HTML 的生成、迭代、元素交互、版本、快照与交付 | 真实用户价值仍需持续观察 |
| Engineering Workspace | Runtime 主线已完成（2026-09-14 关闭），产品化未开始 | E0–E4 已交付：真实目录项目模型与只读发现（E0）、SourceRef/ChangeSet/diff 源码变更（E1）、真实框架服务编排（E2）、前后端联动与诊断（E3）、watcher/冲突/审计/并发锁/崩溃恢复（E4） | odyBox 消费端 UI（项目入口、diff 确认、Services 面板）、§10.2 指标采集、真实用户观察 |

“S4 已完成”和“仍不能完整重构已有工程”并不矛盾。S4 解决的是统一 Runtime；已有工程属于新的源码工作区产品面，不能再作为 S3/S4 的尾项描述。

### 1.3 核心决策

1. 不创建第三个独立 UI 生成产品；Canvas 始终是 odyBox 的工作区。
2. Ody Runtime 是 Agent、工具、权限、Skills、Plugins、MCP、浏览器和工程执行的长期单一事实来源。
3. odyBox 负责产品壳、项目入口、可视化交互、状态呈现和用户审批，不再建设第二套 Agent/文件/命令运行时。
4. 同时保留 Artifact Project 和 Workspace Project；前者服务快速原型，后者服务真实源码。
5. 已有项目绑定真实目录，不复制为 Canvas 私有快照，不以“导出副本”冒充工程写回。
6. 前后端联动由 Ody Runtime 承担；odyBox 通过 Services、Canvas 和 diff UI 呈现过程与结果。
7. OpenDesign 是参考实现和选择性能力来源，不整体复制其 Web 应用或 daemon 领域模型。
8. 后续演进使用版本化窄协议和逐项切流，禁止大爆炸重写。

---

## 2. 产品组合与用户入口

### 2.1 Ody TUI

**核心用户：** 开发者、终端用户和需要在真实代码库内完成复杂任务的人。

**产品承诺：** 在终端内安全、透明、可控地完成经过验证的工程变更。

重点能力是代码库理解、跨文件修改、命令执行、沙箱、审批、协作模式、长任务、Skills、Plugins、MCP 和浏览器控制。Ody TUI 可以处理视觉任务，但不复制 odyBox 的完整图形画布。

### 2.2 odyBox

**核心用户：** 不以终端为主要环境的知识工作者、创作者和轻技术用户。

**产品承诺：** 在一个桌面工作台内完成对话、任务执行、视觉创作和工程交付。

odyBox 的长期定位是：

> **个人 AI 工作台：既能聊天，也能对用户的文件、产物和工程采取行动。**

能力采用渐进式披露：默认使用 Chat；需要文件、工具和多步任务时进入 Work；需要视觉操作时打开 Canvas；需要运行真实应用时显示 Services。

### 2.3 Canvas

Canvas 是 odyBox 中针对 UI、网页和视觉产物的专业交互面，负责：

- Preview、viewport、缩放和响应式检查；
- 元素选择、hover、评论和 free-pin；
- 视觉版本比较、截图和验收；
- 将视觉意图映射为 Artifact patch 或真实源码变更；
- 导出原型，或把通过验证的变更交付到真实工程。

Canvas 不拥有独立登录、模型供应商、会话、Skills、权限、文件系统或进程管理器。

### 2.4 两种项目对象

#### Artifact Project

面向快速原型、单页视觉探索和无需工程环境的交付物。

- 内容权威可以位于 Artifact Store；
- 核心对象是自包含 HTML、Revision、PatchSet、Comment 和 Snapshot；
- 保留离线渲染、安全隔离、PNG/PDF/ZIP 导出；
- 不宣称等价于 React/Vue/Next/Vite 源码工程。

#### Workspace Project

面向新建或已有的真实代码工程。

- 源码权威始终是用户授权的 filesystem roots；
- 核心对象是源文件、页面、组件、路由、服务、测试和 Git diff；
- 修改直接进入原工程；
- Canvas Artifact 可以作为设计参考，但不能覆盖真实源码事实。

### 2.5 三种入口

```text
新建原型             → Artifact Project → Canvas
新建前端工程         → Workspace Project → Work + Canvas
打开已有/前后端工程  → Workspace Project → Work + Canvas + Services
```

用户不需要选择“调用 Ody”或“调用 OpenDesign”。系统根据项目类型和任务意图路由能力。

---

## 3. 当前实现边界

### 3.1 Ody 已有基础

Ody 已具备 Engineering Workspace 所需的大部分底层能力：

- app-server、版本化 protocol、client 和 transport；
- thread、turn、事件、审批、恢复和配置；
- 多个 `runtimeWorkspaceRoots` 与线程 cwd；
- 文件读写、目录和 metadata 协议；
- 命令、PTY、长进程和后台终端；
- Git、沙箱和权限体系；
- Browser 的导航、DOM、日志、截图和 CDP；
- Skills、Plugins、MCP 与扩展机制。

缺口不是重新建设 Agent Runtime，而是为真实项目、源码引用、服务和验证定义稳定的产品领域协议。

### 3.2 odyBox Canvas 当前能力

当前 Canvas 已形成 Artifact 闭环：

```text
需求 → 生成自包含 HTML → Preview → 元素选择/评论
     → 迭代或样式 Patch → Revision/Snapshot → 对比 → 导出
```

但“导入项目”仍是受限快照：

- 只读取白名单文件；
- 最多 200 个文件、12 MiB；
- 内容保存到 odyBox blob；
- 静态资源被打包进自包含 HTML；
- ES Modules 和外部资源不作为真实工程运行；
- 导出创建新的 `canvas-*` 目录，不回写原工程。

模型迭代接收的是上一版 HTML，而不是完整源码依赖图。因此当前 Canvas 适合原型，不适合可靠地重构框架组件、增加真实路由或维护前后端契约。

### 3.3 Ody 当前在 Canvas 中的责任

Ody 已经参与 Canvas，但范围有限：

- Visual Workspace v1 保存 VisualProject、Artifact、Revision、style patch 和 Snapshot；
- odyBox 将本地 Canvas 项目及版本同步给 Runtime；
- Ody 另有完整 Work Mode Agent Runtime，可以在真实目录中读写和执行命令。

当前仍由 odyBox 主导 Canvas 页面、Preview、元素交互、自包含 HTML 生成、本地 Artifact 编排和隔离渲染。

两条能力线尚未打通：Canvas 提交没有作为源码工程 turn 交给 Ody；Visual Workspace v1 也没有 framework、filesystem root、route、service 或 AST/source mapping。

### 3.4 当前不能宣称的能力

- 不能宣称导入已有项目后会直接维护原工程；
- 不能宣称 Canvas 已支持框架源码 round-trip；
- 不能宣称前端和后端会被自动发现、启动和联调；
- 不能把 powered/static preview 等价为真实 dev server；
- 不能把 Artifact Revision 等价为 Git 或源码 checkpoint。

---

## 4. OpenDesign 参考结论

### 4.1 做得更好的部分

OpenDesign 0.20.1 对已有项目采用真实目录模式：`baseDir` 经 realpath 后写入项目 metadata，Agent 和文件工具直接在用户目录中工作，不创建影子副本。

其代码迁移流水线是：

```text
code-import
→ design-extract / token-map
→ rewrite-plan
→ patch-edit ↔ build-test
→ diff-review
→ handoff
```

因此它可以在已有前端工程内搜索和修改页面、组件、样式与路由，并以小步 patch、构建和测试验证结果。这是 Engineering Workspace 应优先吸收的模式。

### 4.2 没有完整解决的部分

OpenDesign 尚未把前后端联动做成一等产品能力：

- 项目终端可以在 cwd 中运行命令，但不等于服务编排；
- 自动发现和启动 Next/Vite 等 dev server 仍是 Draft RFC；
- 自定义 proxy/rewrites 在该 RFC 中被列为 out of scope；
- 没有完整的服务依赖、健康检查、日志关联和网络错误归因；
- 单个 `baseDir` 不等价于正式的多根可写全栈项目模型。

OpenDesign 提供了“真实目录 + 通用终端 + 工程修改闭环”，但没有完成“全栈服务联动产品”。

### 4.3 吸收与隔离原则

优先吸收：

- 真实目录绑定与安全校验；
- 先检查现有源码再修改的约束；
- 小步 patch、build/test 和 diff review；
- iframe bridge、稳定元素身份、inspect/comment/free-pin；
- screenshot、视觉 lint、响应式和设计审查方法。

不整体搬运：

- `apps/web` 应用壳和 daemon API 客户端；
- OpenDesign 专属项目、账户、插件和 sidecar 模型；
- 与 odyBox 重复的会话、设置和模型系统；
- 只有外观、没有闭环价值的组件。

移植必须遵守 Apache-2.0 和 NOTICE，并单独核查字体、图片、模板与第三方依赖。

---

## 5. 目标工作流

### 5.1 从零创建原型

```text
Brief → 设计方向 → 自包含 Artifact → Preview/评论
      → Revision 与视觉验证 → 导出或转入 Workspace Project
```

该流程继续使用当前 Artifact Canvas，不要求安装依赖或理解工程结构。

### 5.2 从零创建真实前端工程

```text
选择技术栈和目标目录
→ Ody 创建工程骨架
→ 安装依赖与建立 Git baseline
→ 创建页面/组件/路由
→ 启动 dev server
→ Canvas 预览与视觉反馈
→ build/typecheck/test
→ diff review
```

项目创建后立即成为 Workspace Project，不先生成自包含 HTML 再尝试反向转换源码。

### 5.3 打开并修改已有工程

```text
目录授权
→ 只读识别技术栈、页面、组件、路由和脚本
→ 建立 SourceRef 与 Git baseline
→ 选择目标或描述任务
→ Ody 小步修改真实源码
→ build/test/browser 验证
→ Canvas 展示结果和视觉差异
→ 用户审核 diff
```

“当前页面”“这个组件”“增加设置页”等指令必须先解析到稳定 SourceRef，不能只把构建后的 DOM 或 HTML 交给模型重写。

### 5.4 前后端分离项目联动

Workspace Project 至少记录：

- 一个或多个授权 `workspaceRoots`；
- `frontendRoot`、`backendRoot` 和可选 package roots；
- 每个服务的启动、停止、构建、测试和 health check；
- 端口、URL、依赖顺序和非敏感环境变量引用；
- Runtime thread、后台终端、Preview session 和 Git baseline。

```text
odyBox 授权前端/后端 roots
  → Ody 识别 manifests、routes、API clients、命令与端口
  → Ody 按依赖顺序启动 backend、frontend 和必要依赖
  → health check 结果进入 Services 面板
  → Browser 打开前端并执行页面操作
  → 汇总 console、网络失败、后端日志、构建和测试
  → Agent 修改前端或后端源码
  → 重复验证
  → 展示跨 roots 的 diff 与验收结果
```

如果前后端位于两个独立目录，必须分别授权并传给 Ody 的 `runtimeWorkspaceRoots`。第一个 root 只是默认 cwd，其他 root 不能降格成附件。未获授权的后端目录不得被暗中读取或修改。

第一版优先支持本机开发闭环：一个前端服务、一个后端服务和可选数据库/依赖服务。远程集群、生产部署、复杂认证和任意容器编排后置。

---

## 6. 目标架构与职责边界

### 6.1 职责矩阵

| 领域 | odyBox / Canvas | Ody Runtime |
|---|---|---|
| 项目入口 | 新建原型、新建工程、打开目录、多根授权 UI | 校验并持有 Runtime workspace 上下文 |
| 可视化交互 | Preview、viewport、选择、批注、视觉比较 | DOM/SourceRef 数据、Agent 解释和变更执行 |
| 源码工程 | 文件树、页面/组件/路由视图、diff 和确认界面 | 搜索、读取、patch、格式化、Git、构建和测试 |
| 服务联动 | Services 面板、日志、启动/停止确认、健康状态 | 后台终端、进程生命周期、端口和 health check |
| 浏览器验证 | 承载预览，展示错误、截图和验收结果 | 导航、DOM、console、网络诊断、截图和自动操作 |
| Artifact | 呈现、评论、比较和导出 | Artifact/Revision/Snapshot 协议与持久化权威 |
| 权限安全 | 解释目标和影响，呈现审批 | roots、沙箱、命令/文件审批和敏感数据边界 |

### 6.2 状态所有权

- **Runtime 权威：** Thread、Turn、工具调用、审批、工作区绑定、服务状态、ChangeSet、验证结果、Artifact 元数据和 Revision 关系。
- **用户工作区权威：** Workspace Project 的真实源码、工程配置和 Git 历史。
- **Artifact Store 权威：** HTML、图片、bundle、截图等大对象，以稳定 ID 和内容 hash 引用。
- **odyBox 权威：** 窗口、布局、主题、面板开关、设备设置和未提交 UI 草稿。
- **Canvas 临时状态：** hover、当前选择、viewport 和缩放；评论、版本和验收结果必须持久化。

迁移期间禁止无版本双写。兼容数据必须带 `schemaVersion`、idempotency key、base hash 和来源标识。

### 6.3 协议边界

renderer 不应直接获得无边界的 `fs/writeFile` 或 `process/spawn` 通道。Ody 应提供面向产品语义的窄协议，在 Runtime 内执行权限、幂等、恢复和清理：

- `workspace/project/*`：绑定、读取、扫描、关闭；
- `workspace/source/*`：索引、SourceRef、ChangeSet、diff；
- `workspace/service/*`：发现、启动、停止、日志、health check；
- `workspace/preview/*`：URL、viewport、浏览器状态和验证；
- `visualWorkspace/*`：Artifact、Revision、Comment、Snapshot 和视觉 patch。

---

## 7. 领域模型与 Visual Workspace 演进

### 7.1 新增实体

- `WorkspaceProjectRef`：项目 ID、roots、权限、技术栈和 Git 状态；
- `WorkspaceRoot`：路径、角色、读写权限和授权来源；
- `SourceArtifact`：页面、组件或路由入口及框架信息；
- `SourceRef`：文件、范围、symbol、route 和内容 hash；
- `WorkspaceService`：命令、cwd、端口、依赖、health check 和进程状态；
- `PreviewSession`：URL、依赖服务、viewport、console/network/runtime errors；
- `ChangeSet`：跨文件 patch、base hash、验证结果和 Git diff；
- `ValidationRun`：build、typecheck、lint、test、browser 和 API 检查。

### 7.2 Visual Workspace v1 的保留边界

当前 v1 有意传输自包含 HTML。`VisualProject` 没有 roots、framework、entry、route 或 service；PatchSet 只支持按 `data-ody-id` 设置样式。

它继续服务 Artifact Project，不直接扩张为含糊的“万能项目”。Workspace Project 应新增 source-backed 表达，再通过稳定引用与 Visual Artifact 关联。

两种项目可以共享 Comment、Snapshot、视觉比较、Artifact 谱系和验收结果，但不能共享错误的存储假设：Artifact 内容可由 Artifact Store 持有，Workspace 源码始终以用户目录为准。

### 7.3 SourceRef 闭环

第一阶段不追求完整 IDE 级 AST 数据库，而是建立足够可靠的定位链：

```text
Canvas ElementRef
↔ Preview DOM / source metadata
↔ SourceRef(file, range, symbol, hash)
↔ ChangeSet
↔ ValidationRun
```

当内容 hash 或外部编辑导致定位失效时，必须重新索引或要求用户确认，不能在猜测位置静默写入。

---

## 8. 安全、版本与可靠性

### 8.1 真实目录

- 目录由用户通过可信选择器授权；
- 保存前执行 realpath，拒绝文件系统根、应用数据目录和越界符号链接；
- 多根项目分别记录权限，不因父会话存在而自动扩大范围；
- 脏工作树默认保留用户改动，不自动 reset、覆盖或删除；
- 修改前建立 Git baseline，修改后提供 diff；
- 无 Git 项目使用显式 checkpoint/backup，不以 Artifact revision 代替源码恢复。

### 8.2 命令与服务

- 安装依赖、启动服务和高风险命令遵循 Ody 审批；
- 环境变量使用受控引用，密钥不进入日志、Artifact、Snapshot 或模型上下文；
- 后台服务必须可列出、停止、清理并在崩溃后恢复状态；
- 端口冲突、启动超时和进程退出产生结构化错误；
- 关闭项目或 Runtime 时不得留下失控进程。

### 8.3 Preview

- Artifact Preview 继续使用 CSP、sandbox、隔离 profile 和资源 allowlist；
- Workspace Preview 使用真实 loopback dev server，但不因此获得 odyBox 应用权限；
- Browser 的网络、日志和截图结果按敏感数据规则裁剪；
- 远程 URL、登录态和生产环境操作需要单独授权策略。

---

## 9. 路线图

时间表示战略顺序，不是未经容量评估的交付承诺。

### 9.1 已完成阶段 S0–S4

| 阶段 | 状态 | 结果 | 审计 |
|---|---|---|---|
| S0 边界与基线 | 已完成 | 产品 ADR、权责矩阵、Canvas 基线、许可证台账 | [S0 审计](./2026-09-06-s0-completion-audit.md) |
| S1 Canvas 最小闭环 | 工程完成 | 生成、Preview、迭代、Revision、恢复和导出 | [S1 审计](./2026-09-06-s1-canvas-minimum-loop-completion-audit.md) |
| S2 双向视觉交互 | 工程完成 | ElementRef、评论、free-pin、PatchSet、快照和比较 | [S2 审计](./2026-09-06-s2-bidirectional-visual-interaction-completion-audit.md) |
| S3 Visual Workspace | 工程基础完成 | v1 协议、持久化、事件、TS client 和灰度切换 | [S3 审计](./2026-09-06-s3-visual-workspace-runtime-completion-audit.md) |
| S4 统一 Agent Runtime | 2026-09-11 关闭 | Work 唯一 Runtime 路径、审批、Skills/MCP、恢复、诊断与发行 | [S4 审计](./2026-09-06-s4-agent-runtime-gateway-completion-audit.md) |

供应商真实密钥首验和真实用户观察继续作为运营验证，不重新定义为 Runtime 架构缺口。

### 9.2 主线 E0–E4（已完成，2026-09-14 关闭）

| 阶段 | 状态 | 结果 | 审计/ADR |
|---|---|---|---|
| E0 项目模型与只读发现 | 已完成 | `workspace/project/*`：多 roots 绑定与恢复、技术栈/包管理器/页面/组件/路由/脚本/Git 状态扫描，扫描只读 | [E0 ADR](./2026-09-13-e0-workspace-project-adr.md) |
| E1 源码级页面与组件变更 | 已完成 | `workspace/source/*`：SourceRef 定位、ChangeSet 生命周期、diff、apply/restore、外部编辑 hash 校验 | [E1 ADR](./2026-09-13-e1-workspace-source-adr.md) |
| E2 真实框架 Preview | 已完成 | 服务 spec 与 store、依赖编排、health check、真实 dev server 生命周期 | [E2 ADR](./2026-09-13-e2-workspace-service-adr.md) |
| E3 前后端联动 | 已完成 | `workspace/preview/diagnose` 浏览器错误到服务/源码根因关联、跨 root 验证、崩溃恢复清理 | [E3 审计](./2026-09-14-e3-completion-audit.md) |
| E4 可靠性与恢复 | 已完成 | project watcher + 外部编辑冲突失效、append-only 审计日志、多窗口写锁、服务状态推送、envRefs/secretValues、Windows Job Object；四场景验收 + 手工 smoke 通过 | [E4 审计](./2026-09-14-e4-completion-audit.md) |


#### E0：项目模型与只读发现

- 新增 Workspace Project，与 Artifact Project 显式区分；
- 绑定并恢复一个或多个真实 roots；
- 识别技术栈、包管理器、页面、组件、路由、脚本和 Git 状态；
- 扫描阶段保持只读。

**退出条件：** 安全打开典型 React/Vite、Next.js、Vue 和前后端分离项目；失败可诊断且不会误写文件。

#### E1：源码级页面与组件变更

- 重构已有页面和组件；
- 在现有路由体系内增加页面；
- 建立 ElementRef 到 SourceRef 的映射；
- 使用小步 ChangeSet 和 Git diff；
- 接入格式化、typecheck、build 和 test。

**退出条件：** 固定工程集上的“修改页面、重构组件、增加路由”均通过原工程验证，用户可以拒绝或恢复变更。

#### E2：真实框架 Preview

- 识别工程已有 dev 脚本；
- 由 Ody 管理 dev server；
- 支持 HMR、端口冲突、启动失败、日志和生命周期清理；
- Browser 对真实 URL 执行视觉与运行时验证。

**退出条件：** Next/Vite/Vue 样本可从打开目录到 ready preview；失败原因可见；关闭项目不会遗留进程。

#### E3：前后端联动

- 支持单根 monorepo 和双根目录；
- 管理服务依赖、health check、日志和启动/停止；
- 将浏览器网络错误关联到后端日志和源码候选；
- 分别执行前后端构建、测试并汇总跨根 diff。

**退出条件：** 固定全栈样本能完成并验证“新增后端接口并在新页面调用”和“修复前后端契约不一致”。

#### E4：可靠性与恢复

- workspace watcher、内容 hash 和外部编辑冲突；
- 多窗口、多会话并发约束；
- Runtime 或服务崩溃后的恢复；
- checkpoint、回滚、审计和敏感环境变量保护。

**退出条件：** 外部 IDE 同时修改、Runtime 重启、服务崩溃和脏工作树场景不会静默丢失或覆盖代码。

**主线之后（2026-09-14 裁定）：** E0–E4 全部交付在 Ody Runtime 侧，`workspace/*` 窄协议已具备可验证能力，但尚无产品消费端。下一阶段为 Engineering Workspace 产品化：odyBox 按 §6.1 职责矩阵建设呈现端——项目入口（打开目录、多根授权）、文件树与页面/组件/路由视图、ChangeSet diff 与确认界面、Services 面板（日志、启停、健康状态）以及外部编辑冲突与写锁占用 UX；并行启动 §10.2 指标采集与真实用户观察。§9.3 不扩张清单继续有效。

### 9.3 E0 之前不扩张

- 模板商城和大型多人协作；
- 生产部署和任意云环境编排；
- 移动端完整源码编辑；
- 全框架统一 AST；
- Canvas 独立品牌或安装包；
- 以更多生成模板替代真实工程闭环。

---

## 10. 指标体系

### 10.1 Artifact Canvas

- 首次成功渲染时间；
- 生成—反馈—接受/导出的闭环完成率；
- 指向式修改命中率和 Revision 接受率；
- desktop/mobile 检查通过率；
- Preview 启动和运行错误率；
- 周复用率。

### 10.2 Engineering Workspace

- 已有项目成功打开率和首次索引耗时；
- 页面、组件、路由和 SourceRef 定位准确率；
- 修改后 build/typecheck/test 通过率；
- 用户接受的 diff 比例和人工返工量；
- dev server 启动成功率与 ready 时间；
- 前后端 health check 通过率；
- 浏览器错误到服务/源码根因的关联准确率；
- 外部编辑冲突、未受控进程和代码丢失事件数；
- Artifact 原型转入真实 Workspace 并最终合入的比例。

### 10.3 Runtime 收敛

- Work/Canvas 工程任务经 Ody Runtime 执行的比例；
- 重复工具、权限和进程实现的剩余数量；
- 协议兼容、Runtime 恢复和升级成功率；
- Runtime 与 UI 状态不一致事件数。

禁止把 Artifact 生成数量作为主要成功指标。大量未采用输出不代表产品价值。

---

## 11. 主要风险与应对

| 风险 | 表现 | 应对 |
|---|---|---|
| odyBox 定位失焦 | 四个操作面同时堆在首页 | 按项目和任务渐进显示 |
| Artifact 与源码混淆 | 看似修改成功，实际只改 HTML | 明确项目类型；Workspace 变更必须落到 SourceRef 和 diff |
| Runtime 再次分叉 | renderer 新增通用文件或进程工具 | 只增加窄协议；工程执行留在 Ody |
| 破坏用户代码 | 覆盖脏工作树或错误路径 | roots 授权、Git baseline、hash 冲突和可恢复 ChangeSet |
| 服务可启动但不可诊断 | 多个终端存在，错误无法关联 | Service/Preview/Validation 结构化事件和统一时间线 |
| 协议被 UI 绑死 | 字段直接映射 React 状态 | 描述领域实体，用 TUI/测试客户端交叉验证 |
| 只复制 OpenDesign 外观 | 更像设计工具但没有工程闭环 | 优先目录、SourceRef、Patch、build/test 和 diff |
| 多端拖累桌面主线 | 移动端被迫完整实现 | 桌面优先；移动端先只读 Preview、评论和审批 |
| 安全边界扩大 | 命令、密钥或本地服务暴露 | Runtime 审批、密钥隔离、loopback 策略和日志脱敏 |

---

## 12. 治理规则

以下变化必须写 ADR 或设计文档：

- Runtime 与 odyBox 之间的状态所有权变化；
- 新增或破坏性修改公共 app-server 协议；
- Artifact、Workspace、SourceRef 或 ChangeSet schema 变化；
- renderer 获得新的文件、命令、进程或网络能力；
- 引入 OpenDesign 源码、素材或依赖；
- Canvas 升级为一级导航或独立产品；
- Workspace roots、Git 和恢复策略变化。

每项迁移必须回答：

1. 谁是单一事实来源？
2. 旧数据和旧项目如何迁移？
3. 进程退出、版本不匹配或外部编辑时如何恢复？
4. 如何比较新旧链路行为？
5. 在什么条件下删除旧实现？

只有 Canvas 同时表现出独立用户画像、独立获客渠道、自足工作流、稳定留存/付费，并且独立收益高于重复基础设施成本时，才重新评估独立产品化。

---

## 13. 源码与审计依据

### 13.1 Ody

- `app-server-protocol/src/protocol/v2/thread.rs`：cwd 与 `runtimeWorkspaceRoots`；
- `app-server-protocol/src/protocol/v2/fs.rs`：文件与目录协议；
- `app-server-protocol/src/protocol/v2/process.rs`：长进程、PTY、输出和终止；
- `app-server-protocol/src/protocol/v2/visual_workspace.rs`：v1 与自包含 HTML 边界；
- `app-server/src/request_processors/visual_workspace_processor.rs`：视觉状态持久化；
- `app-server/src/browser_extension.rs`：Browser 导航、DOM、日志、截图与 CDP；
- `docs/chs/browser-control.md`：浏览器安全与审批；
- `docs/en/design_mode.md`：Design Mode 不等同于 Canvas。

### 13.2 odyBox

- `src/main/canvas-ipc.ts`：快照导入限制、导出目录和隔离渲染；
- `src/renderer/services/canvas/CanvasProjectService.ts`：自包含 HTML bundling；
- `src/renderer/components/canvas/CanvasWorkspace.tsx`：Canvas 生成和交互入口；
- `src/renderer/services/canvas/CanvasSyncService.ts`：Canvas → Ody 同步；
- `src/renderer/services/ody-runtime/work-generation.ts`：Work thread/turn 与多 roots；
- `src/main/ody-runtime-policy.ts`：renderer 到 Runtime 的能力白名单；
- `src/renderer/services/ody-runtime/background-terminals.ts`：后台终端 UI adapter。

### 13.3 OpenDesign 0.20.1

- `packages/contracts/src/api/projects.ts`：已有文件夹直接读写契约；
- `apps/daemon/src/import-export-routes.ts`：folder import、realpath 与安全检查；
- `apps/daemon/src/server.ts`：项目 cwd 和现有文件扫描；
- `apps/daemon/src/runtimes/chat-prompt-inputs.ts`：先检查工作区再修改；
- `apps/daemon/src/routes/terminal.ts`：项目 cwd 中的 PTY；
- `plugins/_official/scenarios/od-code-migration/open-design.json`：迁移流水线；
- `plugins/_official/atoms/patch-edit/SKILL.md`：小步可审核修改；
- `plugins/_official/atoms/build-test/SKILL.md`：构建与测试闭环；
- `docs/rfc-drafts/dev-server-auto-detect.md`：dev server 草案及范围限制；
- `LICENSE` 与 NOTICE：许可证治理依据。

### 13.4 阶段文档

- [S0 产品边界 ADR](./2026-09-06-s0-product-boundary-adr.md)
- [S0 能力权责矩阵](./2026-09-06-s0-capability-ownership-matrix.md)
- [S0 Canvas 基线](./2026-09-06-s0-canvas-baseline.md)
- [S0 OpenDesign 许可证清单](./2026-09-06-s0-opendesign-license-inventory.md)
- [S0 完成审计](./2026-09-06-s0-completion-audit.md)
- [S1 完成审计](./2026-09-06-s1-canvas-minimum-loop-completion-audit.md)
- [S2 完成审计](./2026-09-06-s2-bidirectional-visual-interaction-completion-audit.md)
- [S3 完成审计](./2026-09-06-s3-visual-workspace-runtime-completion-audit.md)
- [S4 完成审计](./2026-09-06-s4-agent-runtime-gateway-completion-audit.md)

---

## 14. 战略结论

长期结构是一个 Runtime、两个主要产品界面和一个内嵌视觉工作区：

- Ody 建设一次 Agent 与工程执行能力；
- Ody TUI 提供高效率终端界面；
- odyBox 提供 Chat、Work、Canvas 和 Services；
- Canvas 同时服务快速 Artifact 原型和真实 Workspace 的可视化操作；
- OpenDesign 只作为真实目录、设计交互和工程闭环的参考来源。

下一阶段不应继续把重点放在增加更多独立 HTML 能力，而应推进 Engineering Workspace：先安全打开真实工程，再完成源码级页面与组件修改，然后接入真实框架 Preview，最后形成可验证的前后端联动闭环。
