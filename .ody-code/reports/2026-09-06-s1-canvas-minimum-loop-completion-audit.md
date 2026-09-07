# S1 Canvas 最小闭环完成审计

**日期：** 2026-09-06  
**状态：** 工程范围完成；真实用户价值与质量指标进入观察期  
**实现仓库：** `/Users/ranwei/workspace/go_work/odyBox-base/`

## 1. 结论

S1 所要求的工程闭环已经落地：用户可以在 odyBox 内创建 Canvas，提交 brief，看到自包含 HTML 预览，针对选定版本提出修改，获得有谱系的新 Revision，恢复历史版本，并导出 HTML 或下载预览截图。

这不等于已经证明 Canvas 有市场价值。战略中的停止条件仍然有效：必须用真实用户完成率、首个可用结果时间、迭代次数、失败分布和导出行为判断是否进入重资产 S2。

## 2. S1 行动与实现证据

| S1 要求 | 状态 | 实现证据 |
|---|---|---|
| 内层生成器获得 brief、会话、设计方向和约束 | 完成 | `design-generator.ts` 注入显式输入、最近 12 条可见文本、语言/主题/模型设置和基础版本 HTML |
| `frontend-design` 选择结果进入内层生成 | 完成 | 新 Design Session 自动选择该 Skill；`tools-builder.ts` 读取所选 Skill 正文并传入生成器；共享正文作为不可用时的同源降级 |
| “继续迭代”与 Artifact Revision | 完成 | Schema 增加 `lineageId/revision/parentArtifactId/revisionReason`；工具接受精确 `baseArtifactId`；Artifact 卡片入口已启用 |
| Chat + Preview 分栏工作区 | 完成 | `/design/$projectId` 使用 `CanvasWorkspace`；左侧复用真实 Design Session，右侧显示所选 Revision |
| desktop/mobile、刷新、运行错误、截图 | 完成 | 390/1280 viewport、iframe 重载、沙箱 postMessage ready/error/snapshot 桥、PNG 下载 |
| 保留自包含 HTML | 完成 | 生成提示和 CSP 继续禁止外部网络依赖；未引入框架工程生成 |
| 指标采集 | 完成 | 失败代码、首个 preview 耗时区间、生成耗时区间、是否迭代、Revision、恢复、截图和导出；不发送用户内容或项目/产物 ID |

## 3. 数据与会话语义

- 旧 Artifact 不带 Revision 字段时按 `lineageId=id、revision=1` 读取，保持向后兼容。
- 迭代永远创建新 Artifact，不覆盖基础版本。
- 恢复旧版同样创建新的 head Revision，并记录 `restoredFromArtifactId`，因此历史可追踪。
- Design Session 和 Project 作为一个用户动作创建；中途失败时回滚本次已创建记录。
- `projectId` 已进入 Session Meta，因此从历史会话列表重新打开时会回到 Canvas，而不是普通聊天页。

## 4. 退出条件核验

```text
生成 → 看到 → 提修改 → 新版本 → 导出
  ✓      ✓       ✓         ✓        ✓
```

- **失败可诊断：** 模型、解析、Schema、Project、基础 Artifact、存储、preview runtime 和 snapshot 均有明确错误类型。
- **版本可回退：** 可选择任何历史 Revision，并以新 head 的方式恢复，不破坏历史。
- **入口可重复：** 可从侧栏 Preview 操作、Design Session 历史项和 Artifact 的“继续迭代”进入。

## 5. 验证记录

在 odyBox 仓库执行：

- `pnpm check`：通过。
- `pnpm check:shared-boundaries`：通过。
- `pnpm eval:canvas:s0`：12 个固定 case manifest 有效。
- 16 个相关测试文件：153 项测试全部通过。
- `cross-env ODYBOX_BUILD_PLATFORM=web electron-vite build`：主进程、preload 和 renderer 生产构建均成功。
- `git diff --check` 与涉及文件 Biome 检查：通过（完成审计后再次复核）。

`pnpm build:web` 在上述生产编译成功后，因仓库既有脚本引用了未跟踪的 `.erb/scripts/delete-source-maps-runner.js` 而在 sourcemap 清理步骤失败；这不是 S1 代码的编译错误，本阶段未扩大范围修复构建脚本。

## 6. 明确保留的限制

- 当前 screenshot 是沙箱内 DOM → SVG foreignObject → PNG 的离线实现；复杂 canvas/video 内容可能失败，并会返回 `snapshot` 错误。S3 接入 Ody Browser 后应切换为浏览器级截图。
- 未引入 S2 的稳定元素 ID、点击检查、评论、free-pin、computed styles 或 SourceRef 映射。
- 未运行付费/真实模型评测，因此不得把 manifest-valid 表述为视觉质量达标。
- 本次尝试了本地浏览器可视验收，但会话缺少 Browser 技能所需的控制执行接口；没有伪造截图或改用未授权替代工具。
- 只为英文、简体中文和繁体中文补齐了新增 Canvas 文案；其他语言按现有 i18n fallback 显示英文，后续由翻译流程补齐。

## 7. 观察期门槛

S1 工程完成后至少持续观察：

1. `canvas_generate_submit → canvas_first_preview_ready` 完成率和耗时区间；
2. `canvas_failure.failure_type` 分布；
3. 单次任务达到可接受结果所需 Revision 数；
4. `canvas_export` / `canvas_snapshot` 是否发生；
5. 用户是否能在无人工引导下再次打开并继续同一 Design Session。

若真实用户不能稳定完成闭环，暂停 S2，优先修复任务价值、入口、生成可靠性或预览诊断；不得用更多画布功能掩盖 S1 未被验证的问题。
