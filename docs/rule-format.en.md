# Log Skimmer Rule Format & State Machine Specification (v0)

> Non-normative English translation. In case of any discrepancy, the normative reference is [docs/rule-format.md](rule-format.md) (Chinese, SSOT).
>
> Scope: this specification — normative in its Chinese version — is the **single source of truth** for the rule YAML format, the line-by-line state machine semantics, the fragment output protocol, and the L0 statistics criteria; it is the shared contract between rule authors and engine implementers. For either side, any question about semantics is settled by the normative version; behavior not defined there is unspecified (implementations must not rely on it).
Version: v0.8 (POC). v0.1 absorbed T2 field-test feedback (A0); v0.2 absorbed a second round of incremental suggestions (intent/always-on coordinates/dual granularity/three states & rerun); v0.3 split intent three ways (failure|warning|info, requester feedback: distinguish problem-prone signals from pure environment snapshots among non-failure information); v0.4 added the ruleset manifest version negotiation + unknown-field rejection (issue #59: engine × ruleset compatibility becomes statically decidable); v0.5 made input classification a single-source predicate (issue #71: whitespace-only input joins the exit-2 data-error family; the zero-block warning's input-shape branching retired) and added leading-BOM stripping to A0 (issue #78) — behavioral semantics change that does not touch the rule file format (`format_version` stays 1, zero rule migration); v0.6 retired the scope/repo fields — a rule's effective scope is now decided solely by its **storage location** (§1.S: two-layer loading domain + project-layer overlay loading), and the fields enter a deprecation window (tolerated if present, values ignored, physically removed in v3; `format_version` 1 → 2, zero rule migration — old files load as before); v0.7 added the output-layer repeat-collapse (§3.R: presentation-layer folding of same-rule cascade blocks + statistics-criteria rulings) — an output-protocol extension (new optional fields), zero change to the rule file format and state-machine semantics (A1-A8), and byte-identical output to v0.6 whenever thresholds are not met; v0.8 added runtime project-layer change detection (issue #58): on every skim/list_rules call the server incrementally reloads the project layer by directory signature, and a runtime load failure = that call errors out (no degradation) — a server-behavior extension, zero change to the rule file format and state-machine semantics. Fields evolve forward by addition only, never removal; semantic changes must bump the version and migrate rules.

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
patterns:                     # state machine pattern chain, ≥1 item (see §2)
  - regex: '^error: '          # patterns[0] is the anchor
  - regex: '^\s'              # subsequent indented lines
    loop: true                 # greedy repeated consumption
kind: signal                  # signal (extract) | noise (mark as noise)
# ---- optional (defaults shown) ----
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
| patterns | non-empty array; each item requires regex; loop defaults to false |
| context.before/after | 0 ≤ value ≤ 10 |
| kind/confidence/category | enum values; category omitted counts as unknown (injected uniformly at the load entry) |
| intent | enum `failure`/`warning`/`info`, omitted = failure; must be omitted when kind: noise |

**Deprecated fields (since v0.6 — do not write them)**: `scope` / `repo` are retired — a rule's effective scope is decided solely by its storage location (§1.S). Both fields are in a **deprecation window**: tolerated if present, values completely ignored (no validation at all — combinations that v1 deemed illegal are also let through, so old rulesets load unbroken); **new rules must not use them**; physically removed in v3.

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

### 1.S Effective Scope & Storage Location (v0.6)

A rule's effective scope is decided solely by its **storage location** — location is a structural fact, a field is merely a declarative copy (under forking, location is automatically correct while fields rot). The `scope`/`repo` fields are retired for this reason (deprecation window: see the note after the §1 field-constraint table).

**Two-layer loading domain**:

| Layer | Storage location | Effective scope | Maintained by |
| --- | --- | --- | --- |
| Global layer | central rules repo (log-skimmer-rules, tool-axis) | all projects | community-maintained + agent-assisted (PR evolution) |
| Project layer | `<repo>/.log-skimmer/rules/` (optional) | that project only | the project itself |

**When is a project layer needed** (decision table — the default is *not* needed; the first two rows are the common case):

| Scenario | Where it goes |
| --- | --- |
| Standard tools only (cargo/nix/gha/…) | Not needed — the central repo already covers them |
| In-house check whose output carries a project-specific marker (e.g. an `[opencode]` prefix) | The central repo, named after the project's tool (the regex carries its own specific anchor; no cross-project false hits) |
| In-house check whose output has no specific marker (a wide regex would false-hit other projects' logs) | The project layer — only this case needs it |

**Fork semantics**: rules travel with the directory — a fork carrying `.log-skimmer/rules/` takes effect for the new repo automatically, no field to change (this is precisely the motivation for retiring scope/repo).

**Cross-layer id conflict = load error**: the same id appearing in both layers during overlay loading → fail-fast (the global-id-uniqueness constraint holds across layers); the error lists the conflicting ids and both sources. Special case: both layers resolving to the **same directory** (e.g. `LOG_SKIMMER_RULES` pointed at the project-layer directory) → the error points at the env misconfiguration instead of suggesting rule renames.

**Project-layer presence predicate and failure semantics (a deliberate asymmetry)**: `.log-skimmer/rules` **being a directory** = the project layer is declared present; the path being absent or a non-directory (e.g. a same-named file created by mistake) = not declared, silently skipped — the project layer is an optional enhancement and must not block startup with a hard error. **Present but failing to load** (zero yaml / validation violations, e.g. an empty directory containing only `.gitkeep`) = startup load error, fail-fast (exit 2, attributed to the project layer, never mislabeled as the global layer) — "a declaration carries the obligation to load", the same philosophy as the engine refusing an empty ruleset (§3 exit 2 family): a broken declaration fails loudly rather than degrading silently. Wrong shape = not declared; declared = must load.

**Project-layer probing (MCP server only)**: at startup the server probes `.log-skimmer/rules/` under its cwd; if present it **overlay-loads** it on top of the global ruleset (three-level resolution, see README) — global layer as before + project layer appended; if absent, global layer only, behavior unchanged. **Runtime change detection (issue #58, v0.8)**: on every skim/list_rules call the server re-probes the project-layer directory's **signature** (recursive "relative path × size × mtime" listing); if it matches the last loaded signature, the cached ruleset is reused (zero reload); otherwise (files added/removed/modified, or the directory removed) the project layer is reloaded and overlaid — making the "create project layer mid-session → verify in the same session" loop possible (the adapter workflow: zero-block advice → the `adapter_guide` tool returns an SOP → write to disk → the next call picks it up, no restart needed). A runtime reload failure = **that call returns an error** (carrying load details); the server does not degrade to silently running the stale ruleset — the same philosophy as startup fail-fast (explicit failure over silent degradation); a removed directory = fall back to the global layer only. The CLI has no probing semantics — the directory given by `--rules` is exactly what gets loaded (a project-layer directory can be passed directly as the `--rules` value to verify it).

### 1.M Ruleset Manifest (set-level version negotiation, optional)

A ruleset root may carry a `manifest.yaml` (a single set-level file, not a per-rule field; the exact filename `manifest.yaml` is a reserved filename at the ruleset root — it is not parsed as a rule). The engine and the ruleset evolve independently (README "Repository Topology"); version negotiation makes the compatibility of an "engine × ruleset" combination **statically decidable**: a downstream packager pinning both sources (engine tag + ruleset rev) can upgrade either side alone and learn compatibility from a single load attempt instead of a full regression.

```yaml
# manifest.yaml (ruleset root, optional)
format_version: 2          # required. Rule-format version, integer (currently 2)
min_engine_version: 0.1.0  # optional. Engine version floor, full major.minor.patch numeric triple
```

Loading semantics (validated when the engine loads a ruleset; a violation = **invalid ruleset**, rejected as a whole, exit 2 — the same "invalid ruleset" exit-code family as the §1 field constraints):

| Check | Violation | Rejection-message requirements |
| --- | --- | --- |
| Format version | `format_version` > the version supported by the engine | State the ruleset's declared version and the engine's supported version; suggest upgrading the engine |
| Engine floor | the engine's own version (build-time `CARGO_PKG_VERSION`) < `min_engine_version` | State the engine version the ruleset needs and this engine's version; suggest upgrading the engine or rolling back the ruleset |
| Manifest itself | YAML parse failure / unknown field / malformed `min_engine_version` | State the manifest's specific defect |

- **No manifest → loads as before** (backward-compatibility window): absence means no constraint; existing rulesets are unaffected. The manifest is an interface for rulesets (e.g. log-skimmer-rules) to adopt later; the adoption timing is the rules repo's own decision.
- **Version history**: v2 = the scope/repo deprecation window (§1.S) — a v2 engine accepts rule files both with and without those two fields (values ignored, zero rule migration); a ruleset declaring `format_version: 2` is rejected by v1 engines (a v1 engine treats scope as required and validates its value domain; v2-form rule files also fail to parse there). v1 = the initial format.
- **Version comparison**: both sides compare as plain `major.minor.patch` numeric triples, no semver library; the triple must be complete and purely numeric (`1` / `1.2` / `1.x.0` are all invalid).
- **Unknown-field rejection (deny_unknown_fields)**: deserialization of rule files (including pattern items and context items) and of the manifest rejects unknown fields — a future new field changes on old engines from "silently ignored" to "explicitly rejected". Silent drift (rule semantics changed while extraction results drift unnoticed) is more dangerous than rejection; introducing a new field must come with engine-side support, otherwise old engines reject explicitly via this section's checks.
- Rejection messages are self-explanatory: which check failed + a suggested action, so the consumer immediately knows which side to upgrade.
- The §1 reserved fields `structured-hint` (YAML hyphenated form) / `attribution` are known fields (not consumed in POC); their presence is legal.

## 2. State Machine Semantics (Axioms A1–A8)

**A0 line consumption semantics**: the raw text with the **file-leading** UTF-8 BOM stripped (U+FEFF, bytes EF BB BF; only the very first occurrence — a U+FEFF mid-first-line or on any later line is an ordinary character (ZWNBSP) and is kept), then split by `lines()`, each line with the trailing `\r` stripped (CRLF normalization); line numbers are 1-based (after BOM stripping, L1 is the first content line and the output **never echoes** the BOM bytes). A file containing only a BOM becomes 0 lines after stripping. Host prefixes (CI platform timestamps, ANSI color codes) get **no engine-side preprocessing** — rules' regexes carry their own tolerance (e.g. an anchor written as `^(timestamp )?error: `; observed odd shapes in docs/poc-report.md §5).

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

### 3.R Output-layer repeat-collapse (v0.7)

**Motivation**: same-cause thousand-line cascades — one rule repeatedly opening blocks for the same root cause (bazel `--keep_going` with 41 ERROR blocks; Alamofire swiftc with ~2500 same-shape type errors in one sample (corpus-measured at 937 blocks, the shape that once forced the rule's retirement — after folding the same sample presents 90 lines; rust-test-panic with 339 blocks folded in a single sample, 1272 lines omitted)). The state machine's per-block output is semantically correct as **evidence** (every block is a real hit) but redundant as **presentation** (the consumer can judge and locate from the anchor block + a count + an anchor-line index). The collapse is a **presentation-layer post-pass after A5 merging**: it never touches the state machine (A1-A8 semantics and their axiom tests unchanged) nor the data side of statistics (the folded blocks' evidence spans are fully retained in the collapse marker and consumed as usual by the statistics layer — R3).

**R1 predicate and thresholds**: partition the A5-merged output-block sequence (in line order) into **maximal runs**. A consecutive block pair (B_k, B_{k+1}) belongs to the same run iff:

- **Same single rule**: both blocks' `rules` are exactly the same single element `[R]`. Multi-rule mixed blocks **never participate** in folding — mixed evidence is dense and the folding trade-off is unclear (a mixed block also severs the runs on both sides).
- **Bounded block gap**: `next_blk_start − cur_blk_end − 1 ≤ max_gap` (default **20**), where a block's span is the hull of its fragments' `block` spans (min first line, max last line) — **context extension does not participate in the predicate** (what is judged is the semantic adjacency of rule hits, not presentation adjacency; A5 already guarantees a block gap ≥ 1 between distinct output blocks).

A run folds only when its length (anchor included) ≥ `min_repeat` (default **3**): keep the run's first block as the **anchor** (fully presented), fold the remaining `run_len − 1` blocks. Runs extend maximally before the threshold check — a run below threshold never folds partially.

**Empirical basis for defaults** (1314-sample corpus profile, block-span criteria): the gap distribution of same-rule adjacent pairs is **bimodal** — a dense cluster at ≤8 (690 pairs, single-line diagnostics) and a **repeating-unit cluster at 13-15** (281 pairs at gap=13; the swiftc type-error cascade has a 16-line diagnostic unit — measured on an Alamofire sample: 937 blocks, 823 adjacent pairs at gap exactly 15, the shape that once forced a rule's retirement, see this section's motivation); from 23 onward the tail is thin (likely unrelated causes interleaving). `max_gap=20` covers both clusters plus 5 lines of margin; threshold sensitivity (corpus-measured Σ omitted lines): gap 10 → 2664 / 15 → 4791 / 20 → 4923 / 30 → 5542 — marginal gain drops sharply beyond 20. `min_repeat=3`: len=2 runs are the most common (~144) and have a poor folding trade-off (hide 1, show 1) — never folded; len≥3 folds only cascades whose repetition pattern is established (~120 runs, including 340/74/19-block cascades).

**R2 folded representation**: the anchor block carries a collapse marker (a `collapse` field; with no folding both this field and `stats.collapse` are **omitted** — the zero-behavior-change guarantee, see R4). The example is a corpus-measured value (the rust-test-panic cascade of a BurntSushi/ripgrep failure sample):

```json
"collapse": {
  "rule": "rust-test-panic", "intent": "failure",
  "repeat_blocks": 327,
  "start_line": 3163, "end_line": 4796,
  "omitted_lines": 981,
  "frag_spans": [[3163, 3165], [3167, 3169], "..."]
}
```

| Field | Definition |
| --- | --- |
| `rule` / `intent` | The folded blocks' single rule id and its effective intent (all fragments in a run share rule and intent — intent is a rule-level constant) |
| `repeat_blocks` | Number of folded **blocks** (anchor excluded; the presentation unit) |
| `start_line`/`end_line` | Hull of the folded blocks' **output ranges** (context extension included; presentation-coordinate reference) |
| `omitted_lines` | Lines no longer presented after this run folds (noise-removed sense; set difference = folded blocks' presented-line set − surviving blocks' presented-line set — adjacent lines overlapping via context are not double-counted when a surviving block presents them) |
| `frag_spans` | Ascending `block` spans of the folded fragments (the evidence unit; anchor lines = each span's first element). **Signature-level index with zero loss**: consumers `read_around` any of them to expand; FP audits can be recomputed offline (span line counts are summable) |

Folded blocks are removed from the `blocks` array (JSON and human-readable output shrink in sync); the data side for restoring statistics = `frag_spans` + `intent` + `rule` (L0 consumes fragment-by-fragment as usual, see R3) — folded fragments' `text` is not retained (the presentation layer does not store it twice; expansion goes through read_around on the original log).

**Human-readable fold line** (emitted after the anchor block's text, single line, not counted into output_lines — metadata, same family as block headers):

```text
⟨×327 rust-test-panic 折叠块 [L3163-L4796] 省略 981 行; 锚行 L3163 L3167 L3171 L3175 L3179 L3183 L3187 L3191 …(共 327 行)⟩
```

Format (literal output keeps the Chinese wording): `⟨×{repeat_blocks} {rule} 折叠块 [L{start}-L{end}] 省略 {omitted} 行; 锚行 {first 8 anchor lines, space-separated}{appended ` …(共 {frag_spans.len()} 行)` when more than 8}⟩`. The CLI stats line appends `, repeat-collapse folded {folded_blocks} blocks (-{omitted_lines} lines)` when folding occurred (not appended at zero folding — zero behavior change).

**R3 statistics criteria (ruling table — every L0 metric under folding)**:

| Criterion | Ruling | Rationale |
| --- | --- | --- |
| `output_lines` (block-granularity compression numerator) | **Presentation scope**: folded blocks' lines not counted (omitted lines count as compressed); the fold line itself is not counted (metadata, same family as block headers); `stats.collapse = {folded_blocks, omitted_lines}` is always present for audit (serialization omitted at zero folding) | output measures what the consumer actually reads (the "index + summary" positioning of §3); omitted lines are fully recoverable via the anchor-line index + read_around — presented volume is the real reading cost. **Invariant: `output_lines` = cardinality of the presented-line set; its difference from the uncollapsed engine is exactly `omitted_lines`** |
| `signature_output_lines` (signature granularity) | **Full scope**: folded fragments' anchor lines still counted (deduplicated counting unchanged) | The anchor-line index is fully retained (`frag_spans` first elements) — zero information loss, the metric measures retained information; identical to the uncollapsed engine, the signature-tier compression is unaffected by folding |
| FN predicate / `failure_frags` / warning/info counts | **Full scope**: folded fragments still counted one-by-one — **a folded block still counts as having blocks** (structurally satisfied: the anchor is always kept, the non-empty-blocks verdict unchanged; the statistics layer consumes `frag_spans`) | Recall-first (§0): folding is a presentation concept; evidence existence is independent of presentation; the FN=0 invariant is immune to folding |
| FP numerator (`failure_frag_lines`) | **Full scope**: folded fragments' block-span line counts still accumulated (`frag_spans` recomputable offline) | FP measures evidence occupancy (how much a successful log is polluted by failure evidence), not presentation; folding removes no evidence |
| `per_rule_hits` / per-rule aggregation | **Full scope**: folded fragments counted one-by-one (N, not 1) | Liveness semantics (issue #19): a rule firing N times is rule-behavior information (dead-rule detection / health profile); the presentation layer must not pollute the rule-health surface |
| `boundary_violations` | **Full scope** (folding untouched) | The state-machine-layer sentinel completes per fragment before folding — folded blocks pass the boundary check too |
| Zero-block verdict | Unchanged: folding never collapses a non-empty output to empty (the anchor is always kept) | `blocks.is_empty()` semantics stable; the consumer decision matrix (§3) unaffected |
| Compression dual-test comparison | Folding only removes presentation (`omitted_lines ≥ 0`); per-sample block-granularity compression is monotonically non-decreasing | The dual-test sentinel only judges regressions (§4) — the fold feature itself can never trigger a compression-regression red; improvements are observable via the dual-test comparison |

**R4 parameterization and the zero-behavior-change guarantee**: the fold parameters `CollapseParams { max_gap, min_repeat }` are engine-level (lib API; `min_repeat` effective floor is 2 — smaller values are treated as 2). Defaults `(20, 3)`; CLI `--no-collapse` turns folding off in one switch; MCP skim always uses the defaults (POC exposes no parameter surface). **When thresholds are not met, output is byte-identical to the uncollapsed engine** — the regression guarantee in mechanical form: absent folding, the `collapse` / `stats.collapse` fields do not appear in JSON and the human-readable output and stats line are unchanged; all existing tests pass unchanged under default parameters.

**R5 multiple runs and edges**: adjacent runs (different rules / severed by a multi-rule block) fold independently, each on its own anchor; the first heterogeneous block after a run presents as usual. The run's **tail block is not kept** (POC simplification: tail evidence is reachable via `frag_spans` anchor lines + read_around; the gain of keeping tail blocks is unvalidated — deferred until consumer feedback).

**R6 intra-block run-length folding (not in this version)**: compressing isomorphic repeated lines inside a single block (the phpstan 6k-line single-cause / redis client-state-dump shapes) first requires defining the "isomorphic" judgment — the corpus profile shows **zero exact-identical intra-block runs**, so the exact criterion has no evidence-backed gain; a template-normalization criterion (stripping line numbers/addresses before comparing) risks mis-folding (violating the recall-first red line) and requires corpus expansion and validation under its own spec section. Explicitly out of scope for this version; inter-block folding (this section) is orthogonal to it — a future introduction reuses this section's ruling framework (presentation scope for output_lines only, everything else full scope).


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
| `blocks[].out_lines` | Lines presented in the block (after noise removal) — **literal value of the presented block** (folded blocks' lines are in no presented block; the fold index is the `collapse` field; the global `stats.out_lines` is full-scope — including folded-omitted accounting; the two semantics in §3.R3) |
| `blocks[].signature_lines` | Deduplicated count of the block's signature lines (each fragment's anchor line) — literal value of the presented block (folded blocks' anchor lines are indexed via `collapse.frag_spans`; the global `stats.signature_lines` is full-scope, §3.R3) |
| `blocks[].collapse` | Collapse-marker passthrough (§3.R2 field set; present only on the anchor block of a folded run) — consumers read `repeat_blocks`/`omitted_lines` to gauge the fold scale and read_around the `frag_spans` anchor lines to expand |
| `fragments` | Each block's **representative fragment** (not the full set; always carries the field set shown in the example above), one-to-one in the same order as `blocks` |
| `stats` | `total_lines`/`out_lines`/`signature_lines` under the same criteria as the CLI; `compression_block`/`compression_signature` = dual-granularity compression ratios (0-1 decimals, four-digit precision), **present only on non-zero blocks** — zero blocks means no output and hence no ratio, both fields omitted (same philosophy as the CLI stats-line `n/a`: refuse fake compression = 1.0 statistics); the zero-block verdict is judged by the presence of `fallback` |
| `fallback` | **Present only on zero blocks**; value domain is the single value `"no-blocks"` — presence is itself the blindness verdict; the consumer unconditionally falls back to the rg baseline (verdict matrix) |
| `advice` | **Present only on zero blocks**; fallback-advice copy (rg baseline command example + rule-contribution guidance), a human-readable hint, not a protocol-parsing target |

skim input validation (all `invalid_params`): `log_text` empty or whitespace-only (verdict source = the input three-way classification predicate above); `log_text` over 32MB (pre-truncate with `tail -c` and submit in segments); `context` outside [0,10] (`context` has the same semantics as the CLI `--context` above).

**read_around response**: `{total_lines, window_start, window_end, truncated, note?, lines[]}` — `window_start`/`window_end` are the actually returned window (after end-shrink and truncation, 1-based, inclusive); `truncated` = whether truncated by the 200-line window cap (`line_end` beyond the log's last line shrinking to the last line does **not** count as truncation); `note` present only on truncation, giving the line number to continue reading from; `lines[]` are the original lines prefixed with `L{n}: `. Input validation (all `invalid_params`): `log_text` empty, whitespace-only, or over 32MB; `line_start < 1` or `line_start > line_end`; `line_start` beyond the log's last line.

**list_rules response**: an array of the rule inventory, sorted by id ascending (predictable output, decoupled from load order); each entry `{id, tool, kind, intent, category}` — `intent` is always `null` when kind: noise (always a valid value for signal). (v0.6: the `scope`/`repo` output fields retire along with the rule fields — the effective scope is decided by storage location, see §1.S, and no longer appears in the inventory output.)

## 4. L0 Statistics Criteria (POC)

Ground-truth unit = **job-level result** (API metadata, not inferred). Corpus labels attach at the (run, attempt) level.

| Metric | Definition |
| --- | --- |
| Compression ratio | `1 - Σ(output lines) / total input lines`, listed per granularity (block-level / signature-line-level) and per tool |
| FP | **Only `intent: failure` rules**: the count of such fragments produced on success jobs (CI passed yet failure evidence was extracted = false positive). Matches of `intent: warning` rules are expected — listed separately as "warning hits", not counted as FP. **FP rate = total block-range line count of failure-intent fragments / total input lines of success samples** (acceptance line ≤2%, denominator criterion is SSOT — tool pollution is measured only on success samples; the numerator takes block ranges excluding context extension, avoiding double counting from overlaps) |
| FN proxy | The count of jobs (complete misses) in failure-family (failure/timed_out) job logs with zero `intent: failure` fragments — **acceptance line = 0, an invariant** (recall first, §0; over-cover at the expense of compression); partial misses need LLM cross-checking (POC spot-check prototype, not automated) |
| Boundary integrity (mechanical) | A fragment's block last line must still be matched by one of that rule's patterns (anchor or extension); violations are counted (guards against context pollution and implementation bugs) |
| Folding criteria (repeat-collapse) | §3.R3 ruling table — **presentation scope for `output_lines` only** (block-granularity compression numerator; omitted lines count as compressed), all other criteria (signature tier / FN / FP numerator / per_rule_hits / boundary) are **full scope** (folded blocks still count as blocks, fragments counted one-by-one); folding only removes presentation, per-sample block-granularity compression is monotonically non-decreasing — the dual-test regression sentinel is immune to folding |
| Positive verification (per_rule_hits) | **Per-rule hit counts, domain = the full set of loaded rules**: for signal rules = fragment count; for noise rules = the count of removal blocks (noise produces no fragments — its output is line removal); **over the full sample domain** (rerun-through included — liveness semantics: a rule hit only by excluded samples is not dead, unlike the FP/FN main metrics' domain filtering). The report is always emitted; `dead_rules` = the subset of rules with zero hits |
| dead-rules assertion (optional) | When the gate runs with `--require-all-rules-hit`, a non-empty dead_rules → fail — **positive verification**: a mistyped regex must not merge all-green; contributors thereby gain confidence that "the rule actually works". Off by default; the existing full set has naturally zero-hit rules on the public corpus (tools the corpus doesn't cover); strict mode fits scenarios where "the corpus should cover every rule" (e.g. after corpus expansion, or a targeted rule subset); single-rule targeted verification of a new rule uses a per_rule_hits count ≥1 as the signal |
| Gate exemption (gate_exempt) | Optional sidecar field `gate_exempt: bool` (defaults to false; old sidecars compatible): `true` samples skip the **FN / FP-rate / zero-block** assertions (never part of the failure verdict), while their metrics are still computed and reported in the **exempt group** — unadapted repos (tools without rules yet) join the corpus without going red; flip to false once adapted (the exemption remainder = the adaptation-progress metric). **Sentinels that are NOT exempted** (orthogonal to rule adaptation): the boundary assertion (an engine construction invariant, domain = all samples), ≥50-samples-in-corpus (domain = all samples, exempt included), and the dual-test per-repo compression compare includes exempt repos (pure engine-variable attribution under the same corpus and rules — exemption ≠ engine-regression exemption). The report splits into **gated / exempt group aggregates** (FN/FP-rate/zero-block/boundary/compression per group) plus a **per-repo breakdown table** (row domain = main metrics; FN/FP/zero-block are group-agnostic — a diagnosis view; an exempt_samples column marks group membership) and a **per-rule × per-repo hit matrix** (domain = the full loaded rule set, zero-hit rules = empty maps; counting matches per_rule_hits — signal fragments ∪ noise removal blocks, full sample domain) |

**cancelled/skipped three states** (asymmetric evidentiary power):

- `skipped`: no log; excluded from statistics entirely.
- `cancelled`: truncated logs **record hits only, never misses** — a failure-intent rule hit counts as a weak TP (under fail-fast collective-failure scenarios a hit is a true signal), reported separately and not folded into the main metrics; a non-hit is not counted as an FN (information incomplete).
- `timed_out` (a distinct GHA conclusion): watch closely — timeout logs are themselves a rule-able failure mode; collect them deliberately when labeling corpus.

**rerun handling** (fetch-layer metadata, no regex): sidecars must carry `attempt` (when missing, the engine treats it as attempt=1 and emits an aggregated single-line warning — the fallback behavior is specified, never silent); rerun-through (attempt>1 with final success) is excluded from the main metrics; its failed attempts are premium failure corpus — labeled per attempt conclusion and taken into the corpus.

**Exit-code layering** (CLI contract, issue #44): collection-layer illegality — corpus directory missing / zero paired samples / invalid rule set — is a data error, exit 2 (the same family as the skim-side exit 2; the caller fixes the environment, not the rules); verdict-layer violations → verdict=fail + exit 1. The ≥50-samples-in-corpus assertion therefore applies only to 0<n<50: a fail on a small targeted-verification corpus is expected (an idle-run floor; positive signals are judged by per_rule_hits), while insufficient samples under the full corpus is the fetch-pipeline failure signal — the corpus-missing note must distinguish these two readings, never accusing only the latter.

## 5. Samples & Pairing (POC Conventions)

- Sample storage: `data/logs/<host>/<owner>/<repo>/<YYYY-MM-DD>/<run_id>-<job_id>-<job_name>-<conclusion>.log` + a same-named `.json` sidecar: `{repo, run_id, job_id, conclusion, workflow_name, created_at, job_name, attempt, ci, fetched_at, gate_exempt}` — **nested natural paths** (requester decision, abandoning `__` flattening: recursive find costs machines nothing, human readability first — date/triggering job/outcome live in the path; bare-numeric filenames are unreadable); `repo` is in the **full `host/owner/repo` form** (metadata SSOT lives in the sidecar; the path is a readability projection); conclusion takes the **job-level result**; `created_at` (run-level) / `job_name` are written from API metadata at fetch time; `job_name` sanitization: `/` and spaces → `-`, truncated to 30 chars; `gate_exempt` is optional (defaults to false — old sidecars compatible), semantics in §4's gate-exemption row (a transitional channel for tools whose rules are not yet adapted; removed per repo once adapted).
- **Corpus as an independent repo (dual-source, requester decision, 2026-10-16)**: corpus does not live in the engine repo — public CI logs are an internet-fetchable resource at any time; keeping them in-repo only rots. Dual-source shape: ① GHA side (public repos) rolling slots stored in the independent corpus repo `github.com/luochen1990/log-skimmer-corpus` (public, **source, self-contained, zero dependencies** — GHA weekly slot refresh, builds no engine, computes no baseline), consumed by the engine repo CI through idempotent fetch scripts; ② **private corpus source** (contains sensitive environment details, not publicly distributed). `data/logs`/`data/rules` are fetch-script working copies (gitignored); the L0 gate's ≥50-samples-in-corpus assertion also backstops fetch failures (compatible with the rules repo's single-source CI). The rule set lives in its own repo `github.com/luochen1990/log-skimmer-rules` (community-maintained + agent-assisted; the quality gate for rule PRs runs in that repo's CI: publish-face engine × corpus × master-vs-PR rule double-run). Compression-ratio regression protection runs as a PR-CI double-run comparison: master vs PR engine × same corpus, same rules — no baseline files, no pairing problem.
- Rule file header comments record source samples (`# samples: <repo>/<run>-<job>, ...`) for manual provenance. **When a new tool has no corpus-repo samples yet** (a tool the corpus doesn't cover, e.g. pytest): write `# samples: self-provided`, and the PR must attach a minimal sample log (a real log fragment that hits the rule) — samples enter the corpus repo after maintainer review, at which point the concrete provenance is backfilled; the provenance field must never be left dangling or fabricated.
- **Orphan-sample visibility**: a sample pair present on one side only (a `*.log` without its sidecar, or vice versa) = a pairing gap — l0 collection warns per orphan on stderr and skips it (not counted), never escalating to a hard error; the gate never silently shrinks due to gaps (requiring the sidecar is the agreed visibility companion, issue #46).

## 6. Explicitly Out of Scope for POC

The MCP service is delivered; explicitly out of scope for POC: Adapter (LLM learning) / on-demand manifest fetching / WebPage / noise prefix-tree optimization / multibyte line-boundary optimization / CI integration / **intra-block run-length folding** (§3.R6 — the exact criterion has zero corpus gain; the template-normalization criterion needs its own design and validation). The engine = load + scan + output + statistics — four things.
