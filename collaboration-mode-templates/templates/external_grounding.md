## External grounding (all collaboration modes)

Before making a solution recommendation, architecture choice, implementation plan, or technology-selection claim, classify external research as **Required** or **Not required**.

External research is **Required** when the answer depends on current or version-sensitive facts; asks for industry practice, best practice, competitors, technology selection, or prior art; introduces a new third-party dependency or a standalone capability with no in-repo precedent; or commits substantial engineering time to a hard-to-reverse choice. It is normally **Not required** for a purely repository-local bug, mechanical refactor, or behavior-preserving change whose source of truth is fully present in the repository.

When research is required:

1. Use `WebSearch` (when available) for discovery, then use `WebFetch`, browser tools, an MCP connector, or an upstream repository checkout to read the original source. Search-result snippets are discovery hints, not evidence.
2. Prefer primary sources in this order: official documentation or standards, upstream source/release notes, then mature open-source implementations. Use secondary articles mainly to discover primary material or compare practitioner experience.
3. Verify version and publication/update date when they can change the decision. Do not invent URLs, quotes, benchmark results, or industry consensus.
4. Tie every cited source to the decision it supports or changes. If research tools are unavailable or a source cannot be read, state that limitation and label the affected claim as unverified instead of silently relying on model memory.

For a finalized Design or Plan artifact, include exactly one `## External Evidence` section with:

- `Status: Required` or `Status: Not required`;
- `Reason:` followed by a concrete task-specific explanation;
- when Required, a table with columns `Claim`, `Source`, `Version/date`, and `Decision impact`, containing at least two distinct external URLs and at least one primary source.

`Status: Not required` is valid only with a concrete repository-local reason. Placeholders such as `N/A`, `none`, `TBD`, or “not needed” without explanation do not satisfy the gate.
