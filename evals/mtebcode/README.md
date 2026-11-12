# MTEB(Code, v1) × codeseek end-to-end evaluation suite

Measures codeseek's retrieval quality on **MTEB(Code, v1)-style code retrieval** (the `Code*` task
family of [MTEB](https://github.com/embeddings-benchmark/mteb), `MTEB(Code, v1)` subset, data mirrored
by [CoIR](https://github.com/CoIR-team/coir)). Primary metric: **nDCG@10**.

The suite is a **sibling of `evals/repoqa`** and follows exactly the same architecture, naming and
error-tolerance conventions (three-phase runner, resumable `raw/`, tolerant JSON parsing, report with
fixed sections). It is pure Python standard library — no `pip install` at all (see
`requirements.txt`), and every artifact is written under `--workdir` (default `/tmp/mtebcode-eval`),
so the repository is never polluted.

> ### ⚠️ Not comparable with the MTEB(Code, v1) leaderboard
>
> This is an **end-to-end retrieval** measurement of a **local code-search engine**, not of an
> embedding model, and the evaluation set is deliberately shrunk so it fits on a laptop:
>
> | dimension | MTEB(Code, v1) official | this suite |
> | --- | --- | --- |
> | corpus | full corpus of the task (20k – 280k docs) | **pooled** to `--corpus-size` docs (default **300**) |
> | queries | all evaluation queries (e.g. 500 / 5,851) | `--limit-queries` sample (default **30**) |
> | system | one embedding model + nearest-neighbour search | codeseek binary: tree-sitter chunking + BM25 (+ LanceDB vectors when a token is configured) |
> | metric | nDCG@10 per task, Mean(Task) | same formula, but on the reduced set |
>
> Pooling makes ranking **easier** (fewer distractors) while the query sample makes the number
> **noisier**. Report the numbers as "codeseek on a pooled 300-doc / 30-query subset", never as
> "MTEB(Code, v1) score".

---

## 1. Pipeline

```
                     ┌──────────────────────────────────────────────────────────────────────┐
  datasets-server ──►│ data.DatasetClient                                                   │
  (JSON REST API)    │   /splits  /rows?offset&length(≤100)  /size                          │
                     │   • disk cache  <workdir>/cache/<ds>/<cfg>/<split>/<offset>.json     │
                     │   • retry 1s/2s/4s (502s and flaky auth are real, see §4)            │
                     │   • proxy: --proxy or https_proxy (direct access times out)          │
                     └───────────────────────────────────┬──────────────────────────────────┘
                                                         ▼
                     ┌──────────────────────────────────────────────────────────────────────┐
  prepare ──────────►│ qrels(test→valid→train) → sample queries (seed) → corpus pool        │
                     │   pool = all gold docs ∪ random distractors, up to --corpus-size     │
                     │   fetch gold docs: positional fast path, else early-stop scan        │
                     │ materialize corpora/<task_slug>/docs/<safe_id>_<sha1[:8]><ext>       │
                     │   + docmap.json {relative path → doc_id} + git init                  │
                     │ → tasks.jsonl + meta.json                                            │
                     └───────────────────────────────────┬──────────────────────────────────┘
                                                         ▼
                     ┌──────────────────────────────────────────────────────────────────────┐
  run ──────────────►│ runner.run                                                           │
                     │  1. serial:   codeseek init   (once per corpus dir, skipped if done) │
                     │  2. parallel: codeseek search "<query>" --json --limit K             │
                     │       adapter.parse_json_tolerant → results                          │
                     │       write raw/<task_id>.json (.json.tmp → atomic rename)           │
                     │  3. adapter.resolve_doc_ids → dedup-preserving ranked doc_id list     │
                     │  4. scorer.score_tasks → metric.nDCG@10 / recall / MRR               │
                     │  5. report.build_report / write_reports                               │
                     └───────────────────────────────────┬──────────────────────────────────┘
                                                         ▼
                          report.json (machine-readable) + report.md (human-readable)
                                                         ▲
  score ────────────►│ re-score the existing raw/*.json only (new k / new limit)            │
```

## 2. Prerequisites

| dependency | notes |
| --- | --- |
| Python ≥ 3.10 | standard library only; type hints use `X \| None` |
| network **through a proxy** | `--proxy http://127.0.0.1:7897` or `export https_proxy=...`; direct access to `datasets-server.huggingface.co` times out from CN networks |
| codeseek binary | `cargo build --release --manifest-path rust-core/Cargo.toml` → `rust-core/target/release/codeseek` (or `--codeseek-bin` / `CODESEEK_BIN` / `--auto-build`); only needed for `run` |
| git | `prepare` runs `git init` inside every corpus dir — codeseek `init` fails with `No project found` outside a git repository |
| embedding token (**decides whether scores are meaningful**) | semantic retrieval only runs when `embedding.api_token` in `~/.codeseek/config.json` is non-empty; otherwise search degrades to function-name substring matching and the report is stamped with a warning |

## 3. The three commands

```bash
cd evals/mtebcode

# (1) data + sampling + materialization  (the only step that needs the network)
python -m mtebcode_eval prepare --workdir /tmp/mtebcode-eval \
    --tasks CosQA,CodeSearchNetRetrieval,SyntheticText2SQL \
    --limit-queries 30 --corpus-size 300 --seed 42 \
    --proxy http://127.0.0.1:7897

# (2) retrieval evaluation (init → search → score → report)
python -m mtebcode_eval run --workdir /tmp/mtebcode-eval \
    --limit 20 --concurrency 4 --timeout 180

# (3) re-score the same raw results with another k (no retrieval re-run)
python -m mtebcode_eval score --workdir /tmp/mtebcode-eval --limit 20 --k 10

# offline sanity check of the whole logic (no network, no binary)
python -m mtebcode_eval selftest
```

A CI-friendly smoke run (data link only + 2 queries per task):

```bash
python -m mtebcode_eval prepare --workdir /tmp/mtebcode-smoke --tasks CosQA \
    --limit-queries 5 --corpus-size 60 --proxy http://127.0.0.1:7897
python -m mtebcode_eval run --workdir /tmp/mtebcode-smoke \
    --limit 10 --limit-queries 2 --concurrency 1 --timeout 120
```

Exit codes: `0` success; `2` precondition missing (`tasks.jsonl` absent, binary not found,
`--require-embedding` unmet, unknown task name, data source unreachable).

**When `datasets-server` rate-limits you** (`/rows` returns HTTP 429 even after 6 retries — measured),
pass `--parquet-cache` so that any `(config, split)` already present in your local HF cache is read
straight from parquet, with no network request:

```bash
python -m mtebcode_eval prepare --workdir /tmp/mtebcode-short \
    --tasks CosQA,CodeEditSearchRetrieval-python,CodeEditSearchRetrieval-go,CodeEditSearchRetrieval-javascript \
    --corpus-size 300 --seed 42 --proxy http://127.0.0.1:7897 \
    --parquet-cache ~/.cache/huggingface/hub
```

(Per-task query counts live in the registry as `TaskSpec.query_quota`, so `--limit-queries` is only the
fallback for tasks that do not set it: batch 2 is 40/20/20/20 = 100 with `--limit-queries 20`.)

### CLI flags

| command | flags |
| --- | --- |
| `prepare` | `--workdir`(=`/tmp/mtebcode-eval`) `--tasks`(=`CosQA,CodeSearchNetRetrieval,SyntheticText2SQL`) `--corpus-size`(300) `--limit-queries`(30) `--seed`(42) `--proxy` `--parquet-cache` `--cache-only` `--rate-limit`(0.15) `--retries`(4) `--force` `--quiet` |
| `run` | `--workdir` `--codeseek-bin` `--repo-root` `--limit`(20) `--k`(10) `--limit-queries`(0=all, **per task**) `--concurrency`(4) `--timeout`(180) `--init-timeout`(1800) `--force` `--auto-build` `--require-embedding` `--quiet` |
| `score` | `--workdir` `--limit`(20) `--k`(10) `--limit-queries`(0=all, per task) `--codeseek-bin` `--repo-root` |
| `selftest` | none (fully offline) |

## 4. Data source (measured, not assumed)

Base: `https://datasets-server.huggingface.co` — the HuggingFace **datasets-server JSON REST API**,
called with `urllib.request`. No `datasets`, no `huggingface_hub`, no `pyarrow`.

| endpoint | purpose | notes |
| --- | --- | --- |
| `GET /splits?dataset=<id>` | enumerate `(config, split)` | used to validate the task registry at `prepare` time |
| `GET /rows?dataset&config&split&offset&length` | read rows | **`length` hard limit is 100**; 101 returns HTTP 400 `Parameter 'length' must not be greater than 100` |
| `GET /size?dataset=<id>` | dataset scale | stored in `meta.json.tasks[].dataset_size` |

Field names are uniform: `corpus` / `queries` rows carry `_id, text, title, partition, language`
(`SyntheticText2SQL` queries additionally carry `context` = SQL schema); `qrels` rows carry
`query-id, corpus-id, score` (int, `1` = relevant).

Robustness that is actually needed in practice (all implemented):

1. **Proxy** — `--proxy` wins over `https_proxy` / `HTTP_PROXY`; implemented with
   `urllib.request.ProxyHandler` in a **local** opener, never touching global state.
2. **Retry** — 4 attempts by default with 1s/2s/4s backoff. Real observed failures: HTTP **502** from
   the proxy on `synthetic-text2sql` corpus / `cosqa` pages, and occasional auth-ish errors on
   `CodeSearchNet` qrels. The final error message contains the URL, the HTTP status and the first
   300 characters of the response body.
3. **Rate limit** — `--rate-limit 0.15` sleeps 0.15 s between real requests (cache hits do not sleep).
4. **Cache** — every page is written to `<workdir>/cache/<dataset>/<config>/<split>/<offset>.json`
   atomically (`.part` → `rename`), so a re-run is free and `--cache-only` can replay offline.

## 5. Task registry

`mtebcode_eval/data.py::TASKS`. Every entry below was verified against `/splits` on this machine;
`unverified: true` is the reserved escape hatch for a future task whose layout is not yet confirmed
(selecting it raises a clear error instead of crashing).

| MTEB task | HF dataset | corpus config/split | queries config/split | qrels config/split (chosen) |
| --- | --- | --- | --- | --- |
| **CosQA** | `CoIR-Retrieval/cosqa` | `corpus`/`corpus` | `queries`/`queries` | `default`/**`test`** (500 rows) |
| **CodeSearchNetRetrieval** | `CoIR-Retrieval/CodeSearchNet` | `python-corpus`/`corpus` | `python-queries`/`queries` | `python-qrels`/**`test`** (14,918 rows) |
| **SyntheticText2SQL** | `CoIR-Retrieval/synthetic-text2sql` | `corpus`/`corpus` | `queries`/`queries` | `default`/**`test`** (5,851 rows) |
| CodeFeedbackST | `CoIR-Retrieval/codefeedback-st` | `corpus`/`corpus` | `queries`/`queries` | `default`/`test` |
| CodeFeedbackMT | `CoIR-Retrieval/codefeedback-mt` | `corpus`/`corpus` | `queries`/`queries` | `default`/`test` |
| AppsRetrieval | `CoIR-Retrieval/apps` | `corpus`/`corpus` | `queries`/`queries` | `default`/`test` |
| StackOverflowQA | `CoIR-Retrieval/stackoverflow-qa` | `corpus`/`corpus` | `queries`/`queries` | `default`/`test` |
| CodeTransOceanContest | `CoIR-Retrieval/codetrans-contest` | `corpus`/`corpus` | `queries`/`queries` | `default`/`test` |
| CodeTransOceanDL | `CoIR-Retrieval/codetrans-dl` | `corpus`/`corpus` | `queries`/`queries` | `default`/`test` |
| CodeSearchNetCCRetrieval | `CoIR-Retrieval/CodeSearchNet-ccr` | `python-corpus`/`corpus` | `python-queries`/`queries` | `python-qrels`/`test` |
| **CodeEditSearchRetrieval-python** | `mteb/CodeEditSearchRetrieval` | `python-corpus`/`train` | `python-queries`/`train` | `python-qrels`/**`train`** (1,000 rows) |
| **CodeEditSearchRetrieval-go** | `mteb/CodeEditSearchRetrieval` | `go-corpus`/`train` | `go-queries`/`train` | `go-qrels`/**`train`** (1,000 rows) |
| **CodeEditSearchRetrieval-javascript** | `mteb/CodeEditSearchRetrieval` | `javascript-corpus`/`train` | `javascript-queries`/`train` | `javascript-qrels`/**`train`** (1,000 rows) |

Details worth knowing:

* **`CodeSearchNet` / `CodeSearchNet-ccr` are multi-language** (`go/java/javascript/php/python/ruby`
  sub-configs). The registry pins the **python** sub-config; change `_lang_layout("python")` to
  evaluate another language.
* **`mteb/CodeEditSearchRetrieval`** is registered as **three per-language subtasks** (not one task).
  Measured config availability (`/splits` + `/rows` + the repo's own parquet files, all cross-checked):

  | sub-config | corpus | queries | qrels | usable? |
  | --- | --- | --- | --- | --- |
  | `python` | ✅ 1,000 rows | ✅ 1,000 rows | ✅ 1,000 rows | **yes** |
  | `go` | ✅ | ✅ | ✅ | **yes** |
  | `javascript` | ✅ | ✅ | ✅ | **yes** |
  | `java` | HTTP 500 | HTTP 500 | **config does not exist** (404; no `java-qrels` parquet either) | no |
  | `ruby` | HTTP 500 | HTTP 500 | HTTP 500 | no (`.rb` is also outside the indexer whitelist) |
  | `typescript` | HTTP 500 | HTTP 500 | HTTP 500 (parquet unreadable server-side) | no |

  Its corpus `text` is **100% a unified diff** (`1000/1000` rows start with `---`/`@@`), so both its
  `_id`s (40-char hashes → scan fallback) and its materialization (→ `diff_postimage`, see §6) are
  special. `query_length_chars` per task is written into `meta.json`, so the short-query premise stays
  auditable.
* **qrels split rule**: `test` → `valid` → `train`, first available wins; the split actually used is
  recorded per task in `meta.json.tasks[].qrels_split` and in every `report.json.meta.dataset_splits`.
  Correctness cross-check: `cosqa` test has exactly **500** qrels rows and `synthetic-text2sql` test
  has exactly **5,851** — both match the query counts MTEB reports for those tasks. For
  `CodeSearchNet` the python test qrels has **14,918 rows**, which is more than a query count because a
  single query can have several graded documents.

### 5.1 Task filtering: the batch-2 "short query only" set

codeseek's retrieval **fails on long queries**: measured directly, an AppsRetrieval query of 1,316
characters returns `[]` while its first line truncated to 152 characters returns 3 hits. Batch 1
(`results/2026-11-12-report.md`) therefore reported AppsRetrieval 0.2252 and CodeTransOceanDL 0.2482 —
those numbers measure the length cut-off, not retrieval quality. Batch 2 selects **only short-query
tasks**, using the measured character length of the query distribution:

| MTEB(Code) task | query length min / median / p90 / max (chars) | decision |
| --- | --- | --- |
| **CosQA** | 18 / **34** / 42 / 62 (sampled 40; 35 over the full 500) | ✅ kept |
| **CodeEditSearchRetrieval-python** | 21 / **45** / 128 / 1,811 (full set) | ✅ kept |
| **CodeEditSearchRetrieval-go** | 19 / **45** / 139 / 1,529 | ✅ kept |
| **CodeEditSearchRetrieval-javascript** | 18 / **43** / 87 / 601 | ✅ kept |
| SyntheticText2SQL | 25 / 79 / 130 / 419 | ❌ corpus is `.sql`, outside the indexer whitelist |
| CodeSearchNetRetrieval | — / 624 / — / — | ❌ query too long **and** corpus is the docstring |
| CodeFeedbackST | — / 540 / — / — | ❌ query too long |
| CodeTransOceanContest | — / 547 / — / — | ❌ query too long |
| StackOverflowQA | — / 883 / — / — | ❌ query too long (34.2% of query text is prose) |
| CodeTransOceanDL | — / 1,211 / — / — | ❌ query too long |
| AppsRetrieval | 50 / 1,316 / 2,491 / 13,955 | ❌ query too long (98.9% > 200 chars) |
| CodeFeedbackMT | — / 3,571 / — / — | ❌ query too long |

Excluded entries stay in the registry with an `excluded_reason` field; selecting one raises a clear
error (`--tasks AppsRetrieval` → *"已被实测剔除…"*) instead of quietly polluting a batch.

Query budget: the intended set was 4 languages × 20 + CosQA 20 = 100, but only **3** of the 4 requested
languages have a complete corpus/queries/qrels triple, so the fallback rule applies:
**CosQA 40 + 3 × 20 = 100**. That is expressed per task via `TaskSpec.query_quota` (a global
`--limit-queries` cannot say "40 here, 20 there").


## 6. Subset & materialization protocol (the core design)

A full MTEB corpus is 20k–280k documents; indexing all of them is out of scope for a laptop-sized
check, so the suite uses a **pooled corpus**, the same philosophy as repoqa's "100-task subset".

`prepare`, per task:

1. Read the qrels of the evaluation split → `query_id → {doc_id: score}` (keep `score > 0` only).
2. **Deterministic query sampling**: sort query ids, then `random.Random(stable_seed(seed, task))`
   `.sample(..., --limit-queries)` — reruns are byte-identical (recorded as `sampling_fingerprint`).
3. **Pool construction**: `pool = all gold docs of the sampled queries ∪ random distractors`, up to
   `--corpus-size`. If the gold docs alone exceed the pool limit, they are truncated at the limit and
   a warning is recorded (`gold_truncated: true`).
4. **Fetch only pool documents**, with an early stop. Two paths:
   * **positional fast path** — CoIR `_id`s follow `prefix + (row index + delta)` (measured: `cosqa`
     offset 0 → `d1,d2,d3`; `CodeSearchNet` python offset 1000 → `c1000`). Two or three 1-row probes
     verify the pattern, then the page offset of each gold doc is computed directly, so only the pages
     containing gold docs are fetched — this is what avoids scanning 280k rows;
   * **scan fallback** — if ids are hashes (`mteb/CodeEditSearchRetrieval`) or probes disagree,
     pages are scanned sequentially and the scan **stops as soon as every wanted id is found**.
   Both paths record `mode` / `pages` / `rows_scanned` in `meta.json.tasks[].fetch`.
5. **Materialize** every pool document as a real file under
   `<workdir>/corpora/<task_slug>/docs/`:
   * filename = sanitized `_id` (non `[A-Za-z0-9._-]` → `_`) + `_` + `sha1(doc_id)[:8]` + extension
     (`.py` for CosQA / CodeSearchNet / CodeEdit, `.sql` for Text2SQL, `.txt` otherwise) — the short
     hash prevents collisions after sanitization;
   * content = `text`; when a non-empty `title` exists **and** the file is code (`.py/.java/.js/.ts/.go/.rs/.cpp`),
     a first-line comment is prepended (`#` for Python, `//` otherwise). `.sql` / `.txt` are **never**
     annotated, so SQL text is not polluted;
   * **diff corpora** (`TaskSpec.diff_postimage=True`, currently the three `CodeEditSearchRetrieval-*`
     tasks): the text is a unified diff, so it is first converted to the **post-image** code — `@@`,
     `---`, `+++` and `\ No newline` lines are dropped, `-` lines are dropped, and exactly one leading
     `+` / `（space）` prefix character is stripped from the kept lines (this preserves relative
     indentation). Only patch markers are removed; no code character is altered. Measured effect on 30
     documents: raw diff → 9 indexed files / 11 symbols, post-image → 17 files / 80 symbols. The flag is
     recorded in `meta.json.tasks[].materialization` and asserted in `selftest`;
   * `docmap.json` = `{relative POSIX path: doc_id}` — scoring looks up ids here and **never guesses
     from the path**;
   * `git init` in the corpus dir (`codeseek init` hard prerequisite).
6. Write `tasks.jsonl`, one line per query:
   `{"task_id","task","hf_dataset","query_id","query_text","gold_doc_ids","corpus_dir","docmap_path","n_pool_docs","qrels","qrels_split","language"}`
   (`task_id` = `<task_slug>__q<idx:04d>`, e.g. `cosqa__q0007`; `qrels` is automatically restricted to
   docs that actually exist in the pool — a gold doc missing from the pool can never be retrieved, so
   scoring it as a miss would be unfair, and such queries are counted as *skipped*).
7. Write `meta.json`: task list, per-task config/split/corpus size/query sample/seed/pool size,
   dataset scale stats, fetch statistics, warnings, `sampling_fingerprint`.

## 7. Scoring protocol

### 7.1 From search results to a ranked doc list

`adapter.resolve_doc_ids(results, docmap, corpus_dir)` maps every `codeseek search` hit to a `doc_id`
by four descending rules:

1. `file_path` / `path` (normalized: backslashes → `/`, `./` stripped, `corpus_dir` prefix removed,
   suffix matched against docmap keys) → `docmap` lookup;
2. `os.path.basename` reverse match against every `docmap` key;
3. the hit's `name` / `symbol_name` equals some `doc_id` (fallback);
4. otherwise unmapped (`n_unmapped`), and the hit is dropped from the ranking.

**Dedupe-preserving order**: the same `doc_id` keeps only its first rank — codeseek may return several
chunks of one file, and without deduplication a single file could fill the whole Top-10 and inflate
nDCG.

### 7.2 Metrics

```
DCG@k  = Σ_{i=0}^{k-1} (2^{rel_i} − 1) / log2(i + 2)
IDCG@k = DCG@k over the ideal (descending) qrels ordering
nDCG@k = DCG@k / IDCG@k          (0.0 if IDCG == 0)
```

`recall@k`, `MRR@k` and `precision@k` use the same relevance definition. A query whose qrels (inside
the pool) have **no positive document returns `None` and is skipped** — it does not contribute `0.0`
to any mean. Primary metric: **nDCG@10**; `recall@k` is additionally reported with `k = --limit`
(the search cut-off), so you can tell ranking problems from recall problems.

### 7.3 Aggregation (MTEB `Mean(Task)` semantics)

* task score = **mean nDCG@10 over that task's scoreable queries**;
* overall score = **arithmetic mean of the task scores** — *not* a pooled mean over all queries,
  otherwise tasks with more queries would dominate. `report.json.aggregate.aggregation` says
  `"mean_of_task_means"` explicitly, and `report.md` labels the row `Mean(Task)`.

### 7.4 Retrieval modes

| mode | meaning |
| --- | --- |
| `hybrid` | results contain `code_block` → genuine semantic retrieval (LanceDB + BM25) |
| `name_fallback` | no `code_block` → `embedding.api_token` missing, codeseek degraded to function-name substring matching |
| `unknown_empty` | empty / unparsable results — cannot be judged |

If `name_fallback` share is > 0 the report **opens with a blockquote warning**: those scores are not
comparable with MTEB and do not reflect codeseek's semantic retrieval ability.

## 8. Report format

`report.json`

```jsonc
{
  "meta": {
    "generated_at": "…", "benchmark": "MTEB(Code, v1)", "command": "run|score",
    "tasks": ["CosQA", "…"], "n_queries": 90, "n_docs_total": 900,
    "k": 10, "search_limit": 20, "seed": 42, "corpus_size": 300, "limit_queries": 30,
    "dataset_splits": {"CosQA": "test", "…": "test"},
    "codeseek_binary": {"path": "…", "mtime_iso": "…", "sha256_prefix": "…"},
    "embedding": {"configured": true, "model": "Qwen/Qwen3-Embedding-4B", "note": "…"},
    "mode_counts": {"hybrid": 88, "name_fallback": 0, "unknown_empty": 2},
    "mode_ratio": {"hybrid": 0.977, "…": 0.0},
    "sampling_fingerprint": "…", "prepare_warnings": ["…"]
  },
  "aggregate": {
    "mean_ndcg_at_10": 0.8123,                     // = mean of by_task scores
    "mean_recall_at_10": 0.85, "mean_mrr_at_10": 0.79, "mean_recall_at_k": 0.9,
    "n_tasks": 3, "n_queries": 90, "n_queries_scored": 88, "n_queries_skipped": 2,
    "aggregation": "mean_of_task_means",
    "by_task": {"CosQA": {"n_queries": 30, "n_scored": 30, "ndcg_at_10": 0.79,
                          "recall_at_10": …, "mrr_at_10": …, "recall_at_k": …,
                          "mode_counts": {"hybrid": 30}, "mean_returned": 18.4,
                          "mean_mapped": 18.4, "mean_unmapped": 0.0, "with_errors": 0}}
  },
  "per_query": [{"task_id": "cosqa__q0000", "task": "CosQA", "query_id": "q20208",
                 "query_text": "…", "ndcg_at_10": 0.0, "recall_at_10": 0.0, "mrr_at_10": 0.0,
                 "recall_at_k": 0.0, "mode": "hybrid", "n_returned": 0, "n_mapped": 0,
                 "n_unmapped": 0, "top5_doc_ids": [], "gold_doc_ids": ["d20208"],
                 "qrels_size": 1, "skipped": false, "error": "", "elapsed_s": 1.9}],
  "failure_top": [{"task_id": "…", "ndcg_at_10": 0.0, "retrieved_not_gold": ["d17"], "…": "…"}],
  "warnings": ["…"]
}
```

`report.md` has exactly five sections: **1. Summary**, **2. Retrieval Mode Statistics**,
**3. Per-Task Breakdown**, **4. Top 10 Worst Queries**, **5. Reproduction Commands** — plus the
`name_fallback` blockquote at the top when applicable.

## 9. Known limitations

* **Pooling lowers difficulty.** 300 documents instead of 20k–280k means far fewer plausible
  distractors; absolute nDCG is therefore optimistic and *not* an MTEB score.
* **Query sample is small.** 30 queries per task gives ~±0.05–0.1 nDCG noise; treat differences below
  that as ties. Increase `--limit-queries` for a tighter number (cost grows linearly).
* **Document granularity ≠ chunk granularity.** codeseek returns *chunks* (functions/classes). A hit
  counts as correct once the chunk's file maps back to the gold `doc_id`, so a file that is "mostly
  right" but from another function still counts — this is more forgiving than a chunk-level benchmark,
  and it is why dedupe-by-doc is mandatory for a sane Top-10.
* **`name_fallback` degrades everything.** Without `embedding.api_token`, queries become function-name
  substring matches: natural-language queries (CosQA, Text2SQL) score near zero. Use
  `--require-embedding` to make that a hard error.
* **SQL corpora may index poorly.** `.sql` documents are materialized as plain text (no comment
  injection, by design); codeseek's tree-sitter chunking has no SQL grammar, so chunks fall back to
  coarser units.
* **Chunk-level scores are not exported.** Chunk text is stored in `raw/` only as a character count
  (`code_block_chars`) to keep raw files small; if you need the blocks, re-run with a patched
  `runner._KEEP_FIELDS`.
* **Positional id fast path is an optimization, not an assumption.** It is verified by probes at
  `offset 0 / mid / last` before use; if a dataset changes its id scheme, the code silently falls back
  to the (slower) early-stop scan and the path used is visible in `meta.json`.
* **`git init` snapshot, not real history.** `codeseek init` only needs a git repo; commit history,
  blame or diffs are not available to the indexer.
* **Single-run variance from the service side.** `coseseek search` talks to the embedding API; identical
  queries can occasionally return empty results (`unknown_empty`). Those are kept in the report as
  `0.0` scores, never silently dropped.
* **`index.hybrid.cutoff` trims hard.** With `abs_floor 0.2` / `rel_ratio 0.55` / `min_keep 1`, a query
  often keeps **one** candidate: batch 2 measured `mean_returned` = 1.00 on CosQA (26 of 40 queries got
  exactly one hit), so nDCG@10 ≈ recall@10 ≈ MRR@10 collapses into a rank-1 hit rate. If you need to
  separate ranking from recall, raise `rrf_top_k` / lower the cut-off in `~/.codeseek/config.json` — the
  numbers here are what the shipped defaults produce.
* **Diff-hunk corpora are only post-images.** `CodeEditSearchRetrieval` documents are hunks (median
  11–14 lines, 259–300 of 300 under 30 lines), not whole files: a commit-message query describes the
  *change*, the document contains only the changed region. Batch 2 shows the consequence honestly
  (0.20–0.45 nDCG@10 with 35–55% empty results); it is a property of the task, not a bug in the harness.

## 10. Directory layout

```
evals/mtebcode/
├── README.md                  # this file
├── requirements.txt           # "no third-party deps required"
├── .gitignore
├── results/
│   └── README.md              # archive convention (no full run archived yet)
└── mtebcode_eval/
    ├── __init__.py            # __version__ + module map
    ├── __main__.py            # from .cli import main; sys.exit(main())
    ├── cli.py                 # argparse: prepare / run / score / selftest
    ├── data.py                # REST client, cache, task registry, sampling, materialization
    ├── adapter.py             # codeseek binary: init / search / tolerant JSON / doc_id resolution
    ├── metric.py              # nDCG@k, recall@k, MRR@k, precision@k (stdlib only)
    ├── scorer.py              # results → doc_id ranking → metrics → Mean(Task) aggregation
    ├── runner.py              # serial init + concurrent search + resumable raw + report
    ├── report.py              # report.json / report.md
    └── selftest.py            # offline self-check (69 assertions, prints SELFTEST OK)
```

`--workdir` layout produced at run time:

```
<workdir>/
├── cache/<dataset>/<config>/<split>/<offset>.json   # raw API pages (reused on re-run)
├── corpora/<task_slug>/                             # one git repo per task
│   ├── docs/<safe_doc_id>_<sha1[:8]><ext>
│   ├── docmap.json
│   └── .git/
├── raw/<task_id>.json                               # per-query retrieval results (resumable)
├── tasks.jsonl
├── meta.json                                        # prepare-stage protocol record
├── report.json
└── report.md
```

## 11. Verification status

| check | result |
| --- | --- |
| `python -m mtebcode_eval selftest` | **`SELFTEST OK`** — 69 assertions, exit 0 (metrics, tolerant parsing, doc_id resolution, materialization incl. `diff_postimage`, layout detection, end-to-end report, CLI wiring, excluded-task rejection, error paths) |
| `python -m py_compile mtebcode_eval/*.py` | clean |
| `--help` for the CLI and all four subcommands | clean |
| real `prepare` smoke (`CosQA`, 5 queries, 60 docs, proxy `127.0.0.1:7897`) | 60 files materialized, `docmap.json` 60 entries, `tasks.jsonl` 5 lines, `meta.json` written, `git init` done; 20 API pages cached; the positional fast path needed 3 corpus pages instead of scanning 20,604 rows |
| real `run` smoke (same workdir, `--limit 10 --limit-queries 5`) | `coseseek init` 17.1 s, 5 queries: 4 `hybrid` + 1 `unknown_empty`, mean **nDCG@10 = 0.8000** (this is a smoke number, not a benchmark result) |
| `score` re-scoring | reproduces the same numbers with `command=score`, `k` configurable |
| `prepare --cache-only` | replays entirely from `cache/` with no network |
| real 100-query run, batch `2026-11-12b` (`CosQA` 40 + `CodeEditSearchRetrieval-{python,go,javascript}` 20 each, 4 × 300 docs) | `prepare` 100/100 tasks, `run` **Mean(Task) nDCG@10 = 0.4266**, 64 `hybrid` / 36 `unknown_empty` / 0 `name_fallback`, wall 306.6 s (init Σ 236.9 s, search mean 2.75 s), archived as `results/2026-11-12b-report.{json,md}` |
| local parquet source (`--parquet-cache`) | 141 pages served from parquet + 1 REST request; byte-identical row shape to `/rows` (same `_id`/`text`/`title` and `query-id`/`corpus-id`/`score` fields), so `prepare` needs no code branch |

---

The sibling suite `evals/repoqa` measures the same binary on RepoQA (BLEU + 10-needle candidate set);
this suite measures it as a **retriever** (ranking quality). Sharing one architecture keeps the two
comparable in *method*, while their numbers answer different questions.
