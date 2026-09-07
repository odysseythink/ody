# S0：odyBox Canvas / design_generate_artifact 基线

**日期：** 2026-09-06  
**评估对象：** odyBox 当前 Design/Artifact 链路  
**基线版本：** S0-v1

## 1. 基线结论

当前链路的**宿主结构是可工作的**，但尚无证据证明其视觉质量或真实模型端到端成功率：

- 固定 Design/Artifact 测试：**14/14 test files、84/84 tests 通过**；
- 固定 Canvas 评测清单：**12/12 cases schema 校验通过**；
- 静态闭环能力：**8/15 已具备**；
- 真实模型交付成功率：**N/A，样本数 n=0**；
- 真实视觉质量得分：**N/A，样本数 n=0**。

`N/A` 是 S0 的真实基线，不以单元测试冒充视觉质量。今后任何模型或生成链路比较都必须使用同一任务集并记录模型、设置、运行时间和完整结果。

## 2. 可复现命令

在 odyBox 仓库运行：

```bash
pnpm eval:canvas:s0
```

该命令校验：

- 12 个 case ID 唯一；
- 每个输入满足 `design_generate_artifact` 当前 `ui/name/description` 契约；
- delivery checks 和 quality dimensions 完整。

提供完成的结果 JSON 后汇总：

```bash
pnpm eval:canvas:s0 -- path/to/results.json
```

评测文件：

- `evals/canvas/s0-cases.json`
- `scripts/eval-canvas-s0.mjs`

## 3. 宿主链路测试基线

2026-09-06 执行以下范围：

- Design schema / Artifact message part；
- design generator 的 JSON、fence、错误和 abort 处理；
- `design_generate_artifact` 工具和持久化；
- ProjectStorage / ArtifactStorage；
- tool result → Artifact message injection；
- Artifact sandbox 与 CSP；
- UIRenderer / ArtifactRenderer / ArtifactCard；
- Project detail route。

结果：

```text
Test Files  14 passed (14)
Tests       84 passed (84)
```

首次执行时，本地 `node_modules` 缺少 `@odybox/core` workspace link，导致 9 个 suite 在 import 阶段失败。执行 `pnpm install --frozen-lockfile` 恢复 lockfile 对应依赖后，同一命令全部通过。该问题属于本地依赖状态，不是 Canvas 断言失败。

## 4. 静态闭环能力基线

| 能力 | S0 状态 | 证据/说明 |
|---|---|---|
| Project schema | 有 | `src/shared/types/design.ts` |
| Artifact schema | 有 | text/blob、kind/status、schemaVersion |
| Design tool 注册 | 有 | `design_generate_artifact` |
| 输入校验 | 有 | Zod schema |
| 模型输出解析与错误分类 | 有 | JSON/fence/parse/schema/model/abort |
| Artifact 持久化 | 有 | ProjectStorage / ArtifactStorage |
| Artifact 注入消息 | 有 | `artifact-injection.ts` |
| 隔离 iframe 渲染 | 有 | blob URL、CSP、`sandbox="allow-scripts"` |
| 完整会话/brief 进入内层生成 | 无 | tool options 的 `messages` 未进入 inner model messages |
| frontend-design 选择进入内层生成 | 无 | outer Skill 与 inner prompt 未形成显式契约 |
| Revision / 继续迭代 | 无 | Continue iteration 按钮禁用 |
| viewport / responsive 控件 | 无 | 固定高度 iframe，无状态矩阵 |
| screenshot 回看 | 无 | 生成器不消费渲染截图 |
| ElementRef / SourceRef | 无 | 无元素选择和源码映射协议 |
| runtime/a11y/视觉验收 gate | 无 | 无统一验证结果进入完成条件 |

静态闭环能力得分为 **8/15**。它只衡量链路结构，不表示视觉审美得分。

## 5. 固定任务集覆盖

12 个 case 覆盖：

- 营销首页：消费品牌、B2B 技术产品；
- 应用界面：金融 dashboard、权限、API Key；
- 表单与交易：医疗预约、电商商品、结账；
- 编辑与移动：艺术节节目页、移动习惯记录；
- 高密度和状态：事故队列、empty/error/recovery；
- 中文与英文、桌面与 390px 移动端；
- 键盘、焦点、reduced-motion、错误非颜色单通道等约束。

## 6. 评分规则

### Delivery success

一个 case 只有五项全部为 true 才算交付成功：

1. `modelResponse`
2. `schemaValid`
3. `persisted`
4. `rendered`
5. `noRuntimeErrors`

### Visual quality

每项使用 0–2 分：

- brief fidelity；
- visual hierarchy；
- typography；
- spacing and layout；
- responsive behavior；
- interaction states；
- accessibility；
- distinctiveness。

其中 0 表示缺失/实质破坏，1 表示存在但通用、不一致或不完整，2 表示有意图、连贯且达到可信产品水平。总质量率为实际得分除以理论满分。

## 7. 后续基线运行要求

每次真实运行必须记录：

- model/provider 和精确 model ID；
- temperature/top-p 等设置；
- commit SHA；
- case ID；
- 五项 delivery 布尔值；
- 八项 0–2 质量评分；
- 截图或 Artifact Revision 引用；
- 评分者和盲评顺序；
- 失败原因与是否重试。

未完成全部 12 cases 的结果，汇总脚本会拒绝计算，避免用挑选后的成功样本制造虚高基线。
