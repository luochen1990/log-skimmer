# Adapter Creation Guide (project-layer ruleset SOP)

> Reader: an agent using the log-skimmer MCP in some repo, led here by the `advice` field of a skim result.
> Goal: create a project-layer ruleset (adapter) for the current repo so skim is no longer blind to this repo's logs.

## Concept

An adapter = a project-layer ruleset, located at `<repo>/.log-skimmer/rules/*.yaml` (one rule per file, subdirectories per tool allowed), layered on top of the global ruleset and effective only for this repo; it travels with the repo (takes effect automatically after a fork).

When you need one (by default you **don't** — check the decision table first):

| Scenario | Where it lands |
|---|---|
| Only standard tools (cargo/nix/gha/pytest/…) | Not needed — the global ruleset already covers them |
| In-house check whose output carries a project-specific marker (e.g. a `[myci]` prefix) | Project layer works directly; a specific-enough anchor is worth contributing to the central repo |
| In-house check whose output has no specific marker (a broad regex would false-positive on other projects' logs) | Project layer — this is the only case that *must* be project-layer |

## SOP (five steps)

### 1. Sample

Collect ≥1 real failing CI log from this repo and locate the failure-evidence lines:

```bash
rg -i -n 'error|fail|panic|fatal|exception' <log>
```

Note each evidence class's **anchor line** (the first evidence line) and its continuation structure (indentation / stack frames of a multi-line error tree). `rg -C 5` helps see context.

### 2. Write rules

Write one rule per evidence class. Minimal template (field constraints in the quick reference at the end):

```yaml
id: myci-build-fail          # must be prefixed with the tool value; lowercase/digits/hyphens; must not duplicate a global rule id
title: myci build-failure evidence
tool: myci                   # this project's custom tool name (lowercase)
kind: signal                 # signal = extract evidence; noise = mark noise
intent: failure              # failure|warning|info; noise rules must omit this field
category: build              # enum in the quick reference below; when unsure = unknown
patterns:
  - regex: '^\[myci\] error:'  # patterns[0] is the anchor-line regex
  - regex: '^\s+at '           # continuation line (error-tree expansion)
    loop: true                 # greedily consume repeated continuation lines
# samples: this repo's CI run #123 failing log (provenance comment, recommended)
```

Key points: regexes are RE2 semantics (no backtracking); give anchor lines the project-specific marker to avoid false hits on other projects; recall first — over-covering (`intent: failure`) only costs a few noise lines, while a miss defeats the purpose of skim.

### 3. Write to disk

```bash
mkdir -p .log-skimmer/rules
# write the yaml from step 2 into .log-skimmer/rules/
```

No MCP server restart needed: skim/list_rules sense project-layer file changes on every call (re-call after saving to take effect). Precondition: the MCP server was started with this repo's root as its working directory (the project-layer detection base is the server's cwd at startup).

### 4. Acceptance check (all three must pass)

Call `skim` again with a real failing log:

1. **Loads**: skim returns normally. If it reports "project-layer ruleset load failed", fix the YAML per the detail and retry;
2. **Hits**: the target rule id appears in `blocks[].rules`, and the top-level `fallback` field disappears;
3. **Semantics**: the hit block's `intent`/`category` match the evidence's nature (failure evidence must be a failure block).

If a check fails: no hit → first verify the anchor regex actually matches the log's real lines with `rg <regex> <log>`; regex matches but skim still returns zero blocks → call `list_rules` to see whether the new rule id is present — absent means the MCP server's working directory is not this repo's root (the project-layer detection base is the server's cwd **at startup**; common when a host client launches the server from another directory); in that case report to the user, have them fix the MCP server launch config (cwd = this repo's root) and retry; too many false hits → tighten the anchor; load error → check field constraints against the quick reference below.

### 5. Persist

```bash
git add .log-skimmer/rules
git commit -m "feat: log-skimmer adapter (project-layer rules)"
```

## (OSS projects) Contributing upstream

If the regex anchor carries a project-specific marker (won't false-positive on other projects' logs), the rule can go straight into the central rules repo, benefiting every project:

1. Open a PR at <https://github.com/luochen1990/log-skimmer-rules> (read that repo's README for contribution guidance);
2. The `attribution` field is mandatory for contributed rules:

```yaml
attribution:
  contributor: <github username>
  generated-by: { harness: <agent/tool name>, model: <model name> }
  adapter-version: <x.y.z>
```

3. Once merged, delete the corresponding rule from this repo's project layer (the global layer now covers it) to avoid duplicate maintenance.

## Appendix: field quick reference

| Field | Constraint |
|---|---|
| id | `^[a-z0-9-]+$`, prefixed with the tool value, globally unique (same id as the global layer = load error) |
| patterns | ≥1 item; each requires `regex`, `loop` defaults to false; `[0]` is the anchor |
| kind | `signal` / `noise` |
| intent | `failure` / `warning` / `info` (omitted = failure); noise must omit it |
| category | `fmt`/`lint`/`typecheck`/`build`/`test`/`security`/`deps`/`infra`/`config`/`unknown` (omitted = unknown) |
| confidence | `high` / `medium` / `low` (omitted = medium) |
| context | `{ before: 0-10, after: 0-10 }` (defaults 0/0) |
| Retired | do not write `scope` / `repo` (retired in v0.6; the storage location determines scope) |

Full spec (state-machine semantics / manifest / output protocol): <https://github.com/luochen1990/log-skimmer/blob/master/docs/rule-format.md>
