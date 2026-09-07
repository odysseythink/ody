# S0：OpenDesign 源码与内置资源许可证台账

**日期：** 2026-09-06  
**扫描对象：** `/Users/ranwei/Downloads/open-design-v0.20.1/`  
**用途：** 约束后续 OpenDesign 代码、Skill、模板和示例的移植。

> 本台账是工程扫描记录，不构成法律意见。npm/pnpm transitive dependencies、远程字体、运行时下载资源和未附带许可证文件的素材仍需在实际引入时单独审查。

## 1. 扫描结论

仓库内共发现 **88 个**名为 `LICENSE*` / `NOTICE*` / `COPYING*` 的文件：

| 范围 | 文件数 | 许可证概况 |
|---|---:|---|
| 根目录 | 1 | Apache-2.0 |
| `apps/` | 1 | MIT（vendored dom-to-pptx） |
| `design-templates/` | 36 | MIT |
| `plugins/` | 29 | MIT |
| `skills/` | 20 | 18 MIT、2 Apache-2.0 |
| `tools/` | 1 | 7-Zip 组合许可说明 |
| **总计** | **88** | **3 Apache-2.0、84 MIT、1 7-Zip 许可** |

未发现根级 `NOTICE` 文件。不能因此假设未来版本或单独依赖不需要 NOTICE；每次升级必须重新扫描。

## 2. 根项目

| 路径 | 许可 | S0 决策 |
|---|---|---|
| `LICENSE` | Apache-2.0 | 可修改和分发；保留许可证、版权/NOTICE 信息并标记修改 |

根许可允许研究和复用，但子目录中明确附带的许可证必须随对应材料处理。

## 3. 应用内 vendored 代码

| 路径 | 许可 | 处理要求 |
|---|---|---|
| `apps/desktop/vendor/dom-to-pptx/LICENSE` | MIT | 移植 vendored 模块时保留版权与许可文本 |

## 4. Design templates

`design-templates/` 的 36 个许可文件均为 MIT，包括：

- `guizang-ppt`；
- `html-ppt`；
- `last30days` 及其 vendored `bird-search`；
- 全部 `html-ppt-zhangzara-*` 模板。

主要版权来源包括 op7418（歸藏）、Zara Zhang、lewis、Matt Van Horn 和 Peter Steinberger。若复制模板代码、版式资源或附带脚本，必须保留对应目录的具体版权文本，不能只附 OpenDesign 根 Apache-2.0。

模板内图片、字体、商标和示例内容可能存在许可证文件没有覆盖的权利。S0 将其标记为 **需按模板逐项素材审查**，不允许批量无差别打包进 odyBox。

## 5. Plugins

`plugins/` 的 29 个许可文件均为 MIT：

- `_official/examples/frontend-slides`；
- `fs-*` 示例；
- `hps-*` 示例；
- `huashu-*` 示例；
- `ve-*` 示例；
- `webgl-*` 示例；
- `community/hallmark`；
- `community/humanize-ppt`。

WebGL 示例的许可证正文包含原作者/Codrops 来源说明。移植效果代码时必须保留这些 attribution，不得仅记录“MIT”。

## 6. Skills

### Apache-2.0

| 路径 | 许可 |
|---|---|
| `skills/frontend-design/LICENSE.txt` | Apache-2.0 |
| `skills/hatch-pet/LICENSE.txt` | Apache-2.0 |

### MIT

其余 18 个 Skill 许可为 MIT：

- `brandkit`
- `brutalist-skill`
- `emil-design-eng`
- `gpt-tasteskill`
- `image-to-code-skill`
- `imagegen-frontend-mobile`
- `imagegen-frontend-web`
- `minimalist-skill`
- `output-skill`
- `redesign-skill`
- `review-animations`
- `soft-skill`
- `stitch-skill`
- `taste-skill-v1`
- `taste-skill`
- `web-clone`
- `web-design-guidelines`
- `writing-guidelines`

其中存在多位不同版权人。任何 Skill 正文或配套脚本的复制都必须随具体 Skill 保存原许可，不能使用一份合并的泛化声明替代。

## 7. 工具资源

| 路径 | 许可 | S0 决策 |
|---|---|---|
| `tools/pack/resources/win/7zip/License.txt` | 7-Zip 组合许可说明 | 不随 Canvas 代码默认移植；确需打包时由 Release/Security Owner 单独审查 |

## 8. 引入分级

| 等级 | 对象 | 当前决策 |
|---|---|---|
| A：优先研究/复刻行为 | iframe bridge、Element ID、inspect/comment、snapshot、lint 思路 | 不复制素材；可先按 Ody 协议独立实现 |
| B：可选择移植代码 | 边界清晰且价值明确的 Apache/MIT 模块 | 建立 source manifest、保留许可证、标记修改、补测试 |
| C：逐项审查 | Skills、templates、WebGL examples | 同时审查代码许可、字体、图片、商标和来源 attribution |
| D：默认不引入 | 整个 `apps/web`、daemon 强耦合层、7-Zip 打包资源 | 除非新的 Design/ADR 证明必要性 |

## 9. 每次引入必须记录

```text
上游路径：
上游版本/commit：
许可证：
版权人：
引入文件：
修改摘要：
是否含字体/图片/商标/数据：
odyBox/Ody 目标路径：
保留的 LICENSE/NOTICE 路径：
责任角色：
```

缺少任一项时不得合入 OpenDesign 来源代码或资源。
