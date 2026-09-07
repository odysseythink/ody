# ADR-S0-001：Ody / odyBox / Canvas 产品与运行时边界

**日期：** 2026-09-06  
**状态：** Accepted  
**决策范围：** Ody、odyBox、Canvas、OpenDesign 复用

## 背景

Ody 当前侧重 TUI AI 编程，odyBox 侧重桌面 AI 助手。若再创建独立 UI 生成桌面产品，将重复模型配置、会话、Agent、工具、权限、Skills、MCP、Artifact、桌面发布和品牌获客能力。

同时，odyBox 已经存在 Project/Artifact Schema、Design 路由、`design_generate_artifact`、HTML Preview 和前端设计 Skill。问题不是缺少第三个产品壳，而是现有视觉链路尚未形成生成、预览、指向反馈、修改、验证和版本管理的闭环。

## 决策

采用以下长期结构：

```text
Ody Runtime
├── Ody TUI
└── odyBox
    ├── Chat
    ├── Work
    └── Canvas（Experimental / Preview）
```

1. 不创建第三个独立 UI 生成桌面产品。
2. Ody TUI 是面向开发者的 AI 编程产品界面。
3. odyBox 是面向非终端用户的个人 AI 工作台。
4. Canvas 是 odyBox 内嵌视觉工作区，不拥有独立账户、模型、会话、Agent、工具和权限体系。
5. Ody Runtime 长期拥有 Agent 编排、工具、权限、Skills、Plugins、MCP、Browser 和文件操作的单一事实来源。
6. odyBox 长期拥有桌面/Web/移动产品壳、消费者交互、设备集成和 Artifact/Canvas 呈现。
7. OpenDesign 是参考实现与选择性移植来源，不作为嵌入 odyBox 的第二套完整产品。
8. 迁移以协议适配和逐项切流完成，不进行一次性重写。

## 状态所有权

| 状态 | 长期权威 | 说明 |
|---|---|---|
| Thread、Turn、工具调用、审批、工作区绑定 | Ody Runtime | 所有客户端消费同一事件语义 |
| Skills、Plugins、MCP、Browser、文件操作 | Ody Runtime | 禁止 odyBox 新增长期平行实现 |
| Artifact 元数据与 Revision 关系 | Ody Runtime | 通过版本化 Visual Workspace 协议暴露 |
| Artifact 大对象 | Artifact Store | HTML、图片、bundle、截图按稳定 ID/hash 引用 |
| 窗口、布局、主题、面板、设备设置 | odyBox | 不进入 Runtime 领域协议 |
| hover、缩放、临时选择 | Canvas 客户端 | 评论、版本等可审计状态必须写回 Runtime |

## 产品呈现规则

- Canvas 当前必须显示 `Preview`；机器状态为 `experimental`。
- 初期通过设计会话和 Artifact 进入，不强制成为一级导航。
- 移动端无需与桌面端功能对等；完整创作优先桌面。
- Ody TUI 可以生成、修改和验证视觉 Artifact，但不复制完整 Canvas。

## 工程治理规则

从本 ADR 接受之日起：

1. 新增 Agent 基础能力前，必须在能力权责矩阵中声明长期权威。
2. 若能力临时落在 odyBox，设计必须写明迁移接口和删除条件。
3. app-server 公共协议、状态所有权和 Artifact schema 的变化必须写 ADR 或 Design 文档。
4. 引入 OpenDesign 源码或资源前必须更新许可证台账。
5. 新建第三个桌面产品或让 Canvas 独立产品化，必须用新的 ADR 显式取代本决策。

## 后果

### 正面

- 产品组合保持清晰，不增加第三套品牌和安装包；
- Ody 的安全、工具和扩展能力可以复用于 GUI；
- odyBox 可以专注消费者体验和视觉交互；
- Canvas 可先低成本验证，再按数据扩大；
- OpenDesign 的价值被吸收到闭环能力，而不是停留在换皮。

### 代价

- 中期必须维护 Ody Runtime 与 odyBox 旧链路的适配期；
- app-server 需要新增视觉领域协议和 TypeScript client；
- odyBox 当前独立 Agent 能力只能逐步删除，短期不会立刻减少代码；
- 跨进程预览、Artifact 和崩溃恢复需要额外工程投入。

## 重新评估条件

只有 Canvas 同时表现出独立用户画像/购买者、独立获客渠道、自足核心工作流、稳定留存/付费，以及与 odyBox 明显分叉的信息架构，才重新评估独立产品。单纯“功能很多”或“界面专业”不是拆分理由。

## 责任角色

| 角色 | S0 后责任 |
|---|---|
| 产品负责人 | 守住产品边界、入口和独立产品决策门 |
| Ody Runtime Maintainer | app-server、Agent、工具、权限和公共协议权威 |
| odyBox Maintainer | 客户端适配、桌面/移动壳和消费者体验 |
| Canvas Maintainer | 视觉交互、Preview、ElementRef、Revision 与质量闭环 |
| Security / Release Owner | HTML 隔离、第三方许可、签名与分发审查 |

具体人员可由团队调整，但责任域不得为空或重复宣称权威。

## 关联记录

- [长期产品与技术战略](./2026-09-06-ody-product-runtime-canvas-long-term-strategy.md)
- [S0 能力权责矩阵](./2026-09-06-s0-capability-ownership-matrix.md)
- [S0 Canvas 基线](./2026-09-06-s0-canvas-baseline.md)
- [S0 OpenDesign 许可证台账](./2026-09-06-s0-opendesign-license-inventory.md)
