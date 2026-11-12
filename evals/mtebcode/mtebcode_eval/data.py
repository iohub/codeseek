"""MTEB(Code, v1) 数据层：REST 客户端 / 磁盘缓存 / 任务注册表 / 池化抽样 / 语料物化。

数据源（全部为**实测结果**，勿凭印象修改）：

* HuggingFace **datasets-server** 的 JSON REST API，纯标准库 ``urllib.request`` 即可，
  不需要 ``datasets`` / ``huggingface_hub`` / ``pyarrow``。

  - ``GET /splits?dataset=<id>``              枚举 ``(config, split)`` 组合
  - ``GET /rows?dataset=&config=&split=&offset=&length=``  取行，**``length`` 硬上限 100**
  - ``GET /size?dataset=<id>``                数据集规模统计

* 国内网络**直连超时**，必须走代理（``--proxy http://127.0.0.1:7897`` 或 ``https_proxy``）。
* 已知偶发故障：``synthetic-text2sql`` 的 corpus 偶发 502、``CodeSearchNet`` 的 qrels 偶发
  认证错误 —— 都靠指数退避重试（默认 4 次，间隔 1s/2s/4s）兜住。
* 字段名统一：corpus / queries 为 ``_id, text, title, partition, language``
  （``synthetic-text2sql`` 的 queries 额外带 ``context`` = SQL schema）；
  qrels 为 ``query-id, corpus-id, score``（``score`` 为 int，1 表示相关）。

池化抽样（与 repoqa 的「100 题子集」哲学一致，但 MTEB 的 corpus 有 2 万~28 万文档，
不可能全量索引）：

1. 拉 qrels 的评测 split，得到 ``query_id -> {corpus_id: score}``（只保留 ``score > 0``）；
2. query_id 排序后按固定 seed 抽样 ``--limit-queries`` 条；
3. corpus 池 = 抽样 query 的**全部 gold doc** ∪ 从 corpus 里随机抽的干扰文档，直到
   ``--corpus-size``；
4. 只物化池内文档到 ``<workdir>/corpora/<task_slug>/docs/``，并写 ``docmap.json``
   （``{相对路径: doc_id}``）供打分时反查 —— **打分不猜路径**；
5. ``git init``（``codeseek init`` 的硬前置），写 ``tasks.jsonl`` + ``meta.json``。

关于「边拉边筛、命中够了就提前停」：coIR 系列数据集的 ``_id`` 呈位置规律
（实测 cosqa 的 offset 0 → ``d1, d2, d3``，即 ``id = prefix + (行号 + 1)``；
CodeSearchNet python 的 offset 1000 → ``c1000``，即 ``id = prefix + 行号``），
所以先探测 2~3 个 offset 验证该规律，命中则可**按 id 直接算出页偏移**，只拉含 gold doc
的那几页；规律不匹配（例如 ``mteb/CodeEditSearchRetrieval`` 用 hash 当 ``_id``）时
回退到「顺序分页扫描 + 命中够了提前停」。两条路径都记进 ``meta.json``，便于复现。
"""

from __future__ import annotations

import hashlib
import json
import os
import random
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import OrderedDict
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Sequence

__all__ = [
    "DEFAULT_TASKS",
    "PAGE_SIZE",
    "PrepareOptions",
    "PreparedTask",
    "Task",
    "TaskSpec",
    "WORKDIR_META",
    "DatasetClient",
    "DatasetError",
    "TASKS",
    "corpus_dir",
    "load_tasks",
    "prepare_task",
    "read_workdir_meta",
    "resolve_layout",
    "resolve_qrels_split",
    "safe_doc_filename",
    "task_slug",
    "write_tasks",
    "write_workdir_meta",
]

# ---------------------------------------------------------------------------
# 常量
# ---------------------------------------------------------------------------

#: datasets-server 基地址。
DATASETS_SERVER = "https://datasets-server.huggingface.co"

#: ``/rows`` 的 ``length`` 硬上限（超过返回 400 ``Parameter 'length' must not be greater than 100``）。
PAGE_SIZE = 100

#: 部分接口在无 UA 时行为不稳定，统一带上。
USER_AGENT = "codeseek-mtebcode-eval/1.0 (+https://github.com/CoIR-team/coir)"

#: workdir 内的固定名字（与 repoqa 保持一致的风格）。
TASKS_FILE = "tasks.jsonl"
RAW_DIR = "raw"
CACHE_DIR = "cache"
CORPORA_DIR = "corpora"
DOCS_SUBDIR = "docs"
DOCMAP_NAME = "docmap.json"
WORKDIR_META = "meta.json"
REPORT_JSON = "report.json"
REPORT_MD = "report.md"

#: 报告里统一的 benchmark 名称。
BENCHMARK = "MTEB(Code, v1)"

#: 代码文件首行 title 注释的前缀（按扩展名取；``.py`` 若写 ``//`` 会破坏语法，
#: 反而让 codeseek 的语法解析失效，因此这里按语言给正确注释符）。
COMMENT_PREFIX: dict[str, str] = {
    ".py": "#",
    ".java": "//",
    ".js": "//",
    ".ts": "//",
    ".go": "//",
    ".rs": "//",
    ".cpp": "//",
    ".rb": "#",
    ".php": "//",
}

#: ``_id`` 位置规律的探查正则（``d123`` / ``c1000`` / ``q42`` 这类 ``前缀 + 数字``）。
_ID_NUMBER_RE = re.compile(r"^(?P<prefix>.*?)(?P<num>\d+)$")


def _utc_now() -> str:
    return datetime.now(tz=timezone.utc).isoformat(timespec="seconds")


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


class DatasetError(RuntimeError):
    """数据源相关错误（下载失败 / config-split 不存在 / qrels 为空等）。"""


# ---------------------------------------------------------------------------
# 任务注册表
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Layout:
    """一个 HF dataset 里三个 config/split 三元组（corpus / queries / qrels）。"""

    corpus_config: str
    corpus_split: str
    queries_config: str
    queries_split: str
    qrels_config: str
    qrels_splits: tuple[str, ...] = ("test", "valid", "train")


@dataclass(frozen=True)
class TaskSpec:
    """MTEB 任务定义。

    :param unverified: ``True`` 表示该条目的 config/split 未实测确认，``--tasks`` 选到它时
        报明确错误而不是崩溃（注册表保留条目，便于后续补测）。
    """

    name: str
    hf_dataset: str
    layout: Layout | None
    doc_ext: str = ".txt"
    code_like: bool = False
    language: str = "text"
    note: str = ""
    unverified: bool = False

    @property
    def slug(self) -> str:
        return task_slug(self.name)


def _lang_layout(
    language: str,
    corpus_split: str = "corpus",
    queries_split: str = "queries",
    qrels_splits: tuple[str, ...] = ("test", "valid", "train"),
) -> Layout:
    """``CodeSearchNet`` 系列的多语言子 config 命名：``<lang>-corpus`` 等。"""
    return Layout(
        corpus_config=f"{language}-corpus",
        corpus_split=corpus_split,
        queries_config=f"{language}-queries",
        queries_split=queries_split,
        qrels_config=f"{language}-qrels",
        qrels_splits=qrels_splits,
    )


#: ``corpus`` / ``queries`` / ``default`` 三件套（CoIR 里绝大多数任务都是这个形状）。
_DEFAULT_LAYOUT = Layout("corpus", "corpus", "queries", "queries", "default", ("test", "valid", "train"))


#: **任务注册表**。前 3 个是默认集，必须开箱可用。
#:
#: config/split 全部于本机实测确认（``/splits`` 接口逐条核对），因此 ``unverified`` 均为
#: ``False``；``unverified`` 机制保留给以后新增、尚未探测的任务。
TASKS: "OrderedDict[str, TaskSpec]" = OrderedDict(
    (
        (
            "CosQA",
            TaskSpec(
                name="CosQA",
                hf_dataset="CoIR-Retrieval/cosqa",
                layout=Layout("corpus", "corpus", "queries", "queries", "default", ("test", "valid", "train")),
                doc_ext=".py",
                code_like=True,
                language="python",
                note="NL -> Python code; qrels test has 500 rows (matches MTEB's 500 queries).",
            ),
        ),
        (
            "CodeSearchNetRetrieval",
            TaskSpec(
                name="CodeSearchNetRetrieval",
                hf_dataset="CoIR-Retrieval/CodeSearchNet",
                layout=_lang_layout("python"),
                doc_ext=".py",
                code_like=True,
                language="python",
                note="code <-> code (python sub-config); python-qrels/test has 14,918 rows.",
            ),
        ),
        (
            "SyntheticText2SQL",
            TaskSpec(
                name="SyntheticText2SQL",
                hf_dataset="CoIR-Retrieval/synthetic-text2sql",
                layout=_DEFAULT_LAYOUT,
                doc_ext=".sql",
                code_like=False,  # 不往 .sql 里塞注释（spec：不要污染 .sql/.txt）
                language="sql",
                note="NL -> SQL; queries carry a `context` schema appended to the query text; qrels test 5,851 rows.",
            ),
        ),
        (
            "CodeFeedbackST",
            TaskSpec(
                name="CodeFeedbackST",
                hf_dataset="CoIR-Retrieval/codefeedback-st",
                layout=_DEFAULT_LAYOUT,
                language="text",
                note="single-turn code feedback; corpus 156,526 rows.",
            ),
        ),
        (
            "CodeFeedbackMT",
            TaskSpec(
                name="CodeFeedbackMT",
                hf_dataset="CoIR-Retrieval/codefeedback-mt",
                layout=_DEFAULT_LAYOUT,
                language="text",
                note="multi-turn code feedback; corpus 66,383 rows.",
            ),
        ),
        (
            "AppsRetrieval",
            TaskSpec(
                name="AppsRetrieval",
                hf_dataset="CoIR-Retrieval/apps",
                layout=_DEFAULT_LAYOUT,
                doc_ext=".py",
                code_like=True,
                language="python",
                note=(
                    "competitive-programming solutions; corpus 8,765 rows. "
                    "doc_ext is .py (not .txt): codeseek's indexer only picks up "
                    "cpp/py/java/rs/ts/tsx/js/go, so .txt corpora index as 0 files."
                ),
            ),
        ),
        (
            "StackOverflowQA",
            TaskSpec(
                name="StackOverflowQA",
                hf_dataset="CoIR-Retrieval/stackoverflow-qa",
                layout=_DEFAULT_LAYOUT,
                language="text",
                note="question -> answer post; corpus 19,931 rows.",
            ),
        ),
        (
            "CodeTransOceanContest",
            TaskSpec(
                name="CodeTransOceanContest",
                hf_dataset="CoIR-Retrieval/codetrans-contest",
                layout=_DEFAULT_LAYOUT,
                doc_ext=".cpp",
                code_like=True,
                language="cpp",
                note=(
                    "contest-level code translation pairs; corpus 1,008 rows. "
                    "corpus text is C++ source, so it is materialized as .cpp "
                    "(codeseek does not index .txt)."
                ),
            ),
        ),
        (
            "CodeTransOceanDL",
            TaskSpec(
                name="CodeTransOceanDL",
                hf_dataset="CoIR-Retrieval/codetrans-dl",
                layout=_DEFAULT_LAYOUT,
                doc_ext=".py",
                code_like=True,
                language="python",
                note=(
                    "deep-learning framework API translation pairs; corpus 816 rows. "
                    "corpus text is Python source, so it is materialized as .py "
                    "(codeseek does not index .txt)."
                ),
            ),
        ),
        (
            "CodeSearchNetCCRetrieval",
            TaskSpec(
                name="CodeSearchNetCCRetrieval",
                hf_dataset="CoIR-Retrieval/CodeSearchNet-ccr",
                layout=_lang_layout("python"),
                doc_ext=".py",
                code_like=True,
                language="python",
                note="code -> code (python sub-config) on CodeSearchNet.",
            ),
        ),
        (
            "CodeEditSearchRetrieval",
            TaskSpec(
                name="CodeEditSearchRetrieval",
                hf_dataset="mteb/CodeEditSearchRetrieval",
                # 该数据集三个 config 的 split 全部叫 train；实测 java 只有
                # java-corpus / java-queries，**没有 java-qrels**，因此固定用 python 子 config。
                layout=_lang_layout(
                    "python", corpus_split="train", queries_split="train", qrels_splits=("train",)
                ),
                doc_ext=".py",
                code_like=True,
                language="python",
                note="code-edit -> description (python sub-config, 1,000 docs, hash _ids -> scan fallback).",
            ),
        ),
    )
)

#: 默认评测集：覆盖 NL→code（CosQA）、code→code（CSN）、NL→SQL（Text2SQL）三种形态。
DEFAULT_TASKS: tuple[str, ...] = ("CosQA", "CodeSearchNetRetrieval", "SyntheticText2SQL")


def task_slug(name: str) -> str:
    """``SyntheticText2SQL`` → ``synthetictext2sql``（目录名 / task_id 前缀）。"""
    return re.sub(r"[^a-z0-9]+", "", name.lower())


def corpus_dir(workdir: Path | str, spec_or_name: "TaskSpec | str") -> Path:
    """语料目录：``<workdir>/corpora/<task_slug>``。"""
    slug = spec_or_name.slug if isinstance(spec_or_name, TaskSpec) else task_slug(spec_or_name)
    return Path(workdir) / CORPORA_DIR / slug


# ---------------------------------------------------------------------------
# REST 客户端：磁盘缓存 + 指数退避重试 + 速率控制 + 代理
# ---------------------------------------------------------------------------


def _env_proxy() -> str | None:
    """默认代理：读 ``https_proxy`` / ``HTTPS_PROXY`` / ``http_proxy`` / ``HTTP_PROXY``。"""
    for key in ("https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"):
        value = os.environ.get(key, "").strip()
        if value:
            return value
    return None


def _proxy_opener(proxy: str | None) -> urllib.request.OpenerDirector:
    """构造 opener（**不改全局** ``urllib.request.install_opener``）。"""
    if proxy:
        handler = urllib.request.ProxyHandler({"http": proxy, "https": proxy})
        return urllib.request.build_opener(handler)
    return urllib.request.build_opener()


def _atomic_write_text(path: Path, text: str) -> None:
    """``.part`` → rename 的原子写（中断不会留下半截 JSON）。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".part")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(path)


@dataclass
class ClientStats:
    """网络访问统计（写进 ``meta.json``，说明这次抓了多少页、命中多少缓存）。"""

    requests: int = 0
    cache_hits: int = 0
    retries: int = 0
    failures: int = 0
    bytes_downloaded: int = 0

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class DatasetClient:
    """datasets-server 的极简客户端。

    * **缓存**：``<workdir>/cache/<dataset>/<config>/<split>/<offset>.json``，重跑直接复用；
    * **重试**：默认 4 次，间隔 1s / 2s / 4s；HTTP 400 视为参数错误，不重试；
    * **速率**：每次**真实**网络请求后 sleep ``rate_limit`` 秒（缓存命中不 sleep）。
    """

    def __init__(
        self,
        workdir: Path | str,
        proxy: str | None = None,
        retries: int = 4,
        rate_limit: float = 0.15,
        timeout: float = 120.0,
        quiet: bool = False,
        cache_only: bool = False,
    ) -> None:
        self.workdir = Path(workdir)
        self.cache_root = self.workdir / CACHE_DIR
        self.proxy = proxy if proxy is not None else _env_proxy()
        self.retries = max(1, int(retries))
        self.rate_limit = max(0.0, float(rate_limit))
        self.timeout = float(timeout)
        self.quiet = quiet
        self.cache_only = bool(cache_only)
        self.stats = ClientStats()
        self._opener = _proxy_opener(self.proxy)

    # -- 内部：缓存路径 ----------------------------------------------------
    def _cache_base(self, dataset: str) -> Path:
        return self.cache_root / dataset.replace("/", "__")

    def _rows_cache_path(self, dataset: str, config: str, split: str, offset: int) -> Path:
        return self._cache_base(dataset) / _safe_segment(config) / _safe_segment(split) / f"{int(offset)}.json"

    def _meta_cache_path(self, dataset: str, name: str) -> Path:
        return self._cache_base(dataset) / f"_{name}.json"

    # -- 内部：请求 --------------------------------------------------------
    def _fetch_json(self, path: str, params: dict[str, Any]) -> dict[str, Any]:
        """GET ``path`` + 查询参数 → dict，失败时抛出带上下文的 :class:`DatasetError`。"""
        query = urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
        url = f"{DATASETS_SERVER}{path}?{query}"
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})

        last_detail = ""
        for attempt in range(1, self.retries + 1):
            try:
                self.stats.requests += 1
                with self._opener.open(request, timeout=self.timeout) as response:
                    body = response.read()
                self.stats.bytes_downloaded += len(body)
                text = body.decode("utf-8", errors="replace")
                if self.rate_limit:
                    time.sleep(self.rate_limit)
                payload = json.loads(text)
                if isinstance(payload, dict) and payload.get("error"):
                    # datasets-server 有时用 200 + {"error": "..."} 表达失败
                    last_detail = f"server error field: {str(payload.get('error'))[:300]}"
                    raise _RetryableError(last_detail)
                if not isinstance(payload, dict):
                    raise _RetryableError(f"response is {type(payload).__name__}, expected object")
                return payload
            except urllib.error.HTTPError as exc:
                detail = _read_error_body(exc)
                last_detail = f"HTTP {exc.code} {exc.reason}"
                if detail:
                    last_detail += f" — {detail}"
                if exc.code == 400:
                    # 参数错误（例如 length > 100）重试没有意义
                    self.stats.failures += 1
                    raise DatasetError(f"请求失败：{last_detail}\n  URL: {url}") from exc
                if attempt >= self.retries:
                    self.stats.failures += 1
                    raise DatasetError(
                        f"请求失败（已重试 {self.retries} 次）：{last_detail}\n  URL: {url}"
                    ) from exc
            except (_RetryableError, urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as exc:
                last_detail = f"{type(exc).__name__}: {exc}"
                if attempt >= self.retries:
                    self.stats.failures += 1
                    raise DatasetError(
                        f"请求失败（已重试 {self.retries} 次）：{last_detail}\n  URL: {url}"
                    ) from exc

            self.stats.retries += 1
            backoff = 2 ** (attempt - 1)  # 1s / 2s / 4s ...
            if not self.quiet:
                _log(f"[data] 重试 {attempt}/{self.retries - 1}（{backoff}s 后）：{last_detail}")
            time.sleep(backoff)

        raise DatasetError(f"请求失败：{last_detail}\n  URL: {url}")  # pragma: no cover - 兜底

    def _cached_json(self, path: Path, fetch, description: str) -> dict[str, Any]:
        """缓存命中直接返回，否则 ``fetch()`` 拉取并原子落盘。"""
        if path.is_file():
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                payload = None
            if isinstance(payload, dict):
                self.stats.cache_hits += 1
                return payload
        if self.cache_only:
            raise DatasetError(
                f"--cache-only 已开启，但缓存缺失：{path}\n"
                "  去掉 --cache-only 让它联网抓取（必要时先用同一条命令缓存一遍）"
            )
        payload = fetch()
        _atomic_write_text(path, json.dumps(payload, ensure_ascii=False))
        if not self.quiet:
            _log(f"[data] 已缓存 {description}")
        return payload

    # -- 公开接口 ----------------------------------------------------------
    def splits(self, dataset: str) -> list[dict[str, str]]:
        """``GET /splits``：返回 ``[{"config":..., "split":...}, ...]``。"""
        path = self._meta_cache_path(dataset, "splits")

        def _fetch() -> dict[str, Any]:
            return self._fetch_json("/splits", {"dataset": dataset})

        payload = self._cached_json(path, _fetch, f"{dataset} /splits")
        splits = payload.get("splits")
        if not isinstance(splits, list):
            raise DatasetError(f"{dataset}: /splits 返回结构异常：{str(payload)[:300]}")
        return [
            {"config": str(item.get("config", "")), "split": str(item.get("split", ""))}
            for item in splits
            if isinstance(item, dict)
        ]

    def size(self, dataset: str) -> dict[str, Any]:
        """``GET /size``：数据集规模统计（写进 ``meta.json``）。"""
        path = self._meta_cache_path(dataset, "size")

        def _fetch() -> dict[str, Any]:
            return self._fetch_json("/size", {"dataset": dataset})

        return self._cached_json(path, _fetch, f"{dataset} /size")

    def rows(self, dataset: str, config: str, split: str, offset: int, length: int = PAGE_SIZE) -> dict[str, Any]:
        """``GET /rows``：取一页（``length <= 100``，严格校验）。"""
        if not 1 <= int(length) <= PAGE_SIZE:
            raise ValueError(f"length 必须在 1..{PAGE_SIZE} 之间（datasets-server 硬上限），收到 {length}")
        offset = max(0, int(offset))
        path = self._rows_cache_path(dataset, config, split, offset)

        def _fetch() -> dict[str, Any]:
            return self._fetch_json(
                "/rows",
                {"dataset": dataset, "config": config, "split": split, "offset": offset, "length": int(length)},
            )

        payload = self._cached_json(path, _fetch, f"{dataset} {config}/{split} @{offset}")
        rows = payload.get("rows")
        if not isinstance(rows, list):
            raise DatasetError(
                f"{dataset} {config}/{split} @{offset}: /rows 返回结构异常：{str(payload)[:300]}"
            )
        return payload

    def num_rows(self, dataset: str, config: str, split: str) -> int:
        """从第一页读 ``num_rows_total``（顺便把 offset=0 的页缓存下来）。"""
        payload = self.rows(dataset, config, split, 0, min(PAGE_SIZE, PAGE_SIZE))
        total = payload.get("num_rows_total")
        try:
            return int(total)  # type: ignore[arg-type]
        except (TypeError, ValueError):
            rows = payload.get("rows") or []
            return len(rows)


class _RetryableError(RuntimeError):
    """内部标记：可重试的软失败。"""


def _read_error_body(exc: urllib.error.HTTPError, limit: int = 300) -> str:
    try:
        raw = exc.read()
    except Exception:  # noqa: BLE001 - 读取错误体失败不影响主流程
        return ""
    text = raw.decode("utf-8", errors="replace").strip()
    return text[:limit]


def _safe_segment(value: str) -> str:
    """把 config/split 名安全化成单层目录名。"""
    return re.sub(r"[^A-Za-z0-9._-]", "_", value) or "_"


# ---------------------------------------------------------------------------
# config / split 解析
# ---------------------------------------------------------------------------


def detect_layout(splits: Sequence[dict[str, str]]) -> Layout | None:
    """从 ``/splits`` 里推断 ``corpus / queries / qrels`` 三件套（注册表未验证时使用）。"""
    by_config: dict[str, list[str]] = {}
    for item in splits:
        by_config.setdefault(item["config"], []).append(item["split"])

    corpus_candidates = [c for c in by_config if c == "corpus" or c.endswith("-corpus")]
    queries_candidates = [c for c in by_config if c == "queries" or c.endswith("-queries")]

    def _prefer(candidates: Sequence[str], exact: str) -> str | None:
        if not candidates:
            return None
        if exact in candidates:
            return exact
        return sorted(candidates)[0]

    corpus_config = _prefer(corpus_candidates, "corpus")
    queries_config = _prefer(queries_candidates, "queries")
    if not corpus_config or not queries_config:
        return None
    corpus_split = "corpus" if "corpus" in by_config[corpus_config] else by_config[corpus_config][0]
    queries_split = "queries" if "queries" in by_config[queries_config] else by_config[queries_config][0]

    qrels_config = None
    qrels_splits: tuple[str, ...] = ()
    ordered = ("test", "valid", "train")
    for config, available in by_config.items():
        if config in (corpus_config, queries_config):
            continue
        hits = tuple(split for split in ordered if split in available)
        if hits:
            qrels_config = config
            qrels_splits = hits
            if config == "default":
                break
    if not qrels_config:
        return None
    return Layout(corpus_config, corpus_split, queries_config, queries_split, qrels_config, qrels_splits)


def resolve_layout(client: DatasetClient, spec: TaskSpec) -> Layout:
    """确认注册表里的 config/split 真实存在；未注册的走 ``detect_layout`` 探测。

    :raises DatasetError: 探测失败 —— 错误信息里列出该数据集可用的 config，可读不崩溃。
    """
    available = client.splits(spec.hf_dataset)
    pairs = {(item["config"], item["split"]) for item in available}

    layout = spec.layout
    if layout is None:
        detected = detect_layout(available)
        if detected is None:
            raise DatasetError(
                f"{spec.name}: 无法从 {spec.hf_dataset} 的 /splits 推断出 corpus/queries/qrels 布局；"
                f"可用 config：{sorted({item['config'] for item in available})}"
            )
        return detected

    required = [
        (layout.corpus_config, layout.corpus_split),
        (layout.queries_config, layout.queries_split),
    ]
    missing = [pair for pair in required if pair not in pairs]
    if missing:
        raise DatasetError(
            f"{spec.name}: {spec.hf_dataset} 上不存在 config/split {missing}；"
            f"可用组合：{sorted(pairs)[:20]}"
        )
    if not any((layout.qrels_config, split) in pairs for split in layout.qrels_splits):
        raise DatasetError(
            f"{spec.name}: {spec.hf_dataset} 上不存在 qrels config={layout.qrels_config} 且 split ∈ "
            f"{list(layout.qrels_splits)}；可用组合：{sorted(pairs)[:20]}"
        )
    return layout


def resolve_qrels_split(client: DatasetClient, spec: TaskSpec, layout: Layout) -> str:
    """qrels split 选择规则：优先 ``test``，其次 ``valid``，最后 ``train``。"""
    pairs = {(item["config"], item["split"]) for item in client.splits(spec.hf_dataset)}
    for split in layout.qrels_splits:
        if (layout.qrels_config, split) in pairs:
            return split
    raise DatasetError(
        f"{spec.name}: {spec.hf_dataset} 的 qrels config={layout.qrels_config} 没有可用的 split"
        f"（尝试过 {list(layout.qrels_splits)}）"
    )


# ---------------------------------------------------------------------------
# 分页遍历 / 按 id 取行 / 随机抽样
# ---------------------------------------------------------------------------


@dataclass
class FetchStats:
    """一次「按 id 取行」的统计（写进 ``meta.json``，说明扫了多少页）。"""

    mode: str = "scan"
    pages: int = 0
    rows_scanned: int = 0
    missing: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


def iter_rows(
    client: DatasetClient,
    dataset: str,
    config: str,
    split: str,
    num_rows: int,
    start_offset: int = 0,
) -> Iterable[list[dict[str, Any]]]:
    """按 ``PAGE_SIZE`` 分页遍历，逐页 yield ``row`` 字典列表。"""
    offset = max(0, int(start_offset))
    while offset < num_rows:
        length = min(PAGE_SIZE, num_rows - offset)
        payload = client.rows(dataset, config, split, offset, length)
        page = [item.get("row") or {} for item in (payload.get("rows") or []) if isinstance(item, dict)]
        if not page:
            return
        yield page
        offset += PAGE_SIZE


def probe_id_scheme(
    client: DatasetClient, dataset: str, config: str, split: str, num_rows: int
) -> tuple[str, int] | None:
    """探测 ``_id`` 是否为 ``prefix + (行号 + delta)``。

    :return: ``(prefix, delta)``；无法确认（id 是 hash、或探测点不一致）时返回 ``None``。
    """
    if num_rows < 2:
        return None
    probes = sorted({0, num_rows // 2, num_rows - 1})
    observations: list[tuple[str, int, int]] = []
    for offset in probes:
        payload = client.rows(dataset, config, split, offset, 1)
        rows = payload.get("rows") or []
        if not rows:
            return None
        doc_id = str((rows[0].get("row") or {}).get("_id") or "")
        match = _ID_NUMBER_RE.match(doc_id)
        if not match:
            return None
        observations.append((match.group("prefix"), int(match.group("num")), offset))
    prefixes = {item[0] for item in observations}
    deltas = {item[1] - item[2] for item in observations}
    if len(prefixes) != 1 or len(deltas) != 1:
        return None
    return prefixes.pop(), deltas.pop()


def _scan_rows(
    client: DatasetClient,
    dataset: str,
    config: str,
    split: str,
    want_ids: Sequence[str],
    num_rows: int,
) -> tuple[dict[str, dict[str, Any]], FetchStats]:
    """顺序分页扫描，**命中够了就提前停**（不强求扫完全部 corpus）。"""
    stats = FetchStats(mode="scan")
    wanted = set(want_ids)
    found: dict[str, dict[str, Any]] = {}
    offset = 0
    while offset < num_rows and len(found) < len(wanted):
        length = min(PAGE_SIZE, num_rows - offset)
        payload = client.rows(dataset, config, split, offset, length)
        stats.pages += 1
        rows = [item.get("row") or {} for item in (payload.get("rows") or []) if isinstance(item, dict)]
        stats.rows_scanned += len(rows)
        if not rows:
            break
        for row in rows:
            doc_id = str(row.get("_id") or "")
            if doc_id in wanted and doc_id not in found:
                found[doc_id] = row
        offset += PAGE_SIZE
    stats.missing = sorted(wanted - set(found))
    return found, stats


def fetch_rows_by_ids(
    client: DatasetClient,
    dataset: str,
    config: str,
    split: str,
    ids: Sequence[str],
    num_rows: int,
) -> tuple[dict[str, dict[str, Any]], FetchStats]:
    """按 ``_id`` 取行。

    两条路径：

    1. **positional**：``_id`` 呈 ``prefix + (行号 + delta)`` 规律时，直接算出页偏移，
       只拉含目标文档的那几页（避免扫完 28 万行）；
    2. **scan**：规律不匹配时顺序分页扫描 + 命中够了提前停。

    路径与页数都记进 :class:`FetchStats`。
    """
    wanted = list(dict.fromkeys(str(item) for item in ids))
    if not wanted:
        return {}, FetchStats(mode="skip")

    scheme = probe_id_scheme(client, dataset, config, split, num_rows)
    if scheme is not None:
        prefix, delta = scheme
        pages: "dict[int, list[str]]" = {}
        resolvable = True
        for doc_id in wanted:
            match = _ID_NUMBER_RE.match(doc_id)
            if not match or match.group("prefix") != prefix:
                resolvable = False
                break
            index = int(match.group("num")) - delta
            if index < 0 or index >= num_rows:
                resolvable = False
                break
            pages.setdefault((index // PAGE_SIZE) * PAGE_SIZE, []).append(doc_id)

        if resolvable:
            stats = FetchStats(mode="positional")
            found: dict[str, dict[str, Any]] = {}
            for page_offset in sorted(pages):
                length = min(PAGE_SIZE, num_rows - page_offset)
                payload = client.rows(dataset, config, split, page_offset, length)
                stats.pages += 1
                rows = [item.get("row") or {} for item in (payload.get("rows") or []) if isinstance(item, dict)]
                stats.rows_scanned += len(rows)
                by_id = {str(row.get("_id") or ""): row for row in rows}
                for doc_id in pages[page_offset]:
                    row = by_id.get(doc_id)
                    if row is not None:
                        found[doc_id] = row
            if len(found) == len(wanted):
                return found, stats

            # 位置规律命中不全（极少见）：用扫描补齐剩余 id
            missing_ids = [doc_id for doc_id in wanted if doc_id not in found]
            extra, extra_stats = _scan_rows(client, dataset, config, split, missing_ids, num_rows)
            stats.pages += extra_stats.pages
            stats.rows_scanned += extra_stats.rows_scanned
            found.update(extra)
            stats.mode = "positional+scan"
            stats.missing = [doc_id for doc_id in wanted if doc_id not in found]
            return found, stats

    found, stats = _scan_rows(client, dataset, config, split, wanted, num_rows)
    return found, stats


def sample_rows(
    client: DatasetClient,
    dataset: str,
    config: str,
    split: str,
    num_rows: int,
    count: int,
    seed: int,
    exclude: set[str],
    max_pages: int = 0,
) -> tuple[list[dict[str, Any]], FetchStats]:
    """按固定 seed **确定性**随机抽 ``count`` 行干扰文档。

    ``exclude`` 会被原地更新（保证结果里没有重复 doc / 没有 gold doc）。
    """
    stats = FetchStats(mode="random")
    if count <= 0 or num_rows <= 0:
        return [], stats

    rng = random.Random(_stable_seed(seed, dataset, config, split))
    page_starts = list(range(0, num_rows, PAGE_SIZE))
    budget = max_pages if max_pages > 0 else len(page_starts)
    picked: list[dict[str, Any]] = []
    visited: set[int] = set()
    attempts = 0

    while len(picked) < count and attempts < budget and len(visited) < len(page_starts):
        attempts += 1
        page_offset = page_starts[rng.randrange(len(page_starts))]
        if page_offset in visited:
            continue
        visited.add(page_offset)
        length = min(PAGE_SIZE, num_rows - page_offset)
        payload = client.rows(dataset, config, split, page_offset, length)
        stats.pages += 1
        rows = [item.get("row") or {} for item in (payload.get("rows") or []) if isinstance(item, dict)]
        stats.rows_scanned += len(rows)
        for row in rows:
            doc_id = str(row.get("_id") or "")
            if not doc_id or doc_id in exclude:
                continue
            exclude.add(doc_id)
            picked.append(row)
            if len(picked) >= count:
                break
    if len(picked) < count:
        stats.missing = [f"short_by_{count - len(picked)}"]
    return picked, stats


def _stable_seed(*parts: Any) -> int:
    """跨进程稳定的 seed（``hash()`` 对 str 加了随机盐，不能用）。"""
    digest = hashlib.sha1("::".join(str(part) for part in parts).encode("utf-8")).hexdigest()
    return int(digest[:16], 16)


# ---------------------------------------------------------------------------
# qrels
# ---------------------------------------------------------------------------


def fetch_qrels(client: DatasetClient, dataset: str, config: str, split: str) -> tuple[dict[str, dict[str, float]], int]:
    """拉完整 qrels → ``{query_id: {doc_id: score}}``（只保留 ``score > 0``）。

    :return: ``(qrels, 读取的行数)``
    """
    num_rows = client.num_rows(dataset, config, split)
    qrels: dict[str, dict[str, float]] = {}
    read = 0
    for page in iter_rows(client, dataset, config, split, num_rows):
        for row in page:
            read += 1
            query_id = str(row.get("query-id") or "").strip()
            doc_id = str(row.get("corpus-id") or "").strip()
            if not query_id or not doc_id:
                continue
            try:
                score = float(row.get("score") or 0)
            except (TypeError, ValueError):
                score = 0.0
            if score <= 0:
                continue
            qrels.setdefault(query_id, {})[doc_id] = score
    return qrels, read


def sample_query_ids(query_ids: Sequence[str], limit: int, seed: int, task_name: str) -> list[str]:
    """query_id 排序后按固定 seed 抽样（确定性，重跑一致）。"""
    ordered = sorted(set(query_ids))
    if limit <= 0 or limit >= len(ordered):
        return ordered
    rng = random.Random(_stable_seed(seed, task_name, "queries"))
    picked = rng.sample(ordered, limit)
    return sorted(picked)


# ---------------------------------------------------------------------------
# 语料物化
# ---------------------------------------------------------------------------


def safe_doc_filename(doc_id: str, ext: str) -> str:
    """doc_id → 文件系统安全的名字：非法字符替换 + 短哈希防碰撞。"""
    stem = re.sub(r"[^A-Za-z0-9._-]", "_", doc_id).strip("_") or "doc"
    if len(stem) > 80:
        stem = stem[:80]
    digest = hashlib.sha1(doc_id.encode("utf-8")).hexdigest()[:8]
    return f"{stem}_{digest}{ext}"


def _doc_text(row: dict[str, Any], spec: TaskSpec) -> str:
    """文档正文：``text``；``title`` 非空且是代码文件时首行加注释。

    ``.sql`` / ``.txt`` **不加**注释（spec 要求不污染这两类文件）。
    """
    text = row.get("text")
    body = text if isinstance(text, str) else ("" if text is None else str(text))
    title = row.get("title")
    title_text = title.strip() if isinstance(title, str) else ""
    if title_text and spec.code_like and spec.doc_ext in COMMENT_PREFIX:
        prefix = COMMENT_PREFIX[spec.doc_ext]
        return f"{prefix} {title_text}\n{body}"
    return body


def materialize_docs(
    spec: TaskSpec,
    rows: Sequence[dict[str, Any]],
    destination: Path | str,
    force: bool = False,
) -> tuple[dict[str, str], int]:
    """把池内文档逐个写成真实文件，返回 ``(docmap, 写出的文件数)``。

    ``docmap`` = ``{相对 POSIX 路径: doc_id}``，落盘到 ``<destination>/docmap.json``。
    """
    dest = Path(destination)
    docs_dir = dest / DOCS_SUBDIR
    docs_dir.mkdir(parents=True, exist_ok=True)
    docmap_path = dest / DOCMAP_NAME

    if not force and docmap_path.is_file():
        try:
            existing = json.loads(docmap_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            existing = None
        if isinstance(existing, dict) and existing:
            return {str(k): str(v) for k, v in existing.items()}, len(existing)

    docmap: "OrderedDict[str, str]" = OrderedDict()
    written = 0
    for row in rows:
        doc_id = str(row.get("_id") or "")
        if not doc_id or doc_id in docmap.values():
            continue
        filename = safe_doc_filename(doc_id, spec.doc_ext)
        relative = f"{DOCS_SUBDIR}/{filename}"
        target = dest.joinpath(*relative.split("/"))
        target.parent.mkdir(parents=True, exist_ok=True)
        # 二进制写入，避免任何换行转换改变内容
        target.write_bytes(_doc_text(row, spec).encode("utf-8"))
        docmap[relative] = doc_id
        written += 1

    _atomic_write_text(docmap_path, json.dumps(docmap, ensure_ascii=False, indent=2))
    return dict(docmap), written


def git_init(directory: Path | str) -> tuple[bool, str]:
    """在语料目录执行 ``git init``（``codeseek init`` 的硬前置）。"""
    dest = Path(directory)
    dest.mkdir(parents=True, exist_ok=True)
    if (dest / ".git").exists():
        return True, "已存在 .git，跳过 git init"
    try:
        completed = subprocess.run(
            ["git", "init", "-q"],
            cwd=str(dest),
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError) as exc:
        return False, f"git init 失败：{type(exc).__name__}: {exc}"
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip()[:300]
        return False, f"git init 返回 {completed.returncode}：{detail}"
    return True, "git init 完成"


# ---------------------------------------------------------------------------
# Task
# ---------------------------------------------------------------------------


@dataclass
class Task:
    """一道 MTEB(Code) 检索题（= 某任务的某条 query）。

    ``qrels`` 是**限定在池内**的 ``{doc_id: score}``：池化后 gold doc 若缺失，它不可能被检索到，
    与其在评分时当成漏检（不公平），不如从 qrels 里去掉并在 ``meta.json`` 里记录告警。
    """

    task_id: str
    task: str
    hf_dataset: str
    query_id: str
    query_text: str
    gold_doc_ids: list[str]
    corpus_dir: str
    docmap_path: str
    n_pool_docs: int
    qrels: dict[str, float] = field(default_factory=dict)
    qrels_split: str = ""
    language: str = ""

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "Task":
        known = {f for f in cls.__dataclass_fields__}  # type: ignore[attr-defined]
        return cls(**{k: v for k, v in payload.items() if k in known})


def write_tasks(tasks: Iterable[Task], path: Path | str) -> Path:
    """写 ``tasks.jsonl``（每题一行）。"""
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    with open(target, "w", encoding="utf-8") as handle:
        for task in tasks:
            handle.write(json.dumps(task.to_dict(), ensure_ascii=False) + "\n")
    return target


def load_tasks(path: Path | str) -> list[Task]:
    """读 ``tasks.jsonl``。"""
    source = Path(path)
    if not source.is_file():
        raise FileNotFoundError(f"题目文件不存在：{source}（先运行 `prepare`）")
    tasks: list[Task] = []
    with open(source, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                tasks.append(Task.from_dict(json.loads(line)))
    return tasks


def write_workdir_meta(workdir: Path | str, meta: dict[str, Any]) -> Path:
    """写 ``<workdir>/meta.json``。"""
    target = Path(workdir) / WORKDIR_META
    _atomic_write_text(target, json.dumps(meta, ensure_ascii=False, indent=2))
    return target


def read_workdir_meta(workdir: Path | str) -> dict[str, Any]:
    """读 ``<workdir>/meta.json``；不存在/损坏时返回空 dict（不抛异常）。"""
    target = Path(workdir) / WORKDIR_META
    if not target.is_file():
        return {}
    try:
        payload = json.loads(target.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return payload if isinstance(payload, dict) else {}


# ---------------------------------------------------------------------------
# prepare 主流程
# ---------------------------------------------------------------------------


@dataclass
class PrepareOptions:
    """``prepare`` 的抽样口径。"""

    workdir: Path
    corpus_size: int = 300
    limit_queries: int = 30
    seed: int = 42
    force: bool = False
    quiet: bool = False


@dataclass
class PreparedTask:
    """单个任务的 prepare 产物：题目列表 + 写进 ``meta.json`` 的摘要。"""

    spec: TaskSpec
    tasks: list[Task]
    meta: dict[str, Any]


def _query_text(row: dict[str, Any]) -> str:
    """query 文本；``SyntheticText2SQL`` 要把 SQL schema（``context``）拼上去。"""
    text = row.get("text")
    base = text if isinstance(text, str) else ("" if text is None else str(text))
    context = row.get("context")
    schema = context.strip() if isinstance(context, str) else ""
    if schema:
        return f"{base}\n\nSchema:\n{schema}"
    return base


def prepare_task(spec: TaskSpec, client: DatasetClient, options: PrepareOptions) -> PreparedTask:
    """为单个任务完成「qrels → 抽 query → 池化 corpus → 物化 → 返回题目与 meta 摘要」。

    :return: :class:`PreparedTask`（``tasks`` 供写 ``tasks.jsonl``，``meta`` 供写 ``meta.json``）。
    """
    warnings: list[str] = []
    if spec.unverified:
        raise DatasetError(
            f"{spec.name}: 注册表里该任务的 config/split 未经验证（unverified=True），"
            "请先手工核对 /splits 再补齐 evals/mtebcode/mtebcode_eval/data.py 的 TASKS"
        )

    layout = resolve_layout(client, spec)
    qrels_split = resolve_qrels_split(client, spec, layout)

    corpus_rows_total = client.num_rows(spec.hf_dataset, layout.corpus_config, layout.corpus_split)
    queries_rows_total = client.num_rows(spec.hf_dataset, layout.queries_config, layout.queries_split)

    qrels_all, qrels_read = fetch_qrels(client, spec.hf_dataset, layout.qrels_config, qrels_split)
    if not qrels_all:
        raise DatasetError(
            f"{spec.name}: {spec.hf_dataset} 的 qrels（{layout.qrels_config}/{qrels_split}）没有 score>0 的记录"
        )

    query_ids = sample_query_ids(list(qrels_all), options.limit_queries, options.seed, spec.name)
    if not query_ids:
        raise DatasetError(f"{spec.name}: 抽样后没有可用 query")

    query_rows, query_stats = fetch_rows_by_ids(
        client, spec.hf_dataset, layout.queries_config, layout.queries_split, query_ids, queries_rows_total
    )
    missing_queries = [qid for qid in query_ids if qid not in query_rows]
    if missing_queries:
        warnings.append(
            f"{len(missing_queries)} 条 query 在 {layout.queries_config}/{layout.queries_split} 里找不到"
            f"（例如 {missing_queries[0]}），已跳过"
        )
        query_ids = [qid for qid in query_ids if qid in query_rows]

    # ---- gold doc ----
    gold_by_query: dict[str, list[str]] = {}
    for query_id in query_ids:
        graded = qrels_all.get(query_id, {})
        gold_by_query[query_id] = [
            doc_id for doc_id, _score in sorted(graded.items(), key=lambda pair: (-pair[1], pair[0]))
        ]
    gold_ids_ordered: list[str] = []
    seen_gold: set[str] = set()
    for query_id in query_ids:
        for doc_id in gold_by_query[query_id]:
            if doc_id not in seen_gold:
                seen_gold.add(doc_id)
                gold_ids_ordered.append(doc_id)

    truncated = False
    if len(gold_ids_ordered) > options.corpus_size:
        truncated = True
        warnings.append(
            f"gold doc 数 {len(gold_ids_ordered)} 超过 --corpus-size {options.corpus_size}，"
            f"已截断到 {options.corpus_size}（部分 query 的 gold 会缺失，指标偏低）"
        )
        gold_ids_ordered = gold_ids_ordered[: options.corpus_size]
        seen_gold = set(gold_ids_ordered)

    need_distractors = max(0, options.corpus_size - len(gold_ids_ordered))
    distractor_rows, distractor_stats = sample_rows(
        client,
        spec.hf_dataset,
        layout.corpus_config,
        layout.corpus_split,
        corpus_rows_total,
        need_distractors,
        options.seed,
        exclude=set(seen_gold),
    )

    gold_rows, gold_stats = fetch_rows_by_ids(
        client, spec.hf_dataset, layout.corpus_config, layout.corpus_split, gold_ids_ordered, corpus_rows_total
    )
    if gold_stats.missing:
        warnings.append(
            f"{len(gold_stats.missing)} 个 gold doc 在 corpus 里找不到"
            f"（例如 {gold_stats.missing[0]}），该 query 的 qrels 会被裁剪"
        )

    pool_rows: list[dict[str, Any]] = []
    seen_docs: set[str] = set()
    for row in list(gold_rows.values()) + distractor_rows:
        doc_id = str(row.get("_id") or "")
        if not doc_id or doc_id in seen_docs:
            continue
        seen_docs.add(doc_id)
        pool_rows.append(row)

    destination = corpus_dir(options.workdir, spec)
    docmap, written = materialize_docs(spec, pool_rows, destination, force=options.force)
    pool_ids = set(docmap.values())
    git_ok, git_detail = git_init(destination)

    # ---- 组装 Task ----
    tasks: list[Task] = []
    for index, query_id in enumerate(query_ids):
        graded = {doc_id: score for doc_id, score in qrels_all.get(query_id, {}).items() if doc_id in pool_ids}
        golds = [doc_id for doc_id in gold_by_query[query_id] if doc_id in pool_ids]
        if not graded:
            warnings.append(f"query {query_id} 的 gold doc 全部不在池内，该题会被跳过")
        tasks.append(
            Task(
                task_id=f"{spec.slug}__q{index:04d}",
                task=spec.name,
                hf_dataset=spec.hf_dataset,
                query_id=query_id,
                query_text=_query_text(query_rows[query_id]),
                gold_doc_ids=golds,
                corpus_dir=str(destination),
                docmap_path=str(destination / DOCMAP_NAME),
                n_pool_docs=len(pool_ids),
                qrels={doc_id: float(score) for doc_id, score in graded.items()},
                qrels_split=qrels_split,
                language=spec.language,
            )
        )

    if not git_ok:
        warnings.append(f"git init 失败（codeseek init 会因 `No project found` 失败）：{git_detail}")

    if not options.quiet:
        _log(
            f"[prepare] {spec.name}: qrels={layout.qrels_config}/{qrels_split} "
            f"({len(qrels_all)} queries, {qrels_read} rows) → 抽 {len(tasks)} 题；"
            f"池 {len(pool_ids)} doc（gold {len(gold_ids_ordered)} + 干扰 {len(distractor_rows)}）；"
            f"corpus 扫描 {gold_stats.mode}/{gold_stats.pages}p + 抽样 {distractor_stats.pages}p"
        )

    return PreparedTask(
        spec=spec,
        tasks=tasks,
        meta={
            "task": spec.name,
            "task_slug": spec.slug,
            "hf_dataset": spec.hf_dataset,
            "corpus_config": layout.corpus_config,
            "corpus_split": layout.corpus_split,
            "queries_config": layout.queries_config,
            "queries_split": layout.queries_split,
            "qrels_config": layout.qrels_config,
            "qrels_split": qrels_split,
            "doc_ext": spec.doc_ext,
            "language": spec.language,
            "num_corpus_rows": corpus_rows_total,
            "num_queries_rows": queries_rows_total,
            "num_qrels_rows": qrels_read,
            "num_queries_sampled": len(tasks),
            "num_pool_docs": len(pool_ids),
            "num_gold_docs": len(gold_ids_ordered),
            "num_distractor_docs": len(distractor_rows),
            "n_pool_docs_target": options.corpus_size,
            "gold_truncated": truncated,
            "docs_written": written,
            "corpus_dir": str(destination),
            "docmap_path": str(destination / DOCMAP_NAME),
            "git": {"ok": git_ok, "detail": git_detail},
            "fetch": {
                "corpus": gold_stats.to_dict(),
                "distractors": distractor_stats.to_dict(),
                "queries": query_stats.to_dict(),
            },
            "warnings": warnings,
        },
    )


def sampling_fingerprint(tasks: Sequence[Task], seed: int, corpus_size: int, limit_queries: int) -> str:
    """抽样指纹：把 task_id / query_id / gold 集合 / 口径参数一起哈希，便于比对两次 prepare。"""
    payload = json.dumps(
        {
            "seed": seed,
            "corpus_size": corpus_size,
            "limit_queries": limit_queries,
            "tasks": [
                {"task_id": task.task_id, "query_id": task.query_id, "gold": sorted(task.gold_doc_ids)}
                for task in tasks
            ],
        },
        ensure_ascii=False,
        sort_keys=True,
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16]


def benchmark_meta_header(source: str, command: str) -> dict[str, Any]:
    """报告 / meta.json 的公共头部字段。"""
    return {
        "generated_at": _utc_now(),
        "benchmark": BENCHMARK,
        "source": source,
        "command": command,
    }
