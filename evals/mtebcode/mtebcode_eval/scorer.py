"""评分：检索结果 → doc_id 排序列表 → nDCG/recall/MRR → 任务级与总体聚合。

关键口径（README 的「打分协议」逐条对应）：

* **doc 粒度去重保序**：codeseek 对同一文件可能返回多个 chunk，映射回 doc_id 后会重复；
  同一 doc 只保留首次出现的排名，否则 nDCG 会被严重高估。
* **任务级分数 = 该任务所有（可评分）query 的 mean nDCG@10**；
  **总体分数 = 任务级分数的算术平均** —— 这正是 MTEB 的 ``Mean(Task)`` 口径，
  不是「把所有 query 混在一起平均」（否则 query 多的任务会主导总分）。
* **没有正例的 query 跳过**（指标返回 ``None``），不计入均值，也不记 0 分。
* 单题异常（raw 缺失 / docmap 读不到 / 检索报错）都会变成一条可追溯的记录，
  **不中断整体聚合**。
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Mapping, Sequence

from . import adapter, metric
from .data import Task

__all__ = ["DEFAULT_K", "load_docmap", "score_tasks"]

#: 主指标口径。
DEFAULT_K = 10


def load_docmap(path: str | Path) -> dict[str, str]:
    """读 ``docmap.json``（``{相对路径: doc_id}``）；失败返回空 dict，不抛异常。"""
    source = Path(path)
    if not source.is_file():
        return {}
    try:
        payload = json.loads(source.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    if not isinstance(payload, dict):
        return {}
    return {str(key): str(value) for key, value in payload.items()}


def score_tasks(
    tasks: Sequence[Task],
    raws: Mapping[str, dict[str, Any]],
    k: int = DEFAULT_K,
    search_limit: int = 20,
) -> dict[str, Any]:
    """对一批题打分，返回 ``per_query`` / ``by_task`` / ``aggregate``。

    :param tasks: ``tasks.jsonl`` 里的题目（含 ``qrels`` 与 ``docmap_path``）。
    :param raws: ``task_id -> raw 结果``；raw 里至少要能取到 ``results``。
    :param k: 主指标截断（默认 10）。
    :param search_limit: ``codeseek search --limit`` 的值，用于 ``recall@k`` 的 k。
    """
    docmap_cache: dict[str, dict[str, str]] = {}
    per_query: list[dict[str, Any]] = []

    for task in tasks:
        raw = raws.get(task.task_id) or {}
        docmap_path = task.docmap_path or ""
        if docmap_path not in docmap_cache:
            docmap_cache[docmap_path] = load_docmap(docmap_path)
        docmap = docmap_cache[docmap_path]

        results = raw.get("results")
        results = results if isinstance(results, list) else []
        error = str(raw.get("error") or "")
        if not docmap:
            error = error or f"docmap 缺失或为空：{docmap_path}（重新运行 `prepare`）"

        resolution = adapter.resolve_doc_ids(results, docmap, task.corpus_dir or None)
        ranked = resolution.ranked

        ndcg = metric.ndcg_at_k(ranked, task.qrels, k=k)
        recall_k = metric.recall_at_k(ranked, task.qrels, k=k)
        mrr = metric.mrr_at_k(ranked, task.qrels, k=k)
        recall_limit = metric.recall_at_k(ranked, task.qrels, k=max(1, int(search_limit)))

        per_query.append(
            {
                "task_id": task.task_id,
                "task": task.task,
                "query_id": task.query_id,
                "query_text": task.query_text,
                "language": task.language,
                "ndcg_at_10": ndcg,
                "recall_at_10": recall_k,
                "mrr_at_10": mrr,
                "recall_at_k": recall_limit,
                "k": int(k),
                "search_limit": int(max(1, int(search_limit))),
                "mode": raw.get("mode") or adapter.MODE_UNKNOWN,
                "n_returned": resolution.n_returned,
                "n_mapped": resolution.n_mapped,
                "n_unmapped": resolution.n_unmapped,
                "n_ranked": len(ranked),
                "top5_doc_ids": ranked[:5],
                "gold_doc_ids": list(task.gold_doc_ids),
                "qrels_size": len(metric.relevant_docs(task.qrels)),
                "skipped": ndcg is None,
                "error": error[:400],
                "elapsed_s": float(raw.get("elapsed_s") or 0.0),
            }
        )

    by_task = _aggregate_by_task(per_query, k)
    aggregate = _aggregate_overall(per_query, by_task, k, search_limit)
    return {
        "k": int(k),
        "search_limit": int(max(1, int(search_limit))),
        "per_query": per_query,
        "by_task": by_task,
        "aggregate": aggregate,
    }


def _task_order(rows: Sequence[dict[str, Any]]) -> list[str]:
    """按首次出现顺序返回任务名（保持 tasks.jsonl 顺序，输出稳定可 diff）。"""
    seen: list[str] = []
    for row in rows:
        task = str(row.get("task") or "")
        if task and task not in seen:
            seen.append(task)
    return seen


def _task_bucket(rows: Sequence[dict[str, Any]], k: int) -> dict[str, Any]:
    """单个任务的聚合：n_queries / n_scored / 各指标均值 / 检索模式分布。"""
    ndcg = metric.summarize([row["ndcg_at_10"] for row in rows])
    recall10 = metric.summarize([row["recall_at_10"] for row in rows])
    mrr = metric.summarize([row["mrr_at_10"] for row in rows])
    recall_k = metric.summarize([row["recall_at_k"] for row in rows])

    mode_counts: dict[str, int] = {}
    for row in rows:
        mode = str(row.get("mode") or adapter.MODE_UNKNOWN)
        mode_counts[mode] = mode_counts.get(mode, 0) + 1

    return {
        "n_queries": len(rows),
        "n_scored": ndcg["n"],
        "n_skipped": ndcg["skipped"],
        "ndcg_at_10": ndcg["mean"],
        "recall_at_10": recall10["mean"],
        "mrr_at_10": mrr["mean"],
        "recall_at_k": recall_k["mean"],
        "k": int(k),
        "mode_counts": mode_counts,
        "mean_returned": _mean_of([row["n_returned"] for row in rows]),
        "mean_mapped": _mean_of([row["n_mapped"] for row in rows]),
        "mean_unmapped": _mean_of([row["n_unmapped"] for row in rows]),
        "with_errors": sum(1 for row in rows if row.get("error")),
    }


def _mean_of(values: Sequence[float]) -> float | None:
    picked = [float(value) for value in values if value is not None]
    return (sum(picked) / len(picked)) if picked else None


def _aggregate_by_task(per_query: Sequence[dict[str, Any]], k: int) -> dict[str, dict[str, Any]]:
    """按任务分组聚合（顺序 = 题目首次出现顺序）。"""
    buckets: dict[str, list[dict[str, Any]]] = {}
    for row in per_query:
        buckets.setdefault(str(row.get("task") or ""), []).append(row)
    return {task: _task_bucket(rows, k) for task, rows in buckets.items()}


def _aggregate_overall(
    per_query: Sequence[dict[str, Any]],
    by_task: Mapping[str, dict[str, Any]],
    k: int,
    search_limit: int,
) -> dict[str, Any]:
    """总体聚合：**任务级均值的算术平均**（MTEB ``Mean(Task)`` 口径）。"""
    task_scores = [
        float(bucket["ndcg_at_10"])
        for bucket in by_task.values()
        if bucket.get("ndcg_at_10") is not None
    ]
    task_recall = [
        float(bucket["recall_at_10"])
        for bucket in by_task.values()
        if bucket.get("recall_at_10") is not None
    ]
    task_mrr = [
        float(bucket["mrr_at_10"]) for bucket in by_task.values() if bucket.get("mrr_at_10") is not None
    ]
    task_recall_k = [
        float(bucket["recall_at_k"]) for bucket in by_task.values() if bucket.get("recall_at_k") is not None
    ]

    mode_counts: dict[str, int] = {}
    for row in per_query:
        mode = str(row.get("mode") or adapter.MODE_UNKNOWN)
        mode_counts[mode] = mode_counts.get(mode, 0) + 1

    return {
        "mean_ndcg_at_10": (sum(task_scores) / len(task_scores)) if task_scores else None,
        "mean_recall_at_10": (sum(task_recall) / len(task_recall)) if task_recall else None,
        "mean_mrr_at_10": (sum(task_mrr) / len(task_mrr)) if task_mrr else None,
        "mean_recall_at_k": (sum(task_recall_k) / len(task_recall_k)) if task_recall_k else None,
        "n_tasks": len(by_task),
        "n_tasks_scored": len(task_scores),
        "n_queries": len(per_query),
        "n_queries_scored": sum(1 for row in per_query if row["ndcg_at_10"] is not None),
        "n_queries_skipped": sum(1 for row in per_query if row["ndcg_at_10"] is None),
        "k": int(k),
        "search_limit": int(max(1, int(search_limit))),
        "mode_counts": mode_counts,
        "task_order": _task_order(per_query),
        # 显式写出聚合方式，避免读报告的人误解成「所有 query 混平均」
        "aggregation": "mean_of_task_means",
    }
