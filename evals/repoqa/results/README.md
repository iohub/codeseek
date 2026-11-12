# RepoQA 实测结果归档

> **主口径（当前结果）：2026-11-12 报告，总体 82.0%（82/100）**，被测二进制 `sha256` 前缀 `38edff1bb1b1d1cd`。
> 本文正文（关键结论 / 评测条件 / 三条关键洞察 / 已知限制）**全部以该批次为准**。
>
> **历史对照（修复前基线）：2026-11-11 报告，总体 74.0%（74/100）**，`sha256` 前缀 `5972636dc15431ee`。除显式标注「基线」的小节外，正文不再引用其数字。

这是 codeseek 在 RepoQA **100 题子集**上的端到端评测结果归档（非估算、非抽样外推），当前包含**两个批次**：2026-11-12 的**修复后结果（主口径，82.0%）**与 2026-11-11 的**修复前基线（74.0%，仅作历史对照）**。

| 文件 | 说明 |
| --- | --- |
| `2026-11-12-report.md` | **主口径**：中文可读报告（总体/分语言/分 repo/失败 Top10/复现命令），总体 82.0%，BLEU backend = `nltk` |
| `2026-11-12-report.json` | **主口径**：机器可读明细（`meta` / `aggregate` / `by_repo` / 逐题 `per_task` / `failure_top`） |
| `2026-11-11-report.md` | **修复前基线（历史对照）**：同一口径的中文可读报告，总体 74.0%，BLEU backend = `nltk` |
| `2026-11-11-report.json` | **修复前基线**：机器可读明细 |
| `2026-11-11-report.builtin.md` | 基线的内置 BLEU 复刻实现副本（`backend=builtin`），用于与 nltk 对拍 |
| `2026-11-11-report.builtin.json` | 同上，机器可读版本 |

所有 `.md` / `.json` 均为 `/tmp/repoqa-eval/`（`--workdir` 默认位置）下同名产物的**原样复制**，仅追加日期前缀重命名，未做任何内容改动；`.md` 由 `repoqa_eval/report.py` 的 `render_markdown()` 渲染（两个批次同一渲染器，见下方「归档时间线」的溯源说明）。

## 归档时间线

| 批次 | 文件（主口径 `nltk`） | 总体 accuracy | rust | 被测二进制 `sha256` 前缀 | 说明 |
| --- | --- | --- | --- | --- | --- |
| **2026-11-12**（**当前主口径**） | `2026-11-12-report.md` / `.json` | **82.0%**（82/100） | 90.0%（18/20） | `38edff1bb1b1d1cd` | 较基线 **+8.0pt**；**8 题翻转为通过、0 题回归**，且 8 题**全部为 rust** |
| 2026-11-11（修复前基线，历史对照） | `2026-11-11-report.md` / `.json` | 74.0%（74/100） | 50.0%（10/20） | `5972636dc15431ee` | 修复前基线，仅用于度量本次改进幅度 |

两次评测的口径完全一致：同一 100 题子集（5 语言 × 2 仓库 × 10 needle）、同一数据集（`sha256` 前缀 `bd3f7cab47283cde`，release `2024-06-23`）、同一 `threshold = 0.8`、同一 BLEU backend（`nltk`）、`hybrid: 100`（`name_fallback: 0`、`unknown_empty: 0`，无退化、无空结果）；两次的 python / cpp / java / typescript 分项**逐项相同**，差异全部来自 rust。

2026-11-12 的 8 道翻转题（`fail → pass`，逐题比对两批 `per_task.passed` 得出；无回归题）：

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

> **`.builtin.*` 后缀的含义**：它表示**用内置 BLEU 复刻后端**（`backend=builtin`，未安装 nltk 时的兜底实现）对同一批检索结果重新判分的副本，用于与主口径对拍。**主口径是 `nltk`**，即不带后缀的 `.md` / `.json`。2026-11-11 批次做过 100 题逐位对拍：逐题 `best_similarity` 无差异、`passed` 无翻转，因此两套分数完全相同。2026-11-12 批次只产出了 `nltk` 主口径，**未生成** `.builtin.*` 副本。
>
> **溯源**：两份 `.md` 均由 `repoqa_eval/report.py` 的 `render_markdown()` 渲染，未手工改数字。验证方式：用该渲染器渲染 `2026-11-11-report.json`，与仓库中已有的 `2026-11-11-report.md` **逐行零差异**（77 行全等），说明两批报告章节结构、表头与措辞同源。

## 1. 关键结论

**总体 accuracy：82.0%（82/100 题通过，threshold = 0.8）**

| 语言 | 通过/总数 | accuracy |
| --- | --- | --- |
| python | 18/20 | 90.0% |
| cpp | 9/20 | 45.0% |
| java | 17/20 | 85.0% |
| typescript | 20/20 | 100.0% |
| rust | 18/20 | 90.0% |
| **all** | **82/100** | **82.0%** |

分 repo 明细见 `2026-11-12-report.md` 第 3 节：rust 侧 `rust-bakery/nom` 8/10（80.0%）、`helix-editor/helix` 10/10（100.0%）；其余 8 个 repo 与基线逐项相同。

### 1.1 相对基线的改进来源（仅 rust）

本次 +8.0pt **完全来自 rust 解析器的两处修复**，代码位于 `rust-core/src/codegraph/treesitter/parsers/rust.rs`（commit `9e74d94`）：

1. **`parse_block` 新增 `mod_item` 分支**（`rust.rs:1039-1043`）。此前 `mod_item` 没有分支，落到 `_ =>` 走 `parse_usages`，而 `parse_usages` 的兜底 `_ => {}` **不递归** → 整个 `#[cfg(test)] mod tests { ... }` 子树零产出（`rust-bakery/nom/src/str.rs` 改前产出 0 个符号）。RepoQA 的 rust needle 大量落在 `mod tests` 内，这是 rust 召回的**主因**。
2. **`impl_item` 的 `full_range` 收窄到 `declaration_range`**（`rust.rs:321-323`）。使 impl 容器 chunk 不再覆盖其成员方法的源码区间，避免「父容器 chunk 是成员的严格超集」导致的 BLEU 稀释。

效果（两批报告 §1 数值之差）：

| 指标 | 基线（2026-11-11） | 当前（2026-11-12） | 变化 |
| --- | --- | --- | --- |
| rust | 50.0%（10/20） | 90.0%（18/20） | **+40.0pt** |
| **总体** | 74.0%（74/100） | **82.0%（82/100）** | **+8.0pt** |
| fail → pass | — | — | **8 题** |
| pass → fail（回归） | — | — | **0 题** |

非 rust 语言分项**逐项未变**（python 18/20、cpp 9/20、java 17/20、typescript 20/20），因此总体提升恰好等于 rust 的净增量（8 题 = 8.0pt）。

## 2. 评测条件

> 下表为 **2026-11-12 主口径**的评测条件。2026-11-11 基线批次口径完全相同，仅三处取值不同：被测二进制为 `sha256` 前缀 `5972636dc15431ee`（mtime `2026-11-11T14:05:20.007098+00:00`）、`embedding` 配置检测结果为「未检测」、`run` 使用 `--concurrency 4`（当前批次为 `1`）。

| 项 | 取值 |
| --- | --- |
| 数据子集口径 | 5 语言 × 2 仓库 × 10 needle = **100 题**（`python/cpp/java/typescript/rust`，**不含 go**；详见 `../README.md` 第 5 节） |
| 数据集 | 官方 release `2024-06-23`，`size_bytes=71510991`，`sha256` 前缀 `bd3f7cab47283cde` |
| 判定阈值 | `threshold = 0.8`（BLEU-4 + Chen & Cherry `method4`，候选集为该 repo 的 10 条 needle） |
| 检索模式 | `hybrid: 100`（100.0% 真语义检索：LanceDB + BM25，结果含 `code_block`） |
| 空结果 | `name_fallback: 0`、`unknown_empty: 0`，即 **0 空结果、0 退化** |
| `search_limit` | 5（每题的 Top-5 都落盘，判分只取 Top-1） |
| tree-sitter | `false`（可选依赖，未安装；只影响从模型输出里取"第一个函数"这一步） |
| 被测二进制 | `rust-core/target/release/codeseek`，`sha256` 前缀 `38edff1bb1b1d1cd`，mtime `2026-11-12T03:05:33.317865+00:00` |
| codeseek 版本 | `codeseek 0.1.31`（`codeseek --version`），含 commit `c56a41f` 的并发修复（`fix(search): stop silent degradation of concurrent hybrid search`） |
| embedding 配置 | `embedding 已配置（model=Qwen/Qwen3-Embedding-4B，api_token 长度 51）` |
| BLEU backend | `nltk`（官方同款实现）；基线批次的内置复刻实现经 **100 题逐位对拍零差异**，故 `report.builtin.*` 与 `report.*` 分数完全相同 |

## 3. 三条关键洞察

### 3.1 通过组的函数边界与 needle 定义同源

当前 82 道通过题中，**81 题**的 `best_similarity` 恰为 `1.0`（BLEU-4 满分，等价于返回的 `code_block` 与 ground truth 完全一致，而非"相似"）。基线批次同样是 74 道通过题中 73 道满分，两批唯一的非满分通过题都是 `python__python-poetry__poetry__n01`（`best_similarity=0.946`）：其 `code_block` 比 GT 多出 `@classmethod` 装饰器一行，仍以 BLEU ≥ 0.8 判过。

> 含义：codeseek 的索引侧切块边界与 RepoQA 的 needle 切片口径高度一致，通过不是靠 BLEU 容错"擦边"进来的。

### 3.2 cpp 45.0% 的主因是切块粒度，不是检索能力（当前最大瓶颈，P0）

当前 cpp 为 **9/20（45.0%）**，11 道失败题中有 **7 道**的 `best_target` 与 needle **同名**（即正确符号已被检索为 Top-1，仅因 `code_block` 粒度与 needle 不符而 BLEU 低于阈值）。例：`ThrowableInformationPatternConverterPrivate` 返回的 `code_block` 是**整个 class/struct**，而 needle 是其中一个**构造器 / 成员方法**，`best_similarity=0.392`。

基线批次是同样的形态（11 道 cpp 失败中 7 道 `best_target` 与 needle 同名），说明这是**索引侧切块**的结构性短板，而非排序或召回问题。

> 结论：cpp 的改进方向是**切块粒度**（构造器/成员方法不应并入外层 class），已列为 P0，见第 4 节。

### 3.3 rust 的提升证明"漏索引"是主因，剩余失败转向边界与排序

基线 rust 的 10 道失败中，**6 道为 `wrong_target`**、4 道为 `below_threshold`——多数情况下返回的符号名根本不是 needle，属**符号未进入索引**（`mod tests` 子树零产出），补上 `mod_item` 递归后即整批消失（8 题翻转）。

当前 rust 仅剩 2 道失败：

| 题 | needle | best_target | sim | 原因 | 性质 |
| --- | --- | --- | --- | --- | --- |
| `rust__rust-bakery__nom__n07` | `be_i8_tests` | `be_i8_tests` | 0.721 | `below_threshold` | **切块边界差异**（符号已召回，块比 needle 长/短） |
| `rust__rust-bakery__nom__n09` | `length_count` | `parse` | 0.017 | `wrong_target` | 仍为排序/同名干扰 |

> 两条主因性质不同：cpp 要改**索引切块粒度**（成员方法不并入外层 class），rust 剩余问题是**切块边界收窄 + 同名符号排序**。

## 4. 已知限制与后续

- **P0 · cpp 切块粒度**：cpp 45.0% 是当前最大瓶颈。父容器 chunk（class/struct）是成员的**严格超集**，且**抢占 rank1**——11 道失败题中 7 道 `best_target` 与 needle 同名，纯粹是"块太大"导致 BLEU 分母被稀释。修复方向：索引侧对 cpp 容器符号与成员符号做父子包含剪枝（成员独立成块，容器块不覆盖成员区间），与本次 rust `impl_item` 的 `full_range` 收窄同构。
- **新报告失败 Top1 是边界问题、不是召回问题**：`rust__rust-bakery__nom__n07`（`be_i8_tests`）`best_target` 与 needle **同名**，仅 `best_similarity=0.721 < 0.8`，属**切块边界差异**；该题在基线中是 `wrong_target`（返回 `many1_count`），本次已从"召回失败"改善为"边界不符"，是修复的合理残留而非回归。
- **`code_block` 粒度直接影响 BLEU**：hybrid 结果返回的代码块由索引侧切分，可能包含装饰器/注释或截断；`threshold=0.8` 对粒度差异相当敏感（`be_i8_tests` 的 BLEU 与阈值相差不到 0.1）。
- **非 rust 分项已达平台期**：python 90.0%、java 85.0%、typescript 100.0% 两批完全一致；python/java 的 5 道失败以 `wrong_target` 为主（同名符号排序），不属于切块问题。
- **未做粒度容忍口径度量**：本归档只报告官方口径（严格的 `best_target == GT and BLEU ≥ 0.8`）。"GT 落在返回块内即算通过"的宽松口径会显著抬高 cpp 分数，但该值与官方榜不可比，故不写入结论。
- 其他口径限制（候选集仅 10 条 needle、`git init` 快照而非真实历史、无 token 时退化为名字匹配等）见上一级 [`../README.md`](../README.md) 第 5 节。

## 5. 复现

```bash
cd evals/repoqa
pip install -r requirements.txt                                    # 装 nltk 以使用官方同款 BLEU
python -m repoqa_eval prepare --workdir /tmp/repoqa-eval           # 下载数据 + 抽 100 题子集 + git init
python -m repoqa_eval run --workdir /tmp/repoqa-eval --limit 5 --threshold 0.8 --concurrency 1
python -m repoqa_eval score --workdir /tmp/repoqa-eval             # 换阈值重判，不重跑检索
```

> 两次批次唯一的运行参数差异是 `--concurrency`（基线用 `4`，当前批次用 `1`）；并发修复（`c56a41f`）后两者结果等价。

评测口径、判分实现细节与已知限制见上一级 [`../README.md`](../README.md)。
