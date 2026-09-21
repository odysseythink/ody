---
name: game
description: "单入口游戏开发助手。/game 后跟任意一句话，flow 自动路由三条轨道：A 快速原型（概念→GDD→Web 选型→按机制并行实现→浏览器试玩）、B 商业项目（概念→商业 GDD→架构选型报告含 C/S 拓扑与引擎矩阵（Cocos/Unity/Godot/UE/three.js/纯前端），确认后接执行计划）、C 实施辅助（经 gamedev-router 加载引擎/学科/品类技能解答已有项目的具体问题）。Use for 做游戏/游戏原型/游戏选型/游戏架构/游戏开发问答/商业游戏规划。"
type: flow
---

# Game — 单入口游戏开发助手

`/game <任意一句话>`。flow runtime 先做意图分类（triage），再按轨道执行；每条轨道结束都在 flow result 里给出结构化 `nextSteps`（下一阶段建议）。

## 轨道

- **A 快速原型**：一句话点子 → 30 秒可玩 Web 原型。概念收敛 → GDD → Web 技术选型（DOM/Canvas/Phaser/Three）→ 每个机制一个 agent 并行实现 → browser-control 试玩 + GDD 对照审查。实现 Phaser/Three/Pixi 时会先 skills.read 对应的 vendored 引擎技能。
- **B 商业项目**：认真的游戏项目 → 项目定位（平台/受众/商业化/规模）→ 商业 GDD（机制+系统+内容规模+合规约束）→ 架构选型报告（C/S 拓扑 T0–T3 + 引擎决策矩阵，含 Cocos；决策框架见 `references/architecture-selection.md`）。**报告产出后流程停止**；用户在主会话确认选型后，由主会话用 roadmap-architect 生成执行计划、executing-plans 断点续作。
- **C 实施辅助**：已有项目/具体技术问题。agent 通过 skills.list + skills.read 加载 `gamedev-router`，严格按其路由算法（引擎指纹识别 → 任务分类 → 最小技能集）加载 vendored 技能后回答。

## 边界

- 编辑器实操（Unity/UE/Cocos/Godot 工程的真实改动）需要对应引擎的 MCP server，不在本技能范围内；未配置时轨道 B 止步于选型报告与执行计划。
- 轨道 A 是 game-create 的上层入口（提示词同源）；直接 `/game-create` 仍可用。
- 动效手感规范来自 `motion-design`（技能依赖，见 `agents/odysseythink.yaml`）；玩法级 juice 的引擎中立做法见 vendored `game-feel` 技能。
