## Out-of-scope / false-positive discipline

In addition to the source-grounding mandate above, every rigor-tier plan MUST include an explicit `## Out-of-scope` section that lists name-matches which LOOK like the target concept but are a DIFFERENT concept. This prevents over-deletion.

## Mandatory Out-of-scope section

Before finalizing the plan, create a `## Out-of-scope` section with:

1. A heading `## Out-of-scope`.
2. For every name-match that is NOT the same concept as the target:
   - The exact symbol, path, or field name.
   - A one-line reason explaining WHY it is a different concept.
   - A note on whether it needs no action, a rename, or explicit preservation.

## Name-matching is not concept-matching

A name that contains the target word is NOT automatically in scope. Verify the actual semantics before deciding. Examples from verified prior work:

- `windows-sandbox-rs` OS-level "account" constructs are operating-system identities, not application user accounts. Leave them untouched.
- `ext/goal/accounting.rs` token-balance accounting is a domain term unrelated to a user account model. Leave it untouched.
- An external schema field named `creator_account_user_id` belongs to an external system and is referenced, not owned, by this codebase. Preserve the reference; do not delete it as part of removing the internal account model.

If you are unsure whether a hit is the same concept, open the file and inspect the surrounding code. When in doubt, list the item in `## Out-of-scope` with the uncertainty noted.
