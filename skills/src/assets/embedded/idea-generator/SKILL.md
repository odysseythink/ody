---
type: inline
name: idea-generator
description: >
  Systematically generate startup idea candidates using 7 proven recipes adapted for the user's
  specific context (skills, industry exposure, connections, assets). Use this skill whenever the user
  is stuck in "don't know what to build" mode, says "I need ideas", "what should I work on",
  "help me brainstorm directions", "I'm not finding anything", or has been in open exploration for
  2+ weeks without locking a direction. Also trigger when the user finishes a round of idea-evaluator
  scoring and all candidates scored ≤4, indicating the current idea pool is exhausted and needs
  replenishment. This skill outputs a ranked list of concrete idea candidates ready to be fed into
  idea-evaluator. It is a DIVERGENT tool (expand options) that pairs with idea-evaluator (CONVERGENT
  tool, narrow options). Do not use this skill if the user already has a clear direction — use
  idea-evaluator instead.
hiddenInModes:
  - plan
  - design
  - office-hours
  - game-design
---

# SKILL: Idea Generator (想法发生器) v1

## Position in Decision Flow

```
没方向 → [Idea Generator 想法发生器] → 候选清单
                                        ↓
                                   [Idea Evaluator 想法质检] → 评分≥6 → 六模式审评
```

This skill answers: **"What directions should I even be looking at?"**
Idea Evaluator answers: "Is this specific idea any good?"

---

## Pre-Generation: Context Inventory (上下文盘点)

Before running the 7 recipes, inventory the user's available inputs. Each input is fuel for idea generation — the more specific, the better the output.

```yaml
inventory_checklist:
  skills:
    - Core technical skills (languages, frameworks, domains)
    - Non-technical skills (sales, writing, domain knowledge, languages spoken)
    - Rare skill combinations (e.g., "Go + quantitative finance + AI agents")

  industry_exposure:
    - Current job industry and daily workflow observations
    - Previous job industries
    - Outsourcing/freelance client industries
    - Industries of close friends/family

  pain_points_observed:
    - Problems you personally experience repeatedly
    - Problems you've seen colleagues/clients struggle with
    - Complaints you've heard from specific people (not abstract "market needs")
    - Broken workflows you've witnessed firsthand

  assets:
    - Existing code/products (even frozen ones)
    - Existing user base (even small)
    - Domain knowledge accumulated
    - Relationships/access to specific user groups

  recent_changes:
    - New technologies that just became viable
    - Regulatory changes affecting industries you know
    - Behavioral shifts you've noticed (post-COVID, AI adoption, etc.)
    - Industry disruptions in your orbit
```

**Output**: A filled context card. This card is referenced by every recipe below.

---

## The 7 Recipes

### Recipe 1: Start From Your Own Problems (从自身痛点出发)

```yaml
method: >
  List every friction/annoyance/inefficiency you personally encounter
  in a typical week — at work, at home, managing finances, learning,
  communicating, building software, managing projects.

prompt_questions:
  - "What did you do this week that felt unnecessarily painful or slow?"
  - "What task do you keep postponing because it's annoying?"
  - "What manual process do you repeat that should be automated?"
  - "What tool do you use daily that frustrates you?"

quality_filter: >
  Best results come from problems you experience as a PROFESSIONAL,
  not as a consumer. Consumer problems attract massive competition.
  Professional/B2B problems are often invisible to outsiders.

output: List of [problem + who else has it + current workaround]
```

### Recipe 2: Start From What You're Uniquely Good At (从独特能力出发)

```yaml
method: >
  Identify your rare skill combinations. A single skill (Go, AI, frontend)
  is common. Two skills combined (Go + AI) is less common. Three skills
  combined (Go + AI + quantitative trading infrastructure) is rare.
  Look for problems that sit at the intersection of your rare combination.

prompt_questions:
  - "What can you build in a weekend that would take most teams a month?"
  - "What do people ask you for help with that they can't easily get elsewhere?"
  - "What technical problem have you solved that most engineers haven't faced?"

⚠️ SISP guard: >
  This recipe is the highest-risk for SISP (Bug 5). The output must be
  [capability → problem it solves → who has that problem], NOT
  [capability → cool thing I could build]. If you can't name a specific
  person with the problem, the idea is SISP. Flag it and move on.

output: List of [rare capability intersection + specific problem it solves + for whom]
```

### Recipe 3: Start From Changes You've Noticed (从你观察到的变化出发)

```yaml
method: >
  What has recently changed in technology, regulation, behavior, or
  industry structure that creates a new gap? The best startup timing
  comes from riding a wave that just started — not one that's peaking.

prompt_questions:
  - "What became possible in the last 12 months that wasn't before?"
  - "What industry is being forced to change by new regulation or technology?"
  - "What behavior shift have you observed in people around you?"
  - "What used to be expensive/hard that is now cheap/easy?"

timing_check: >
  Cross-ref with Model 06 (Contrarian Timing). If "everyone" already
  knows about this change, you're late. Look for changes that are
  obvious to insiders but invisible to outsiders.

output: List of [change + new gap created + who is affected + timing assessment]
```

### Recipe 4: Start From Industries You're Inside (从你接触的行业出发)

```yaml
method: >
  Your current job, freelance clients, and personal network give you
  insider access to specific industries. Most outsiders can't see the
  real problems inside these industries. This is your information
  advantage — use it.

prompt_questions:
  - "What do people in your industry complain about at lunch?"
  - "What process at your company is shockingly manual or outdated?"
  - "What data exists in your industry that no one is using well?"
  - "What do your outsourcing clients keep asking for that isn't a product yet?"
  - "What would make your current job 10x easier?"

⚠️ schlep_check: >
  Industry problems often involve schlep — messy integrations, legacy
  systems, regulatory compliance, relationship-dependent sales.
  If you feel yourself dismissing an idea because "it's too industry-specific"
  or "not techy enough", that's Bug 6 (Schlep Blindness). Flag it
  and force yourself to score it anyway.

output: List of [industry + specific problem + who owns budget for this + how they cope now]
```

### Recipe 5: Start From Recent Tech Breakthroughs (从技术突破出发)

```yaml
method: >
  A new technology capability (LLMs, AI agents, multimodal models,
  cheap inference, voice AI, etc.) has just made something possible
  that wasn't before. What specific workflow can now be 10x better?

prompt_questions:
  - "What task required a human expert last year that AI can now handle?"
  - "What was too expensive to automate before but is now cheap?"
  - "What data was unstructured/unusable before but LLMs can now parse?"
  - "What manual QA/review/analysis process can now be automated?"

⚠️ SISP guard (strict): >
  This recipe has the HIGHEST SISP risk. The question is NOT
  "what can AI do?" but "what painful workflow can AI fix for
  a specific person who will pay?" If your answer starts with
  "AI can..." instead of "[Person X] struggles with...", stop.
  Reverse the direction.

⚠️ tar_pit guard: >
  "AI + [broad category]" is almost always a tar pit.
  "AI writing assistant", "AI study tool", "AI personal assistant"
  — these are graveyards. Be specific: AI for [specific role]
  doing [specific task] in [specific industry].

output: List of [new capability + specific workflow improved + who benefits + why now]
```

### Recipe 6: Talk to People (跟人聊)

```yaml
method: >
  Structured conversations with potential users — not friends, not
  other developers. People who do real work in real industries.
  The goal is NOT to pitch an idea. The goal is to discover problems.

conversation_template:
  1. "What's the most tedious part of your job?"
  2. "What tools do you use daily? What do you hate about them?"
  3. "If you could wave a magic wand and fix one thing, what would it be?"
  4. "How much time/money does [that problem] cost you per month?"
  5. "Have you tried to fix it? What happened?"

who_to_talk_to:
  - Your outsourcing clients (you already have access)
  - Colleagues at your current company
  - People in WeChat/Telegram industry groups
  - Friends who work in non-tech industries (they have unsolved problems tech people never see)

minimum_bar: >
  Talk to 5 people outside your immediate circle before concluding
  "there are no good ideas". If you haven't talked to 5, you haven't
  looked — you've just thought.

output: List of [person's role + their problem + severity + current solution + willingness to pay]
```

### Recipe 7: Find Incumbent Weaknesses (找巨头的弱点)

```yaml
method: >
  Large companies and established products always have blind spots.
  They can't serve small niches profitably. They can't move fast.
  They can't customize. They can't care about individual users.
  Find a specific segment they're underserving.

prompt_questions:
  - "What large software product do people in [industry] use reluctantly?"
  - "What's the most common complaint about [dominant tool]?"
  - "Who is being overcharged by [incumbent] relative to the value they get?"
  - "What user segment is too small for [big company] to care about?"

quality_signal: >
  The best answers come from actual user complaints — app store reviews,
  Reddit threads, V2EX posts, industry forums. Not from your imagination.

output: List of [incumbent + their weak point + underserved segment + what "good enough" alternative looks like]
```

---

## Post-Generation: Candidate Assembly

After running relevant recipes (not all 7 are needed every time — pick 3-4 most relevant based on the context inventory), assemble candidates.

### Deduplication & Clustering

Group similar ideas together. Often multiple recipes point at the same underlying opportunity from different angles — that's a strong signal.

### Quick Viability Filter

Before sending to idea-evaluator, apply a 3-second gut check on each candidate:

```yaml
kill_if:
  - You can't describe who pays and why in one sentence
  - The only user you can imagine is "everyone"
  - You feel excited about the TECHNOLOGY but bored by the USER PROBLEM
  - You've seen this exact idea fail 3+ times (tar pit)

keep_if:
  - You can name a specific person who has this problem
  - The idea makes you slightly uncomfortable (schlep signal)
  - You can see how to test it in <48 hours
  - Multiple recipes converged on this direction
```

---

## Output Format

```
## 想法生成报告 (Idea Generation Report)

### 上下文盘点
- 技能栈: [summary]
- 行业接触: [summary]
- 已观察痛点: [summary]
- 已有资产: [summary]
- 近期变化: [summary]

### 使用的Recipes
[List which recipes were run and why]

### 候选清单

| # | 想法 | 来源Recipe | 目标用户 | 核心痛点 | Schlep指数 | 初筛 |
|---|------|-----------|---------|---------|-----------|------|
| 1 | [idea] | R[X] | [who] | [pain] | 高/中/低 | ✅/❌ |
| 2 | ... | ... | ... | ... | ... | ... |

### 收敛信号
[哪些方向被多个recipe指向？这些是最值得优先评估的]

### 下一步
对初筛通过的候选，逐个运行 idea-evaluator 想法质检。
建议优先评估: #[X] 和 #[Y]（理由）
```

---

## Usage Notes

- **不需要每次跑全部7个recipe**。根据上下文盘点结果，选3-4个最有燃料的recipe跑。
- **Recipe 4（行业内部）和 Recipe 6（跟人聊）产出质量通常最高**，因为它们基于真实信息而非推测。
- **Recipe 2 和 Recipe 5 的SISP风险最高**，每个产出都要过SISP检测。
- **Schlep指数高的想法不是坏想法**——恰恰相反，它们往往竞争更少。标注它是为了让你正视它，而不是过滤它。
- **如果跑完所有recipe产出为零**，问题不是"没有好想法"，而是你的观察输入不够。回到Recipe 6，跟5个真实用户聊。
- **每轮生成建议间隔2周**。连续生成想法不会提高质量——中间需要新的信息输入（工作观察、用户对话、行业动态）才能产出不同的候选。

## Final step
After completing the analysis, call the tool `SaveIdeaReport` with:
- `title`: a concise, filesystem-safe title for the idea
- `content`: the full Markdown report content (include `#` headings, problem, alternatives, recommendation, next steps)
- `type`: `"generator"` (required — must be exactly `"generator"`)

Do not write the file directly; always use `SaveIdeaReport` so the output is validated and stored under `.ody-code/ideas/`.
