# RepoQA 实测结果归档

这是 codeseek 在 RepoQA **100 题子集**上的端到端评测结果归档（非估算、非抽样外推），当前包含**两个批次**：2026-11-11 的**修复前基线**（74.0%）与 2026-11-12 的**修复后结果**（82.0%）。

| 文件 | 说明 |
| --- | --- |
| `2026-11-12-report.md` | **最新（修复后）**：中文可读报告（总体/分语言/分 repo/失败 Top10/复现命令），总体 82.0%，BLEU backend = `nltk` |
| `2026-11-12-report.json` | **最新（修复后）**：机器可读明细（`meta` / `aggregate` / `by_repo` / 逐题 `per_task` / `failure_top`） |
| `2026-11-11-report.md` | **修复前基线**：同一口径的中文可读报告，总体 74.0%，BLEU backend = `nltk` |
| `2026-11-11-report.json` | **修复前基线**：机器可读明细 |
| `2026-11-11-report.builtin.md` | 修复前基线的内置 BLEU 复刻实现副本（`backend=builtin`），用于与 nltk 对拍 |
| `2026-11-11-report.builtin.json` | 同上，机器可读版本 |

所有 `.md` / `.json` 均为 `/tmp/repoqa-eval/`（`--workdir` 默认位置）下同名产物的**原样复制**，仅追加日期前缀重命名，未做任何内容改动；`.md` 由 `repoqa_eval/report.py` 的 `render_markdown()` 渲染（两个批次同一渲染器，见下方"归档时间线"的溯源说明）。

## 归档时间线

| 批次 | 文件（主口径 `nltk`） | 总体 accuracy | rust | 被测二进制 `sha256` 前缀 | 说明 |
| --- | --- | --- | --- | --- | --- |
| **2026-11-11**（修复前基线） | `2026-11-11-report.md` / `.json` | **74.0%**（74/100） | 50.0%（10/20） | `5972636dc15431ee` | 修复前基线，作为对照基线保留 |
| **2026-11-12**（修复后） | `2026-11-12-report.md` / `.json` | **82.0%**（82/100） | 90.0%（18/20） | `38edff1bb1b1d1cd` | 较基线 **+8.0pt**；**8 题翻转为通过、0 题回归**，且 8 题**全部为 rust** |

两次评测的口径完全一致：同一 100 题子集（5 语言 × 2 仓库 × 10 needle）、同一数据集（`sha256` 前缀 `bd3f7cab47283cde`，release `2024-06-23`）、同一 `threshold = 0.8`、同一 BLEU backend（`nltk`）、`hybrid: 100`（`name_fallback: 0`、`unknown_empty: 0`，无退化、无空结果）；两次的 python / cpp / java / typescript 分项**逐项相同**，差异全部来自 rust。

2026-11-12 的 8 道翻转题（`fail → pass`，逐题比对 `per_task.passed` 得出；无回归题）：

| # | 题目 id | 仓库 |
| --- | --- | --- |
| 1 | `rust__rust-bakery__nom__n01` | rust-bakery/nom |
| 2 | `rust__rust-bakery__nom__n03` | rust-bakery/nom |
| 3 | `rust__rust-bakery__nom__n04` | rust-bakery/nom |
| 4 | `rust__rust-bakery__nom__n06` | rust-bakery/nom |
| 5 | `rust__rust-bakery__nom__n08` | rust-bakery/nom |
| 6 | `rust__helix-editor__helix__n00` | helix-editor/helix |
| 7 | `rust__helix-editor__helix__n06` | helix-editor/helix |
| 8 | `rust__helix-editor__helix__n09` | helix-editor/helix |

> **`.builtin.*` 后缀的含义**：它表示**用内置 BLEU 复刻后端**（`backend=builtin`，未安装 nltk 时的兜底实现）对同一批检索结果重新判分的副本，用于与主口径对拍。**主口径是 `nltk`**，即不带后缀的 `.md` / `.json`。2026-11-11 批次做过 100 题逐位对拍：逐题 `best_similarity` 变化 0/100，`passed` 翻转 0，因此两套分数完全相同。2026-11-12 批次只产出了 `nltk` 主口径，**未生成** `.builtin.*` 副本。
>
> **溯源**：两份 `.md` 均由 `repoqa_eval/report.py` 的 `render_markdown()` 渲染，未手工改数字。验证方式：用该渲染器渲染 `2026-11-11-report.json`，与仓库中已有的 `2026-11-11-report.md` **逐行零差异**（77 行全等），说明两批报告章节结构、表头与措辞同源。

## 1. 关键结论

> 本节及第 3 节的分析均针对 **2026-11-11 修复前基线**（保留原文不改，便于与历史结论对照）；最新结果见上方"归档时间线"与 [`2026-11-12-report.md`](2026-11-12-report.md)。

**总体 accuracy：74.0%（74/100 题通过，threshold = 0.8）**

| 语言 | 通过/总数 | accuracy |
| --- | --- | --- |
| python | 18/20 | 90.0% |
| cpp | 9/20 | 45.0% |
| java | 17/20 | 85.0% |
| typescript | 20/20 | 100.0% |
| rust | 10/20 | 50.0% |
| **all** | **74/100** | **74.0%** |

## 2. 评测条件

> 下表为 **2026-11-11 修复前基线**的评测条件。2026-11-12 批次口径完全相同，仅两处取值不同：被测二进制改为 `sha256` 前缀 `38edff1bb1b1d1cd`（`size_bytes=60583760`，mtime `2026-11-12T03:05:33.317865+00:00`），`embedding` 配置检测结果为"已配置（model=Qwen/Qwen3-Embedding-4B，api_token 长度 51）"而非"未检测"。

| 项 | 取值 |
| --- | --- |
| 数据子集口径 | 5 语言 × 2 仓库 × 10 needle = **100 题**（`python/cpp/java/typescript/rust`，**不含 go**；详见 `../README.md` 第 5 节） |
| 数据集 | 官方 release `2024-06-23`，`size_bytes=71510991`，`sha256` 前缀 `bd3f7cab47283cde` |
| 判定阈值 | `threshold = 0.8`（BLEU-4 + Chen & Cherry `method4`，候选集为该 repo 的 10 条 needle） |
| 检索模式 | `hybrid: 100`（100% 真语义检索：LanceDB + BM25，结果含 `code_block`） |
| 空结果 | `name_fallback: 0`、`unknown_empty: 0`，即 **0 空结果、0 退化** |
| `search_limit` | 5（每题的 Top-5 都落盘，判分只取 Top-1） |
| tree-sitter | `false`（可选依赖，未安装；只影响从模型输出里取"第一个函数"这一步） |
| 被测二进制 | `rust-core/target/release/codeseek`，`size_bytes=60583728`，`sha256` 前缀 `5972636dc15431ee`，mtime `2026-11-11T14:05:20.007098+00:00` |
| codeseek 版本 | `codeseek 0.1.31`（`codeseek --version`），**含 commit `c56a41f` 的并发修复**（`fix(search): stop silent degradation of concurrent hybrid search`） |
| BLEU backend | `nltk`（官方同款实现）；内置复刻实现经 **100 题逐位对拍零差异**（逐题 `best_similarity` 变化 0/100，max abs diff `0.0`，`passed` 翻转 0），故 `report.builtin.*` 与 `report.*` 分数完全相同 |

## 3. 三条关键洞察

### 3.1 通过组的函数边界与 needle 定义同源

74 道通过题中，**73 题**的输出与 ground truth **逐字相同**（不是"相似"，是字符串级相等）。
唯一的例外是 `python__python-poetry__poetry__n01`：code_block 比 GT 多了 `@classmethod` 装饰器一行（225 vs 208 字符），仍以 BLEU ≥ 0.8 判过。

> 含义：codeseek 的索引侧切块边界与 RepoQA 的 needle 切片口径高度一致，通过不是靠 BLEU 容错"擦边"进来的。

### 3.2 cpp 45.0% 的主因是切块粒度，不是检索能力

11 道 cpp 失败题的 **recall@5 达 81.8%（9/11）**——正确符号其实已经被检索到了，但返回的 `code_block` 是**整个 class/struct**，而 needle 是一个**构造器 / 成员方法**。
例：`ThrowableInformationPatternConverterPrivate` 返回 12 行 420 字符，GT 只有 3 行 185 字符（字符比 2.27），BLEU = 0.392 < 0.8。

若改用"GT 落在返回块内即算通过"的粒度容忍口径，cpp 从 **45.0% → 75.0%（+30.0pt）**，总体从 74.0% → 82.0%（+8.0pt）。这说明 cpp 的短板是**切块**，不是**排序**。

> 注：这里的 82.0% 是**粒度容忍口径下的假想值**，与 2026-11-12 修复后**实测**的 82.0% 仅是数值巧合相同，两者的口径与构成完全不同，勿混淆（后者仍是不容忍粒度的原口径，见上方"归档时间线"）。

### 3.3 rust 50.0% 的主因是检索选错符号

10 道 rust 失败题的 **recall@5 仅 20.0%（2/10）**，其中 6 道是 `wrong_target`（返回的符号名与 needle 不同），例如 `tagtr_succeed` → 返回 `test`（内层 helper fn）。
在"GT 被包含即算通过"口径下 rust 仍是 **50.0%（+0.0pt）**——因为 GT 根本不在返回块里，包容忍也救不回来。

> 两条主因性质不同：cpp 要改**索引切块粒度**（构造器/成员方法不并入外层 class），rust 要改**检索排序**（同为测试函数时区分 inner fn 与外层 test fn）。参见 `../README.md` 第 5 节"已知限制与风险"。

## 4. 复现

```bash
cd evals/repoqa
pip install -r requirements.txt                                    # 装 nltk 以使用官方同款 BLEU
python -m repoqa_eval prepare --workdir /tmp/repoqa-eval           # 下载数据 + 抽 100 题子集 + git init
python -m repoqa_eval run --workdir /tmp/repoqa-eval --limit 5 --threshold 0.8 --concurrency 4
python -m repoqa_eval score --workdir /tmp/repoqa-eval             # 换阈值重判，不重跑检索
```

评测口径、判分实现细节与已知限制见上一级 [`../README.md`](../README.md)。
