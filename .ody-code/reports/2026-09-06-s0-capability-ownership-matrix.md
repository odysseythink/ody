# S0：Ody × odyBox 能力权责与迁移矩阵

**日期：** 2026-09-06  
**状态：** Baseline v1  
**用途：** 防止新增第三套实现，并为 S1–S4 切流提供单一台账。

## 状态图例

- **Authority**：长期单一事实来源；
- **Temporary**：迁移期间保留，达到删除条件后移除；
- **Client-only**：合法的客户端专属职责；
- **Missing**：目标能力尚不存在；
- **Adapter**：只做协议转换，不拥有业务事实。

## 能力矩阵

| 能力 | Ody 当前 | odyBox 当前 | 长期权威 | 当前状态 | 责任角色 | 目标阶段 / 删除条件 |
|---|---|---|---|---|---|---|
| 模型 Provider 抽象 | 已有多 Provider 运行链路 | 独立 AI SDK Provider 链路 | Ody Runtime | 双实现 | Runtime Maintainer | S4；odyBox 全部受支持模型经 Runtime 通过兼容测试 |
| 模型配置与凭据使用 | Ody 配置/认证体系 | odyBox 设置与 Provider 配置 | Ody Runtime（执行）；odyBox（设置 UI） | 分裂 | Runtime + odyBox | S4；GUI 仅提交/读取协议化配置，不直接执行模型 |
| Thread / Turn 生命周期 | app-server protocol 已有 | 独立 Session/Message/Thread | Ody Runtime | 双实现 | Runtime Maintainer | S4；迁移、恢复、fork 和流式行为对等 |
| Agent loop / 工具调度 | Ody core | GenerationService / orchestration | Ody Runtime | 双实现 | Runtime Maintainer | S4；Work/Canvas 默认走 Ody 并稳定一个发布窗口 |
| 流式事件 | app-server notifications | odyBox 自有 stream 状态 | Ody Runtime；odyBox 只投影 | 双实现 | Runtime + odyBox | S4；TS client 覆盖 text/reasoning/tool/pause/error |
| Context compaction | Ody core | odyBox 独立实现 | Ody Runtime | 双实现 | Runtime Maintainer | S4；历史会话迁移和恢复验证通过 |
| 工具审批 | app-server 审批协议 | odyBox paused tool call / approval | Ody Runtime | 双实现 | Runtime + Security | S4；拒绝、继续、重启恢复语义对等 |
| 命令执行与沙箱 | 原生沙箱与 exec policy | Electron Main sandbox | Ody Runtime | 双实现 | Runtime + Security | S4；平台覆盖、安全测试和性能达标 |
| 文件读取/搜索/写入/编辑 | Ody tools | odyBox filesystem toolset | Ody Runtime | 双实现 | Runtime Maintainer | S4；路径、AGENTS、审批和 diff 行为对等 |
| AGENTS.md 注入 | Ody 原生 | odyBox 工作目录读取注入 | Ody Runtime | 双实现 | Runtime Maintainer | S4；odyBox 删除 prompt 侧重复注入 |
| Skills 发现/加载/注入 | core-skills + skills extension | Main/Renderer 独立 Skills | Ody Runtime | 双实现 | Runtime Maintainer | S4；设置 UI 变为 Runtime catalog client |
| Plugins | Ody plugin/marketplace | 无等价统一插件层 | Ody Runtime | Ody 领先 | Runtime Maintainer | S3/S4；odyBox 仅呈现可用插件和授权 |
| MCP | Ody extension / app-server | odyBox 独立 MCP client | Ody Runtime | 双实现 | Runtime Maintainer | S4；连接配置和工具事件协议化 |
| Browser Control | 内置 CDP、安全与审批 | 无同等完整闭环 | Ody Runtime | Ody Authority | Runtime + Security | S3；Canvas screenshot/inspect 首先复用 Ody |
| Web Search | Ody 工具/连接器 | odyBox 独立 scope | Ody Runtime | 双实现 | Runtime Maintainer | S4；结果、引用和权限行为对等 |
| Knowledge Base / RAG | 可通过扩展/工具承载 | odyBox 已有产品能力 | Ody Runtime 扩展；odyBox UI | odyBox Temporary | Runtime + odyBox | S4；先定义检索协议与存储边界 |
| Chat 消息编辑与分支 UI | TUI 交互不同 | odyBox 消费者级完整 UI | odyBox Client-only；Runtime 保存规范状态 | 合法差异 | odyBox Maintainer | 不要求界面对等；协议需支持所需操作 |
| Chat / Work 模式呈现 | collaboration modes | Chat/Work Agent Mode | Runtime 定义能力；odyBox 定义呈现 | 语义未统一 | Product + Runtime | S4；建立 mode capability mapping |
| Design 文档模式 | Ody `/design` 规范探索 | 不等价 | Ody Runtime/TUI | Ody Authority | Runtime Maintainer | 保留；不得与 Canvas 混称同一能力 |
| Project / Artifact Schema | 尚无 Visual Workspace 域 | 已有 schemaVersion 1 | Ody Runtime | odyBox Temporary | Canvas + Runtime | S3；协议可迁移既有数据后切权威 |
| Artifact 大对象存储 | 文件/Artifact 能力分散 | local storage + persisted sandbox artifacts | Artifact Store，经 Runtime 引用 | 分裂 | Runtime + Canvas | S3；稳定 ID/hash、配额、清理和迁移明确 |
| Artifact 消息卡片 | TUI 可显示文件/链接 | 已有 ArtifactCard | 各客户端 Client-only | odyBox Authority | odyBox Maintainer | 保留；消费统一 Artifact 事件 |
| HTML Preview server | Browser/本地文件能力 | Main preview server | Ody Preview service；odyBox Renderer | 待收敛 | Canvas + Security | S3；隔离、恢复和资源解析达到现状 |
| Canvas 路由与布局 | 无 | `/design/$projectId` | odyBox Client-only | Experimental | Canvas Maintainer | S1；形成 Chat + Preview 最小闭环 |
| Canvas 产品状态 | 无 | 原先未显式 | odyBox Client-only | 已标记 Preview | Product + Canvas | 稳定门槛达到前保持 experimental |
| UI Artifact 生成 | 可通过 Agent/文件工具间接完成 | `design_generate_artifact` 孤立生成器 | Ody Runtime visual extension | odyBox Temporary | Canvas + Runtime | S3；完整 brief、版本和事件对等后切流 |
| Design direction / templates | 无系统目录 | 单个 frontend-design Skill | Ody Skill/Plugin catalog | Missing/Fragmented | Canvas + Runtime | S1/S3；目录、来源、许可和选择结果可追踪 |
| Artifact Revision / undo | 无视觉域 | 缺失 | Ody Runtime | Missing | Canvas + Runtime | S1/S3；schema 与恢复测试通过 |
| Viewport / responsive matrix | Browser 可调整页面环境 | Canvas UI 缺失 | Canvas UI + Runtime snapshot | Missing | Canvas Maintainer | S1/S2 |
| Screenshot feedback | Browser screenshot 已有 | Design 生成链未闭环 | Ody Runtime | 可复用但未接入 | Canvas + Runtime | S2/S3 |
| ElementRef / SourceRef | 缺失视觉协议 | 缺失 | Ody Runtime protocol；Canvas 采集 | Missing | Canvas + Runtime | S2/S3；定位准确率有基线 |
| Inspect / comment / free-pin | 缺失视觉协议 | 缺失 | Canvas UI；Runtime 持久化 | Missing | Canvas Maintainer | S2 |
| CSS override / PatchSet | apply_patch 可执行 | 缺少视觉映射 | Ody Runtime | Missing bridge | Canvas + Runtime | S2/S3；可回滚且映射可靠 |
| 视觉 lint / a11y / runtime errors | 工具可组合 | 无统一 gate | Ody Runtime validation extension | Missing | Canvas + Runtime | S2/S5 |
| 窗口、主题、面板与设备能力 | 不适用 | Electron/Web/Mobile 完整 | odyBox Client-only | odyBox Authority | odyBox Maintainer | 永久保留在客户端 |
| 更新、签名、桌面发布 | Ody CLI 发布 | odyBox Electron/移动发布 | 各产品独立 | 合法差异 | Release Owner | 不创建第三套发布链 |
| OpenDesign 适配 | 无 | 局部概念已引入 | Adapter，不拥有状态 | Missing | Canvas Maintainer | S1 spike；按许可证台账治理 |

## 迁移优先级

1. **S1：** 不改 Runtime 权威，先验证 Canvas 最小闭环和用户价值。
2. **S2：** 建立双向视觉交互，明确真正需要的协议，不用假设驱动 app-server 扩表。
3. **S3：** 先迁 Browser、Snapshot、Artifact、Patch 等视觉链路到 Ody Runtime。
4. **S4：** 依次迁文件/沙箱/审批、Skills/MCP、Work loop、会话流和普通 Chat。

## 新能力准入模板

新增跨产品能力时必须在 PR/Design 中填写：

```text
能力：
长期权威：Ody Runtime / odyBox Client / Canvas Client / Artifact Store
临时实现：
公共协议：
迁移阶段：
删除旧实现的可验证条件：
状态与数据迁移：
责任角色：
```

若无法回答“长期权威”和“删除条件”，该能力不得以第三套实现进入主线。
