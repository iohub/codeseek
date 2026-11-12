# MTEB(Code, v1) measured results archive

This directory archives codeseek's end-to-end results on the **MTEB(Code, v1)-style retrieval
subset** produced by `evals/mtebcode` (primary metric **nDCG@10**, overall score = arithmetic mean
of per-task means).

> **It is currently empty on purpose.** No full run has been archived yet: the suite has been
> verified with `selftest` (61 offline checks) and with a 5-query smoke run on `CosQA`
> (see "Smoke run" below), but a smoke run is **not** a benchmark result and must not be
> archived or quoted as one.

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
