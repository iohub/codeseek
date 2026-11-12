# RepoQA 实测结果归档

> **主口径（当前结果）：2026-11-12（b）报告，总体 84.0%（84/100）**，被测二进制 `sha256` 前缀 `b1532bb1b43e3d65`。
> 本文正文（已实测否决的方案 / 关键结论 / 评测条件 / 三条关键洞察 / 已知限制）**全部以该批次为准**。
>
> **历史对照（上一轮基线）：2026-11-12 报告（首批），总体 82.0%（82/100）**，`sha256` 前缀 `38edff1bb1b1d1cd`，对应 rust `mod_item` 修复。
> **历史对照（修复前基线）：2026-11-11 报告，总体 74.0%（74/100）**，`sha256` 前缀 `5972636dc15431ee`。
> 除显式标注「基线」的小节外，正文不再引用这两批的数字。

这是 codeseek 在 RepoQA **100 题子集**上的端到端评测结果归档（非估算、非抽样外推），当前包含**三个批次**：2026-11-12（b）的**排序期包含降权结果（主口径，84.0%）**、2026-11-12 的**上一轮基线（82.0%，rust `mod_item` 修复）**、2026-11-11 的**修复前基线（74.0%，仅作历史对照）**。

| 文件 | 说明 |
| --- | --- |
| `2026-11-12b-report.md` | **主口径**：中文可读报告（总体/分语言/分 repo/失败 Top10/复现命令），总体 84.0%，BLEU backend = `builtin` |
| `2026-11-12b-report.json` | **主口径**：机器可读明细（`meta` / `aggregate` / `by_repo` / 逐题 `per_task` / `failure_top`） |
| `2026-11-12-report.md` | **上一轮基线**：rust `mod_item` 修复后的报告，总体 82.0%，BLEU backend = `nltk` |
| `2026-11-12-report.json` | 同上，机器可读明细 |
| `2026-11-11-report.md` | **修复前基线（历史对照）**：同一口径的中文可读报告，总体 74.0%，BLEU backend = `nltk` |
| `2026-11-11-report.json` | **修复前基线**：机器可读明细 |
| `2026-11-11-report.builtin.md` | 基线的内置 BLEU 复刻实现副本（`backend=builtin`），用于与 nltk 对拍 |
| `2026-11-11-report.builtin.json` | 同上，机器可读版本 |

所有 `.md` / `.json` 均为 `/tmp/repoqa-eval/`（`--workdir` 默认位置）下同名产物的**原样复制**，仅追加日期前缀重命名，未做任何内容改动；`.md` 由 `repoqa_eval/report.py` 的 `render_markdown()` 渲染（三个批次同一渲染器，见下方「归档时间线」的溯源说明）。

> **为什么文件名带 `b` 后缀**：主口径批次的 `meta.generated_at` 为 `2026-11-12T07:18:32+00:00`，日期与已归档的 `2026-11-12-report.*`（上一轮基线）**冲突**。为不覆盖、不改写历史归档，该批次命名为 `2026-11-12b-report.json` / `.md`，表示**同一天的第二批**；两批的 `meta.generated_at` 时间戳与二进制 `sha256` 前缀均可区分。

## 归档时间线

| 批次 | 文件（主口径 `nltk`/`builtin`） | 总体 accuracy | cpp | rust | 被测二进制 `sha256` 前缀 | 说明 |
| --- | --- | --- | --- | --- | --- | --- |
| **2026-11-12b**（**当前主口径**） | `2026-11-12b-report.md` / `.json` | **84.0%**（84/100） | **55.0%（11/20）** | 90.0%（18/20） | `b1532bb1b43e3d65` | 较上一轮基线 **+2.0pt**；改动来源=**排序期父子包含降权（仅 cpp）**，cpp **+2 题（45.0% → 55.0%）**，**其余四语言零回归** |
| 2026-11-12（上一轮基线：rust `mod_item` 修复） | `2026-11-12-report.md` / `.json` | 82.0%（82/100） | 45.0%（9/20） | 90.0%（18/20） | `38edff1bb1b1d1cd` | 较修复前基线 **+8.0pt**；**8 题翻转为通过、0 题回归**，且 8 题**全部为 rust** |
| 2026-11-11（修复前基线，历史对照） | `2026-11-11-report.md` / `.json` | 74.0%（74/100） | 45.0%（9/20） | 50.0%（10/20） | `5972636dc15431ee` | 修复前基线，仅用于度量改进幅度 |

三批评测的检索口径完全一致：同一 100 题子集（5 语言 × 2 仓库 × 10 needle）、同一数据集（`sha256` 前缀 `bd3f7cab47283cde`，release `2024-06-23`）、同一 `threshold = 0.8`、`hybrid: 100`（`name_fallback: 0`、`unknown_empty: 0`，无退化、无空结果）。

- **74.0% → 82.0%**：差异**全部来自 rust**（50.0% → 90.0%，+40.0pt）；python / cpp / java / typescript 分项逐项相同。
- **82.0% → 84.0%**：差异**全部来自 cpp**（45.0% → 55.0%，+2 题）；python / java / typescript / rust 的 **80 道非 cpp 题 `best_similarity` 逐位相同**（不是"聚合分相同"，是浮点取值完全一致），因此「其余四语言零回归」是可逐题核验的硬事实。

> **BLEU backend 说明（主口径批次为 `builtin`）**：84.0% 批次运行时环境未安装 nltk，`meta.bleu_backend = builtin`（内置复刻实现），因此该批 `report.md` 顶部带一条 ⚠️ 警示。三条证据表明这不影响可比性：(1) 2026-11-11 批次做过 `nltk` / `builtin` **100 题逐位对拍**，逐题 `best_similarity` 无差异、`passed` 无翻转；(2) 本批与上一轮（`nltk`）的 **80 道非 cpp 题 `best_similarity` 取值完全相同**；(3) 本批两处翻转题的相似度都直接跳到 **1.0**（与 GT 完全一致的 BLEU-4 满分），与 BLEU 实现的版本级差异无关。装好 nltk 后可用同一批 `raw/` 执行 `score` 重判复核。

从修复前基线（74.0%）到主口径（84.0%），共 **10 道题 `fail → pass`、0 道回归**（逐题比对两批 `per_task.passed` 得出）：

| # | 题目 id | 仓库 | 归因 |
| --- | --- | --- | --- |
| 1 | `rust__rust-bakery__nom__n01` | rust-bakery/nom | rust `mod_item` 递归（+8.0pt 那一轮） |
| 2 | `rust__rust-bakery__nom__n03` | rust-bakery/nom | 同上 |
| 3 | `rust__rust-bakery__nom__n04` | rust-bakery/nom | 同上 |
| 4 | `rust__rust-bakery__nom__n06` | rust-bakery/nom | 同上 |
| 5 | `rust__rust-bakery__nom__n08` | rust-bakery/nom | 同上 |
| 6 | `rust__helix-editor__helix__n00` | helix-editor/helix | 同上 |
| 7 | `rust__helix-editor__helix__n06` | helix-editor/helix | 同上 |
| 8 | `rust__helix-editor__helix__n09` | helix-editor/helix | 同上 |
| 9 | `cpp__apache__logging-log4cxx__n00` | apache/logging-log4cxx | cpp 排序期包含降权（`best_similarity` 0.246 → **1.0**） |
| 10 | `cpp__skypjack__uvw__n03` | skypjack/uvw | cpp 排序期包含降权（`best_similarity` 0.217 → **1.0**） |

> 第 9、10 两题翻转前 `best_target` **与 needle 同名**（正确符号已被检索为 Top-1），只是返回的 `code_block` 是父容器（class/struct）的严格超集，BLEU 被稀释到 0.2 左右；降权容器后成员块顶到 rank1，相似度直接满分。

> **`.builtin.*` 后缀的含义**：它表示**用内置 BLEU 复刻后端**（`backend=builtin`，未安装 nltk 时的兜底实现）对同一批检索结果重新判分的副本，用于与主口径对拍。**2026-11-11 与 2026-11-12（首批）的主口径是 `nltk`**，即不带后缀的 `.md` / `.json`；**2026-11-12b 的主口径本身就是 `builtin`**（见上方警示），故不再另存 `.builtin.*` 副本。2026-11-11 批次做过 100 题逐位对拍：逐题 `best_similarity` 无差异、`passed` 无翻转，因此两套分数完全相同。
>
> **溯源**：三份 `.md` 均由 `repoqa_eval/report.py` 的 `render_markdown()` 渲染，未手工改数字。验证方式：用该渲染器渲染 `2026-11-12-report.json`，与仓库中已有的 `2026-11-12-report.md` **逐字节零差异**（3554 字节全等）；渲染 `2026-11-12b-report.json` 的输出与 `/tmp/repoqa-eval/report.md` **逐字节零差异**（md5 `083ee19e80de07531d876586283236e7`），说明三批报告章节结构、表头与措辞同源。

## 1. 关键结论

**总体 accuracy：84.0%（84/100 题通过，threshold = 0.8）**

| 语言 | 通过/总数 | accuracy |
| --- | --- | --- |
| python | 18/20 | 90.0% |
| cpp | 11/20 | 55.0% |
| java | 17/20 | 85.0% |
| typescript | 20/20 | 100.0% |
| rust | 18/20 | 90.0% |
| **all** | **84/100** | **84.0%** |

分 repo 明细见 `2026-11-12b-report.md` 第 3 节：`psf/black` 10/10、`xenova/transformers.js` 10/10、`expressjs/express` 10/10、`helix-editor/helix` 10/10（均 100.0%）；`square/retrofit` 9/10（90.0%）；`python-poetry/poetry` 8/10、`google/gson` 8/10、`rust-bakery/nom` 8/10（均 80.0%）；`skypjack/uvw` 6/10（60.0%）；`apache/logging-log4cxx` 5/10（50.0%）。逐项之和与上表一致（18+11+17+20+18 = 84）。

### 1.1 两轮改进的来源

| 轮次 | 改动 | 位置 | 效果 |
| --- | --- | --- | --- |
| 第一轮（74.0% → 82.0%） | `parse_block` 新增 `mod_item` 分支；`impl_item` 的 `full_range` 收窄到 `declaration_range` | `rust-core/src/codegraph/treesitter/parsers/rust.rs`（commit `9e74d94`） | rust 50.0% → 90.0%（+40.0pt），总体 **+8.0pt**，8 题翻转、0 回归 |
| 第二轮（82.0% → 84.0%） | **排序期**父子包含降权 `demote_enclosing_candidates`，语言白名单仅 `cpp` | `rust-core/src/services/hybrid_search.rs`（`ENCLOSING_DEMOTE_LANGUAGES = ["cpp"]`，commit `571fe6c`） | cpp 45.0% → 55.0%（+2 题），总体 **+2.0pt**，其余四语言零回归 |

第一轮的两处修复（详见 `../README.md` 第 5 节与 `hybrid_search.rs` 的注释）：

1. **`mod_item` 分支**：此前 `mod_item` 落到 `_ =>` 走 `parse_usages`，而 `parse_usages` 的兜底 `_ => {}` **不递归** → 整个 `#[cfg(test)] mod tests { ... }` 子树零产出。RepoQA 的 rust needle 大量落在 `mod tests` 内，这是 rust 召回的主因。
2. **`impl_item` 的 `full_range` 收窄**：使 impl 容器 chunk 不再覆盖其成员方法的源码区间。

第二轮的思路与第一轮不同——**不动切块（索引侧），只改排序**：不修改任何 chunk 的 `content`，仅在 hybrid 融合后的候选列表里识别「候选 i 的 `[line_start, line_end]` 严格包含候选 j」的父子关系，对被包含 ≥1 次者乘 `0.5`、被 ≥2 条包含再乘一次 `0.5`（最低 ×0.25），从而把「巨型容器块」从 rank1 挤下去、让成员块上位。门控只对 cpp 生效的原因见第 2 节实测表。

## 2. 已实测否决的方案（避免重走）

以下三次尝试均为**清索引、全量 100 题重跑**得到的结果，与当前主口径只差被测改动这一项。记录在此以便后来者不必重复踩坑。

| 方案 | 做法 | 实测 | 结论 |
| --- | --- | --- | --- |
| 切块层内容降级 | 父容器 chunk 的 `content` 从 `full_range` 改为 `declaration_range` 签名骨架（两份 chunker 同步） | **78.0%**（cpp 55%↑ 但 typescript 100%→70%、rust 90%→85%） | **否决**。降级后容器变成「超短文档 + 符号名完美命中」，BM25 长度归一化反而把它推到 rank1，正文只剩签名 → BLEU≈0 |
| 全语言排序期包含降权 | `demote_enclosing_candidates` 对所有语言生效 | **81.0%**（cpp 55%、java 90%，但 typescript 85%、rust 85%） | **否决**。typescript 的 needle 常是 class 本身、rust 的正确块会被 60 行窗口 span 误判，均造成回退 |
| BM25 存真实 `line_end` | tantivy schema 增 STORED `line_start` / `line_end` 并按边界回填 | **48.0%**（rust 全 `unknown_empty`、express 10 题全空） | **回退**。按 `line_end` 切片产生空正文，已 `git revert` |
| **排序期包含降权（仅 cpp）** | 同上但语言门控为 `cpp` | **84.0%（cpp 55%，其余零回归）** | **采用**（当前主口径） |

> 四次尝试的共同前提：**切块层（chunker）的任何"减内容"改动都会同时改变 BM25 文档长度与排序**，改善一个语言往往换来另一个语言回退；而排序期改动只影响名次、可逐语言门控，这正是「只对 cpp 生效」能拿到净收益 +2 题且零回归的原因。
>
> 注意 `78.0%` 与 `81.0%` 这两个中间结果的 cpp 都是 **55.0%**（与主口径相同），说明"把 cpp 从 45% 抬到 55%"有多种实现路径，但只有限定在 cpp 的排序期降权**不伤其他语言**。

## 3. 评测条件

> 下表为 **2026-11-12b 主口径**的评测条件。前两批口径相同，仅三处取值不同：被测二进制（上一轮 `38edff1bb1b1d1cd`、修复前 `5972636dc15431ee`）、BLEU backend（前两批为 `nltk`）、`run` 的 `--concurrency`（修复前基线为 `4`，其余为 `1`）。

| 项 | 取值 |
| --- | --- |
| 数据子集口径 | 5 语言 × 2 仓库 × 10 needle = **100 题**（`python/cpp/java/typescript/rust`，**不含 go**；详见 `../README.md` 第 5 节） |
| 数据集 | 官方 release `2024-06-23`，`size_bytes=71510991`，`sha256` 前缀 `bd3f7cab47283cde` |
| 判定阈值 | `threshold = 0.8`（BLEU-4 + Chen & Cherry `method4`，候选集为该 repo 的 10 条 needle） |
| 检索模式 | `hybrid: 100`（100.0% 真语义检索：LanceDB + BM25，结果含 `code_block`） |
| 空结果 | `name_fallback: 0`、`unknown_empty: 0`，即 **0 空结果、0 退化** |
| `search_limit` | 5（每题的 Top-5 都落盘，判分只取 Top-1） |
| tree-sitter | `false`（可选依赖，未安装；只影响从模型输出里取"第一个函数"这一步） |
| 被测二进制 | `rust-core/target/release/codeseek`，`sha256` 前缀 `b1532bb1b43e3d65`，`size_bytes=60600320`，mtime `2026-11-12T07:05:19.955112+00:00` |
| codeseek 版本 | `codeseek 0.1.31`（`codeseek --version`） |
| embedding 配置 | `embedding 已配置（model=Qwen/Qwen3-Embedding-4B，api_token 长度 51）` |
| BLEU backend | `builtin`（内置复刻实现，运行时未安装 nltk）；等价性论证见上方「BLEU backend 说明」 |
| 运行规模 | `generated_at=2026-11-12T07:18:32+00:00`，`elapsed_s=198.2`，`concurrency=1`，`init_failures=[]` |

## 4. 三条关键洞察

### 4.1 通过组的函数边界与 needle 定义同源

当前 84 道通过题中，**83 题**的 `best_similarity` 恰为 `1.0`（BLEU-4 满分，等价于返回的 `code_block` 与 ground truth 完全一致，而非"相似"）。三批一致的唯一非满分通过题是 `python__python-poetry__poetry__n01`（`best_similarity=0.946`）：其 `code_block` 比 GT 多出 `@classmethod` 装饰器一行，仍以 BLEU ≥ 0.8 判过。

> 含义：codeseek 的索引侧切块边界与 RepoQA 的 needle 切片口径高度一致，通过不是靠 BLEU 容错"擦边"进来的。

### 4.2 cpp 45.0% → 55.0% 已由排序期包含降权拿下；瓶颈随之从"块太大"转为"召回/排序"

两处翻转题（`cpp__apache__logging-log4cxx__n00` needle `ConfiguratorWatchdog`、`cpp__skypjack__uvw__n03` needle `write_req`）在上一轮基线里 `best_target` **与 needle 同名**，但 `best_similarity` 只有 **0.246 / 0.217**——即"符号已召回、块是父容器"。降权把父容器压到 rank2 之后，成员块顶到 rank1，相似度直接变成 **1.0**。这与上一轮的判断（"抢 rank1 的是容器 chunk，不是成员 chunk"）完全吻合。

但**剩余 9 道 cpp 失败题的性质已经变了**：

| 性质 | 题数 | 题号 | 说明 |
| --- | --- | --- | --- |
| `below_threshold`（同名但块不符） | **2** | `logging-log4cxx__n04`（`catch`，0.024）、`uvw__n04`（`recv`，0.035） | 仍是"块边界"问题，但相似度极低（0.02~0.04），说明返回的块与 needle 几乎完全不同，不是"稍微大一点" |
| `wrong_target`（返回了别的符号） | **7** | `logging-log4cxx__n05` `n06` `n07` `n08`、`uvw__n02` `n06` `n07` | 返回的 `best_target` 与 needle 不同名（如 `PropertiesPatternConverterPrivate` → `ConfiguratorWatchdog`），属**未召回** |

> 结论：cpp 的"切块粒度 = P0"问题**已通过排序期降权收口两题**；剩下的 9 题里 7 题是**召回失败**（needle 根本没进 Top-5 的可用块），已不属于切块粒度问题。这也是为什么继续在切块层做"内容降级"会把 typescript/rust 拖下水却拿不到额外收益（见第 2 节）。

### 4.3 rust 保持 90.0%，剩余 2 题是边界与同名排序

| 题 | needle | best_target | sim | 原因 | 性质 |
| --- | --- | --- | --- | --- | --- |
| `rust__rust-bakery__nom__n07` | `be_i8_tests` | `be_i8_tests` | 0.721 | `below_threshold` | **切块边界差异**（符号已召回，块比 needle 长/短） |
| `rust__rust-bakery__nom__n09` | `length_count` | `parse` | 0.017 | `wrong_target` | 仍为召回/同名干扰 |

`nom__n07` 是全部 16 道失败题中"最接近通过"的一题（0.721 vs 阈值 0.8），且该题在修复前基线里是 `wrong_target`（返回 `many1_count`），本轮已从"未召回"改善为"边界不符"。

## 5. 已知限制与后续

- **cpp 的下一步是召回而非切块**：剩余 9 道 cpp 失败中 7 道 `wrong_target`（第 4.2 节）。继续改 chunker 的收益已被第 2 节的实测否决（78.0% / 81.0%）；可探索的方向是 cpp 符号覆盖（宏展开、模板特化、匿名 namespace 内的静态函数）与同名符号排序。
- **排序期降权依赖 `line_start` / `line_end` 的准确性**：dense-only 路径的 span 来自索引内的真实行号（可信）；而经过 `hydrate_code_blocks` 回填的路径在 `line_end` 缺失时会退化为「`line_start` + 59」的固定窗口推算值，因此**超过 60 行的容器可能识别不完整**。该问题是否影响剩余 cpp 失败题**未经验证**，仅作为已知精度损失记录在此。
- **降权方向是"压容器"而非"抬容器"**：本方案假定"巨型容器几乎不可能是 needle"。这在 cpp 成立（+2 题），但在 typescript（needle 常是 class 本身）与 rust（`impl`/`mod` 常是答案）会反向受损（第 2 节 81.0% 那行），故门控不可放宽。
- **`code_block` 粒度直接影响 BLEU**：hybrid 结果返回的代码块由索引侧切分，可能包含装饰器/注释或截断；`threshold=0.8` 对粒度差异相当敏感（`be_i8_tests` 的 BLEU 与阈值相差不到 0.1）。
- **非 cpp 分项已进入平台期**：python 90.0%、java 85.0%、typescript 100.0%、rust 90.0%，三批完全一致；python/java 的 5 道失败以 `wrong_target` 为主（同名符号排序），不属于切块问题。
- **未做粒度容忍口径度量**：本归档只报告官方口径（严格的 `best_target == GT and BLEU ≥ 0.8`）。"GT 落在返回块内即算通过"的宽松口径会显著抬高 cpp 分数，但该值与官方榜不可比，故不写入结论。
- 其他口径限制（候选集仅 10 条 needle、`git init` 快照而非真实历史、无 token 时退化为名字匹配等）见上一级 [`../README.md`](../README.md) 第 5 节。

## 6. 复现

```bash
cd evals/repoqa
pip install -r requirements.txt                                    # 装 nltk 以使用官方同款 BLEU
python -m repoqa_eval prepare --workdir /tmp/repoqa-eval           # 下载数据 + 抽 100 题子集 + git init
python -m repoqa_eval run --workdir /tmp/repoqa-eval --limit 5 --threshold 0.8 --concurrency 1
python -m repoqa_eval score --workdir /tmp/repoqa-eval             # 换阈值重判，不重跑检索
```

> 三批之间唯一的运行参数差异是 `--concurrency`（修复前基线用 `4`，后两批用 `1`）；并发修复（`c56a41f`）后两者结果等价。主口径批次运行时未安装 nltk，故 `meta.bleu_backend=builtin`；装上 nltk 后执行 `score` 即可用同一批 `raw/` 复核（不会重跑检索）。
>
> 主口径批次的被测二进制为 `rust-core/target/release/codeseek`（`sha256` 前缀 `b1532bb1b43e3d65`），对应 commit `571fe6c`（`fix(hybrid-search): revert line_end schema, gate demotion to cpp`）。

评测口径、判分实现细节与已知限制见上一级 [`../README.md`](../README.md)。
