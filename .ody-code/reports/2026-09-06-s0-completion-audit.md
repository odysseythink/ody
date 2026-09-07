# 阶段 S0 完成审计：冻结边界与建立基线

**日期：** 2026-09-06  
**结论：** Complete  
**对应战略：** [Ody / odyBox / Canvas 长期产品与技术战略](./2026-09-06-ody-product-runtime-canvas-long-term-strategy.md)

## 1. S0 交付物

| S0 要求 | 状态 | 交付物/证据 |
|---|---|---|
| 正式记录“不创建第三个桌面 UI 产品” | 完成 | [ADR-S0-001](./2026-09-06-s0-product-boundary-adr.md) |
| Canvas 标记为 odyBox 实验性工作区 | 完成 | odyBox `CANVAS_PRODUCT_STATUS = 'experimental'`、项目页 `Preview` Badge、`docs/product/canvas.md` |
| 盘点重复能力 | 完成 | [能力权责矩阵](./2026-09-06-s0-capability-ownership-matrix.md) |
| 定义 Runtime capability parity 表 | 完成 | 同上；包含当前状态、长期权威、责任角色、阶段和删除条件 |
| 建立 Design 生成质量/成功率基线 | 完成 | [Canvas S0 基线](./2026-09-06-s0-canvas-baseline.md)、12-case manifest、汇总脚本、84 tests |
| 建立 OpenDesign 许可清单 | 完成 | [许可证台账](./2026-09-06-s0-opendesign-license-inventory.md) |

## 2. 代码与产品变更

odyBox：

- `src/shared/types/design.ts`：新增机器可读实验状态；
- `src/renderer/routes/design/$projectId.tsx`：设计项目显示 Preview；
- `src/renderer/routes/design/$projectId.test.tsx`：锁定 Preview 行为；
- `docs/product/canvas.md`：记录 Canvas 产品边界；
- `evals/canvas/s0-cases.json`：12 个固定真实任务；
- `scripts/eval-canvas-s0.mjs`：manifest/result 校验和指标汇总；
- `package.json`：新增 `pnpm eval:canvas:s0`。

Ody：

- 长期战略报告标记 S0 已完成；
- 新增正式 ADR、能力矩阵、基线、许可台账和本完成审计。

## 3. 验证结果

```text
pnpm eval:canvas:s0
status: manifest-valid
caseCount: 12

Vitest targeted Design/Artifact suite
Test Files: 14 passed (14)
Tests:      84 passed (84)
```

真实模型指标的 S0 值明确记录为 `N/A (n=0)`。这不是遗漏：当前没有指定固定 model/provider、运行额度和已保存输出，伪造一个百分比会破坏后续比较。S0 已完成评测协议、固定数据集和拒绝不完整结果的汇总工具；第一次真实模型运行可以直接产生可比较的 v1 数字。

## 4. 退出条件审计

### 每项能力都有长期权威

通过。矩阵逐项指定 Ody Runtime、odyBox Client、Canvas Client 或 Artifact Store，未把 OpenDesign 声明为业务权威。

### 每项能力都有迁移状态

通过。矩阵区分 Authority、Temporary、Client-only、Missing 和 Adapter，并为重复能力给出目标阶段/删除条件。

### 每项能力都有负责人

通过。责任按 Product、Runtime、odyBox、Canvas、Security/Release 角色分配。实际人员调整不能改变单一权威原则。

### 不再新增第三套实现

通过治理约束实现。ADR 要求新能力声明长期权威、临时实现和删除条件；第三个桌面产品必须由新 ADR 显式取代当前决策。

## 5. 留给 S1 的明确边界

以下事项没有偷跑进 S0，属于 S1：

- 将完整 brief/会话上下文送入内层生成器；
- 把 frontend-design 选择显式连接到 Artifact 生成；
- Artifact Revision 和继续迭代；
- Chat + Preview 分栏；
- viewport、刷新、截图和运行错误呈现；
- 使用 12-case 数据集执行指定模型的第一次真实视觉基线。

S1 开始前不需要重新讨论产品归属，但需要针对数据模型、生成上下文和 Revision 生命周期完成 Design。
