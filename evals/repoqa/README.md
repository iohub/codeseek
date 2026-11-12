# RepoQA × codeseek 端到端评测套件

用 [RepoQA](https://github.com/evalplus/repoqa_release)（arXiv:2406.06025，ICML 2024）基准评测
codeseek 的**长上下文代码检索**能力。整套实现只用 Python 标准库（+ 可选 nltk），
所有产物落在 `--workdir`（默认 `/tmp/repoqa-eval`），**不污染仓库**。

> 判分逻辑逐字复刻官方 `repoqa/metric.py` 与 `repoqa/compute_score.py`：
> BLEU-4（按空白切词）+ Chen & Cherry `method4` 平滑 + 该 repo 的 10 条 needle 作候选集 + `threshold=0.8`。
> **没有任何 LLM 参与打分。**

---

## 1. 整体链路

```
                       ┌──────────────────────────────────────────────┐
  GitHub release ──────►│ data.download_dataset  (12.5MB gz → 71.5MB)  │
  (可用 REPOQA_DATA_    │   ├─ 缓存复用 / .part 原子改名                │
   OVERRIDE_PATH 离线)   │   └─ validate_dataset（6 语言 × 10 repo）    │
                       └───────────────┬──────────────────────────────┘
                                       ▼
                       ┌──────────────────────────────────────────────┐
   prepare             │ data.select_subset  5 语言 × 2 repo × 10     │
                       │   → 100 题（确定性，非随机）                  │
                       │ data.materialize_repo → repos/<lang>/<slug>/ │
                       │   + git init（codeseek init 的硬性前提）      │
                       │ data.write_tasks → tasks.jsonl + meta.json   │
                       └───────────────┬──────────────────────────────┘
                                       ▼
                       ┌──────────────────────────────────────────────┐
   run                 │ runner.run                                   │
                       │   ① 串行：codeseek init（每 repo 一次）        │
                       │   ② 并发：codeseek search "<description>"     │
                       │        --json --limit K                      │
                       │      adapter.parse_json_tolerant → results    │
                       │      adapter.build_outputs → ```lang 代码块   │
                       │      落盘 raw/<task_id>.json（断点续跑）       │
                       │   ③ scorer.score_tasks（BLEU + 10 needles）   │
                       │   ④ report.build_report / write_reports      │
                       └───────────────┬──────────────────────────────┘
                                       ▼
                    report.json（机器可读） + report.md（中文可读）
                                       ▲
   score  ──────── 只读 raw/*.json 重新判分（换 threshold 不必重跑检索）
```

## 2. 前置条件

| 依赖 | 说明 |
| --- | --- |
| Python ≥ 3.10 | 只用标准库；类型标注使用 `X | None` 语法 |
| codeseek 二进制 | `cargo build --release --manifest-path rust-core/Cargo.toml` → `rust-core/target/release/codeseek`（也可 `--codeseek-bin` / `CODESEEK_BIN` / `--auto-build`） |
| nltk（**可选但强烈建议**） | `pip install -r requirements.txt`。缺省时自动使用内置 method4 复刻实现，报告的 `meta.bleu_backend` 会写 `builtin` 并给出警示 |
| embedding token（**决定分数是否可比**） | `~/.codeseek/config.json` 的 `embedding.api_token` 非空才走 hybrid 语义检索 |
| git | `materialize_repo` 会 `git init` 一次；codeseek `init` 在非 git 目录会直接报 `No project found` |
| tree-sitter（可选） | 官方 `sanitize_output` 用它取"第一个函数定义"；本套件缺失时直接用代码块全文，不报错 |

## 3. 三条命令

```bash
cd evals/repoqa
pip install -r requirements.txt          # 可选：装 nltk

# ① 准备：下载数据 + 抽子集 + 落盘仓库 + git init + tasks.jsonl
python -m repoqa_eval prepare --workdir /tmp/repoqa-eval
#    只跑 python/rust 两种语言、每种 1 个 repo（10 题）：
python -m repoqa_eval prepare --workdir /tmp/repoqa-eval --languages python,rust --repos-per-language 1

# ② 评测（会先 build 检测，再串行 init、并发 search）
python -m repoqa_eval run --workdir /tmp/repoqa-eval --limit 5 --concurrency 4
#    冒烟：只跑前 3 题；要求必须启用语义检索，否则退出码 2
python -m repoqa_eval run --workdir /tmp/repoqa-eval --limit-questions 3 --require-embedding

# ③ 换阈值重判（不重跑检索，只读 raw/）
python -m repoqa_eval score --workdir /tmp/repoqa-eval --threshold 0.7

# ④ 离线自检（不联网、不需要 codeseek 二进制、不需要 nltk）
python -m repoqa_eval selftest
```

常用环境变量：

| 变量 | 作用 |
| --- | --- |
| `REPOQA_DATA_OVERRIDE_PATH` | 指向本地已有 `repoqa-2024-06-23.json`（或 `.json.gz`），跳过下载 |
| `CODESEEK_BIN` | codeseek 二进制路径 |
| `CODESEEK_CONFIG_PATH` | 覆盖 `~/.codeseek/config.json` 路径（便于用假配置验证告警逻辑） |
| `https_proxy` / `http_proxy` | 下载走代理（也可 `prepare --proxy http://127.0.0.1:7897`） |

## 4. 判分口径（逐条）

1. **切词**：`re.split(r"\s+", text.strip())` —— 按空白切，不是 AST/token 级。
   注意官方实现**不丢弃空串**（空输入会得到 `[""]`），本套件保持一致。
2. **相似度**：`nltk.translate.bleu_score.sentence_bleu([ref], cand, smoothing_function=SmoothingFunction().method4)`，
   默认权重 `(0.25, 0.25, 0.25, 0.25)`（BLEU-4）。
   内置复刻实现的要点（与 nltk 3.10.4 逐位一致，已对拍 308 个样例，最大误差 `0.0`）：
   - modified precision 的分母是 `max(1, hyp 的 n-gram 数)`；
   - **先短路**：任何 unigram 都没命中 → 直接 0（平滑救不回来）；
   - method4：`incvnt` 从 1 开始、只在替换时自增，替换值
     `(1 / (2**incvnt * k / log(hyp_len))) / denominator`，`k=5`，且只在 `p_i.numerator == 0 and hyp_len > 1` 时生效；
   - 最终 `bp * exp(fsum(w_i * log(p_i) for p_i > 0))`。
3. **sanitize**：官方正则 `^```(?:\w+)?\s*\n(.*?)(?=^```)````（`re.DOTALL|re.MULTILINE`）取第一个围栏代码块；
   抓不到就用整段输出；若装了 tree-sitter，再取块内第一个函数定义。
4. **候选集**：**只有该 repo 的 10 条 needle**（不是全仓库函数）。
   候选切片 `"\n".join(content[path].split("\n")[start_line:end_line])`，行号为文件内 0-based、`end` 不含。
5. **判定**：`best_target == ground_truth_name and best_similarity >= threshold`，
   `threshold` 默认 `0.8`；`best_target` 是 10 条 needle 中 BLEU **最高**的那条（严格 `>` 比较，并列取先出现者）。
6. **输出只用 Top-1**：官方只取 `output[0]`；本套件把 codeseek 的 Top-1 检索结果组装成 fenced 代码块作为 `output[0]`。
7. **无 LLM 参与**：整条链路没有任何生成式模型打分。

## 5. 已知限制与风险

- **无 token 时分数不可比**：`~/.codeseek/config.json` 的 `embedding.api_token` 为空时，
  codeseek 的 `search` 退化为 `graph.find_functions_by_name(query)` 的**函数名子串匹配**
  （JSON 元素没有 `code_block`）。此时分数衡量的是"名字匹配"而非语义检索，
  `report.md` 顶部会置顶 blockquote 警示，`meta.mode_counts.name_fallback` 给出题数。
- **候选集只有 10 条 needle**，比"全仓库函数检索"宽松得多；官方榜分数不可与自建全仓库评测互换。
- **官方 release 实为 6 语言 600 题**（README 写 5 语言 500 题）。本套件默认取
  `python/cpp/java/typescript/rust` 5 语言（不含 `go`），可通过 `--languages` 调整。
- **`code_block` 粒度可能与 needle 不一致**：hybrid 结果返回的代码块由索引侧切分，
  可能包含装饰器/注释或截断，直接影响 BLEU（这也是 BLEU≥0.8 才判过的主要噪声源）。
- **`position_ratio` 实为 0.05~0.95**：官方按 `(i + 0.5) / 10` 计算，本套件照抄，仅用于报告展示。
- **与官方 `repoqa.compute_score` 的差异**：本套件不强制 tree-sitter；
  另外"Top-1 取不到源码时用下一条能取到源码的结果顶上"（官方没有这个概念，它只有一次模型输出）。
  两者都会在 `report.md` 中注明（tree-sitter 状态见 `meta.treesitter_available`）。
- **`git init` 后索引的是快照而非原仓库历史**：RepoQA 的 `content` 只含被引用的源文件，
  因此覆盖的符号数少于真实仓库，检索难度与官方口径存在系统性差异。
- **`codeseek init` 的 stdout 有进度条污染**：`adapter.init_repo` 不解析输出内容，只看退出码；
  `search` 的 JSON 用三段式容错解析（整体 → `[..]`/`{..}` 截取 → 逐行 NDJSON）。
- **【P0 · 当前最大瓶颈】cpp 切块粒度 = 45.0%**：`cpp` 11 道失败中 **7 道** `best_target` 与 needle
  **同名**（正确符号已被检索为 Top-1），仅因返回的 `code_block` 是父容器（class/struct）的**严格超集**
  而 BLEU 低于 0.8（例 `ThrowableInformationPatternConverterPrivate`，sim=0.392）。**抢占 rank1 的是
  容器 chunk，不是成员 chunk**，属索引侧切块问题，不是检索能力问题。修复方向与 `rust` 的 `impl_item`
  `full_range` 收窄同构：容器符号的 chunk 不得覆盖成员符号的源码区间。
- **失败 Top1 是边界差异、不是召回失败**：`rust__rust-bakery__nom__n07`（`be_i8_tests`）
  `best_target` 与 needle **同名**但 `best_similarity=0.721 < 0.8`，属**切块边界差异**；
  该题在修复前基线里是 `wrong_target`（返回 `many1_count`），现已从"未召回"改善为"边界不符"。

## 6. 目录与产物

```
evals/repoqa/
├── README.md
├── requirements.txt
├── results/                # 已跑通的实测结果归档（见第 8 节）
│   ├── README.md
│   ├── 2026-11-12-report.md           # 主口径 backend=nltk，82.0%
│   ├── 2026-11-12-report.json
│   ├── 2026-11-11-report.md           # 修复前基线 backend=nltk，74.0%
│   ├── 2026-11-11-report.json
│   ├── 2026-11-11-report.builtin.md   # backend=builtin（对拍基线）
│   └── 2026-11-11-report.builtin.json
└── repoqa_eval/
    ├── __init__.py
    ├── data.py      # 下载/缓存/子集抽样/仓库快照/Task/needle 切片
    ├── bleu.py      # BLEU（nltk 优先 + 内置 method4 复刻）
    ├── scorer.py    # sanitize + 10-needle 候选集 + threshold
    ├── adapter.py   # binary 检测 / init / search / JSON 容错 / 输出组装
    ├── runner.py    # 串行 init + 并发 search + 断点续跑 + 判分编排
    ├── report.py    # report.json / report.md
    ├── cli.py       # prepare / run / score / selftest
    └── __main__.py  # python -m repoqa_eval
```

`<workdir>` 布局：

```
/tmp/repoqa-eval/
├── repoqa-2024-06-23.json.gz     # 原始 gz（12.5MB）
├── repoqa-2024-06-23.json        # 解压后（71.5MB）
├── meta.json                     # dataset_path / languages / repos_per_language
├── tasks.jsonl                   # 每题一行（id/language/repo/needle/description/position_ratio...）
├── repos/<language>/<owner>__<name>/   # 仓库快照 + .git + .repoqa_manifest.json
├── raw/<task_id>.json            # 每题检索结果（outputs/mode/elapsed/error）
├── report.json
└── report.md
```

## 7. 断点续跑与排错

- **续跑**：`raw/<task_id>.json` 已存在则跳过检索；若某 repo 全部题目都有 raw，
  连 `codeseek init` 也跳过（最贵的一步）。`--force` 强制重跑。
- **单 repo init 失败**：该 repo 下的题全部记 fail（raw 里 `error="init_failed: ..."`），
  不中断全局；`report.md` 会列出失败仓库。
- **单题 search 超时/非零退出**：记 fail 并继续，`raw.error` 保留 stdout/stderr 前 500 字符。
- **找不到二进制**：报错会直接给出 `cargo build --release --manifest-path rust-core/Cargo.toml`；
  加 `--auto-build` 可让脚本自己构建（仅在你显式要求时执行）。
- **报告里 `unknown_empty` 很多**：说明 search 没返回可解析 JSON（索引没建好 / 二进制不匹配），
  先手工跑一次 `cd <repo> && codeseek init && codeseek search "..." --json` 定位。

## 8. 实测结果

**当前主口径：2026-11-12 报告，总体 82.0%（82/100）**（100 题子集，5 语言 × 2 仓库 × 10 needle，
`threshold=0.8`，`backend=nltk`，`hybrid` 检索 100%，0 空结果）：

| 语言 | 通过/总数 | accuracy |
| --- | --- | --- |
| python | 18/20 | 90.0% |
| cpp | 9/20 | 45.0% |
| java | 17/20 | 85.0% |
| typescript | 20/20 | 100.0% |
| rust | 18/20 | 90.0% |
| **all** | **82/100** | **82.0%** |

- 报告与逐题明细：[`results/README.md`](results/README.md)（`results/2026-11-12-report.md` / `.json` 为主口径）
- 改进来源：`rust` 解析器的 `mod_item` 分支（`rust-core/src/codegraph/treesitter/parsers/rust.rs` 的
  `parse_block`）+ `impl_item` 的 `full_range` 收窄到 `declaration_range`，使 rust 由 50.0% → 90.0%
  （+40.0pt），总体由 74.0% → 82.0%（+8.0pt），**8 题翻转、0 回归**。
- 三条关键洞察（切块与 needle 同源、cpp 45.0% 主因是切块粒度=P0、rust 漏索引是主因）见该目录说明。

**历史对照（修复前基线）**：2026-11-11 首次端到端评测，总体 74.0%（74/100），rust 50.0%（10/20），
二进制 `sha256` 前缀 `5972636dc15431ee`；该批次仅作基线保留（`results/2026-11-11-report.md` / `.json`，
另有 builtin BLEU 对拍基线 `*.builtin.*`）。
