"""报告输出：``report.json``（机器可读）+ ``report.md``（英文可读）。

``report.md`` 固定 5 个小节（与 README 的「打分协议」一一对应）：

1. **Summary** —— 总体 Mean nDCG@10 + 按任务表；
2. **Retrieval Mode Statistics** —— ``hybrid`` / ``name_fallback`` / ``unknown_empty`` 占比；
3. **Per-Task Breakdown** —— 每任务的 nDCG / recall / MRR / 映射率；
4. **Top 10 Worst Queries** —— 按 nDCG 升序，附「检索到但未命中」的样例；
5. **Reproduction Commands**。

若 ``name_fallback`` 占比 > 0，**开头置顶 blockquote 警告**：语义检索未启用，分数不可与
MTEB 官方榜单比较。分数统一保留 4 位小数（nDCG 的差异粒度远小于 accuracy 的百分数）。
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

from .data import BENCHMARK, REPORT_JSON, REPORT_MD

__all__ = ["build_report", "render_markdown", "write_reports"]


def _fmt(value: Any, digits: int = 4) -> str:
    """``None`` → ``n/a``，数字 → 固定小数位。"""
    if value is None:
        return "n/a"
    try:
        return f"{float(value):.{digits}f}"
    except (TypeError, ValueError):
        return "n/a"


def _ratio(part: int, total: int) -> float:
    return (part / total) if total else 0.0


def build_report(
    scoring: Mapping[str, Any],
    raws: Mapping[str, dict[str, Any]],
    meta: Mapping[str, Any],
) -> dict[str, Any]:
    """把评分结果 + raw 元信息合成最终报告 dict。

    :param scoring: ``scorer.score_tasks`` 的返回值。
    :param raws: ``task_id -> raw``（补 ``retrieved_at`` 等运行期字段）。
    :param meta: run / score 阶段的元信息（二进制指纹、数据集 split、embedding 状态等）。
    """
    per_query: list[dict[str, Any]] = []
    for row in scoring.get("per_query", []):
        raw = raws.get(row["task_id"]) or {}
        per_query.append({**row, "retrieved_at": raw.get("retrieved_at", "")})

    aggregate_src = scoring.get("aggregate", {}) or {}
    by_task_src = scoring.get("by_task", {}) or {}

    by_task: "dict[str, dict[str, Any]]" = {}
    for task, bucket in by_task_src.items():
        by_task[task] = {
            "n_queries": bucket.get("n_queries", 0),
            "n_scored": bucket.get("n_scored", 0),
            "n_skipped": bucket.get("n_skipped", 0),
            "ndcg_at_10": bucket.get("ndcg_at_10"),
            "recall_at_10": bucket.get("recall_at_10"),
            "mrr_at_10": bucket.get("mrr_at_10"),
            "recall_at_k": bucket.get("recall_at_k"),
            "k": bucket.get("k", scoring.get("k", 10)),
            "mode_counts": bucket.get("mode_counts", {}),
            "mean_returned": bucket.get("mean_returned"),
            "mean_mapped": bucket.get("mean_mapped"),
            "mean_unmapped": bucket.get("mean_unmapped"),
            "with_errors": bucket.get("with_errors", 0),
        }

    mode_counts: dict[str, int] = dict(meta.get("mode_counts") or aggregate_src.get("mode_counts") or {})
    total_queries = len(per_query)
    fallback_ratio = _ratio(int(mode_counts.get("name_fallback", 0)), total_queries)

    failures = sorted(
        (row for row in per_query if row.get("ndcg_at_10") is not None),
        key=lambda row: (float(row.get("ndcg_at_10") or 0.0), row.get("task_id", "")),
    )
    failure_top: list[dict[str, Any]] = []
    for row in failures[:10]:
        gold = set(row.get("gold_doc_ids") or [])
        top5 = [doc for doc in (row.get("top5_doc_ids") or []) if doc not in gold]
        failure_top.append(
            {
                "task_id": row.get("task_id"),
                "task": row.get("task"),
                "query_id": row.get("query_id"),
                "query_text": (row.get("query_text") or "")[:200],
                "ndcg_at_10": row.get("ndcg_at_10"),
                "recall_at_10": row.get("recall_at_10"),
                "mode": row.get("mode"),
                "n_returned": row.get("n_returned"),
                "n_mapped": row.get("n_mapped"),
                "n_unmapped": row.get("n_unmapped"),
                "gold_doc_ids": sorted(gold)[:5],
                "retrieved_not_gold": top5[:5],
                "error": (row.get("error") or "")[:200],
            }
        )

    report_meta = {
        **dict(meta),
        "generated_at": meta.get("generated_at")
        or datetime.now(tz=timezone.utc).isoformat(timespec="seconds"),
        "benchmark": meta.get("benchmark") or BENCHMARK,
        "command": meta.get("command") or "unknown",
        # 抽样口径：prepare 阶段的取值；直接用 score_only 打分时可能是 None（字段仍然存在）
        "seed": meta.get("seed"),
        "corpus_size": meta.get("corpus_size"),
        "limit_queries": meta.get("limit_queries"),
        "tasks": list(meta.get("tasks") or sorted(by_task)),
        "dataset_splits": dict(meta.get("dataset_splits") or {}),
        "n_queries": meta.get("n_queries", total_queries),
        "k": int(meta.get("k") or scoring.get("k") or 10),
        "search_limit": int(meta.get("search_limit") or scoring.get("search_limit") or 20),
        "mode_counts": mode_counts,
        "mode_ratio": {
            "hybrid": _ratio(int(mode_counts.get("hybrid", 0)), total_queries),
            "name_fallback": fallback_ratio,
            "unknown_empty": _ratio(int(mode_counts.get("unknown_empty", 0)), total_queries),
        },
    }

    aggregate = {
        "mean_ndcg_at_10": aggregate_src.get("mean_ndcg_at_10"),
        "mean_recall_at_10": aggregate_src.get("mean_recall_at_10"),
        "mean_mrr_at_10": aggregate_src.get("mean_mrr_at_10"),
        "mean_recall_at_k": aggregate_src.get("mean_recall_at_k"),
        "n_tasks": aggregate_src.get("n_tasks", len(by_task)),
        "n_tasks_scored": aggregate_src.get("n_tasks_scored", len(by_task)),
        "n_queries": aggregate_src.get("n_queries", total_queries),
        "n_queries_scored": aggregate_src.get("n_queries_scored", total_queries),
        "n_queries_skipped": aggregate_src.get("n_queries_skipped", 0),
        "k": report_meta["k"],
        "search_limit": report_meta["search_limit"],
        "aggregation": aggregate_src.get("aggregation", "mean_of_task_means"),
        "by_task": by_task,
    }

    return {
        "meta": report_meta,
        "aggregate": aggregate,
        "per_query": per_query,
        "failure_top": failure_top,
        "warnings": _warnings(report_meta, mode_counts, fallback_ratio, per_query),
    }


def _warnings(
    meta: Mapping[str, Any],
    mode_counts: Mapping[str, int],
    fallback_ratio: float,
    per_query: Sequence[dict[str, Any]],
) -> list[str]:
    """生成需要在报告顶部醒目标注的警示。"""
    warnings: list[str] = []
    if mode_counts.get("name_fallback", 0) > 0:
        warnings.append(
            f"{mode_counts['name_fallback']} queries ({fallback_ratio * 100:.1f}%) went through "
            "`name_fallback`: semantic retrieval was not enabled, so codeseek degraded to "
            "**function-name substring matching**. Scores on these queries do **not** reflect its "
            "semantic retrieval ability and must not be compared with the MTEB(Code, v1) leaderboard."
        )
    if mode_counts.get("unknown_empty", 0) > 0:
        warnings.append(
            f"{mode_counts['unknown_empty']} queries returned empty or unparsable search results "
            "(`unknown_empty`); check their `error` field and whether `codeseek init` succeeded."
        )
    embedding = meta.get("embedding")
    if isinstance(embedding, Mapping) and embedding.get("configured") is False:
        warnings.append(
            "Config check: `"
            + str(meta.get("embedding_note") or "embedding not configured")
            + "`. Fix: put a usable token into `embedding.api_token` of `~/.codeseek/config.json`, "
            "then re-run `prepare` + `run --force`."
        )

    init_failures = meta.get("init_failures") or []
    if init_failures:
        tasks = ", ".join(str(item.get("task")) for item in init_failures[:5])
        warnings.append(
            f"{len(init_failures)} corpora failed to build an index ({tasks}); "
            "all of their queries are recorded as failures. See `meta.init_failures`."
        )

    unmapped = [row for row in per_query if int(row.get("n_unmapped") or 0) > 0]
    if unmapped:
        total_unmapped = sum(int(row.get("n_unmapped") or 0) for row in unmapped)
        warnings.append(
            f"{len(unmapped)} queries returned results that could not be mapped back to a doc_id "
            f"({total_unmapped} results in total); they are excluded from the ranking. "
            "This usually means codeseek reported paths outside the corpus directory."
        )

    skipped = [row for row in per_query if row.get("skipped")]
    if skipped:
        warnings.append(
            f"{len(skipped)} queries had no positive gold document inside the sampled pool and were "
            "skipped by the metrics (they do not contribute 0.0 to the mean)."
        )

    meta_warnings = meta.get("prepare_warnings") or []
    if meta_warnings:
        warnings.append(
            f"{len(meta_warnings)} prepare-stage warning(s) were recorded (e.g. {meta_warnings[0]})"
        )
    return warnings


def render_markdown(report: Mapping[str, Any]) -> str:
    """把报告 dict 渲染成英文 markdown。"""
    meta = report["meta"]
    aggregate = report["aggregate"]
    by_task: Mapping[str, Mapping[str, Any]] = aggregate.get("by_task") or {}
    mode_counts = meta.get("mode_counts") or {}
    ratio = meta.get("mode_ratio") or {}
    k = aggregate.get("k", 10)
    search_limit = aggregate.get("search_limit", 20)

    lines: list[str] = []
    lines.append("# MTEB(Code, v1) × codeseek Evaluation Report")
    lines.append("")
    lines.append(
        f"- Generated at: `{meta.get('generated_at')}`  Tasks: **{meta.get('n_tasks') or aggregate.get('n_tasks')}**  "
        f"Queries: **{aggregate.get('n_queries')}**  Docs indexed: **{meta.get('n_docs_total')}**"
    )
    lines.append(
        f"- Protocol: primary metric **nDCG@{k}**, overall score = **arithmetic mean of per-task means** "
        f"(MTEB `Mean(Task)`); `search_limit={search_limit}` (recall@k uses k={search_limit}); "
        f"command=`{meta.get('command', 'unknown')}`; seed=`{meta.get('seed', 'n/a')}`"
    )
    if meta.get("dataset_splits"):
        splits = ", ".join(f"{task}={split}" for task, split in (meta["dataset_splits"] or {}).items())
        lines.append(f"- qrels splits: `{splits}`")
    if meta.get("codeseek_binary"):
        binary = meta["codeseek_binary"]
        lines.append(
            f"- Binary under test: `{binary.get('path')}` "
            f"(mtime={binary.get('mtime_iso')}, sha256:{binary.get('sha256_prefix')}...)"
        )
    lines.append(
        "- **Not comparable with the MTEB(Code, v1) leaderboard**: the corpus is pooled down to a few "
        "hundred documents per task and only a small query sample is evaluated (see `README.md`)."
    )
    lines.append("")

    warnings = report.get("warnings") or []
    if warnings:
        for warning in warnings:
            lines.append(f"> ⚠️ {warning}")
            lines.append(">")
        lines.append("")

    # ---------- 1. Summary ----------
    lines.append("## 1. Summary")
    lines.append("")
    lines.append(
        f"**Overall Mean nDCG@{k}: {_fmt(aggregate.get('mean_ndcg_at_10'))}**  "
        f"(recall@{k}: {_fmt(aggregate.get('mean_recall_at_10'))}, "
        f"MRR@{k}: {_fmt(aggregate.get('mean_mrr_at_10'))}, "
        f"recall@{search_limit}: {_fmt(aggregate.get('mean_recall_at_k'))})"
    )
    lines.append("")
    lines.append(f"| Task | Queries | nDCG@{k} | recall@{k} | MRR@{k} | recall@{search_limit} (search limit) |")
    lines.append("| --- | --- | --- | --- | --- | --- |")
    for task, bucket in by_task.items():
        lines.append(
            f"| {task} | {bucket.get('n_scored')}/{bucket.get('n_queries')} | "
            f"{_fmt(bucket.get('ndcg_at_10'))} | {_fmt(bucket.get('recall_at_10'))} | "
            f"{_fmt(bucket.get('mrr_at_10'))} | {_fmt(bucket.get('recall_at_k'))} |"
        )
    lines.append(
        f"| **Mean(Task)** | {aggregate.get('n_queries_scored')}/{aggregate.get('n_queries')} | "
        f"**{_fmt(aggregate.get('mean_ndcg_at_10'))}** | {_fmt(aggregate.get('mean_recall_at_10'))} | "
        f"{_fmt(aggregate.get('mean_mrr_at_10'))} | {_fmt(aggregate.get('mean_recall_at_k'))} |"
    )
    lines.append("")
    lines.append(
        "> The `Mean(Task)` row is the arithmetic mean of the per-task means, **not** a pooled mean "
        "over all queries; 4 decimals are kept because nDCG differences are much finer than "
        "percentage-point accuracy."
    )
    lines.append("")

    # ---------- 2. Retrieval mode statistics ----------
    total_queries = int(aggregate.get("n_queries") or 0) or 1
    lines.append("## 2. Retrieval Mode Statistics")
    lines.append("")
    lines.append("| Mode | Queries | Share | Meaning |")
    lines.append("| --- | --- | --- | --- |")
    lines.append(
        f"| hybrid | {mode_counts.get('hybrid', 0)} | {ratio.get('hybrid', 0.0) * 100:.1f}% | "
        "true semantic retrieval (LanceDB + BM25, results include `code_block`) |"
    )
    lines.append(
        f"| name_fallback | {mode_counts.get('name_fallback', 0)} | {ratio.get('name_fallback', 0.0) * 100:.1f}% | "
        "degrades to function-name substring matching when `embedding.api_token` is absent (no `code_block`) |"
    )
    lines.append(
        f"| unknown_empty | {mode_counts.get('unknown_empty', 0)} | {ratio.get('unknown_empty', 0.0) * 100:.1f}% | "
        "empty or unparsable search results, cannot be judged |"
    )
    lines.append("")
    lines.append(f"- Mode counts sum to **{sum(mode_counts.values())}** of {total_queries} queries.")
    embedding = meta.get("embedding") or {}
    if embedding:
        lines.append(
            f"- Embedding config check: `configured={embedding.get('configured')}`, "
            f"model=`{embedding.get('model')}` — {meta.get('embedding_note') or 'n/a'}"
        )
    lines.append("")

    # ---------- 3. Per-task breakdown ----------
    lines.append("## 3. Per-Task Breakdown")
    lines.append("")
    lines.append(
        f"| Task | Queries (scored/total) | nDCG@{k} | recall@{search_limit} (limit) | returned | mapped | unmapped | modes | errors |"
    )
    lines.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for task, bucket in by_task.items():
        modes = ", ".join(f"{name}×{count}" for name, count in sorted((bucket.get("mode_counts") or {}).items()))
        lines.append(
            f"| {task} | {bucket.get('n_scored')}/{bucket.get('n_queries')} | "
            f"{_fmt(bucket.get('ndcg_at_10'))} | {_fmt(bucket.get('recall_at_k'))} | "
            f"{_fmt(bucket.get('mean_returned'), 1)} | {_fmt(bucket.get('mean_mapped'), 1)} | "
            f"{_fmt(bucket.get('mean_unmapped'), 1)} | {modes or 'n/a'} | {bucket.get('with_errors', 0)} |"
        )
    lines.append("")
    lines.append(
        "> `returned` / `mapped` / `unmapped` are per-query averages of "
        "`codeseek search` results, of results mapped back to a `doc_id`, and of results that could "
        "not be mapped (dropped from the ranking)."
    )
    lines.append("")

    # ---------- 4. Top 10 worst queries ----------
    lines.append(f"## 4. Top 10 Worst Queries (sorted by nDCG@{k}, ascending)")
    lines.append("")
    failures = report.get("failure_top") or []
    if not failures:
        lines.append("No scoreable queries.")
    else:
        lines.append("| # | Task | query_id | nDCG | mode | returned | gold (top3) | retrieved but not gold |")
        lines.append("| --- | --- | --- | --- | --- | --- | --- | --- |")
        for index, row in enumerate(failures, start=1):
            gold = ", ".join(f"`{item}`" for item in (row.get("gold_doc_ids") or [])[:3]) or "—"
            misses = ", ".join(f"`{item}`" for item in (row.get("retrieved_not_gold") or [])[:3]) or "—"
            lines.append(
                f"| {index} | {row.get('task')} | `{row.get('query_id')}` | "
                f"{_fmt(row.get('ndcg_at_10'))} | {row.get('mode')} | {row.get('n_returned')} | "
                f"{gold} | {misses} |"
            )
        lines.append("")
        lines.append(
            "> `retrieved but not gold` lists the highest-ranked documents that are **not** in the "
            "gold set — the most useful signal when debugging ranking quality."
        )
    lines.append("")

    # ---------- 5. Reproduction ----------
    lines.append("## 5. Reproduction Commands")
    lines.append("")
    lines.append("```bash")
    lines.append("cd evals/mtebcode")
    lines.append("python -m mtebcode_eval selftest                     # offline, no network / binary")
    lines.append(
        "python -m mtebcode_eval prepare --workdir /tmp/mtebcode-eval "
        "--tasks CosQA,CodeSearchNetRetrieval,SyntheticText2SQL "
        f"--limit-queries {meta.get('limit_queries', 30)} --corpus-size {meta.get('corpus_size', 300)} "
        f"--seed {meta.get('seed', 42)} --proxy http://127.0.0.1:7897"
    )
    lines.append(
        "python -m mtebcode_eval run --workdir /tmp/mtebcode-eval "
        f"--limit {search_limit} --concurrency {meta.get('concurrency', 4)}"
    )
    lines.append(
        f"python -m mtebcode_eval score --workdir /tmp/mtebcode-eval --limit {search_limit} --k {k}"
        "   # re-score the same raw results with another k"
    )
    lines.append("```")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append(
        "See `evals/mtebcode/README.md` for the data protocol, the sampling scheme and the known "
        "limitations; metrics live in `mtebcode_eval/metric.py`, the aggregation rule in "
        "`mtebcode_eval/scorer.py`."
    )
    lines.append("")
    return "\n".join(lines)


def write_reports(workdir: Path | str, report: Mapping[str, Any]) -> tuple[Path, Path]:
    """写出 ``report.json`` 与 ``report.md``，返回两个路径。"""
    root = Path(workdir)
    root.mkdir(parents=True, exist_ok=True)
    json_path = root / REPORT_JSON
    md_path = root / REPORT_MD

    json_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    md_path.write_text(render_markdown(report), encoding="utf-8")
    return json_path, md_path
