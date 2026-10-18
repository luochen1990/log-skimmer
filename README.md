# Log Skimmer

[English](README.md) | [简体中文](README.zh-CN.md)

High signal-to-noise-ratio extraction for CI logs — a deterministic rule set compresses failed CI logs into summaries that carry line coordinates and semantic annotations, ready for AI agents to consume.

> The engine and rule set have been backtested on 67 pass/fail sample pairs from 3 public GitHub repositories + 1 private corpus source (see data below); available in two forms — CLI and MCP server (`log-skimmer-mcp`).

## Motivation

When an AI agent investigates CI failures, log consumption is structurally hard:

- **Full logs are too large**: a single failed job easily spans thousands to tens of thousands of lines. Feeding it wholesale into context is expensive, and the key errors drown in noise.
- **Keyword baselines produce many false positives**: the agent's fallback, `rg -i 'error|warn'`, treats the many harmless "warning" mentions in successful jobs as error signals. POC (proof-of-concept, the product's current stage) measurement: this baseline produced **230 lines of false positives** across 18 successful jobs (FP rate 100%, averaging 12.8 lines/job); Log Skimmer under the same criteria produced **0 lines**.
- **Single-line regexes cannot capture error trees**: failure evidence from tools like nix/rustc is multi-line structure (the largest nix error tree in the POC corpus spans 219 lines). `rg` only echoes the isolated anchor line (the first line of an error tree, e.g. `error: ...`) — enough to "glimpse" an error, not to "locate" it.

Log Skimmer's answer is a **learned rule set + a pure-regex, line-by-line state machine at runtime**: rules describe "what failure evidence from each tool looks like" (anchor line + multi-line extension), and the engine extracts complete error trees per those rules, attaching coordinates and semantic annotations. The LLM participates only in the future learning phase (rule-generation Adapter); the reading phase has zero LLM, zero network, and reproducible results.

POC backtesting corpus: **3 public GitHub repositories + 1 private corpus source, 67 pass/fail sample pairs (49 failed / 18 succeeded), ~60k lines of logs**, covering both GitHub Actions and Forgejo Actions CI platforms:

| Repository | Toolchain | Log shape |
| --- | --- | --- |
| anomalyco/opencode (formerly sst/opencode, ownership changed 2026-10) | bun / eslint / playwright (slots rotate weekly; 0 samples in the current public snapshot) | GHA timestamp prefix + ANSI color codes |
| niri-wm/niri | cargo / rustc / rust-test | Same as above |
| nix-community/home-manager | nix / git / nixfmt | Full multi-line error trees (largest: 219 lines) |
| Private corpus source | nix / just / treefmt / nim / systemd | Forgejo sandbox polling echo prefix |

Core results: FN (failure completely missed) 0/49; FP 0 lines vs the baseline's 230 lines; dual-granularity compression ratios — block-level 83.9%–98.9%, signature-line-level (anchor lines only) 99.18%–99.73% (giant trees benefit most: a 219-line error tree converges to a 1-line index at signature-line granularity).

## Repository Topology

The project consists of three public repositories, with all dependencies one-way (acyclic):

| Repo | Role | Cadence | Dependencies |
| --- | --- | --- | --- |
| `luochen1990/log-skimmer` (this repo) | **Engine** — CLI + MCP server | Irregular releases | None (consumed by the rules repo CI) |
| `luochen1990/log-skimmer-rules` | **Rule library** — the project's core asset, community-maintained + agent-assisted | Community PRs | One-way runtime dependency on this repo's engine |
| `luochen1990/log-skimmer-corpus` | **Corpus library** — regression corpus, source, self-contained | GHA weekly slot refresh | **Zero dependencies** |

```
rules repo ◀──one-way runtime dependency── CLI (log-skimmer skim --rules)
rules repo CI ──rule validation──▶ this repo's engine × corpus (master-vs-PR rule double-run)
```

Invariant: one-way dependencies — corpus is the source (zero dependencies, depended upon), rules are only consumed, no cycles.

## Key Features

- **Three-way intent**: every rule labels its extraction intent — `failure` (failure evidence; matching in a successful job is the only way it counts as a false positive) / `warning` (problem-prone but not a failure, e.g. deprecation) / `info` (pure environment snapshot, e.g. runner version). This layering makes FP statistics semantically sound: "a warning matched in a successful job" is expected behavior, not a false alarm.
- **Closed category enum**: 10 closed values (`fmt` / `lint` / `typecheck` / `build` / `test` / `security` / `deps` / `infra` / `config` / `unknown`) describing the information category, orthogonal to intent. Agents can filter on it programmatically instead of re-interpreting text.
- **Zero-block safety net**: the base of the rule set always carries two-sided platform safety nets — the failure side (CI platform annotations / runner failure signals) catches failed logs, and the success side (environment `info` rules such as runner version) catches successful logs. Thereby "any real CI log produces non-zero skim output" becomes a **structural promise** (POC: honored 67/67); a zero-block result means the rule set is blind to that log, regardless of job outcome.
- **Structural promise & reproducibility**: zero LLM, zero network; a line-by-line state machine + RE2-semantics regexes (linear complexity); identical input always yields identical output. The output is positioned as "index + summary": every block always carries line coordinates and metadata (rule / category / intent / confidence), so agents decide whether to expand context accordingly rather than treating the output as a log replacement.
- **Rule folksonomy**: no central registry — a rule file's existence defines the tool ID (`<tool-id>/<rule-id>.yaml`, one rule per file). The rule set is independent of the engine and lives in [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules) (community-maintained + agent-assisted); currently 30 rules across 15 tools, covering bun / cargo / eslint / forgejo-actions / git / github-actions / just / nim / nix / nixfmt / playwright / rust / rustc / systemd / treefmt.

Roadmap: the LLM rule-learning Adapter (log → rule-draft PR + backtest report), two-layer Validator verification, and a WebPage rule browser are future plans; the MCP server (`server/`) is delivered.

## Quick Start

Build (Rust toolchain) + fetch the rule set (rules live in a separate repo, community-maintained):

```bash
cargo build --release
# artifact: target/release/log-skimmer
git clone --depth 1 https://github.com/luochen1990/log-skimmer-rules /path/to/rules
```

Two output forms:

```bash
# Human-readable (default): blocks separated by ---, block header carrying line coordinates,
# intent/category and matched rules, trailing line with compression stats.
# (Human-readable output goes to stdout as a whole — `> out.txt` keeps everything;
#  only errors/warnings go to stderr.)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules

# JSON protocol (--json): structured coordinates and metadata for blocks/fragments + L0 stats,
# for programmatic agent consumption. (L0 = Level-0 gate: the lowest tier of
# whole-corpus regression gating, implemented by the `l0` subcommand.)
# (stdout is pure JSON; diagnostics such as the stats line go to stderr.)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules --json

# --context N: globally override context extension to N lines before / N lines after
# (default: each rule's own context; valid range 0-10, out-of-range is a usage error;
#  values starting with - must use the --context=N form)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules --context 3

# `-` reads the log from stdin (the frequent pipe path for CI triage — no temp file needed)
curl -sL <ci-log-url> | target/release/log-skimmer skim - --rules /path/to/rules
```

A single invocation accepts exactly one log file (the `<LOG_FILE>` positional) —
skim several logs with separate invocations; passing multiple files is a parse-time
usage error (exit 64).

Exit code semantics (caller contract):

| Exit code | Meaning | Caller action |
| --- | --- | --- |
| 0 | Completed normally | Consume the output |
| 2 | Data error: invalid rule set (load-time validation failed) / log read failure / empty log (0 lines, likely a truncated download) | Fix the data, then retry |
| 3 | Zero-block blindness — the rule set matched nothing in this log | Unconditionally fall back to the `rg -i 'error\|warn'` baseline |
| 64 | Usage error — argument parsing failed (BSD sysexits `EX_USAGE`; `--help`/`--version` still exit 0) | Check the command spelling (clap output on stderr); fixing data won't help |

### MCP Server (agent integration form)

`log-skimmer-mcp` exposes three tools over stdio transport: `skim(log_text, context?)` (extract blocks + fragment summaries + compression fields (omitted on zero blocks; the stats object itself is retained); context range 0-10, out-of-range reports invalid_params; on zero blocks it returns `fallback: "no-blocks"` with fallback-advice fields instead of an error) / `read_around(log_text, line_start, line_end)` (fetch an original-text window by coordinates, capped at 200 lines per call) / `list_rules()` (the loaded rule inventory).

Protocol behavior: the server identifies itself in `initialize` as `log-skimmer-mcp` (crate version), and declares only the `tools` capability. All parameter errors — out-of-range values, wrong types, missing required fields — uniformly return a JSON-RPC `-32602 invalid_params` error (never an `isError=true` normal response), so clients can judge by the error code alone; methods of undeclared capabilities (`resources/*`, `prompts/*`) return `-32601 method not found`. Responses may arrive out of order (in completion order); clients should match requests to responses by `id`.

```bash
cargo build --release -p log-skimmer-server
# artifact: target/release/log-skimmer-mcp (stdio transport)
```

Example client config — opencode (`mcp` section of `opencode.json`; environment variables via the `environment` field):

```json
{
  "mcp": {
    "log-skimmer": {
      "type": "local",
      "command": ["/path/to/log-skimmer-mcp"],
      "environment": { "LOG_SKIMMER_RULES": "/path/to/log-skimmer-rules" },
      "enabled": true
    }
  }
}
```

Claude Desktop (`mcpServers` section of `claude_desktop_config.json`; environment variables via the `env` key):

```json
{
  "mcpServers": {
    "log-skimmer": {
      "command": "/path/to/log-skimmer-mcp",
      "env": { "LOG_SKIMMER_RULES": "/path/to/log-skimmer-rules" }
    }
  }
}
```

Rule set resolution (resolved once at startup; a missing directory / no `*.yaml` / load-time validation failure all fail fast with exit code 2):

1. The **rule set directory** pointed to by the `LOG_SKIMMER_RULES` environment variable (a clone of the log-skimmer-rules repo, containing `*.yaml`) — if set, it is adopted unconditionally with no fallback probing (an invalid directory fails fast; a path that exists but is not a directory is reported as such rather than as "missing"); the standard deployment means when the rule set doesn't sit next to the binary. Setting it to an **empty string** also fails fast — the error names the variable (`LOG_SKIMMER_RULES 为空串`) instead of silently falling through to probing, which catches empty-variable plumbing bugs in deploy scripts;
2. Default probing — from the executable's directory, try adjacent `rules/` → `../rules/` → `../../rules/` in order (the first that is a directory wins; covers both the "rules next to bin" deployment layout and the cargo `target/{debug,release}/` build layout). Note the probing base is the **real binary path** (`current_exe` resolves symlinks): with a symlinked install (e.g. `/usr/local/bin/x -> /opt/pkg/bin/x`), a `rules/` placed next to the *symlink* is **not** found — put the rules next to the real binary or use `LOG_SKIMMER_RULES`;
3. A source-tree fixture path embedded at compile time — a dev/unit-test fallback, usually absent after deployment. **Caution**: on a build machine with the source tree present and no env/adjacent config, this level *succeeds* — but with the small dev fixture rule set (far smaller coverage than the real ruleset), which means silent under-detection on failed logs. Every such start prints a loud `⚠ fixture fallback` warning line to stderr; if you see it in production, configure `LOG_SKIMMER_RULES`.

**Ruleset manifest (optional version contract)**: a ruleset may carry a root-level `manifest.yaml` declaring `format_version` (rule-format version, currently 1) and an optional `min_engine_version` (engine version floor, `major.minor.patch`). At load time the engine rejects the ruleset (exit 2, self-explanatory message naming the failed check and the remedy) when the declared format version is newer than the engine supports or the engine is older than the floor; unknown fields in rule files or the manifest are likewise rejected instead of silently ignored. A ruleset without a manifest loads exactly as before (backward-compatibility window). This makes engine × ruleset compatibility statically decidable — downstream packagers pinning both sources can upgrade either side alone and learn compatibility from one load attempt instead of a full regression (spec: docs/rule-format.md §1.M).

Input cap (**MCP side only**): a single `log_text` above 32MB reports invalid_params and suggests truncating with `tail -c` and submitting in segments; CLI file input (`skim <log>`) has no such limit.

The server process's exit code is not part of the MCP tool-caller contract — the table above only constrains the `log-skimmer` CLI: a `log-skimmer-mcp` startup exit 2 = rule set load failure, fail-fast (a deployment-environment problem, see above); troubleshoot via stderr (the MCP protocol only occupies stdout; stderr is the log channel) — fixing data or retrying the call won't help.

Development verification:

```bash
cargo test --workspace     # unit tests: engine state-machine axioms + rule-loading regression + MCP stdio smoke
nix develop -c just check  # no system rust: quick loop inside the devShell (fmt + clippy + test)
```

Regression corpus: [log-skimmer-corpus](https://github.com/luochen1990/log-skimmer-corpus) (rolling snapshots of public-repo CI logs, auto-refreshed weekly by GHA).

## Contributing

- To add or modify rules: open a PR in [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules) and read that repo's README (rule contribution guide and labeling discipline).
- Normative SSOT for the rule format, state-machine semantics, and output protocol: [docs/rule-format.md](docs/rule-format.md) (Chinese; see the non-normative English translation in [docs/rule-format.en.md](docs/rule-format.en.md)). The spec carries its own independent version number (vN) — a separate axis from the engine crate version (e.g. 0.1.0); the two have no correspondence.

### Contributor local verification (public artifacts only)

Complete local verification of a rule contribution needs only the three public repos — no private credentials:

```bash
# ① The trio: engine (this repo) + public corpus repo + rules repo fork (do your work on a fork branch)
cargo build --release
git clone --depth 1 https://github.com/luochen1990/log-skimmer-corpus /path/to/corpus
git clone --depth 1 https://github.com/luochen1990/log-skimmer-rules /path/to/rules

# ② Quick check: single-sample skim (exit code 3 = the rule set matched nothing in that log;
#    whether a specific rule matched is attributed by ③'s per_rule_hits)
target/release/log-skimmer skim failed-job.log --rules /path/to/rules

# ③ Full gate: whole-corpus regression (FN=0 / FP rate ≤2% / zero-blocks=0 /
#    corpus presence ≥50) + per-rule hit report
#    (per_rule_hits). Positive signal for a new rule: count ≥1 in per_rule_hits
#    (i.e. absent from the dead_rules list). Stream separation: the full JSON
#    verdict report goes to stdout (redirectable for archiving, e.g.
#    `> report.json`); stderr carries a single-line `l0-gate: verdict=...` summary.
target/release/log-skimmer l0 --corpus /path/to/corpus --rules /path/to/rules

# ④ Strict mode (optional): fail if any rule has zero hits across the corpus. Note the current
#    rule set contains tools with no samples in the public corpus yet (see the dead_rules list),
#    so a full strict run will list them — strict mode suits "corpus should cover all rules"
#    scenarios; for targeted verification of a new rule, the signal in ③ is authoritative.
target/release/log-skimmer l0 --corpus /path/to/corpus --rules /path/to/rules --require-all-rules-hit
```

Note on exit codes (l0): a missing corpus dir or zero paired samples is a data error (exit 2 — fix the environment/fetch); running ③ on a hand-assembled mini corpus (<50 paired samples) fails the presence assertion by design (exit 1) — for targeted verification read `per_rule_hits` instead. Samples are `*.log` + same-name `*.log.json` pairs; unpaired orphans are warned on stderr and skipped (not counted).

Corpus accounting: the public corpus repo currently holds 57 paired samples; the README's backtest figure of 67 = 57 public + 10 from the private source (slots rotate weekly, so the pass/fail mix drifts with each snapshot).

Please attach a minimal sample log to PRs for new tool rules (provenance convention: spec §5 `samples: self-provided`).

Note: the dev repo's internal `just ci-merge` full chain includes a private corpus fetching stage — that is the intranet maintainers' path (a public clone does not contain that recipe); community contributors complete equivalent rule quality verification via the public-artifact path above.

## License

- Engine and services: Apache-2.0 (see [LICENSE](LICENSE)).
- Rule set [log-skimmer-rules](https://github.com/luochen1990/log-skimmer-rules): CC0 1.0 (rules and docs, no rights reserved).
- Corpus repo [log-skimmer-corpus](https://github.com/luochen1990/log-skimmer-corpus): in-repo orchestration and metadata under CC0 1.0; the log sample content remains the copyright of its original authors (redistribution terms in that repo's README).
