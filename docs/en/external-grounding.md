# External grounding for recommendations

Ody injects one shared external-grounding contract into Default, Plan, and Design modes. The model must classify external research before making a solution recommendation or technology choice.

Research is required for current/version-sensitive claims, industry or competitor comparisons, new third-party choices, standalone capabilities without repository precedent, and costly hard-to-reverse decisions. Purely repository-local fixes and behavior-preserving refactors can declare research not required when the checked-in code and tests are the complete source of truth.

## Evidence loop

When `[services.webSearch]` is configured, Ody exposes two tools:

1. `WebSearch` discovers candidate sources and returns structured search metadata.
2. `WebFetch` reads a selected original HTTP(S) page. Search snippets alone do not count as evidence.

Prefer official documentation, standards, upstream source, and release notes. Mature open-source implementations are useful for validating operational patterns; secondary articles are mainly discovery and comparison material.

`WebFetch` is guardian-approved unless approval policy is `never`. It blocks credentials and non-public network destinations, validates redirects, rejects binary content, and limits response bodies.

## Design and Plan completion gate

Final Design and Plan artifacts need one section in this form:

```markdown
## External Evidence
Status: Required
Reason: This choice depends on the current API and established upstream behavior.

| Claim | Source | Version/date | Decision impact |
|---|---|---|---|
| ... | https://official.example/docs | 2026-08 | ... |
| ... | https://github.com/example/project | v2.1 | ... |
```

When research is genuinely unnecessary, use `Status: Not required` and give a concrete repository-local reason. Placeholder reasons do not pass finalization. The gate validates the declaration, explanation, table contract, and source count; the model remains responsible for source quality and for identifying at least one primary source.
