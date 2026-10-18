# Log Skimmer Rule Format & State Machine Specification (v0)

> Non-normative English translation. In case of any discrepancy, the normative reference is [docs/rule-format.md](rule-format.md) (Chinese, SSOT).
>
> Scope: this specification — normative in its Chinese version — is the **single source of truth** for the rule YAML format, the line-by-line state machine semantics, the fragment output protocol, and the L0 statistics criteria; it is the shared contract between rule authors and engine implementers. For either side, any question about semantics is settled by the normative version; behavior not defined there is unspecified (implementations must not rely on it).
> Version: v0.5 (POC). v0.1 absorbed T2 field-test feedback (A0); v0.2 absorbed a second round of incremental suggestions (intent/always-on coordinates/dual granularity/three states & rerun); v0.3 split intent three ways (failure|warning|info, requester feedback: distinguish problem-prone signals from pure environment snapshots among non-failure information); v0.4 added the ruleset manifest version negotiation + unknown-field rejection (issue #59: engine × ruleset compatibility becomes statically decidable); v0.5 made input classification a single-source predicate (issue #71: whitespace-only input joins the exit-2 data-error family; the zero-block warning's input-shape branching retired) and added leading-BOM stripping to A0 (issue #78) — behavioral semantics change that does not touch the rule file format (`format_version` stays 1, zero rule migration). Fields evolve forward by addition only, never removal; semantic changes must bump the version and migrate rules.

## 0. Design Soul (constrains all future evolution)

- **The LLM lives only in the learning phase (Adapter); the reading phase (Reader/engine) is purely deterministic**: the runtime described by this spec has zero LLM, zero network, is reproducible and auditable.
- **Recall first** (requester decision, 2026-10-16): the cost of an FN (a missed failure) is the agent misjudging the outcome or losing its locating ability and falling back to the full log — the tool loses its reason to exist; the cost of an FP is merely a few extra lines of noise; the compression ratio is a derived metric. **Priority is fixed: misses = 0 > false-positive tolerance > compression ratio** — over-cover at the expense of compression rather than miss anything. Structural corollaries: ① **the two-sided platform safety net permanently anchors the rule set base** — the failure side (annotations/runner failure signals) catches failed logs, the success side (environment `info` rules such as runner version) catches successful logs (at least one info match per successful log is a rule-set obligation); ② the zero-block warning is unconditional (§3) — the two-sided safety net makes "any CI log yields non-zero skim output" a structural promise; zero blocks = blindness, the caller falls back unconditionally, and the verdict relies on no external expectation metadata.
- **Streaming-friendly**: the state machine consumes line by line; engine implementations must not require the whole file in memory (the POC implementation may read it all at once for convenient statistics, but the state-machine semantics themselves must be derivable line by line).
- **Linear complexity, structurally guaranteed**: regexes are restricted to RE2 semantics (see §2.A8); matching each line × each rule is O(line length).

## 1. Rule File Format

- Path: `rules/<tool-id>/<rule-id>.yaml`, one rule per file.
- `tool-id`: lowercase letters/digits/hyphens; a folksonomy convention name (no central registry — a rule's existence defines the ID).
- `rule-id`: `<tool-id>-<semantic-name>`, globally unique (loading duplicate ids is an error).

```yaml
# ---- required ----
id: nix-eval-error            # <tool-id>-<semantic-name>, globally unique
title: nix evaluation error tree   # one-line human-readable description
tool: nix                     # tool dimension (matches the directory name)
scope: global                 # global | repo
patterns:                     # state machine pattern chain, ≥1 item (see §2)
  - regex: '^error: '          # patterns[0] is the anchor
  - regex: '^\s'              # subsequent indented lines
    loop: true                 # greedy repeated consumption
kind: signal                  # signal (extract) | noise (mark as noise)
# ---- optional (defaults shown) ----
repo: null                    # required when scope: repo, "host/owner/repo" (e.g. github.com/sst/opencode; the host disambiguates multiple CI platforms)
category: unknown             # closed enum of 10 values (see table below); omitted = unknown (injected uniformly at the load entry — prefer missing over wrong)
intent: failure               # failure|warning|info — see the semantics table below; must be omitted for noise rules
confidence: medium            # high|medium|low (inherited by fragments, used for ordering)
context: { before: 0, after: 0 }   # output boundary extension (line counts; not part of the state machine)
structured-hint: null         # future field, not implemented in POC, reserved placeholder
attribution: null             # omitted for hand-written POC rules; required when Adapter-generated
#   attribution: { contributor: str, generated-by: {harness: str, model: str}, adapter-version: str }
```

Semantics of `intent` (the basis for FP/FN statistics layering; orthogonal to `category` — category says "what kind of information", intent says "what a match means for the outcome verdict"):

| Value | Semantics | Examples | Statistics treatment |
| --- | --- | --- | --- |
| `failure` | Extracts failure evidence; matching in a passing job = FP | rustc `error[E0308]` / nix error trees / pytest `FAILED` | **FP is counted only for rules of this class**; participates in FN |
| `warning` | **Problem-prone** but not a failure — will break later / worth attention | `warning: ... is deprecated` / `colliding subpath` | Not counted as FP; does not participate in FN |
| `info` | **Pure environment/context snapshot**, no problem tendency — useful for ruling out environment factors and for reproduction | runner/toolchain version lines / test-pass summaries (`N pass / 0 fail`) | Not counted as FP; does not participate in FN |

The distinction between warning and info is "problem tendency": the former may hint at the root cause (high priority when diagnosing), the latter is an environmental fact (use as needed). Consumer-side presentation layering (e.g. info collapsed by default) is to be finalized for the MVP; engine/statistics treat both equivalently.

Field constraints (validated at load time; violation = invalid rule set, rejected wholesale):

| Field | Constraint |
| --- | --- |
| id | matches `^[a-z0-9-]+$`, and is prefixed by the `tool` value |
| scope/repo | when `scope: repo`, repo is required; when `scope: global`, repo must be null |
| patterns | non-empty array; each item requires regex; loop defaults to false |
| context.before/after | 0 ≤ value ≤ 10 |
| kind/confidence/category | enum values; category omitted counts as unknown (injected uniformly at the load entry) |
| intent | enum `failure`/`warning`/`info`, omitted = failure; must be omitted when kind: noise |
| repo | full `host/owner/repo` form; a bare `owner/repo` lacking the host is invalid |

Closed `category` enum (finalized by survey, see docs/category-survey.md; two mixed domains — domain membership is only for docs/WebPage grouping, not part of the enum values):

| Value | Domain | Definition (baseline: the tool's role in CI first, log shape as auxiliary evidence) |
| --- | --- | --- |
| `fmt` | Quality gate | Drift reports from formatters |
| `lint` | Quality gate | Static-analysis/style/anti-pattern warnings (excluding type diagnostics and security scans) |
| `typecheck` | Quality gate | Type diagnostics of standalone type checkers/compilers (tsc/mypy/rustc `error[E...]`) |
| `build` | Quality gate | Compile/link/package/build-system failures (non-type diagnostics) |
| `test` | Quality gate | Assertion/case failures of any test framework (unit/integration/VM/e2e unified; test type is expressed by tool) |
| `security` | Quality gate | Vulnerability/secret-leak/dangerous-pattern scan alerts (includes dependency-vulnerability **content**; dependency **fetch** failures go to deps) |
| `deps` | Environmental root cause | Dependency resolution/download/version-conflict/hash-verification failures |
| `infra` | Environmental root cause | Network/credentials/resources (OOM/disk)/timeouts/runner failures/toolchain crashes/git operation failures |
| `config` | Environmental root cause | Config-file/declarative-evaluation/generated-artifact drift (nix eval, `nginx -t`, systemd verify, gen sentinels) |
| `unknown` | Fallback | Cannot be uniquely categorized |

Labeling discipline (**prefer missing over wrong**): mislabeling pollutes query semantics (agents filter programmatically; undetectable), while unknown only loses that one rule's filtering value. Use unknown when: multiple causes mixed / category outside the enum (a signal for set evolution — record samples in the PR) / insufficient samples / L0 backtesting finds a rule matching multiple categories. An unknown share persistently >40% triggers a set review; upgrading an unknown to a named category goes through PR with human confirmation. flaky is an attribute, not a category — it does not enter category (a separate dimension in the future).

### 1.M Ruleset Manifest (set-level version negotiation, optional)

A ruleset root may carry a `manifest.yaml` (a single set-level file, not a per-rule field; the exact filename `manifest.yaml` is a reserved filename at the ruleset root — it is not parsed as a rule). The engine and the ruleset evolve independently (README "Repository Topology"); version negotiation makes the compatibility of an "engine × ruleset" combination **statically decidable**: a downstream packager pinning both sources (engine tag + ruleset rev) can upgrade either side alone and learn compatibility from a single load attempt instead of a full regression.

```yaml
# manifest.yaml (ruleset root, optional)
format_version: 1          # required. Rule-format version, integer (currently 1)
min_engine_version: 0.1.0  # optional. Engine version floor, full major.minor.patch numeric triple
```

Loading semantics (validated when the engine loads a ruleset; a violation = **invalid ruleset**, rejected as a whole, exit 2 — the same "invalid ruleset" exit-code family as the §1 field constraints):

| Check | Violation | Rejection-message requirements |
| --- | --- | --- |
| Format version | `format_version` > the version supported by the engine | State the ruleset's declared version and the engine's supported version; suggest upgrading the engine |
| Engine floor | the engine's own version (build-time `CARGO_PKG_VERSION`) < `min_engine_version` | State the engine version the ruleset needs and this engine's version; suggest upgrading the engine or rolling back the ruleset |
| Manifest itself | YAML parse failure / unknown field / malformed `min_engine_version` | State the manifest's specific defect |

- **No manifest → loads as before** (backward-compatibility window): absence means no constraint; existing rulesets are unaffected. The manifest is an interface for rulesets (e.g. log-skimmer-rules) to adopt later; the adoption timing is the rules repo's own decision.
- **Version comparison**: both sides compare as plain `major.minor.patch` numeric triples, no semver library; the triple must be complete and purely numeric (`1` / `1.2` / `1.x.0` are all invalid).
- **Unknown-field rejection (deny_unknown_fields)**: deserialization of rule files (including pattern items and context items) and of the manifest rejects unknown fields — a future new field changes on old engines from "silently ignored" to "explicitly rejected". Silent drift (rule semantics changed while extraction results drift unnoticed) is more dangerous than rejection; introducing a new field must come with engine-side support, otherwise old engines reject explicitly via this section's checks.
- Rejection messages are self-explanatory: which check failed + a suggested action, so the consumer immediately knows which side to upgrade.
- The §1 reserved fields `structured-hint` (YAML hyphenated form) / `attribution` are known fields (not consumed in POC); their presence is legal.

## 2. State Machine Semantics (Axioms A1–A8)

**A0 line consumption semantics**: the raw text with the **file-leading** UTF-8 BOM stripped (U+FEFF, bytes EF BB BF; only the very first occurrence — a U+FEFF mid-first-line or on any later line is an ordinary character (ZWNBSP) and is kept), then split by `lines()`, each line with the trailing `` stripped (CRLF normalization); line numbers are 1-based (after BOM stripping, L1 is the first content line and the output **never echoes** the BOM bytes). A file containing only a BOM becomes 0 lines after stripping. Host prefixes (CI platform timestamps, ANSI color codes) get **no engine-side preprocessing** — rules' regexes carry their own tolerance (e.g. an anchor written as `^(timestamp )?error: `; observed odd shapes in docs/poc-report.md §5).

**A1 independent scanning**: each rule maintains an independent state machine scanning the log line by line; rules do not interfere with each other.

**A2 anchor**: `patterns[0].regex` matching the current line → **opens a block**, consumes that line, and enters the extension state. The anchor is the only way to open a block.

**Anchoring convention**: anchoring a rule regex with `^` at line start is a **writing convention, not an engine enforcement** — the engine compiles the pattern verbatim per A8 and injects no `^`; a pattern written without `^` matches anywhere within the line. Consequence of the convention: when error evidence is embedded mid-line rather than at line start (e.g. a single-line-JSON-shaped log line `{"level":"error",...}`), an anchored rule will not match, and the log may end up zero-block into fallback (§3). For tools whose evidence is known to be always embedded mid-line, the rule regex expresses the tolerance itself (same approach as the A0 host-prefix handling).

**A3 extension (greedy + ordered pass-through)**: after a block opens, the state machine holds an **extension pointer** `p`, initially pointing at `patterns[1]`. For each new line:

- Try `patterns[p]` on the current line: match → consume the line; if `loop: true`, `p` stays (the next line retries the same pattern — greedy); if `loop: false`, `p += 1`.
- No match → `p += 1`, and **the next pattern from `patterns[p+1..]** keeps trying the **same line** (pass-through); if none of the remaining patterns match (`p` runs past the end) → **close the block** (the line does not belong to the block).
- Closing a block yields a fragment: block = the contiguous line range `[first, last]` consumed by this block.

Corollary: all of `patterns[1..]` are optional extensions — an anchor match alone with zero extension matches still closes and yields a block (a single-line block). There is no "broken-chain penalty".

**A4 context extension**: after a block closes, extending the block by `context.before` lines toward earlier lines and `context.after` lines toward later lines yields the fragment's output range (clamped to log boundaries). Context affects only output — not the state machine, and not merge decisions with other blocks.

**A5 merging**: all signal fragments are sorted by `(start, end)`; **overlapping or adjacent** ones (`next.start <= cur.block_end + 1`, judged on block ranges) merge into one output block. The output block's `start/end` = the hull of the member fragments' **output ranges** (block ∪ context extension, after clamping). Each fragment's metadata is retained inside the block. Merging happens only at the output aggregation layer; the state-machine layer is unaware of it.

**A6 noise rules**: rules with `kind: noise` run the same state machine; lines covered by their fragments are **removed** from signal output (line-level removal against signal blocks: at output time a set difference is taken against the noise line set; signal fragment metadata is retained, its text renders as the surviving lines, and if no lines survive the whole fragment is dropped).

**A7 output order**: output blocks are ordered by line; the engine does not reorder fragment content (original log order preserved, timeline respected).

**A8 regex constraints**: Rust `regex` crate syntax (RE2 semantics: no lookahead/lookbehind/backreferences, named groups `(?P<name>...)` supported but POC does not consume groups). Rules are compiled with the regex crate at load time; a compile failure = invalid rule.

### Worked Example

```text
Log:                            Rule: patterns:
L1: build start                 - regex: '^error: '
L2: error: undefined variable   - regex: '^\s'
L3:   at /flake.nix:42:3          loop: true
L4: build failed
```

- L1: anchor doesn't match; nothing happens.
- L2: anchor matches → block opens, L2 consumed.
- L3: patterns[1] (`^\s`, loop) matches → consumed, greedy stay.
- L4: doesn't match patterns[1] → block closes. block = [L2, L3]; fragment = lines L2–L3.
- If L4 were rewritten as `  more context`, it would be swallowed greedily (regex constraints curb over-greed: extension regexes should carry indentation/prefix/structure constraints where possible).

## 3. Fragment Output Protocol (JSON)

The output is positioned as an "**index + summary**", not a log replacement — every fragment always carries coordinates and metadata; the consumer (agent) decides whether to expand context accordingly.

The example below matches the actual CLI `skim --json` output **field-for-field** (complete field set, not an excerpt; values and block count are illustrative):

```json
{
  "file": "path/or:-",
  "input_lines": 1000,
  "output_lines": 42,
  "blocks": [
    {
      "start_line": 120, "end_line": 135,
      "rules": ["nix-eval-error", "nix-eval-error-summary"],
      "fragments": [
        { "rule": "nix-eval-error", "kind": "signal", "intent": "failure", "category": "typecheck",
          "confidence": "high", "block": [120, 128], "out": [120, 135],
          "text": "...(full text after context extension)..." },
        { "rule": "nix-eval-error-summary", "kind": "signal", "intent": "failure", "category": "typecheck",
          "confidence": "medium", "block": [128, 128], "out": [128, 135],
          "text": "...(text within this fragment's output range)..." }
      ]
    }
  ],
  "stats": {
    "input_lines": 1000, "output_lines": 42, "signature_output_lines": 12,
    "noise_lines_removed": 200, "boundary_violations": []
  }
}
```

- `start/end_line`: the merged block's output-range hull (union of member fragments' output ranges, context extension included; 1-based, inclusive).
- `blocks[].rules`: the full set of rule ids hit inside the merged block (deduplicated, first-occurrence order) — a block-level hit index, so consumers can see the hit surface without walking `fragments`.
- A fragment's `block` is the range the rule itself consumed (no context); `text` includes the context extension.
- `output_lines` = total lines of merged blocks (after noise removal) — **block-level granularity**.
- `signature_output_lines` = the count of each fragment's block first line (anchor/signature line, deduplicated by line number) — **signature-line-level granularity**. Compression ratios are reported at both granularities (the two numbers can differ a lot; acceptance lines must not be mixed).
- `input_lines`/`output_lines` appear **duplicated with identical values** at the top level and inside `stats`: an implementation artifact (top-level summary fields + a self-contained `stats` aggregate), not two different figures — consumers may read either side.
- `stats.boundary_violations`: the violation records of the §4 "boundary integrity (mechanical)" check, elements `{rule, line_no, text}`; **always empty by state-machine construction** (an implementation sentinel: the block's tail line is by construction consumed by one of the rule's patterns; non-empty = an engine bug) — consumers normally never need to read it; `[]` in the example is the steady-state shape.
- CLI `--context N` (default: each rule's own context): presentation-layer global override to before=N/after=N, a purely mechanical cut by coordinates, no rule involvement; valid range 0 ≤ N ≤ 10 (same boundary as the rule `context` field in §1 and the MCP skim `context` parameter), out-of-range is a parse-time usage error.
- CLI stream-separation contract: human-readable output (block content, the stats line, and its leading blank line) goes to stdout as a whole — `> file` redirection keeps the full output; errors and warnings (including the zero-block warning) go to stderr. In `--json` mode stdout is pure JSON; diagnostics such as the stats line go to stderr.
- Input three-way classification and empty-log hard rejection (exit 2, issues #71/#78): after normalization the log is classified by a **single engine-layer predicate** — `Empty` (0 lines; includes BOM-only files, which are empty after stripping) / `WhitespaceOnly` (≥1 line and every character of every line is Unicode whitespace; the criterion is fixed as `char::is_whitespace`, including the full-width space U+3000, with empty lines counting as whitespace) / `Content` (at least one line with a non-whitespace character). `Empty` and `WhitespaceOnly` both belong to the **data-error family** (likely a truncated download or an empty run): on the CLI they are hard-rejected with exit 2 and never enter the zero-block verdict — symmetric with "empty rule set hard-rejected", prefer explicit failure over silent pass; the exit-2 family = invalid rule set / log read failure / empty log (0 lines or whitespace-only lines). MCP `skim`/`read_around` entry validation consumes **the same predicate**: `Empty` or `WhitespaceOnly` log_text is always rejected as `invalid_params` (different channel — CLI exit code / MCP parameter error — but the same single-source verdict). The zero-block verdict (exit 3) is reachable only for `Content` input — whitespace-only input leaves the zero-block path entirely, so its troubleshooting guidance (check the log source) never shares the screen with the fallback-to-rg advice.
- Stats-line compression ratio: shown as `n/a` in zero-block scenarios (no output means no compression ratio — refuse fake 100.0% statistics); all other numbers print as usual.
- CLI zero-block warning (**unconditional**, the runtime companion of §0 "recall first"): "any real CI log yields non-zero skim output" is the rule set's **structural promise**, honored by the two-sided safety net — failure side (CI-platform annotation/runner failure-signal rules) + success side (platform environment `info` rules, e.g. runner version lines; at least one info match per successful log is a rule-set obligation, not a suggestion). Zero blocks = the rule set is **blind** to that log, regardless of job outcome → prominent stderr warning + **exit code 3**, and the caller unconditionally falls back to the rg baseline. No expectation metadata needs to be passed in by the caller — the verdict is fully endogenous to the rule set's coverage promise.

**Consumer decision matrix** (agent usage contract; warning/info are outcome-agnostic and play no part in the verdict):

| Job conclusion (known to the consumer) | skim blocks | failure-intent blocks | Verdict |
| --- | --- | --- | --- |
| Any | zero | — | Structural blindness → the tool already exited 3; unconditional fallback |
| Success | non-zero | 0 | Normal (target state; a quality metric, not an invariant) |
| Success | non-zero | non-zero | FP (mild cost: a few extra noise lines; tolerated by acceptance lines, no fallback needed) |
| Failure | non-zero | non-zero | Normal (structurally guaranteed by the failure-side safety net) |
| Failure | non-zero | 0 | **Anomaly** — known boundaries: truncated logs (runner crash losing the tail failure signal) or rule blindness; without a conclusion the tool cannot self-judge — the consumer's one-line check: `conclusion=failure while failure blocks are zero → treat as blindness → fallback` |
- Human-readable output (default): blocks separated by `---` (**no leading blank line before the first block** — output starts flush at the top, #78b; the blank lines between blocks are kept as separators), block header `[L120-L135] [failure/typecheck] rule1,rule2` (intent/category from the block's representative fragment — intent priority failure > warning > info, then confidence > in-block original order at equal level; single semantic source: engine `OutputBlock::representative`, same source for the MCP BlockSummary), followed by the text.

### MCP Tool Response Protocol (skim / read_around / list_rules)

Tool responses of the MCP server (`log-skimmer-mcp`, stdio) are a **consumer-side view** layered on top of this section's base protocol — unlike the CLI `--json`, which returns the engine's `SkimResult` directly (top level `file`/`input_lines`/`output_lines`/`stats`, blocks with fragments embedded), MCP skim returns a parallel array of block summaries + representative fragments. Successful results are JSON embedded in text content; input-validation failures are MCP `invalid_params` errors (structured and recoverable, not a crash). Zero-block semantics, two surfaces: CLI = stderr warning + exit 3 (above); MCP = **normal return** (not an error) + the `fallback` field present — the same blindness verdict expressed as a field.

**skim response** (non-zero-block example; `fallback`/`advice` present only on zero blocks, `stats.compression_*` omitted on zero blocks):

```json
{
  "blocks": [
    { "start_line": 3, "end_line": 3, "rules": ["cargo-could-not-compile"],
      "intent": "failure", "category": "build", "out_lines": 1, "signature_lines": 1 }
  ],
  "fragments": [
    { "rule": "cargo-could-not-compile", "kind": "signal", "intent": "failure",
      "category": "build", "confidence": "high", "block": [3, 3], "out": [3, 3], "text": "..." }
  ],
  "stats": { "total_lines": 100, "out_lines": 1, "signature_lines": 1,
             "compression_block": 0.99, "compression_signature": 0.99 }
}
```

| Field | Definition |
| --- | --- |
| `blocks` | Array of block summaries, **sorted by intent priority (failure > warning > info, then ascending start_line at equal level)** — the failure evidence the consumer cares about most always comes first. This re-sorting happens only in the MCP view layer; the engine output layer is always in line order (A7 constrains the engine layer, unaffected) |
| `blocks[].start_line` / `end_line` | The output-block range hull (context extension included, 1-based, inclusive) |
| `blocks[].rules` | The full set of rule ids hit in the block (deduplicated in first-seen order) |
| `blocks[].intent` / `category` | The representative fragment's intent/category (single semantic source = `OutputBlock::representative`, same source as the CLI block header) |
| `blocks[].out_lines` | Lines presented in the block (after noise removal) |
| `blocks[].signature_lines` | Deduplicated count of the block's signature lines (each fragment's anchor line) |
| `fragments` | Each block's **representative fragment** (not the full set; always carries the field set shown in the example above), one-to-one in the same order as `blocks` |
| `stats` | `total_lines`/`out_lines`/`signature_lines` under the same criteria as the CLI; `compression_block`/`compression_signature` = dual-granularity compression ratios (0-1 decimals, four-digit precision), **present only on non-zero blocks** — zero blocks means no output and hence no ratio, both fields omitted (same philosophy as the CLI stats-line `n/a`: refuse fake compression = 1.0 statistics); the zero-block verdict is judged by the presence of `fallback` |
| `fallback` | **Present only on zero blocks**; value domain is the single value `"no-blocks"` — presence is itself the blindness verdict; the consumer unconditionally falls back to the rg baseline (verdict matrix) |
| `advice` | **Present only on zero blocks**; fallback-advice copy (rg baseline command example + rule-contribution guidance), a human-readable hint, not a protocol-parsing target |

skim input validation (all `invalid_params`): `log_text` empty or whitespace-only (verdict source = the input three-way classification predicate above); `log_text` over 32MB (pre-truncate with `tail -c` and submit in segments); `context` outside [0,10] (`context` has the same semantics as the CLI `--context` above).

**read_around response**: `{total_lines, window_start, window_end, truncated, note?, lines[]}` — `window_start`/`window_end` are the actually returned window (after end-shrink and truncation, 1-based, inclusive); `truncated` = whether truncated by the 200-line window cap (`line_end` beyond the log's last line shrinking to the last line does **not** count as truncation); `note` present only on truncation, giving the line number to continue reading from; `lines[]` are the original lines prefixed with `L{n}: `. Input validation (all `invalid_params`): `log_text` empty, whitespace-only, or over 32MB; `line_start < 1` or `line_start > line_end`; `line_start` beyond the log's last line.

**list_rules response**: an array of the rule inventory, sorted by id ascending (predictable output, decoupled from load order); each entry `{id, tool, kind, intent, category, scope, repo}` — `intent` is always `null` when kind: noise (always a valid value for signal), `repo` is non-`null` only when `scope: repo`.

## 4. L0 Statistics Criteria (POC)

Ground-truth unit = **job-level result** (API metadata, not inferred). Corpus labels attach at the (run, attempt) level.

| Metric | Definition |
| --- | --- |
| Compression ratio | `1 - Σ(output lines) / total input lines`, listed per granularity (block-level / signature-line-level) and per tool |
| FP | **Only `intent: failure` rules**: the count of such fragments produced on success jobs (CI passed yet failure evidence was extracted = false positive). Matches of `intent: warning` rules are expected — listed separately as "warning hits", not counted as FP. **FP rate = total block-range line count of failure-intent fragments / total input lines of success samples** (acceptance line ≤2%, denominator criterion is SSOT — tool pollution is measured only on success samples; the numerator takes block ranges excluding context extension, avoiding double counting from overlaps) |
| FN proxy | The count of jobs (complete misses) in failure-family (failure/timed_out) job logs with zero `intent: failure` fragments — **acceptance line = 0, an invariant** (recall first, §0; over-cover at the expense of compression); partial misses need LLM cross-checking (POC spot-check prototype, not automated) |
| Boundary integrity (mechanical) | A fragment's block last line must still be matched by one of that rule's patterns (anchor or extension); violations are counted (guards against context pollution and implementation bugs) |
| Positive verification (per_rule_hits) | **Per-rule hit counts, domain = the full set of loaded rules**: for signal rules = fragment count; for noise rules = the count of removal blocks (noise produces no fragments — its output is line removal); **over the full sample domain** (rerun-through included — liveness semantics: a rule hit only by excluded samples is not dead, unlike the FP/FN main metrics' domain filtering). The report is always emitted; `dead_rules` = the subset of rules with zero hits |
| dead-rules assertion (optional) | When the gate runs with `--require-all-rules-hit`, a non-empty dead_rules → fail — **positive verification**: a mistyped regex must not merge all-green; contributors thereby gain confidence that "the rule actually works". Off by default; the existing full set has naturally zero-hit rules on the public corpus (tools the corpus doesn't cover); strict mode fits scenarios where "the corpus should cover every rule" (e.g. after corpus expansion, or a targeted rule subset); single-rule targeted verification of a new rule uses a per_rule_hits count ≥1 as the signal |

**cancelled/skipped three states** (asymmetric evidentiary power):

- `skipped`: no log; excluded from statistics entirely.
- `cancelled`: truncated logs **record hits only, never misses** — a failure-intent rule hit counts as a weak TP (under fail-fast collective-failure scenarios a hit is a true signal), reported separately and not folded into the main metrics; a non-hit is not counted as an FN (information incomplete).
- `timed_out` (a distinct GHA conclusion): watch closely — timeout logs are themselves a rule-able failure mode; collect them deliberately when labeling corpus.

**rerun handling** (fetch-layer metadata, no regex): sidecars must carry `attempt`; rerun-through (attempt>1 with final success) is excluded from the main metrics; its failed attempts are premium failure corpus — labeled per attempt conclusion and taken into the corpus.

**Exit-code layering** (CLI contract, issue #44): collection-layer illegality — corpus directory missing / zero paired samples / invalid rule set — is a data error, exit 2 (the same family as the skim-side exit 2; the caller fixes the environment, not the rules); verdict-layer violations → verdict=fail + exit 1. The ≥50-samples-in-corpus assertion therefore applies only to 0<n<50: a fail on a small targeted-verification corpus is expected (an idle-run floor; positive signals are judged by per_rule_hits), while insufficient samples under the full corpus is the fetch-pipeline failure signal — the corpus-missing note must distinguish these two readings, never accusing only the latter.

## 5. Samples & Pairing (POC Conventions)

- Sample storage: `data/logs/<host>/<owner>/<repo>/<YYYY-MM-DD>/<run_id>-<job_id>-<job_name>-<conclusion>.log` + a same-named `.json` sidecar: `{repo, run_id, job_id, conclusion, workflow_name, created_at, job_name, attempt, ci, fetched_at}` — **nested natural paths** (requester decision, abandoning `__` flattening: recursive find costs machines nothing, human readability first — date/triggering job/outcome live in the path; bare-numeric filenames are unreadable); `repo` is in the **full `host/owner/repo` form** (metadata SSOT lives in the sidecar; the path is a readability projection); conclusion takes the **job-level result**; `created_at` (run-level) / `job_name` are written from API metadata at fetch time; `job_name` sanitization: `/` and spaces → `-`, truncated to 30 chars.
- **Corpus as an independent repo (dual-source, requester decision, 2026-10-16)**: corpus does not live in the engine repo — public CI logs are an internet-fetchable resource at any time; keeping them in-repo only rots. Dual-source shape: ① GHA side (public repos) rolling slots stored in the independent corpus repo `github.com/luochen1990/log-skimmer-corpus` (public, **source, self-contained, zero dependencies** — GHA weekly slot refresh, builds no engine, computes no baseline), consumed by the engine repo CI through idempotent fetch scripts; ② **private corpus source** (contains sensitive environment details, not publicly distributed). `data/logs`/`data/rules` are fetch-script working copies (gitignored); the L0 gate's ≥50-samples-in-corpus assertion also backstops fetch failures (compatible with the rules repo's single-source CI). The rule set lives in its own repo `github.com/luochen1990/log-skimmer-rules` (community-maintained + agent-assisted; the quality gate for rule PRs runs in that repo's CI: publish-face engine × corpus × master-vs-PR rule double-run). Compression-ratio regression protection runs as a PR-CI double-run comparison: master vs PR engine × same corpus, same rules — no baseline files, no pairing problem.
- Rule file header comments record source samples (`# samples: <repo>/<run>-<job>, ...`) for manual provenance. **When a new tool has no corpus-repo samples yet** (a tool the corpus doesn't cover, e.g. pytest): write `# samples: self-provided`, and the PR must attach a minimal sample log (a real log fragment that hits the rule) — samples enter the corpus repo after maintainer review, at which point the concrete provenance is backfilled; the provenance field must never be left dangling or fabricated.
- **Orphan-sample visibility**: a sample pair present on one side only (a `*.log` without its sidecar, or vice versa) = a pairing gap — l0 collection warns per orphan on stderr and skips it (not counted), never escalating to a hard error; the gate never silently shrinks due to gaps (requiring the sidecar is the agreed visibility companion, issue #46).

## 6. Explicitly Out of Scope for POC

The MCP service is delivered; explicitly out of scope for POC: Adapter (LLM learning) / on-demand manifest fetching / WebPage / noise prefix-tree optimization / multibyte line-boundary optimization / CI integration. The engine = load + scan + output + statistics — four things.
