# 游戏开发全流程辅助：调研、重叠分析与实施方案（2026-09-21）

## 背景与目标

用户确认现有 `game-create` 是"一句话 → Web 原型"的 5 阶段 flow，不是目标形态。真实需求：

1. 辅助设计（概念/GDD）
2. 技术栈选型（客户端/服务端架构；Cocos vs Unity vs three.js vs 纯前端 vs 桌面引擎）
3. 生成执行计划（接 ody 现有 roadmap-architect / executing-plans 体系）

最终目标：完成商业级游戏；若有更成熟工具可放弃自研。UX 要求：**单入口，按用户问题自动路由，完成后给出下一阶段建议**。

## 外部调研（一手来源）

### GitHub（通过 GitHub Search API + 仓库 README 直读）

| 项目 | Stars | 定位 | 与本方案的关系 |
|---|---|---|---|
| gamedev-skills/awesome-gamedev-agent-skills | 1079 | 73 个 SKILL.md + router，覆盖 Godot/Unity/UE/Phaser/PixiJS/three.js/Bevy/pygame/LÖVE/Roblox，按项目文件指纹识别引擎自动加载 | **已 vendor 子集**（见下） |
| PlayableIntelligence/game-creator | 331 | `/viral-game` 一键管线 + `/make-game` 多会话里程碑 ADR 工作流，Web 限定（Phaser/Three.js） | 流程设计参考；不引入 |
| bmad-code-org/bmad-module-game-dev-studio | 238 | 概念→GDD→叙事→机制的结构化多角色工作流（BMAD 方法） | 设计阶段结构参考；不复制内容 |
| echoo19/hearth | 56 | agent 原生游戏开发桌面应用 | 太早期，观望 |
| Thepizzapie/BuildersGate | 26 | 多工种（美术/玩法/叙事/QA/音频）并行 agent 座位 | 太早期，观望 |
| Coding-Solo/godot-mcp | 5762 | Godot MCP | 实施期"手"，按需配置 |
| AnkleBreaker-Studio/unity-mcp-server | 471 | Unity MCP（268 工具） | 同上 |
| DaxianLee/cocos-mcp-server | 1408 | Cocos Creator 3.8+ MCP（中文社区） | Cocos 轨道实施期的手 |
| ChiR24/Unreal_mcp | 880 | UE MCP（活跃；chongdashu/unreal-mcp 2081★ 已停更 2025-04） | 同上 |

### skills.yangsir.net（SkillForge）

「游戏开发」领域现有 98 个技能（https://skills.yangsir.net/domains/game-dev）。值得注意：`bmad-gds`（同 BMAD）、`gh-design-game`（游戏 UI/UX 审计→评分→改进清单）、`godot-master`（94 蓝图知识库）、`multiplayer-game`（rivet 出品，匹配/网络同步）、`ssh2-game-design-theory`。绝大多数是引擎级局部技能，**没有覆盖"架构选型 + 全流程"的产品**。

### 结论

没有现成产品完整覆盖"设计辅助 → 架构选型 → 执行计划"全链路；所有现有方案都假设引擎已选定。不存在值得放弃本工具的成熟替代品（商业游戏也无法被任何现有工具"完成"，均为辅助）。

## 重叠分析（awesome 73 技能 vs 现有 game-create + motion-design，逐项核对）

| awesome 侧 | 现有侧 | 判定 |
|---|---|---|
| `disciplines/game-feel`（玩法打击感，引擎中立） | `motion-design`（UI 动效）+ game-create `references/motion-presets.md` | 边界互补。motion-design 管 UI 动效，game-feel 管 gameplay juice；motion-presets.md 是唯一小重叠点（约 20 行 CSS 预设），保留并在注释中互相指向 |
| `workflows/prototype-fast`（单机制 keep/kill 验证） | `game-create`（完整小游戏） | 意图层互补；prototype-fast 是商业档立项前的验证环节 |
| `web-engines/phaser-*`/`threejs-*` | game-create tech-select（只选型） | 衔接不重叠；implement 阶段注入可提升原型质量 |
| engines×45 / genres×9 / disciplines 其余 / 发布工作流 | 无 | 零重叠，纯增量 |
| `router`（引擎指纹识别 + 任务分类 → 最小技能集） | 无 | 零重叠；正是"自动路由"的现成实现 |

**awesome 未覆盖（本方案自研增量）**：① 新项目架构选型（C/S 拓扑 + 引擎对比决策矩阵，它的 router 只识别已有项目）；② Cocos；③ 与 ody 执行计划体系衔接；④ 完成后给下一阶段建议的链路。

## 用户已确认的决策（2026-09-21）

1. awesome 引入方式：**(c) vendor 子集进 ody 仓库内置系统技能**
2. 入口形态：**`/game` 单入口**，内部三轨道路由
3. 商业档引擎矩阵：**纳入 Cocos**
4. 架构选型阶段：产出报告后**停下等用户确认**，再生成执行计划
5. 本调研**存档**（即本文件）

## Vendor 范围

来源：https://github.com/gamedev-skills/awesome-gamedev-agent-skills
Pin commit：`b105e1cf617adf0b68ed98790a716bbb60993179`（2026-09-21 浅克隆 HEAD）
许可证：Apache-2.0（合规登记见 `skills/THIRD_PARTY.md`）

- **引入（64 技能 + router）**：router（改名 `gamedev-router`，原名太泛）、godot×15、unity×8、unreal×6、web-engines×6、other-engines/bevy-ecs×1、disciplines×15、genres×9、workflows×4
- **排除（9 个）**：roblox×7（闭源平台生态，非目标）、pygame-core、love2d-core（非商业主力栈）。bevy 保留（Rust 生态，与用户技术栈同源）
- 全部为**逐字 vendoring**（非蒸馏），每个文件加来源注释头；产品策略按 game-create 先例标 `ody`

## `/game` 单入口设计

flow.yaml 不支持条件分支（`core-skills/src/flow.rs`：phases 顺序执行，step 仅 agent/pipeline/parallel），因此 `/game` 采用 **flow.star（Starlark）载体**获得真实 if/else：

```
/game <任意一句话>
  phase triage：agent 结构化分类（schema: track ∈ A/B/C + brief + reason）
  ├─ 轨道A 快速原型档：concept → gdd → tech-select（Web 四栈）→ 按机制并行 implement → 双 agent playtest
  │     （移植自 game-create/flow.yaml；implement 提示注入对应 awesome web 技能）
  ├─ 轨道B 商业档：concept → gdd → 架构选型（C/S 拓扑 + 引擎决策矩阵，含 Cocos）
  │     → 产出选型报告后**结束**，nextSteps 提示"确认后生成执行计划"
  │     → 用户确认后由主会话用 roadmap-architect 生成执行计划、executing-plans 断点续作
  └─ 轨道C 实施辅助：agent 通过 skills.list/skills.read 加载 gamedev-router，按其路由表加载
        对应引擎/学科/品类技能后直接回答（不跑管线）
每个轨道 result 均含结构化 nextSteps（下一阶段建议）
```

触发词约定（triage）：提到已有项目/引擎具体问题 → C；提到"商业/正式/长期/选型/架构/上线/赚钱" → B；其余默认 A。

`game-create` 保持不变（向后兼容），其 SKILL.md 后续可加一行指向 `/game`（本次未动）。

## 验证

- `cargo nextest run -p ody-skills`（嵌入技能安装 + 产品策略过滤测试会加载全部新技能，flow.star 语法由 core-skills loader 在加载时校验）
