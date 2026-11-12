# MTEB(Code, v1) measured results archive

This directory archives codeseek's end-to-end results on the **MTEB(Code, v1)-style retrieval
subset** produced by `evals/mtebcode` (primary metric **nDCG@10**, overall score = arithmetic mean
of per-task means).

> **One batch is archived so far (`2026-11-12`).** It is a real 100-query measurement over four
> tasks with a 300-document pool per task — but pooled corpus + a 25-query sample per task means
> it is **not** comparable with the MTEB(Code, v1) leaderboard (see the batch section below).
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
