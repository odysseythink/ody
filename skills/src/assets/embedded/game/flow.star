# /game — 单入口游戏开发助手：triage 后按 A/B/C 三轨道执行。
# 只用已验证的 Starlark 构造：def 内局部变量、单参数渲染函数（pipeline 每项自带上下文）、
# 单行 f-string、"\n".join 拼长 prompt、json.decode/encode。

TRIAGE_SCHEMA = {
    "type": "object",
    "required": ["track", "brief", "reason"],
    "properties": {
        "track": {"type": "string", "enum": ["A", "B", "C"]},
        "brief": {"type": "string"},
        "reason": {"type": "string"},
    },
}

CONCEPT_SCHEMA = {
    "type": "object",
    "required": ["pitch", "coreLoop", "input", "winLose", "fun"],
    "properties": {
        "pitch": {"type": "string"},
        "coreLoop": {"type": "string"},
        "input": {"type": "string"},
        "winLose": {"type": "string"},
        "fun": {"type": "string"},
    },
}

GDD_SCHEMA = {
    "type": "object",
    "required": ["mechanics", "constraints", "feedback"],
    "properties": {
        "mechanics": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["name", "spec"],
                "properties": {"name": {"type": "string"}, "spec": {"type": "string"}},
            },
        },
        "constraints": {"type": "array", "items": {"type": "string"}},
        "feedback": {"type": "array", "items": {"type": "string"}},
    },
}

PLAN_SCHEMA = {
    "type": "object",
    "required": ["stack", "files", "run"],
    "properties": {
        "stack": {"type": "string"},
        "files": {"type": "array", "items": {"type": "string"}},
        "run": {"type": "string"},
    },
}

B_CONCEPT_SCHEMA = {
    "type": "object",
    "required": ["pitch", "coreLoop", "audience", "platforms", "monetization", "scope", "fun"],
    "properties": {
        "pitch": {"type": "string"},
        "coreLoop": {"type": "string"},
        "audience": {"type": "string"},
        "platforms": {"type": "array", "items": {"type": "string"}},
        "monetization": {"type": "string"},
        "scope": {"type": "string", "enum": ["small", "medium", "large"]},
        "fun": {"type": "string"},
    },
}

B_GDD_SCHEMA = {
    "type": "object",
    "required": ["mechanics", "systems", "contentScope", "constraints"],
    "properties": {
        "mechanics": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["name", "spec"],
                "properties": {"name": {"type": "string"}, "spec": {"type": "string"}},
            },
        },
        "systems": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["name", "spec"],
                "properties": {"name": {"type": "string"}, "spec": {"type": "string"}},
            },
        },
        "contentScope": {"type": "string"},
        "constraints": {"type": "array", "items": {"type": "string"}},
    },
}


def render_mechanic(item):
    # pipeline 渲染器：只读 item（纯函数，不调用 agent）。
    return "\n".join([
        f"实现游戏机制「{item['name']}」：{item['spec']}",
        f"技术方案：{item['stack']}；文件：{item['files']}；约束：{item['constraints']}",
        f"玩家反馈要求：{item['feedback']}",
        "若技术方案选了 Phaser 3：先用 skills.list + skills.read 读取 phaser-core 与 phaser-arcade-physics 技能再动手；",
        "选了 Three.js：读取 threejs-scene-setup（按需加 threejs-gltf-loading / threejs-materials-lighting）；",
        "选了 PixiJS：读取 pixijs-rendering。",
        "动效与手感规范（必须遵守；细节用 skills.list 找到 motion-design 包后 skills.read 读取其 SKILL.md）：",
        "- 入场/出场 ease-out：cubic-bezier(0.23,1,0.32,1)；UI 动效 ≤300ms",
        "- 入场起始 scale(0.92)+opacity 0，禁止 scale(0)",
        "- 高频操作（每帧移动、连续输入）零装饰动画；按下即响应（100ms 内反馈）",
        "- 命中反馈：缩放脉冲 scale(0.95)→1 用 120ms；弹簧 {type:\"spring\",duration:0.5,bounce:0}，",
        "  仅投掷/拖拽类交互可用 bounce 0.2；预设见本技能 references/motion-presets.md",
        "输出实现摘要：改了哪些文件、该机制如何手动验证。",
    ])


def render_passthrough(item):
    # pipeline 每项即完整 prompt，用于并行执行两个不同的长 prompt。
    return item


def triage(text):
    phase("triage · 意图识别")
    prompt = "\n".join([
        "你是游戏开发助手的意图分类器。把用户输入分类到三条轨道之一：",
        "- A 快速原型：想立刻玩到一个小游戏/demo/原型；一句话游戏点子默认归此类",
        "- B 商业项目：认真/正式/长期做游戏，提到商业化、上线、赚钱、团队、选型、架构、执行计划；",
        "  或要求完整 GDD",
        "- C 实施辅助：已有游戏项目或代码，问具体实现、调试、优化、引擎用法；",
        "  出现「我的项目/现有/这个错误/怎么实现」默认归此类",
        "拿不准时：一句话点子 → A；出现「商业/正式/长期/选型/架构/上线」任一词 → B；",
        "提到已有项目或具体技术问题 → C。",
        "输出 track（A/B/C）、brief（用户意图的一句话重述，保留所有关键信息）、reason（分类理由）。",
        f"用户输入：「{text}」",
    ])
    return json.decode(agent(prompt, schema = TRIAGE_SCHEMA))


def track_a(brief):
    phase("A1 · 概念收敛")
    concept_prompt = "\n".join([
        f"你是游戏策划。把主题「{brief}」收敛成一个 30 秒内可验证的最小 Web 游戏概念：",
        "一句话玩法、单一核心乐趣、一种主要输入、明确的胜负或结束条件。",
        "主题过大就砍到最小可玩闭环；拒绝空泛设定。",
    ])
    concept = json.decode(agent(concept_prompt, schema = CONCEPT_SCHEMA))

    phase("A2 · GDD")
    gdd_prompt = "\n".join([
        "基于概念生成游戏设计文档（GDD），JSON 输出。",
        f"概念：{concept['pitch']}",
        f"核心循环：{concept['coreLoop']}；输入：{concept['input']}",
        f"胜负：{concept['winLose']}；乐趣点：{concept['fun']}",
        "要求：",
        "- mechanics 拆成 3-5 个可独立实现、可独立验证的机制，每项含 name 与 spec",
        "  （spec 写明行为、参数、与其他机制的关系）",
        "- constraints 为技术约束：单文件优先、无外部素材、Web 平台、60fps 性能预算",
        "- feedback 列出每个玩家操作对应的视听反馈（命中、得分、失败、连击等）",
    ])
    gdd = json.decode(agent(gdd_prompt, schema = GDD_SCHEMA))

    phase("A3 · 技术选型")
    plan_prompt = "\n".join([
        f"依据约束 {json.encode(gdd['constraints'])} 为游戏选择 Web 技术方案，JSON 输出：",
        "- stack：一句话方案加理由。选择规则：UI 轻量/文字/卡片类 → 纯 DOM+CSS；",
        "  精灵/碰撞/粒子/大量运动对象 → Canvas 2D；瓦片地图/关卡/平台物理 → Phaser 3；",
        "  真 3D → Three.js。原型默认选最简单的可行项。",
        "- files：文件清单（默认单 index.html，复杂时拆 main.js / style.css）",
        "- run：本地运行方式（如 python3 -m http.server 或 npx serve）",
        "完整决策表见本技能 references/tech-selection.md",
        "（系统技能目录内，可用文件读取工具查看）。",
    ])
    plan = json.decode(agent(plan_prompt, schema = PLAN_SCHEMA))

    phase("A4 · 按机制并行实现")
    items = []
    for m in gdd["mechanics"]:
        items.append({
            "name": m["name"],
            "spec": m["spec"],
            "stack": plan["stack"],
            "files": json.encode(plan["files"]),
            "constraints": json.encode(gdd["constraints"]),
            "feedback": json.encode(gdd["feedback"]),
        })
    implementations = pipeline(items, render_mechanic)

    phase("A5 · 试玩与审查")
    playtest_prompt = "\n".join([
        f"试玩验证。按 {plan['run']} 启动本地静态服务，用 browser-control 工具：",
        "navigate 打开游戏（loopback 自动豁免审批）、screenshot 检查渲染、evaluate 模拟按键/点击。",
        "逐项检查：画面渲染正常、控制台无报错、输入有响应、胜负路径可达、无肉眼卡顿。",
        "若 browser-control 不可用，改为静态审查并在结论中说明如何手动验证。",
        "输出：阻塞性问题清单（无问题则明确写「通过」）。",
    ])
    review_prompt = "\n".join([
        f"代码审查 {json.encode(implementations)}：",
        "- 边界情况：极值输入、连续快速操作、同时多键",
        f"- 对照反馈要求 {json.encode(gdd['feedback'])}：每个玩家操作是否都有对应视听反馈",
        f"- 约束符合度 {json.encode(gdd['constraints'])}：单文件、无外部素材、仅 transform/opacity 动画",
        "输出：按严重度排序的问题清单。",
    ])
    review = pipeline([playtest_prompt, review_prompt], render_passthrough)

    return {
        "track": "A",
        "concept": concept,
        "gdd": gdd,
        "plan": plan,
        "implementations": implementations,
        "review": review,
        "nextSteps": [
            f"试玩：{plan['run']}，然后浏览器打开对应地址",
            "想打磨某个机制或加新机制：直接说，/game 会按轨道 C 处理",
            "想把原型升级成正式项目：对我说「把它做成正式项目」，/game 会走商业档产出架构选型报告",
        ],
    }


def track_b(brief):
    phase("B1 · 项目定位")
    concept_prompt = "\n".join([
        "你是资深游戏制作人。把用户的游戏想法收敛成一个可商业化的项目定位（JSON）：",
        f"「{brief}」",
        "- pitch：一句话卖点（面向玩家，不是功能罗列）",
        "- coreLoop：核心循环（30 秒-5 分钟一局）",
        "- audience：目标玩家画像（谁、在什么设备上玩、为此付钱的同类游戏）",
        "- platforms：目标平台（如 [\"微信小游戏\", \"iOS\", \"Android\", \"Steam\"]）",
        "- monetization：商业化模式（买断/内购/广告变现/订阅，主选+理由）",
        "- scope：small（1-3 人月）/ medium（3-12 人月）/ large（12+ 人月）",
        "- fun：单一核心乐趣",
        "想法过大时明确砍出第一期范围，并在 pitch 中体现。",
    ])
    concept = json.decode(agent(concept_prompt, schema = B_CONCEPT_SCHEMA))

    phase("B2 · 商业 GDD")
    gdd_prompt = "\n".join([
        "基于项目定位生成商业档 GDD（JSON）。",
        f"定位：{json.encode(concept)}",
        "要求：",
        "- mechanics：3-7 个核心机制，每项含 name 与 spec（行为、参数、机制间关系、数值基调）",
        "- systems：支撑系统清单（存档/经济/匹配/成长线/运营活动等），每项含 name 与 spec",
        "- contentScope：内容规模估算（关卡数/卡牌数/英雄数/剧情量，按 scope 给出第一期砍法）",
        "- constraints：技术与合规约束（目标平台限制、包体、性能预算、版号/备案要求）",
    ])
    gdd = json.decode(agent(gdd_prompt, schema = B_GDD_SCHEMA))

    phase("B3 · 架构选型")
    arch_prompt = "\n".join([
        "你是游戏技术总监。为以下项目做架构选型，输出一份中文选型报告（markdown，≤120 行）。",
        f"项目定位：{json.encode(concept)}",
        f"GDD：{json.encode(gdd)}",
        "先用 skills.list 找到 game 技能，skills.read 读取其 SKILL.md，",
        "再用文件读取工具查看系统技能目录下 game/references/architecture-selection.md 的决策框架；",
        "读不到就按你自身的引擎与后端知识评估，并在报告中说明。",
        "报告必须包含：",
        "1. topology：C/S 拓扑（T0 纯客户端 / T1 客户端+轻量后端 / T2 权威服务器实时联机 / T3 MMO）及理由",
        "2. engine：主选引擎（候选：Cocos Creator / Unity / Godot / Unreal / three.js / 纯前端）及理由",
        "3. alternatives：1-2 个备选及放弃原因",
        "4. backend：若 topology ≥ T1，给出后端方案（BaaS / 自建 / 房间制框架）",
        "5. risks：前 3 大风险与缓解",
        "6. milestones：3-5 个里程碑的高层拆分（每个一句话，供后续生成执行计划）",
        "最后明确列出「待确认决策点」清单（需要用户拍板的事项）。",
    ])
    report = agent(arch_prompt)

    return {
        "track": "B",
        "concept": concept,
        "gdd": gdd,
        "archReport": report,
        "nextSteps": [
            "请审阅选型报告的「待确认决策点」并拍板",
            "确认后回复「生成执行计划」：我会用 roadmap-architect 把里程碑拆成可执行计划",
            "对选型有异议：直接说你的倾向（如「我更想用 Godot」），我会重做对比",
        ],
    }


def track_c(brief):
    phase("C · 实施辅助")
    prompt = "\n".join([
        "你是游戏开发专家助手。",
        f"用户问题：「{brief}」",
        "严格按以下步骤：",
        "1. 用 skills.list 找到 gamedev-router 技能，skills.read 读取其 SKILL.md",
        "2. 按其路由算法：先检测当前工作目录的项目指纹确定引擎",
        "   （读不到项目文件就按用户点名的引擎；都没有就在回答开头说明假设），",
        "   再按问题措辞分类任务（学科/品类/工作流）",
        "3. 用 skills.read 读取路由表指定的最少技能集（不要全读）",
        "4. 基于加载的技能回答；技能与你的判断冲突时以技能为准并说明",
        "若 gamedev-router 不可用，直接用你的引擎知识回答并说明。",
    ])
    answer = agent(prompt)
    return {
        "track": "C",
        "answer": answer,
        "nextSteps": [
            "继续追问即可，/game 会继续按轨道 C 处理",
            "想把当前项目升级为正式商业规划：说「帮我做选型」，/game 会走轨道 B",
            "想验证某个新机制值不值得做：说「做个原型试试」，/game 会走轨道 A",
        ],
    }


def main():
    text = args.get("text", "")
    if text == "":
        return {
            "error": "缺少输入",
            "nextSteps": ["用法：/game <你的游戏想法、项目描述或具体问题>"],
        }
    t = triage(text)
    log(f"路由决定：轨道 {t['track']} — {t['reason']}")
    if t["track"] == "B":
        return track_b(t["brief"])
    elif t["track"] == "C":
        return track_c(t["brief"])
    else:
        return track_a(t["brief"])


result = main()
