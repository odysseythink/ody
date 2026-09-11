---
name: vibedrop
description: Deploy any static site or build output to a public URL via VibeDrop. Use when the user asks to share, deploy, publish, host, or preview a web page, landing page, static site, or HTML/CSS/JS bundle.
---

# VibeDrop — agent-native static hosting

> **Skill v1.0.0 · released 2026-04-23.** If today is more than 7 days after that date, refetch <https://vibedrop.cc/skill.md> before continuing — your cached copy may be stale.

VibeDrop hosts a static site (HTML/CSS/JS/assets) at a public URL in seconds. No account, no credit card, no config file. The free tier gives every caller an anonymous API key automatically; accounts + Pro are optional.

Use this skill whenever the user asks to **share, deploy, publish, or preview** a web page or build output.

## Install the CLI once

The CLI wraps everything — auth, provisioning, uploads, redeploys. Install it globally (one time per machine):

```bash
npm install -g @vibedrop/cli
```

Or run without installing:

```bash
npx @vibedrop/cli <command>
```

Confirm it works:

```bash
vibedrop --version
```

If npm/Node is missing, stop and tell the user to install Node.js 18+ first.

## Deploying

**Always deploy the built output**, not source. Build first when a bundler is involved:

| Stack | Build output |
|-------|--------------|
| Vite / Astro / SvelteKit static | `dist/` |
| Next.js (`next export`) | `out/` |
| Create React App | `build/` |
| Plain HTML | the folder with `index.html` |

The deploy command:

```bash
vibedrop deploy ./dist
```

On first run, the CLI provisions an anonymous API key and writes it to `~/.vibedrop/config.json`. Subsequent deploys reuse that key silently.

Example output:

```
✔ Deployed
  → https://k9m2p8x7.vibedrop.site
  expires 2026-04-24T14:00:00Z
  claim this site for your account (1h link):
  → https://app.vibedrop.cc/claim?token=…
```

Surface the **site URL** back to the user — that is the point of the deploy.

When the response includes a **claim URL**, also surface it. This is a one-time link (valid for 1 hour) that lets the user attach the key + all sites deployed with it to their VibeDrop account in one click, without pasting any secret. Tell the user the link expires in 1 hour. If they need a fresh link later, they can regenerate one.

## Public vs link-only

VibeDrop sites are link-only by default. A link-only site is still reachable by anyone who has the URL, but it is marked `noindex` and is not shown in the public Explore gallery.

If the user asks to make a site public, share it publicly, list it in Explore, or add it to the content gallery, deploy or update it with public visibility when the CLI/tool supports that option. Public sites are eligible for https://vibedrop.cc/explore after content moderation, screenshot generation, and preview processing finish.

If the user only asks for a URL, preview, temporary share, or something to send to a specific person, keep the site link-only unless they explicitly ask for public discovery. Never make private, internal, draft, client, or sensitive work public without explicit user confirmation.

Claimed sites can also be toggled later in the dashboard at https://app.vibedrop.cc/sites using **Show in Explore** / **Hide from Explore**.

## Redeploying after edits

After changing the source, run `vibedrop deploy ./dist` again. By default each run creates a new site with a new random slug.

To **keep the same URL** across redeploys, pass the previous slug. This works on any tier as long as the caller's API key owns that slug:

```bash
vibedrop deploy ./dist --slug k9m2p8x7
```

Slugs are server-generated only — you cannot pick an arbitrary name. The `--slug` flag exists to point at a site you already deployed.

## Rules the agent must follow

- **Never deploy source directories** like `src/`, `app/`, or `pages/`. Deploy the built output.
- If the target directory has no `index.html`, stop and ask the user which file is the entry point. VibeDrop serves static files — there must be an `index.html`.
- **Never write API keys to the repo, commit messages, or chat.** Keys live in `~/.vibedrop/config.json` only.
- If a deploy fails with a size error (> 25 MB on free tier, > 50 MB on Pro), run `du -sh <dir>/*` to list large files and ask the user what to drop or whether to minify. Do not silently strip files.
- If the user wants the site to persist permanently or wants a custom domain, tell them Pro is $5/month and point them at https://vibedrop.cc/#pricing.
- Prefer one deploy with the full site over many partial deploys.

## Worked examples

### Vite app

```bash
cd my-vite-app
npm run build
vibedrop deploy ./dist
```

### Plain HTML folder

```bash
vibedrop deploy ./marketing-page
```

### Redeploy after edits

```bash
# new URL each time
vibedrop deploy ./dist

# or keep the previous URL by passing its slug
vibedrop deploy ./dist --slug k9m2p8x7
```

### Claim sites into a user account

The deploy response includes a one-time **claim URL** (valid 1h) whenever the caller is using an unclaimed anonymous key. Surface that URL — the user opens it, signs in with email, and the anonymous key plus every site deployed with it lands in their dashboard (https://app.vibedrop.cc/sites).

If the claim URL expired before the user got to it, the CLI / MCP can mint a new one at any time. That's the preferred path — users should never need to see or paste the raw `vd_anon_…` key.

## When to use the MCP server instead

If the user prefers tool-call integration (the agent calls `deploy` as a native tool rather than shelling out to the CLI), install the MCP server:

```bash
claude mcp add vibedrop -- npx @vibedrop/mcp
```

Both paths hit the same backend. Skill + CLI is the recommended default because it works in any agent that can run shell commands.

## Troubleshooting

- **`ENOTFOUND api.vibedrop.cc`** — network is blocked. Check the user's firewall/VPN.
- **`401 Unauthorized`** — the key in `~/.vibedrop/config.json` was revoked. Delete the file and re-run `vibedrop deploy` to reprovision.
- **Deploy succeeds but page is blank** — the directory probably missed a bundler step. Rebuild and redeploy.

Docs and support: https://vibedrop.cc · hello@vibedrop.cc
