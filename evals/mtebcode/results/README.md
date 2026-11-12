# MTEB(Code, v1) measured results archive

This directory archives codeseek's end-to-end results on the **MTEB(Code, v1)-style retrieval
subset** produced by `evals/mtebcode` (primary metric **nDCG@10**, overall score = arithmetic mean
of per-task means).

> **Three batches are archived: `2026-11-12`, `2026-11-12b` and `2026-11-12c`.** Each is a real
> 100-query measurement with a 300-document pool per task, but pooled corpus + a small query sample
> per task means none of them is **comparable with the MTEB(Code, v1) leaderboard** (see the batch
> sections below).
>
> `2026-11-12` and `2026-11-12b` use **different task sets** and are therefore **not comparable with
> each other**. `2026-11-12c`, by contrast, re-runs **batch 2's exact 100 questions on batch 2's exact
> corpora** (`workdir`/`tasks.jsonl`/`seed`/`sampling_fingerprint` all unchanged) with only
> `index.hybrid.cutoff.enabled` flipped to `false` — so **b ↔ c is a true A/B pair** and the only
> same-questions comparison in this archive.
>
> The older 5-query `CosQA` smoke run is kept at the bottom for reference only.

## Naming convention

| pattern | meaning |
| --- | --- |
| `YYYY-MM-DD-report.json` | first batch of that day, machine-readable (`meta` / `aggregate` / `per_query` / `failure_top` / `warnings`) |
| `YYYY-MM-DD-report.md` | same batch, human-readable (fixed 5 sections) |
| `YYYY-MM-DD[b|c…]-report.{json,md}` | 2nd/3rd batch of the same day (a day can produce several batches when the binary under test changes) |

Files are copied verbatim from the same-named artifacts under the `--workdir` used for the run
(default `/tmp/mtebcode-eval/`) and only renamed with a date prefix. `.md` files are always produced
by `render_markdown()` in `mtebcode_eval/report.py` — never edited by hand, so that
`md == render_markdown(json)` stays verifiable. Note that this invariant holds **against the renderer
version that produced the file**: from batch `2026-11-12c` on, §5 derives the real commands from
`meta`, so re-rendering an *older* archive with the current renderer changes §5 and no longer compares
equal. Archived files are never retro-edited; the batch sections below quote the commands verbatim.

## What each archive entry must record

Before committing a report, make sure `meta` carries all of the following (the runner fills them in
automatically; only `--workdir` artifacts differ):

* `seed`, `corpus_size`, `limit_queries`, `sampling_fingerprint` — the sampling protocol;
* `dataset_splits` — which qrels split was used per task (`test` → `valid` → `train` fallback);
* `codeseek_binary` — `path` / `mtime_iso` / `sha256_prefix` of the binary under test;
* `embedding.configured` + `embedding.model` — whether semantic retrieval was actually enabled;
* `mode_counts` — `hybrid` vs `name_fallback` vs `unknown_empty` query counts;
* `n_queries`, `n_docs_total`, `k`, `search_limit`, `command` (`run` or `score`).

A report whose `mode_counts.name_fallback > 0` is **not comparable with the MTEB(Code, v1)
leaderboard** and must say so at the top (the renderer adds that blockquote automatically).

## Batch `2026-11-12` — 100 queries / 4 tasks

Files: `2026-11-12-report.json` + `2026-11-12-report.md` (copied verbatim from the `--workdir`
artifacts; `md == render_markdown(json)` was re-verified after copying).

| item | value |
| --- | --- |
| date (UTC) | `2026-11-12`, run started `10:31:34Z`, report written `10:32:44Z` |
| tasks (4) | `CosQA`, `AppsRetrieval`, `CodeTransOceanContest`, `CodeTransOceanDL` |
| queries | **100** total = 25 per task (`--limit-queries 25`), `--seed 42` |
| corpus pool | **300 documents per task** (1,200 total), sampling fingerprint `3a2f136b62c306f0` |
| qrels splits | `default/test` (CoIR layout) for all four tasks |
| binary | `rust-core/target/release/codeseek`, 60,600,320 B, sha256 prefix **`b1532bb1b43e3d65`** |
| embedding | `Qwen/Qwen3-Embedding-4B` (api_token configured, `configured=True`) |
| search params | `--limit 20 --concurrency 4 --timeout 180`, k=10 |
| **Mean(Task) nDCG@10** | **0.4897** (mean recall@10 0.5900, mean MRR@10 0.4596, mean recall@20 0.6300) |

Per-task nDCG@10 / recall@10 / MRR@10:

| task | nDCG@10 | recall@10 | MRR@10 | mode split |
| --- | ---: | ---: | ---: | --- |
| CosQA | 0.7452 | 0.7600 | 0.7400 | 20 hybrid / 5 empty |
| AppsRetrieval | 0.2252 | 0.2400 | 0.2200 | 14 hybrid / 11 empty |
| CodeTransOceanContest | 0.7400 | 0.7600 | 0.7333 | 23 hybrid / 2 empty |
| CodeTransOceanDL | 0.2482 | 0.6000 | 0.1450 | 25 hybrid / 0 empty |

Mode distribution: **82 `hybrid` / 0 `name_fallback` / 18 `unknown_empty`** (82.0% / 0.0% / 18.0%).
`n_unmapped` is **0 for every query** (mean `n_mapped` = 8.46), i.e. every returned chunk was mapped
back to a pool `doc_id`; the docmap/path resolution is healthy.

Timing (this machine, warm index):

| stage | value |
| --- | --- |
| run wall clock (init + 100 searches) | **~70 s** |
| `codeseek init` per corpus (incremental, already indexed) | 0.3 / 0.4 / 0.7 / 0.8 s (Σ 2.2 s) |
| `codeseek init` per corpus (cold, first build) | CosQA 73 s, AppsRetrieval 64 s, CodeTransOceanContest 154 s, CodeTransOceanDL 98 s |
| search per query | mean **2.68 s**, median 2.86 s, p0 0.73 s, max 6.94 s (per task: CosQA 3.44 s, Apps 3.56 s, Contest 2.72 s, DL 0.98 s) |

### Why 4 tasks and not the originally planned 5

The first attempt used `CosQA, CodeSearchNetRetrieval, SyntheticText2SQL, CodeFeedbackST,
StackOverflowQA`. Four of the five turned out to be **structurally unusable with codeseek** (not a
config/split error — those were all verified against `/splits`), so the query budget was
re-allocated to 4 usable tasks × 25 = 100:

| dropped task | measured reason |
| --- | --- |
| `CodeFeedbackST` | corpus `_id`s are hashes, so positional lookup fails; the pool build needs an in-order scan of 156,526 corpus rows (~1,566 pages) and its qrels had already passed 12,400 rows without finishing — >1 h for one task |
| `CodeSearchNetRetrieval` | `CoIR-Retrieval/CodeSearchNet` `python-corpus.text` is the **docstring**, not code: of 300 materialized files only 3 contained `def`/`class`; `codeseek init` indexed just 1 file / 3 symbols, so every query returned nothing |
| `SyntheticText2SQL` | corpus materializes as `.sql`, which is **not** in codeseek's indexer whitelist (`cpp/py/java/rs/ts/tsx/js/go`) → 0 files indexed |
| `StackOverflowQA` | corpus is natural-language answers materialized as `.txt` → not in the whitelist → 0 files indexed |

Replacements were registered with a code extension matching the corpus payload:
`AppsRetrieval` → `.py` (indexed 439 functions / 184 files), `CodeTransOceanContest` → `.cpp`
(1,571 / 284), `CodeTransOceanDL` → `.py` (1,399 / 271).

### Known anomaly in this batch: 18 `unknown_empty`

18 of 100 queries returned zero results with an **empty** `error` field (11 AppsRetrieval, 5 CosQA,
2 CodeTransOceanContest). All 18 were re-run **serially** afterwards (same binary, same corpus, no
concurrency) and **all 18 still returned 0 results** — so this is codeseek's deterministic
cutoff behaviour (the `index.hybrid.cutoff` thresholds in `~/.codeseek/config.json`), not a
concurrency flake. They are scored as 0.0 and are included in the per-task means above.

## Batch `2026-11-12b` — 第 2 批：全短 query 任务集 (100 queries / 4 tasks)

Files: `2026-11-12b-report.json` + `2026-11-12b-report.md` (copied verbatim from the `--workdir`
artifacts; `md == render_markdown(json)` re-verified after copying).

**Why a second batch.** Batch 1 mixed tasks whose queries are 540–3,571 characters long. Measured
directly with the same binary: an AppsRetrieval query returns `[]` at 1,316 characters, and the same
query truncated to its first line (152 characters) returns 3 hits. So batch 1's AppsRetrieval 0.2252
and CodeTransOceanDL 0.2482 measure codeseek's **query-length cut-off**, not retrieval quality.
Batch 2 therefore keeps **only short-query tasks** and drops everything long.

| task | query chars min / median / p90 / max | decision |
| --- | --- | --- |
| CosQA | 18 / **34** / 42 / 62 (sampled 40; 35 over the full 500) | kept (40 queries) |
| CodeEditSearchRetrieval-python | 21 / **45** / 128 / 1,811 (full set) | kept (20) |
| CodeEditSearchRetrieval-go | 19 / **45** / 139 / 1,529 | kept (20) |
| CodeEditSearchRetrieval-javascript | 18 / **43** / 87 / 601 | kept (20) |
| SyntheticText2SQL | 25 / 79 / 130 / 419 | dropped: `.sql` corpus is outside the indexer whitelist |
| CodeSearchNetRetrieval | — / 624 / — / — | dropped: long query + docstring corpus |
| CodeFeedbackST | — / 540 / — / — | dropped: long query |
| CodeTransOceanContest | — / 547 / — / — | dropped: long query |
| StackOverflowQA | — / 883 / — / — | dropped: long query (34.2% prose) |
| CodeTransOceanDL | — / 1,211 / — / — | dropped: long query |
| AppsRetrieval | 50 / 1,316 / 2,491 / 13,955 | dropped: long query (98.9% > 200 chars) |
| CodeFeedbackMT | — / 3,571 / — / — | dropped: long query |

Intended budget was 4 languages × 20 + CosQA 20; only 3 of the 4 requested CodeEditSearchRetrieval
languages have a complete corpus/queries/qrels triple (**java has no `java-qrels` at all** — the config
does not exist and the repo has no such parquet; ruby/typescript `/rows` return HTTP 500), so the
fallback rule applied: **CosQA 40 + 3 × 20 = 100**.

| item | value |
| --- | --- |
| date (UTC) | `2026-11-12`, run finished `11:38:33Z` |
| tasks (4) | `CosQA` (40), `CodeEditSearchRetrieval-python` (20), `-go` (20), `-javascript` (20) |
| queries | **100** total, `--seed 42`, per-task `query_quota` (40/20/20/20) |
| corpus pool | **300 documents per task** (1,200 total), `sampling_fingerprint` **`2986c8ffabe1d0e2`** |
| config/split | `corpus/corpus`+`queries/queries`+`default/test` (CosQA); `<lang>-corpus/train` + `<lang>-queries/train` + `<lang>-qrels/train` ×3 |
| materialization | CodeEdit corpora are 100% unified diffs → materialized as **diff post-image code** (`diff_postimage=True`, recorded per task in `meta.json`); extensions `.py` / `.go` / `.js` |
| data source | **local HF parquet** (`--parquet-cache`): 141 parquet pages + 1 REST request — `datasets-server /rows` returned HTTP 429 for 6 consecutive retries |
| binary | `rust-core/target/release/codeseek`, 60,600,320 B, sha256 prefix **`b1532bb1b43e3d65`** (same build as batch 1) |
| embedding | `Qwen/Qwen3-Embedding-4B`, `configured=True`, `name_fallback` = **0** |
| search params | `--limit 20 --concurrency 4 --timeout 180`, k=10 |
| **Mean(Task) nDCG@10** | **0.4266** (mean recall@10 0.4312, mean MRR@10 0.4250, mean recall@20 0.4312) |

Per-task metrics (all 100 queries are scoreable, n_skipped = 0):

| task | nDCG@10 | recall@10 | MRR@10 | recall@20 | mode split | mean returned |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| CosQA | **0.7565** | 0.7750 | 0.7500 | 0.7750 | 32 hybrid / 8 empty | 1.00 |
| CodeEditSearchRetrieval-python | **0.3000** | 0.3000 | 0.3000 | 0.3000 | 9 hybrid / 11 empty | 0.55 |
| CodeEditSearchRetrieval-go | **0.4500** | 0.4500 | 0.4500 | 0.4500 | 13 hybrid / 7 empty | 1.45 |
| CodeEditSearchRetrieval-javascript | **0.2000** | 0.2000 | 0.2000 | 0.2000 | 10 hybrid / 10 empty | 0.90 |

Mode distribution: **64 `hybrid` / 0 `name_fallback` / 36 `unknown_empty`** (64% / 0% / 36%).
`n_unmapped` is **0 for every query** (mean `n_mapped` = **0.98** = (40×1.00 + 20×0.55 + 20×1.45 +
20×0.90)/100, mean returned 0.98), i.e. every returned chunk mapped back to a pool `doc_id`.

Exact reproduction of this batch (report.md §5 inside *this* archived artifact prints the *generic*
template of the time — from batch `2026-11-12c` on, §5 derives the real commands from `meta` — so the
real commands are pinned here):

```bash
cd evals/mtebcode
python -m mtebcode_eval prepare --workdir /tmp/mtebcode-short \
  --tasks CosQA,CodeEditSearchRetrieval-python,CodeEditSearchRetrieval-go,CodeEditSearchRetrieval-javascript \
  --limit-queries 20 --corpus-size 300 --seed 42 \
  --proxy http://127.0.0.1:7897 --parquet-cache ~/.cache/huggingface/hub
python -m mtebcode_eval run --workdir /tmp/mtebcode-short --limit 20 --concurrency 4 --timeout 180
```

Timing (this machine, cold index):

| stage | value |
| --- | --- |
| run wall clock (4 × init + 100 searches) | **306.6 s** |
| `codeseek init`, one per corpus | 33.5 s (CosQA) / 68.8 s (py) / 68.2 s (go) / 66.4 s (js), **Σ 236.9 s** |
| search per query | mean **2.75 s**, median 2.44 s, min 1.99 s, max 6.37 s |

### Diagnosis of the 36 `unknown_empty` (measured, not assumed)

The empty share is 20% (CosQA) / 55% (python) / 35% (go) / 50% (javascript) — three of four exceed
20%, so each cause was checked explicitly:

1. **Not query length.** All 36 empty queries are short: max 189 chars, and the per-task medians are
   34 / 51 / 44 / 44 chars. The `error` field is empty on all 36, so this is not a failure either.
2. **Not an indexing gap.** `codeseek skeleton` run over the whole pool shows 297/300 (py), 289/300 (go),
   300/300 (js) files yield at least one symbol, and the **gold docs specifically** are indexable
   20/20, 19/20, 20/20.
3. **It is query↔hunk semantic mismatch plus the similarity cut-off.** Reproduced by hand:
   `codeseek search "Change if condition to while with reformat"` (gold `a16cffb7…`, a 12-line
   post-image hunk with `def prime_factors`) returns `[]` at `--limit 5` and `--limit 20`, while
   `codeseek search "prime factors while loop number"` on the same corpus returns exactly that gold
   file (hybrid, `final_score` 0.998). So the document is in the index and retrievable by a code-named
   query; the commit-message-style query simply does not match the hunk text well enough, and
   `index.hybrid.cutoff` (`abs_floor 0.2`, `rel_ratio 0.55`, `min_keep 1`) then trims to **one**
   candidate — meta `mean_returned` is 1.00 for CosQA (26 of 40 queries got exactly one result).
   With one result per query, nDCG@10 degenerates into a rank-1 hit rate, which is why nDCG ≈ recall
   ≈ MRR in every row above.

### Note on the CodeEditSearchRetrieval corpus (materialization A/B, measured)

Its `corpus.text` is a unified diff in 1,000/1,000 rows, and only 15–26% of the added lines contain a
`def`/`class`/`func`/`function`. Materialized **raw** as `.py`, 30 documents yielded **9 indexed files /
11 symbols**; materialized as the **diff post-image** (drop `@@`/`---`/`+++`/`-` lines, strip one
`+`/space prefix) the same 30 documents yielded **17 files / 80 symbols**. The post-image conversion is
opt-in per task (`TaskSpec.diff_postimage`), recorded as `meta.json.tasks[].materialization.diff_postimage`,
and covered by selftest assertions — every code character is preserved, only patch markers are removed.

### Why 0.4266 must not be read as "short queries score higher"

Batch 1 = 0.4897 over `CosQA / AppsRetrieval / CodeTransOceanContest / CodeTransOceanDL`;
batch 2 = 0.4266 over `CosQA / CodeEditSearchRetrieval-{python,go,javascript}`. **Different task sets →
different difficulty; the two overall means are not comparable and no "improvement/regression" claim is
made from them.** What *is* comparable inside a single batch:

* CosQA is the only task present in both. Batch 1 sampled 25 of 500 queries → **0.7452**; batch 2
  sampled 40 of 500 (different subset, different pool of distractors) → **0.7565**. The +0.011 is far
  inside the ±0.05–0.1 sampling noise documented in `../README.md` §9, i.e. **no change**.
* The batch-2 mean is *lower* than batch 1's, but that is a task-mix effect: two cheap tasks were
  replaced by three diff-corpus tasks that score 0.20–0.45. Short queries did not raise the score —
  they made the score **interpretable**, because no query in this batch is long enough to trip the
  length cut-off.

## Batch `2026-11-12c` — 第 3 批：关闭 adaptive cutoff (100 queries / 4 tasks, **identical to batch 2**)

Files: `2026-11-12c-report.json` + `2026-11-12c-report.md`.

**Why a third batch.** Batch 2 measured a *truncated* ranker, not a ranker: `mean_returned` was 0.98
and 36 of 100 queries came back empty, so nDCG ≈ recall ≈ MRR in every row. Batch 3 re-runs the
**exact same 100 questions on the exact same corpora** with one config change, to isolate what the
ranking actually does when the result list is not chopped to one item.

| item | value |
| --- | --- |
| config change | `~/.codeseek/config.json` → `index.hybrid.cutoff.enabled = false` (everything else untouched) |
| backup of the original config | `~/.codeseek/config.json.mtebcode-backup-20261112T194850` |
| unchanged under the hood | same workdir `/tmp/mtebcode-short`, same `tasks.jsonl`, same 4 corpora, `--seed 42`, `sampling_fingerprint` **`2986c8ffabe1d0e2`** (byte-identical to batch 2) |
| binary | `rust-core/target/release/codeseek`, 60,600,320 B, sha256 prefix **`b1532bb1b43e3d65`** — no Rust code was touched |
| embedding | `Qwen/Qwen3-Embedding-4B`, `configured=True`, `name_fallback` = **0** |
| search params | `--limit 20 --concurrency 4 --timeout 180`, k=10 |
| **Mean(Task) nDCG@10** | **0.5339** (mean recall@10 **0.6000**, mean MRR@10 **0.5128**, mean recall@20 **0.6000**) |
| run finished (UTC) | `2026-11-12T11:51:54Z` |

Because the question set and the corpus pool are identical, **every number below is directly
comparable with batch 2** — this is the only pair in this archive where an A/B really is an A/B.

### Verification that the switch took effect (measured before the full run)

Three CosQA queries that returned **exactly 1** result in batch 2 were re-run by hand
(`codeseek search "<query>" --json --limit 20`, cwd `corpora/cosqa`):

| query | returned before | returned after |
| --- | ---: | ---: |
| `python file opening modes` (`cosqa__q0036`) | 1 | **20** |
| `python check file is readonly` (`cosqa__q0000`) | 1 | **20** |
| `write in bold and read in color of the print mesaage in python` (`cosqa__q0017`) | 1 | **20** |

`RUST_LOG=debug` on the first query logs the new branch verbatim:

```
DEBUG codeseek::services::hybrid_search: 自适应 cutoff 已关闭：20 → 20 条（仅按 k_final=20 截断）
```

i.e. the config was read, the three-gate cascade was skipped, and only `truncate(k_final)` remained.

### Per-task metrics

| task | nDCG@10 | recall@10 | MRR@10 | recall@20 | mode split | mean returned |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| CosQA | **0.9690** | 1.0000 | 0.9583 | 1.0000 | 40 hybrid | 20.00 |
| CodeEditSearchRetrieval-python | **0.4131** | 0.5000 | 0.3833 | 0.5000 | 20 hybrid | 20.00 |
| CodeEditSearchRetrieval-go | **0.4882** | 0.5500 | 0.4696 | 0.5500 | 20 hybrid | 20.00 |
| CodeEditSearchRetrieval-javascript | **0.2653** | 0.3500 | 0.2400 | 0.3500 | 20 hybrid | 20.00 |

Mode distribution: **100 `hybrid` / 0 `name_fallback` / 0 `unknown_empty`** (was 64/0/36).
`mean_returned` = **20.00**, `mean_mapped` = **20.00**, `n_unmapped` = **0** (every chunk mapped back
to a pool `doc_id`), `n_queries_skipped` = **0**, `warnings` = **[]**. `recall@10 == recall@20` because
each question has exactly one gold document: nothing extra is found by looking past rank 10.

### Batch 2 (cutoff ON) vs batch 3 (cutoff OFF) — same 100 questions

| task | nDCG@10 on → off | recall@10 on → off | MRR@10 on → off |
| --- | --- | --- | --- |
| CosQA | 0.7565 → **0.9690** (Δ +0.2125) | 0.7750 → **1.0000** (Δ +0.2250) | 0.7500 → **0.9583** (Δ +0.2083) |
| CodeEditSearchRetrieval-python | 0.3000 → **0.4131** (Δ +0.1131) | 0.3000 → **0.5000** (Δ +0.2000) | 0.3000 → **0.3833** (Δ +0.0833) |
| CodeEditSearchRetrieval-go | 0.4500 → **0.4882** (Δ +0.0382) | 0.4500 → **0.5500** (Δ +0.1000) | 0.4500 → **0.4696** (Δ +0.0196) |
| CodeEditSearchRetrieval-javascript | 0.2000 → **0.2653** (Δ +0.0653) | 0.2000 → **0.3500** (Δ +0.1500) | 0.2000 → **0.2400** (Δ +0.0400) |
| **Mean(Task)** | 0.4266 → **0.5339** (Δ **+0.1073**) | 0.4312 → **0.6000** (Δ **+0.1688**) | 0.4250 → **0.5128** (Δ **+0.0878**) |

Where the gain comes from (same 100 questions, paired comparison; the two means below are **pooled
over the queries in each subset**, so they are not additive with the `Mean(Task)` row above — the
subset sizes differ from the task sizes):

| batch-2 subset | n | batch-2 mean nDCG@10 | batch-3 mean nDCG@10 |
| --- | ---: | ---: | ---: |
| queries that returned **0** results | 36 | 0.0000 | **0.3287** (17 of 36 now retrieve the gold doc) |
| queries that returned **1** result | 47 | 0.8298 | 0.8432 (46 unchanged — its single hit was already rank 1 — 1 improved, 0 worse) |

So the +0.1073 is **not** a better ranker; it is 36 questions that previously scored a forced 0.0
now being scored on their actual rank.

### Did nDCG finally separate from recall / MRR? (the point of this batch)

| metric | batch 2 (cutoff ON) | batch 3 (cutoff OFF) |
| --- | ---: | ---: |
| Mean nDCG@10 | 0.4266 | **0.5339** |
| Mean recall@10 | 0.4312 | **0.6000** |
| Mean MRR@10 | 0.4250 | **0.5128** |
| spread (max − min) | **0.0062** | **0.0872** |

In batch 2 all three metrics agreed to within 0.0062 — they were measuring the same thing because
every query had at most one result, so "retrieved" forced "rank 1". In batch 3 they are distinct:
**recall@10 − nDCG@10 = +0.0661** and **nDCG@10 − MRR@10 = +0.0211**. Read as two separate
capabilities of codeseek on this task set:

* **retrieval (does the right document make the top 10 at all) = 0.6000**
* **ranking (how high it lands, graded) = 0.5339**
* the gap of 0.0661 is the share lost purely to ordering. Decomposing the 0.4661 of missing nDCG:
  `1 − 0.6000 = 0.4000` is outright retrieval failure (gold never in the top 10) and
  `0.6000 − 0.5339 = 0.0661` is ordering loss among the hits (`0.4000 + 0.0661 = 0.4661`). Ordering
  is therefore the *smaller* of the two problems — codeseek's bottleneck on this task set is recall,
  not rank quality.

nDCG@10 and MRR@10 stay close (+0.0211) **by construction**: with exactly one gold document,
`nDCG@10 = 1/log2(rank+1)` and `MRR@10 = 1/rank`, so the two are monotone transforms of the same
rank and nDCG ≥ MRR for every possible rank. Their nearness is arithmetic, not a defect; the metric
that genuinely decouples is recall.

### Root cause (confirmed at source level, no source change made)

`AdaptiveCutoffConfig` defaults (`rust-core/src/services/hybrid_search.rs:27-64`):
`enabled=true, abs_floor=0.2, rel_ratio=0.55, rel_ratio_rrf=0.40, gap_min=0.30, min_keep=1`.
`adaptive_cutoff()` (same file, `:640-815`) applies a three-gate cascade over normalised scores and
then truncates the list; gate 1 (`abs_floor`) also clamps `min_keep`, i.e.
`final_len = len_gap.max(min_keep.min(len_abs))`. On short queries the second result's relative
score collapses below `rel_ratio_rrf × top1`, so `len_abs = 1` and `min_keep = 1` keeps exactly one
candidate (and, when even the top candidate is below `abs_floor`, zero). `enabled=false` restores the
previous plain `truncate(k_final)` behaviour. There is no CLI flag and no environment variable for
this — `~/.codeseek/config.json` is the only override path, which is why the config was temporarily
patched and then restored (see the note at the end of this file).

Worst 5 queries in batch 3 (all ranked, none truncated — these are real retrieval misses):

| # | task_id | query (abridged) | top3 doc_id (abridged) | gold doc_id | nDCG |
| --- | --- | --- | --- | --- | ---: |
| 1 | `codeeditsearchretrievalgo__q0001` | ``Rename `r` variable to `o` …for `output`…`` | `5241cdb4…`, `0e93f179…`, `949b5960…` | `e992a8d7…` | 0.0 |
| 2 | `codeeditsearchretrievalgo__q0002` | `Test that Add actually works.` | `872b8636…`, `bcb9ce24…`, `777289c1…` | `0d36314b…` | 0.0 |
| 3 | `codeeditsearchretrievalgo__q0003` | `Remove useless function that already exist in strings pakage.` | `bcb9ce24…`, `adedb818…`, `8be6da3b…` | `3125d02c…` | 0.0 |
| 4 | `codeeditsearchretrievalgo__q0005` | `Clean up and improve relevance of example code.` | `8d436c8d…`, `adedb818…`, `f9a8c9a6…` | `95431df4…` | 0.0 |
| 5 | `codeeditsearchretrievalgo__q0009` | `Use absolute value for NTP drift` | `00ab1bb9…`, `78ebc2e8…`, `695d264c…` | `a27a91a9…` | 0.0 |

All five are Go commit-message queries whose diff post-image contains no mention of the words in the
query ("Add", "strings pakage", "NTP drift"); they now return 20 results and simply miss.

Timing (warm index, `--force` re-ran every search but `codeseek init` was an incremental no-op):

| stage | value |
| --- | --- |
| run wall clock | **68.9 s** (batch 2: 306.6 s) |
| `codeseek init`, one per corpus | 0.3 s / 0.3 s / 0.3 s / 0.3 s, **Σ 1.2 s** (batch 2 cold: Σ 236.9 s) |
| search per query | mean **2.68 s**, median 2.54 s, p90 3.37 s, min 1.95 s, max 5.76 s (Σ 268.3 s ÷ 4 workers) |

Exact reproduction (identical to batch 2 except for the config line; the commands below are now also
what `report.md` §5 generates dynamically — the hard-coded template that batch 2's archive had to
work around is gone):

```bash
# 1) turn the adaptive cutoff off (merge, do not rewrite the file)
python3 - <<'PY'
import json, os
p = os.path.expanduser('~/.codeseek/config.json')
d = json.load(open(p))
d.setdefault('index', {}).setdefault('hybrid', {}).setdefault('cutoff', {})['enabled'] = False
json.dump(d, open(p, 'w'), ensure_ascii=False, indent=2)
PY

# 2) same workdir / same questions / same pool as batch 2 → directly comparable
cd evals/mtebcode
python3 -m mtebcode_eval run --workdir /tmp/mtebcode-short --limit 20 --k 10 --concurrency 4 --force

# 3) restore your original config afterwards
cp ~/.codeseek/config.json.mtebcode-backup-<timestamp> ~/.codeseek/config.json
```

**Raw artifacts.** `/tmp/mtebcode-short/raw/` currently holds the **cutoff-OFF** results (batch 3).
The cutoff-ON baseline of batch 2 is preserved separately at `/tmp/mtebcode-short/raw.cutoff-on/`
(100 files, verified same count). The global config was restored to its pre-run state right after the
run; the archive entry above is the durable record, not the tmp directory.

## Smoke run (reference only — not an archive entry, not a benchmark result)

Measured on this machine with `cargo build --release` binary `sha256` prefix `b1532bb1b43e3d65`,
proxy `http://127.0.0.1:7897`, `embedding.model=Qwen/Qwen3-Embedding-4B`:

| item | value |
| --- | --- |
| task set | `CosQA` only |
| sampling | `--limit-queries 5 --corpus-size 60 --seed 42` |
| docs materialized | 60 files (`gold 5 + distractor 55`), `coseseek init` 17.1 s |
| queries / retrieval | 5 queries, 4 `hybrid` + 1 `unknown_empty` |
| mean nDCG@10 | **0.8000** (4 hits at rank 1, 1 query returned no result at all) |
| workdir | `/tmp/mtebcode-smoke` |

That 0.8 is a *5-query* number on a *60-document* pool — it only proves the pipeline runs, and it is
recorded here so nobody mistakes the smoke output for a measurement.
