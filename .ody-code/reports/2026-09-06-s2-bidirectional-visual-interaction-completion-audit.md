# S2 Canvas 双向视觉交互完成审计

**日期：** 2026-09-06  
**状态：** 工程范围完成；真实浏览器与用户价值进入观察期  
**实现仓库：** `/Users/ranwei/workspace/go_work/odyBox-base/`

## 1. 结论

S2 的工程退出条件已实现：用户可直接点击预览元素或在任意位置打点，不再依赖“左上角第二个按钮”这类自然语言定位；评论以结构化 `ElementRef / SourceRef` 交给 Agent；直接样式修改会生成持久化 `PatchSet` 和新的 Artifact Revision，而不是只改变 iframe 内存状态。

这表示双向视觉链路在 odyBox 自包含 HTML Artifact 范围内闭合，不表示真实前端工程同步、视觉质量或市场价值已经验证。S3 的 Ody app-server / Visual Workspace 协议迁移未提前实施。

## 2. S2 行动与实现证据

| S2 要求 | 状态 | 实现证据 |
|---|---|---|
| 稳定元素 ID 与 ElementRef | 完成 | `canvas-html.ts` 将 `data-ody-id` 写入源 HTML，保留已有 ID，并通过唯一语义匹配跨 Agent Revision 恢复 ID；`ElementRefSchema` 同时记录 artifact、lineage、revision、bounds、styles 和 SourceRef |
| 选择、hover、bounds、computed styles | 完成 | sandbox bridge 使用受限 inspect 模式和覆盖层，宿主校验并裁剪 iframe 回传数据，只接受白名单 computed styles |
| 元素评论与 free-pin | 完成 | 评论 anchor 支持 element / pin；开放评论作为编号标记回显到 iframe；可解决评论 |
| 结构化发送给 Agent | 完成 | 每次迭代携带精确 `baseArtifactId` 与 `<canvas_feedback_json>`，其中包含选择和开放评论，而非把 DOM 位置重新翻译成模糊文字 |
| CSS override → PatchSet → 源修改 | 完成 | CSS property/value 经限制后写入目标 `data-ody-id` 元素的源 HTML，创建 `style-patch` Revision，并记录 base/result artifact 与 operation |
| 截图回看、响应式矩阵、a11y/runtime 检查 | 完成 | desktop 1280、tablet 768、mobile 390 顺序审计并留档；检查 alt、控件名称、表单标签、重复 id、横向溢出和 runtime error |
| 可重复的前后视觉评测 | 完成 | 同一 viewport、两个不同 Artifact Revision 的 PNG 解码为同尺寸 RGBA，使用固定阈值计算 changed-pixel ratio，并并排显示前后图 |

## 3. 持久化与生命周期

- 新生成 Artifact 和 Agent Revision 在落盘前完成元素身份注入。
- S1 遗留 Artifact 不会被静默改写；用户通过“启用视觉映射”创建新的 `instrument` Revision，历史版本保持原样。
- 元素直接修改永远创建新 Revision，版本选择和恢复继续复用 S1 谱系。
- 评论、Snapshot metadata 和 screenshot Blob 分离存储；单截图限制为 12 MiB，每条 lineage 保留最近 30 张。
- 删除 Design Session 时，在删除 Project 前级联删除评论、截图元数据和 Blob，避免孤儿数据。
- iframe 回传的 ElementRef 和 ValidationResult 在宿主侧经过 schema 校验；截图时移除 hover、comment 和 bridge 覆盖层，降低非内容像素噪声。

## 4. 退出条件核验

```text
预览中点击元素 / 任意位置打点
              ↓
稳定 nodeId + bounds + styles + SourceRef
              ↓
评论 ─────────┼───────── 直接 CSS declaration
              ↓                         ↓
结构化 Agent feedback            PatchSet + 新 Revision
              └──────────┬──────────────┘
                         ↓
三视口审计 + 截图留档 + 跨 Revision 对比
```

- **无需口头定位：** 点击选择产出稳定 `nodeId` 和源选择器。
- **修改可持久化：** Agent 修改和直接 CSS 修改都创建 Revision；后者额外保留机器可读 PatchSet。
- **结果可回看：** Snapshot 绑定 artifact/revision/viewport/validation，可跨版本比较。
- **旧数据可迁移：** 遗留 Artifact 显式升级为映射 Revision，不破坏历史。

## 5. 验证记录

在 odyBox 仓库执行：

- `pnpm check`：通过。
- `pnpm check:shared-boundaries`：通过。
- 20 个相关测试文件：181 项测试全部通过；新增 instrumentation 迁移测试后相关集合为 182 项，最终复核见本审计提交前命令输出。
- `pnpm eval:canvas:s0`：12 个固定 case manifest 有效，S0/S1 基线未退化。
- `cross-env ODYBOX_BUILD_PLATFORM=web electron-vite build`：main、preload、renderer 生产构建成功。
- `git diff --check` 与涉及文件 Biome 检查：通过；仓库既有文件仍有非阻断 lint warning，未在 S2 扩大清理范围。

## 6. 明确保留的限制

- 当前 SourceRef 指向自包含 Artifact HTML 中的稳定 selector，不是 React/Vue/多文件工程的 AST 源位置；后者属于 S3/S4 协议和工程写回范围。
- screenshot 仍是 iframe 内 DOM → SVG foreignObject → PNG 的离线实现；复杂 canvas/video 内容需要 S3 接 Ody Browser 后升级为浏览器级截图。
- 基础 a11y 检查是确定性静态/运行态规则，不能替代键盘导航、读屏器和人工验收。
- 当前只比较完整截图的像素变化率，不做感知模型评分、区域忽略或动态内容稳定化。
- 本会话环境缺少 Browser 技能要求的浏览器控制执行接口，因此未进行真实浏览器可视验收；没有使用未授权的 Playwright 替代。
- 未运行真实模型或真实用户任务，因此不得把工程完成表述为视觉质量、S1 停止条件或商业价值已经达标。

## 7. S2 观察期门槛

进入 S3 前至少验证：

1. 用户能否无引导完成“选择 → 评论/直改 → 新版本 → 对比”；
2. ElementRef 在真实多轮 Agent 迭代后的存活率和错误绑定率；
3. desktop/tablet/mobile 审计中的误报、漏报和截图失败率；
4. 评论交给 Agent 后一次修改命中率，以及平均 Revision 数是否下降；
5. Snapshot 存储增长、12 MiB 拒绝率和每 lineage 30 张保留策略是否合适。

若上述数据不成立，应修正元素身份、截图或交互设计，不应以 S3 运行时迁移掩盖 S2 闭环质量问题。
