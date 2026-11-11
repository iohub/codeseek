"""报告输出：``report.json``（机器可读）+ ``report.md``（中文可读）。

``report.md`` 必须包含（与 README 的"口径说明"对应）：

1. 结论摘要（总体 accuracy + 按语言表）；
2. **检索模式警示**：``name_fallback`` 占比 > 0 时置顶 blockquote，明确说明
   "本次评测未能启用语义检索，codeseek 退化为函数名子串匹配，分数不代表其语义检索能力"；
3. 按语言 × repo 的细分表；
4. 失败案例 Top 10（按 ``best_similarity`` 从高到低，帮你看"差一点"的题）；
5. 复现命令。

分数统一保留 **1 位小数**（与官方榜一致），并在表头注明 threshold。
"""

from __future__ import annotations

import json
from collections import OrderedDict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping

from .data import REPORT_JSON, REPORT_MD

__all__ = ["build_report", "render_markdown", "write_reports"]


def _percent(value: float) -> str:
    """accuracy 展示为 1 位小数的百分数。"""
    return f"{value * 100:.1f}%"


def build_report(
    scoring: dict[str, Any],
    raws: Mapping[str, dict[str, Any]],
    meta: dict[str, Any],
) -> dict[str, Any]:
    """把判分结果 + raw 元信息合成最终报告 dict。

    :param scoring: ``scorer.score_tasks`` 的返回值。
    :param raws: ``task_id -> raw 结果``（提供 mode / elapsed / error / results_count）。
    :param meta: run 阶段的元信息（二进制、数据集指纹、并发度等）。
    """
    per_task: list[dict[str, Any]] = []
    for row in scoring["per_task"]:
        raw = raws.get(row["id"], {})
        per_task.append(
            {
                **row,
                "mode": raw.get("mode"),
                "results_count": raw.get("results_count", 0),
                "search_elapsed_s": raw.get("elapsed_s", 0.0),
                "search_error": (raw.get("error") or "")[:300],
                "retrieved_at": raw.get("retrieved_at", ""),
                "top_symbols": [item.get("symbol") for item in (raw.get("results_meta") or [])[:3]],
            }
        )

    aggregate = {
        "all": _with_percent(scoring["aggregate"]["all"]),
        "by_language": OrderedDict(
            (language, _with_percent(value))
            for language, value in scoring["aggregate"]["by_language"].items()
        ),
    }

    by_repo: "OrderedDict[str, dict[str, Any]]" = OrderedDict()
    for row in per_task:
        key = f"{row['language']}/{row['repo']}"
        bucket = by_repo.setdefault(
            key,
            {
                "language": row["language"],
                "repo": row["repo"],
                "total": 0,
                "passed": 0,
                "modes": {},
            },
        )
        bucket["total"] += 1
        bucket["passed"] += 1 if row.get("passed") else 0
        mode = row.get("mode") or "unknown_empty"
        bucket["modes"][mode] = bucket["modes"].get(mode, 0) + 1
    for bucket in by_repo.values():
        bucket["accuracy"] = bucket["passed"] / bucket["total"] if bucket["total"] else 0.0
        bucket["accuracy_percent"] = _percent(bucket["accuracy"])

    failures = sorted(
        (row for row in per_task if not row.get("passed")),
        key=lambda row: row.get("best_similarity", 0.0),
        reverse=True,
    )

    mode_counts = dict(meta.get("mode_counts") or {})
    total = len(per_task)
    fallback = int(mode_counts.get("name_fallback", 0))
    unknown = int(mode_counts.get("unknown_empty", 0))
    hybrid = int(mode_counts.get("hybrid", 0))
    fallback_ratio = (fallback / total) if total else 0.0

    return {
        "meta": {
            **meta,
            "generated_at": meta.get("generated_at")
            or datetime.now(tz=timezone.utc).isoformat(timespec="seconds"),
            "mode_counts": mode_counts,
            "mode_ratio": {
                "hybrid": (hybrid / total) if total else 0.0,
                "name_fallback": fallback_ratio,
                "unknown_empty": (unknown / total) if total else 0.0,
            },
        },
        "aggregate": aggregate,
        "by_repo": list(by_repo.values()),
        "per_task": per_task,
        "failure_top": failures[:10],
        "warnings": _warnings(meta, mode_counts, fallback_ratio),
    }


def _with_percent(aggregate: dict[str, Any]) -> dict[str, Any]:
    """给聚合结果补一个 ``accuracy_percent``（1 位小数）。"""
    return {
        **aggregate,
        "accuracy_percent": _percent(float(aggregate.get("accuracy", 0.0))),
    }


def _warnings(
    meta: Mapping[str, Any], mode_counts: Mapping[str, int], fallback_ratio: float
) -> list[str]:
    """生成需要在报告顶部醒目标注的警示。"""
    warnings: list[str] = []
    if mode_counts.get("name_fallback", 0) > 0:
        warnings.append(
            f"本次评测有 {mode_counts['name_fallback']} 题（{fallback_ratio * 100:.1f}%）"
            "走的是 `name_fallback`：codeseek 未启用语义检索，退化为**函数名子串匹配**，"
            "这些题的分数**不代表其语义检索能力**，也不可与官方榜分数直接比较。"
        )
    if mode_counts.get("unknown_empty", 0) > 0:
        warnings.append(
            f"有 {mode_counts['unknown_empty']} 题检索结果为空（`unknown_empty`），"
            "无法从 JSON 结构判断检索路径；请检查这些题的错误信息与索引是否建成。"
        )
    if meta.get("embedding_configured") is False:
        warnings.append(
            "配置检测：`" + str(meta.get("embedding_note") or "embedding 未配置") + "`。"
            "修复方式：在 `~/.codeseek/config.json` 的 `embedding.api_token` 填入可用 token 后重跑 "
            "`codeseek init`（或 `--force` 重跑本评测）。"
        )
    init_failures = meta.get("init_failures") or []
    if init_failures:
        names = ", ".join(f"{item['language']}/{item['repo']}" for item in init_failures[:5])
        warnings.append(
            f"{len(init_failures)} 个仓库建索引失败（{names}），其下题目全部记为 fail；"
            "详情见 `meta.init_failures`。"
        )
    if meta.get("bleu_backend") == "builtin":
        warnings.append(
            "BLEU 使用**内置复刻实现**（未安装 nltk）。与官方 `repoqa.compute_score` 的差异"
            "仅在 nltk 版本级细节上，建议 `pip install nltk` 后用同一批 raw 结果执行 "
            "`score` 复核。"
        )
    return warnings


def render_markdown(report: dict[str, Any]) -> str:
    """把报告 dict 渲染成中文 markdown。"""
    meta = report["meta"]
    all_agg = report["aggregate"]["all"]
    threshold = meta.get("threshold")
    lines: list[str] = []

    lines.append("# RepoQA × codeseek 评测报告")
    lines.append("")
    lines.append(
        f"- 生成时间：`{meta.get('generated_at')}`　题目数：**{meta.get('tasks')}**　"
        f"仓库数：**{meta.get('repos')}**"
    )
    lines.append(
        f"- 判分口径：BLEU-4 + Chen&Cherry method4（backend=`{meta.get('bleu_backend')}`），"
        f"候选集为该 repo 的 10 条 needle，判定阈值 **{threshold}**（≥ 阈值且 BLEU 最高者为 ground truth 才通过）"
    )
    if meta.get("dataset"):
        dataset = meta["dataset"]
        lines.append(
            f"- 数据集：`{dataset.get('release')}`（`sha256:{dataset.get('sha256_prefix')}...`，"
            f"{dataset.get('size_bytes', 0)} bytes）"
        )
    if meta.get("codeseek_binary"):
        binary = meta["codeseek_binary"]
        lines.append(
            f"- 被测二进制：`{binary.get('path')}`"
            f"（mtime={binary.get('mtime_iso')}，sha256:{binary.get('sha256_prefix')}...）"
        )
    lines.append("")

    warnings = report.get("warnings") or []
    if warnings:
        for warning in warnings:
            lines.append(f"> ⚠️ {warning}")
            lines.append(">")
        lines.append("")

    # ---------- 1. 结论摘要 ----------
    lines.append("## 1. 结论摘要")
    lines.append("")
    lines.append(
        f"**总体 accuracy：{all_agg['accuracy_percent']}**"
        f"（{all_agg['passed']}/{all_agg['total']} 题通过，threshold={threshold}）"
    )
    lines.append("")
    lines.append("| 语言 | 通过/总数 | accuracy |")
    lines.append("| --- | --- | --- |")
    for language, agg in report["aggregate"]["by_language"].items():
        lines.append(f"| {language} | {agg['passed']}/{agg['total']} | {agg['accuracy_percent']} |")
    lines.append(f"| **all** | {all_agg['passed']}/{all_agg['total']} | {all_agg['accuracy_percent']} |")
    lines.append("")

    # ---------- 2. 检索模式 ----------
    mode_counts = meta.get("mode_counts") or {}
    ratio = meta.get("mode_ratio") or {}
    lines.append("## 2. 检索模式统计")
    lines.append("")
    lines.append("| 模式 | 题数 | 占比 | 含义 |")
    lines.append("| --- | --- | --- | --- |")
    lines.append(
        f"| hybrid | {mode_counts.get('hybrid', 0)} | {ratio.get('hybrid', 0.0) * 100:.1f}% | "
        "真语义检索（LanceDB + BM25，结果含 `code_block`） |"
    )
    lines.append(
        f"| name_fallback | {mode_counts.get('name_fallback', 0)} | "
        f"{ratio.get('name_fallback', 0.0) * 100:.1f}% | "
        "无 `embedding.api_token` 时退化为函数名子串匹配（无 `code_block`） |"
    )
    lines.append(
        f"| unknown_empty | {mode_counts.get('unknown_empty', 0)} | "
        f"{ratio.get('unknown_empty', 0.0) * 100:.1f}% | 检索结果为空或解析失败，无法判定 |"
    )
    lines.append("")
    lines.append(f"- embedding 配置检测：`{meta.get('embedding_note') or '未检测'}`")
    lines.append(f"- tree-sitter（可选，用于取第一个函数定义）：`{meta.get('treesitter_available')}`")
    lines.append("")

    # ---------- 3. 语言 × repo 细分 ----------
    lines.append("## 3. 按语言 × repo 细分")
    lines.append("")
    lines.append("| 语言 | repo | 通过/总数 | accuracy | 检索模式 |")
    lines.append("| --- | --- | --- | --- | --- |")
    for bucket in report.get("by_repo", []):
        modes = ", ".join(f"{k}×{v}" for k, v in sorted(bucket["modes"].items()))
        lines.append(
            f"| {bucket['language']} | {bucket['repo']} | "
            f"{bucket['passed']}/{bucket['total']} | {bucket['accuracy_percent']} | {modes} |"
        )
    lines.append("")

    # ---------- 4. 失败案例 Top 10 ----------
    lines.append("## 4. 失败案例 Top 10（按 best_similarity 降序）")
    lines.append("")
    failures = report.get("failure_top") or []
    if not failures:
        lines.append("无失败案例。")
    else:
        lines.append("| # | 语言 | repo | needle（ground truth） | best_target | sim | 模式 | 原因 |")
        lines.append("| --- | --- | --- | --- | --- | --- | --- | --- |")
        for index, row in enumerate(failures, start=1):
            lines.append(
                f"| {index} | {row['language']} | {row['repo']} | `{row['needle_name']}` | "
                f"`{row.get('best_target')}` | {row.get('best_similarity', 0.0):.3f} | "
                f"{row.get('mode')} | {row.get('reason')} |"
            )
        lines.append("")
        lines.append(
            "> `sim` 列保留 3 位小数（便于看\"差一点\"的情况）；aggregate 里的 accuracy 保留 1 位小数。"
        )
    lines.append("")

    # ---------- 5. 复现 ----------
    lines.append("## 5. 复现命令")
    lines.append("")
    lines.append("```bash")
    lines.append("cd evals/repoqa")
    lines.append("cargo build --release --manifest-path ../../rust-core/Cargo.toml")
    lines.append("pip install -r requirements.txt   # 可选：装上 nltk 以使用官方同款 BLEU")
    lines.append("python -m repoqa_eval prepare --workdir /tmp/repoqa-eval")
    lines.append(
        "python -m repoqa_eval run --workdir /tmp/repoqa-eval "
        f"--limit {meta.get('search_limit', 5)} --threshold {threshold} "
        f"--concurrency {meta.get('concurrency', 4)}"
    )
    lines.append("python -m repoqa_eval score --workdir /tmp/repoqa-eval   # 换阈值重判，不重跑检索")
    lines.append("```")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append(
        "评测口径与已知限制见 `evals/repoqa/README.md`；判分实现为 `repoqa_eval/scorer.py` + "
        "`repoqa_eval/bleu.py`（逐字复刻官方 `repoqa/metric.py` 与 `compute_score.py`）。"
    )
    lines.append("")
    return "\n".join(lines)


def write_reports(workdir: Path | str, report: dict[str, Any]) -> tuple[Path, Path]:
    """写出 ``report.json`` 与 ``report.md``，返回两个路径。"""
    root = Path(workdir)
    root.mkdir(parents=True, exist_ok=True)
    json_path = root / REPORT_JSON
    md_path = root / REPORT_MD

    json_path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    md_path.write_text(render_markdown(report), encoding="utf-8")
    return json_path, md_path
