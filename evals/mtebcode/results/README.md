# MTEB(Code, v1) measured results archive

This directory archives codeseek's end-to-end results on the **MTEB(Code, v1)-style retrieval
subset** produced by `evals/mtebcode` (primary metric **nDCG@10**, overall score = arithmetic mean
of per-task means).

> **Two batches are archived: `2026-11-12` and `2026-11-12b`.** Each is a real 100-query measurement
> with a 300-document pool per task, but pooled corpus + a small query sample per task means neither is
> **comparable with the MTEB(Code, v1) leaderboard** (see the batch sections below). Their task sets
> differ, so **they are not comparable with each other either**.
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
`md == render_markdown(json)` stays verifiable.

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

Exact reproduction of this batch (report.md §5 inside the archived artifact prints the *generic*
template, so the real commands are pinned here):

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
