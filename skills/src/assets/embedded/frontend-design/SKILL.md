---
name: frontend-design
description: Guidance for distinctive, intentional visual design when building new UI or reshaping an existing one. Helps with aesthetic direction, typography, and making choices that don't read as templated defaults.
---

# Frontend Design

Approach this as the design lead at a small studio known for giving every client a visual identity that could not be mistaken for anyone else's. This client has already rejected proposals that felt templated, and is paying for a distinctive point of view: make deliberate, opinionated choices about palette, typography, and layout that are specific to this brief, and take one real aesthetic risk you can justify.

## Ground it in the subject

If the brief does not pin down what the product or subject is, pin it yourself before designing: name one concrete subject, its audience, and the page's single job, and state your choice. If there's any information in your memory about the human's preferences, context about what they're building, or designs you've made before – use that as a hint. The subject's own world, its materials, instruments, artifacts, and vernacular, is where distinctive choices come from. Build with the brief's real content and subject matter throughout.

Direction has an owner, and it is usually not you. If `.ody-code/frontend/design-system.md` records a pack as `[C:USER]` for this screen, that is the owner's answer — build inside it. Otherwise the next step belongs to `frontend-style` (nominate three identities, render them on the real page, ask once, then record which one won), and the principles below apply *after* that choice rather than instead of it. `[C:INFERRED]` does not count as chosen: it marks a pack applied when nobody could answer, so a page carrying one is still owed the question.

## Design principles

For web designs, the hero is a thesis. Open with the most characteristic thing in the subject's world, in whatever form makes sense for it: a headline, an image, an animation, a live demo, an interactive moment. Be deliberate with your choice: a big number with a small label, supporting stats, and a gradient accent is the template answer, only use if that's truly the best option. When that thing is a photograph, `references/imagery.md` covers where to source it, the license record it needs, the markup that keeps the layout from shifting, and the honest path when no usable image can be found.

Typography carries the personality of the page. Pair the display and body faces deliberately, not the same families you would reach for on any other project, and set a clear type scale with intentional weights, widths, and spacing. Make the type treatment itself a memorable part of the design, not a neutral delivery vehicle for the content.

Structure is information. Structural devices, numbering, eyebrows, dividers, labels, should encode something true about the content, not decorate it. Many generic designs use numbered markers (01 / 02 / 03), but that's only appropriate if the content actually is a sequence - like a real process or a typed timeline where order carries information the reader needs. Question if choices like numbered markers actually make sense before incorporating them.

Use non-user-triggered motion sparingly and deliberately, only to draw attention: a single orchestrated moment — one page-load sequence or one reveal — lands harder than scattered effects. Fade-and-slide-up entrances on every section and decorative hover transitions on every card are the generic default and read as AI-generated. Motion that answers a person's action (opening, expanding, confirming, pressing) is different: it is welcome whenever it shows what changed, and the interaction baseline below is part of the quality floor, not decoration.

Match complexity to the vision. Maximalist directions need elaborate execution; minimal directions need precision in spacing, type, and detail. Elegance is executing the chosen vision well.

Consider written content carefully. Often a design brief may not contain real content, and it's up to you to come up with copy. Copy can make a design feel as templated as the design itself. See the below section on writing for more guidance.

## Interaction baseline: nothing clickable feels dead

The quality floor is not static. Every interactive element responds to hover and press — this feedback is hygiene, not decoration, and does not count against your one bold move. The restraint above applies to gratuitous decoration (parallax, particles, scattered entrances), never to feedback.

- Buttons and links: hover color/background shift; press feedback `transform: scale(0.97)` on `:active`, `transition: transform 100–160ms ease-out`. `cursor: pointer` on anything clickable.
- Clickable cards, list rows, data entries: a hover state that matches the design (background tint, border accent, or a 1–2px lift with a softening shadow), 150–250ms ease-out.
- Entrances start from `scale(0.9–0.97)` + `opacity: 0` — never `scale(0)`. Group entrances stagger 30–80ms.
- Animate only `transform` and `opacity`; `transition: all` is banned. Gate `:hover` motion behind `@media (hover: hover) and (pointer: fine)`; under `prefers-reduced-motion` keep opacity/color feedback, drop movement.

For anything beyond this baseline — orchestrated hero sequences, scroll-triggered reveals, springs, drag — load the `motion-design` skill before writing that code and use its exact values.

## Process: brainstorm, explore, plan, critique, build, critique again

For calibration: AI-generated design right now clusters around three looks: (1) a warm cream background (near #F4F1EA) with a high-contrast serif display and a terracotta accent; (2) a near-black background with a single bright acid-green or vermilion accent; (3) a broadsheet-style layout with hairline rules, zero border-radius, and dense newspaper-like columns. All three are legitimate for some briefs, but they are defaults rather than choices, and they appear regardless of subject. Where the brief pins down a visual direction, follow it exactly — the brief's own words always win, including when it asks for one of these looks. Where it leaves an axis free, don't spend that freedom on one of these defaults.

Work in two passes. First, brainstorm a short design plan based on the human's design brief: create a compact token system with color, type, layout, and signature. Color: describe the palette as 4–6 named hex values. Type: the typefaces for 2+ roles (a characterful display face that's used with restraint, a complementary body face, and a utility face for captions or data if needed). Layout: a layout concept, using one-sentence prose descriptions and ASCII wireframes to ideate and compare. Signature: the single unique element this page will be remembered by that embodies the brief in an appropriate way.

Then review that plan against the brief before building: if any part of it reads like the generic default you would produce for any similar page rather than a choice made for this specific brief, revise that part. Only after confirming the relative uniqueness of the design plan should you write the code, following the revised plan exactly and deriving every color and type decision from it.

When writing the code, be careful of CSS selector specificity. It is easy to generate classes that cancel each other out, especially around section spacing and element-level rules. Keep the cascade intentional.

Do planning and iteration internally, and only show ideas to the user when there is higher confidence they will delight them.

## Restraint and self-critique

Spend your boldness in one place. Let the signature element be the one memorable thing, keep everything around it quiet and disciplined, and cut any decoration that does not serve the brief. Not taking a risk can be a risk itself. Build to a quality floor without announcing it: responsive down to mobile, visible keyboard focus, reduced motion respected. Critique your own work as you build. Before finishing, look again and remove one accessory.

## More on writing in design

Words appear in a design for one reason: to make it easier to understand, and therefore easier to use. They are design material, not decoration. Bring the same intentionality to copy that you bring to spacing and color. Before writing anything, ask what the design needs to say and how it can best help the person navigate the experience.

Write from the end user's side of the screen. Name things by what people control and recognize, never by how the system is built. A person manages notifications, not webhook config. Describe what something does in plain terms rather than selling it. Being specific is always better than being clever.

Use active voice by default. A control should say exactly what happens when it is used: "Save changes," not "Submit." An action keeps the same name through the whole flow, so a button that says "Publish" produces a toast that says "Published." Cohesion and consistency are how people learn their way around.

Treat failure and emptiness as moments for direction, not mood. Explain what went wrong and how to fix it in the interface's voice. Errors are never vague. An empty screen is an invitation to act.

Keep the register conversational and tuned: plain verbs, sentence case, no filler, with tone matched to the brand and audience. Let each element do exactly one job.

---

Sources: adapted from [anthropics/skills](https://github.com/anthropics/skills) `frontend-design` (Apache-2.0) @ `41bbe19d`; interaction-baseline values distilled under MIT from [emilkowalski/skills](https://github.com/emilkowalski/skills) @ `2d228630`. See `THIRD_PARTY.md` in this repository's skills crate.
