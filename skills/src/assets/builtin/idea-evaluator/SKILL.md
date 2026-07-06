---
type: inline
name: idea-evaluator
description: >
  Evaluate the quality of a startup or product idea BEFORE committing to the six-model review.
  Use this skill whenever the user describes a new startup idea, product concept, or business direction
  they're considering — especially during the P1 open exploration phase. Also trigger when the user says
  "I have an idea", "what do you think of this direction", "should I explore X", "I noticed a problem
  with Y", or presents any concept that could become a product or business. This skill runs 10 evaluation
  questions, 4 mistake checks, 3 counter-intuitive signal checks, and outputs a structured quality score.
  The output feeds directly into the six-model review: ideas scoring ≥6/10 proceed to full review,
  ideas scoring 3-5 get specific improvement suggestions, ideas scoring ≤2 get a recommendation to
  drop or radically rethink. Always use this skill BEFORE the user starts building, designing architecture,
  or running the six-model review on a new idea. This is the first gate in the funnel.
hiddenInModes:
  - plan
  - design
  - office-hours
  - game-design
---

# SKILL: Idea Evaluator (想法质检) v1

## Position in Decision Flow

```
新想法 → [Idea Evaluator 想法质检] → 评分≥6 → 六模式审评 → 执行
                                   → 评分3-5 → 改进建议 → 重新评估
                                   → 评分≤2 → 建议放弃或彻底重构
```

This skill answers: **"Is this idea worth putting through the full review?"**
The six-model review answers: "Should I do it and how?"
MVP First answers: "How do I validate it cheaply?"

---

## Phase 1: Mistake Scan (4项排雷)

Before scoring, check whether the idea has any of these structural problems. Any hit = mandatory warning before proceeding.

### M1: SISP — 拿方案找问题 (Solution In Search of a Problem)

```yaml
detection:
  - User starts from technology ("I can use X to do Y")
  - User starts from capability ("I know how to build X, what can I do with it?")
  - The "problem" was found AFTER the solution was conceived
  - Cross-ref: Bug 5 SISP variant

warning: >
  ⚠️ SISP detected. You started from a technology/capability, not from a pain point.
  Reverse the question: Who is in pain? How bad? How are they coping now?
  If you can't answer those three without referencing your tech stack, this is SISP.
```

### M2: Tar Pit — 结构性死胡同

```yaml
detection:
  - Problem is real, solution sounds reasonable, but many have tried and failed
  - Classic tar pits: social plans apps, restaurant discovery, general AI assistants,
    "Uber for X" without network density, todo apps, podcast discovery
  - Signal: you can easily find 5+ dead startups that tried the same thing

action: >
  Search for predecessors. If 3+ similar attempts failed for similar structural reasons
  (not just bad execution), this is a tar pit. State the structural reason explicitly.
  User must explain why their approach avoids this specific structural trap.
```

### M3: Evaluation Laziness — 没做功课

```yaml
detection:
  - User cannot name a single competitor or existing alternative
  - User has not talked to any potential user
  - User says "I think people need this" with zero evidence

warning: >
  ⚠️ You haven't done basic homework. Before scoring this idea:
  1. Google "[your idea] + startup/app/tool" — what comes up?
  2. Name 3 people who have this problem. Have you talked to any of them?
  3. What do people currently use to solve this? (The answer is never "nothing")
```

### M4: Waiting for Perfect — 等待完美想法

```yaml
detection:
  - User has been exploring for weeks without committing to test any direction
  - User keeps generating new ideas but never validates any
  - User says "this one isn't good enough either"

warning: >
  ⚠️ You're in idea-shopping mode. Good ideas often don't look good at first.
  Airbnb sounded terrible. Stripe sounded boring. Pick the best of what you have
  and run a 48-hour validation test. A tested mediocre idea > an untested brilliant one.
```

---

## Phase 2: Ten-Question Scoring (10问评分)

Score each question 0/1/2. Total range: 0-20, normalized to 0-10 for final score.

### Q1: Founder-Market Fit (创始人-市场匹配)

```
2 = You have deep domain experience or are the target user yourself
1 = You have adjacent experience or strong technical fit
0 = No connection to this market/problem — you chose it because it "seems big"
```

### Q2: Market Size (市场规模)

```
2 = Large existing market OR small market growing rapidly
1 = Medium market with clear expansion path
0 = Niche with no growth path, or "huge market" you can't realistically capture
```

### Q3: Problem Acuteness (问题急迫度)

```
2 = Hair-on-fire problem — users actively spending money/time to solve it NOW
1 = Real annoyance — users complain but tolerate it
0 = Nice-to-have — users don't lose sleep over this
```

### Q4: Competitive Landscape (竞争格局)

```
2 = Competitors exist but you have a specific insight they're missing
1 = No direct competitors (could mean untapped OR no market)
0 = Strong incumbents with no clear differentiation for you
```

### Q5: Personal Desire (自己想用)

```
2 = You would be a daily/weekly user of this product
1 = You can see yourself using it occasionally
0 = You would never use this — you're building for a user you don't understand
```

### Q6: Timing — Recently Possible or Necessary (时机)

```
2 = New technology/regulation/behavior shift just made this possible or urgent
1 = Gradual trend moving in your favor
0 = Could have been built 5 years ago — why wasn't it? (Answer matters)
```

### Q7: Proxy Validation (有无参照物)

```
2 = Similar model proven in another market/geography (your version for your context)
1 = Partial proxy — some elements validated elsewhere
0 = Completely novel — no evidence this model works anywhere
```

### Q8: Long-term Commitment (长期投入意愿)

```
2 = Genuinely excited to work on this for 2+ years
1 = Willing to commit 6-12 months to see if it works
0 = Already bored thinking about it — just want quick money
```

### Q9: Scalability (可规模化)

```
2 = Clear path to serve 100x users with <10x effort (software/platform)
1 = Can scale with moderate effort increase (productized service)
0 = Revenue scales linearly with your time (pure consulting/freelance)
```

### Q10: Idea Space Quality (赛道质量)

```
2 = Rich idea space — even if this specific angle fails, adjacent pivots exist
1 = Some pivot room but limited
0 = Dead-end — if this specific implementation fails, nothing else to try
```

---

## Phase 3: Counter-Intuitive Signal Check (3个反直觉加分项)

These are POSITIVE signals that most people misread as negative. Each adds +1 to the raw score.

### S1: Hard to Get Started (启动困难)

```
+1 if: The idea requires significant schlep to get going — regulatory hurdles,
       complex integrations, cold-start problems, industry relationships needed.
Why positive: High barrier to entry = high barrier for competitors too.
Cross-ref: Bug 6 (Schlep Blindness) — if you're tempted to skip this idea
           BECAUSE of the schlep, that's exactly why it might be good.
```

### S2: Boring Space (无聊领域)

```
+1 if: The industry/problem sounds boring — no one brags about working on it.
       Insurance, compliance, logistics, procurement, HR, accounting, PCB manufacturing...
Why positive: Smart people avoid boring problems → less competition → more opportunity.
```

### S3: Existing Competitors (已有竞品)

```
+1 if: There are existing players but they're complacent, outdated, or have
       obvious gaps you can exploit.
Why positive: Competitors = validated market. Beating them is easier than
              creating a market from scratch.
```

---

## Phase 4: Schlep Blindness Reverse Check (Schlep盲区逆向检测)

After scoring, run this additional check:

```yaml
question: >
  Did you skip or dismiss any directions BEFORE arriving at this idea?
  If yes: what was the reason you dismissed them?

schlep_detection:
  If dismissed reasons include:
    - "Too much non-coding work"
    - "Would need to talk to too many people"
    - "Not technical enough"
    - "Too much industry-specific knowledge needed"
    - "Would need to do sales/BD"
  Then: ⚠️ Schlep blindness may have filtered out better ideas.
  Suggest: Re-evaluate dismissed directions using the "investor test" —
           "If someone else was doing this, would you invest?"
```

---

## Output Format

```
## 想法质检报告 (Idea Evaluation Report)

**想法概述**: [one sentence]

### 排雷扫描
- M1 SISP: ✅ Clear / ⚠️ Detected — [detail]
- M2 Tar Pit: ✅ Clear / ⚠️ Detected — [structural reason]
- M3 Homework: ✅ Done / ⚠️ Not done — [what's missing]
- M4 Idea Shopping: ✅ Clear / ⚠️ Detected

### 十问评分

| # | 维度 | 分数 | 依据 |
|---|------|------|------|
| Q1 | Founder-Market Fit | X/2 | [one line] |
| Q2 | Market Size | X/2 | [one line] |
| Q3 | Problem Acuteness | X/2 | [one line] |
| Q4 | Competition | X/2 | [one line] |
| Q5 | Personal Desire | X/2 | [one line] |
| Q6 | Timing | X/2 | [one line] |
| Q7 | Proxy | X/2 | [one line] |
| Q8 | Commitment | X/2 | [one line] |
| Q9 | Scalability | X/2 | [one line] |
| Q10 | Idea Space | X/2 | [one line] |

**基础分**: X/20

### 反直觉加分
- S1 启动困难: +1 / +0 — [reason]
- S2 无聊领域: +1 / +0 — [reason]
- S3 已有竞品: +1 / +0 — [reason]

**调整后总分**: (基础分 + 加分) / 2.3 = **X/10**

### Schlep盲区逆向检测
[结果]

### 判定

🟢 ≥6/10 — 值得进入六模式审评
🟡 3-5/10 — 需改进，具体建议：[list weak dimensions + how to improve]
🔴 ≤2/10 — 建议放弃或彻底重构方向

### 下一步
[Specific action based on score — either "proceed to six-model review"
 or "do X to improve before re-evaluating" or "drop and try next direction"]
```

---

## Usage Notes

- This skill is designed for rapid evaluation — a single idea should take 5-10 minutes to score.
- Multiple ideas can be scored in parallel for comparison. When comparing, present a summary table at the end.
- The score is a screening tool, not a verdict. A 4/10 idea with one fixable weakness might be better than a 7/10 idea you're not excited about.
- If the user is comparing multiple directions, run this on each and present a comparison matrix.
- After scoring ≥6, hand off to the six-model review (starting with Model 04 Fix the Roof).

## Final step
After completing the evaluation, call the tool `SaveIdeaReport` with:
- `title`: a concise, filesystem-safe title for the evaluation
- `content`: the full Markdown evaluation content (include `#` headings, criteria, scores, summary)
- `type`: `"evaluator"` (required — must be exactly `"evaluator"`)
- `score`: the final 0–10 score from the evaluation (optional but recommended)

Do not write the file directly; always use `SaveIdeaReport` so the output is validated and stored under `.ody-code/ideas/`.
