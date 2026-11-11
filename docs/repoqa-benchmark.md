# RepoQA 评测基准调研

> 一句话简介：RepoQA 是首个面向长上下文代码理解（long-context code understanding）的评测基准，首个任务 SNF（Searching Needle Function）要求模型在 16K token 的仓库代码上下文中，根据一段不含函数名的自然语言描述检索出目标"针函数"。

## 目录

- [1. 基本信息](#1-基本信息)
- [2. 任务形式（SNF）](#2-任务形式snf)
- [3. 数据构成](#3-数据构成)
- [4. 评测指标与判定规则](#4-评测指标与判定规则)
- [5. 使用方式（CLI）](#5-使用方式cli)
- [6. 官方榜单（16K 上下文，thresh = 0.8）](#6-官方榜单16k-上下文thresh--08)
- [7. 论文主要结论](#7-论文主要结论)
- [8. 常见误传与澄清](#8-常见误传与澄清)
- [9. 维护状态](#9-维护状态)
- [10. 参考链接](#10-参考链接)
- [11. 信息核实说明（含未获证实的常见误传）](#11-信息核实说明含未获证实的常见误传)

## 1. 基本信息

| 项 | 内容 |
|----|------|
| 全称 / 简称 | RepoQA（"Repository Question Answering" 只是社区按字面展开的说法，论文与仓库均未给出正式全称，**未核实**） / RepoQA |
| 提出机构 | UIUC（伊利诺伊大学厄巴纳-香槟分校），Jiawei Liu 等 8 人，Lingming Zhang 为末位作者 |
| 论文 | arXiv:2406.06025，v1 提交于 2024-06-10，学科分类 cs.SE |
| 发表 | ICML 2024 LCFM Workshop（Long-Context Foundation Models Workshop） |
| 代码仓库 | <https://github.com/evalplus/repoqa> ，许可证 Apache-2.0，属 EvalPlus 系列项目 |
| 首个任务 | SNF（Searching Needle Function） |

RepoQA 的定位是把"长上下文能力"从自然语言长文阅读迁移到代码检索场景：模型不再被要求写代码，而是被要求在一个真实仓库的完整上下文中**定位**一段功能已知、但名字未知的代码。

## 2. 任务形式（SNF）

SNF（Searching Needle Function）的输入是一个三元组：

1. **仓库代码上下文**：按文件依赖排序拼接而成的大段仓库代码，长度被固定到 16K token；
2. **目标函数的自然语言描述**：由人工撰写的功能说明，**故意不含函数名等关键词**，避免模型靠字符串匹配"作弊"；
3. **检索指令**：要求模型输出符合该描述的函数实现。

输出即目标函数的代码本身（而不是函数名或文件路径）。

```
[输入 ②] 目标函数的自然语言描述（不含函数名）
"该函数读取项目配置文件，合并默认值后返回最终配置……"

[输入 ①] 仓库代码上下文（按文件依赖排序，固定约 16K tokens）
file_01.ext … file_10.ext（同一仓库的多个源文件顺序拼接）

[输入 ③] 检索指令
"请在上面的仓库代码中找到符合该描述的函数，并输出该函数的完整实现。"

[输出] 目标函数代码（"针函数"，needle function）
```

> 上文代码块是**结构示意**，用于说明三元组的组合方式，并非官方 prompt 的逐字原文。

这正是代码领域的 needle-in-a-haystack：整段仓库上下文是"草堆"，需要被描述定位的那个函数是"针"，而描述刻意避开了函数名这类可直接字符串匹配的锚点。与大海捞针式的检索任务相比，SNF 的难点在于**语义匹配必须精确到函数粒度**——同一仓库里往往存在若干功能相近的函数。

## 3. 数据构成

| 维度 | 内容 |
|------|------|
| 测试数量 | **500** = 5 语言 × 10 仓库 × 10 针函数 |
| 语言 | Python / C++ / Java / TypeScript / Rust |
| 仓库 | 每语言 **10** 个 ≥100 stars 的热门仓库，共 **50** 个 |
| 上下文长度 | 固定 **16K tokens**。注意三处口径不一致（原文如此）：论文用 CodeLlama tokenizer；README 的 CLI 默认 `--code-context-size 16384` 但写的是 DeepSeekCoder tokenizer；官方 homepage 写的是 CodeLlama |
| 针函数位置 | 每个仓库内按 **10%、20% … 100%** 的深度均匀植入 |

"深度"指针函数在上下文中的相对位置：10% 表示针函数出现在靠近上下文开头的位置，100% 表示出现在末尾。每个仓库植入 10 个针函数、覆盖 10 个深度档位，因此同一个仓库会被测量 10 次，用于观察"位置越靠后是否越难"。

需要留意的是，16K 是**固定窗口**而非可变区间；数据集并不覆盖 2K 或 100K 级别的长度（见 [第 8 节](#8-常见误传与澄清)）。

## 4. 评测指标与判定规则

只有单一指标：**Retrieval Accuracy（0–100%）**。对每个测试而言，判定必须**同时**满足以下条件：

1. **后处理**：用 tree-sitter 从模型输出中提取第一个语法正确的代码块；
2. **(i) 相对排序第一**：该函数与针函数的相似度，在全部候选函数中最相似（候选函数同样经 tree-sitter 解析）；
3. **(ii) 绝对阈值**：相似度 ≥ 默认阈值 **0.8**；相似度采用带平滑的 BLEU（Chen & Cherry, 2014）。

**不使用 LLM-as-judge**：判分完全由语法解析 + BLEU 相似度决定，不存在"让另一个大模型打分"的环节，因此分数是可复现的、确定性的。

README 对判定规则的英文原文：

> "The evaluator passes a test if the searched function is syntactically closest to the ground-truth compared against other functions (systematically parsed by treesitter) and the similarity is greater than a user defined threshold (by default 0.8)."

论文 §3.2 对总分的定义（score computation）：

> "By aggregating the success rate of retrieving needle functions across all tests, we obtain a final accuracy score from 0 to 100 (%) to represent the long-context code retrieval ability of the model."

由于判定要求"相似度最高"与"相似度 ≥ 0.8"同时成立，模型即使输出了一个高度相似的函数，只要仓库中存在另一个更相似的函数，该测试仍记为失败——这使指标比单纯的 BLEU 阈值更严格。

## 5. 使用方式（CLI）

安装后通过两个入口使用：`search_needle_function` 负责跑模型并落盘结果，`compute_score` 负责重算分数。

```bash
# 1) 用指定模型/后端执行 SNF 检索，结果写入 results/
repoqa.search_needle_function --model "gpt4-turbo" --backend openai   # 亦支持 anthropic / vllm / hf / google

# 2) 仅重算分数（不重新调用模型），用于调整阈值或复现榜单
repoqa.compute_score --model-output-path={model-output}.jsonl
```

输出路径约定：

| 产物 | 路径 |
|------|------|
| 原始输出 | `results/ntoken_{code-context-size}/{model}.jsonl` |
| 打分结果 | `results/ntoken_{code-context-size}/{model}-SCORE.json` |

其中 `{code-context-size}` 默认为 `16384`，因此默认目录是 `results/ntoken_16384/`。这种"原始输出 + 独立打分"的拆分意味着：修改阈值、重新实现相似度计算，都可以在不重复调用（付费）模型的前提下完成。

## 6. 官方榜单（16K 上下文，thresh = 0.8）

**榜单不在 GitHub README 里**，而托管在项目 homepage：<https://evalplus.github.io/repoqa.html> 。页面共 **33 行**（第 22 位并列两名），列序为 `.py / .cpp / .rs / .java / .ts / Avg`，相似度阈值 `thresh = 0.8`。

| # | Model | 宣称上下文 | Py | C++ | Rust | Java | TS | Avg |
|---|-------|-----------|----|-----|------|------|----|-----|
| 🥇 | gpt-4o-2024-05-13 | 128k | 95 | 80 | 85 | 96 | 97 | 90.6 |
| 🥇 | gemini-1.5-pro-latest | 1000k | 91 | 81 | 91 | 94 | 96 | 90.6 |
| 🥇 | claude-3-opus-20240229 | 200k | 93 | 83 | 88 | 95 | 94 | 90.6 |
| 4 | gemini-1.5-flash-latest | 1000k | 93 | 79 | 87 | 94 | 97 | 90.0 |
| 5 | claude-3-sonnet-20240229 | 200k | 88 | 81 | 85 | 92 | 91 | 87.4 |
| 6 | DeepSeek-V2-Chat | 128k | 90 | 76 | 77 | 91 | 83 | 83.4 |
| 7 | Meta-Llama-3-70B-Instruct | 8k | 83 | 70 | 81 | 86 | 91 | 82.2 |
| 8 | claude-3-haiku-20240307 | 200k | 80 | 75 | 74 | 90 | 90 | 81.8 |
| 9 | c4ai-command-r-plus | 128k | 81 | 74 | 76 | 84 | 77 | 78.4 |
| 10 | gpt-4-turbo-2024-04-09 | 128k | 84 | 79 | 75 | 89 | 55 | 76.4 |
| 11 | Mixtral-8x7B-Instruct-v0.1 | 32k | 66 | 65 | 64 | 71 | 74 | 68.0 |
| 12 | Mixtral-8x22B-Instruct-v0.1 | 64k | 60 | 67 | 74 | 83 | 55 | 67.8 |
| 13 | Qwen1.5-72B-Chat | 32k | 62 | 60 | 68 | 75 | 70 | 67.0 |
| 14 | Phi-3-medium-128k-instruct | 128k | 57 | 54 | 62 | 69 | 74 | 63.2 |
| 15 | CodeQwen1.5-7B-Chat | 64k | 69 | 47 | 57 | 74 | 67 | 62.8 |
| 16 | Mistral-7B-Instruct-v0.3 | 32k | 61 | 57 | 51 | 61 | 80 | 62.0 |
| 17 | gpt-3.5-turbo-0125 | 16k | 43 | 65 | 60 | 76 | 58 | 60.4 |
| 18 | Meta-Llama-3-8B-Instruct | 8k | 54 | 48 | 51 | 53 | 62 | 53.6 |
| 19 | deepseek-coder-33b-instruct | 16k | 59 | 44 | 23 | 53 | 63 | 48.4 |
| 20 | Mistral-7B-Instruct-v0.2 | 32k | 38 | 50 | 44 | 45 | 60 | 47.4 |
| 21 | CodeLlama-13b-Instruct-hf | 16k | 45 | 30 | 31 | 50 | 57 | 42.6 |
| 22 | DeepSeek-V2-Lite-Chat | 32k | 39 | 37 | 45 | 41 | 46 | 41.6 |
| 22 | CodeLlama-34b-Instruct-hf | 16k | 41 | 31 | 53 | 40 | 43 | 41.6 |
| 24 | Phi-3-small-128k-instruct | 128k | 25 | 48 | 30 | 46 | 49 | 39.6 |
| 25 | Qwen1.5-32B-Chat | 32k | 36 | 28 | 25 | 32 | 48 | 33.8 |
| 26 | CodeLlama-7b-Instruct-hf | 16k | 20 | 41 | 22 | 25 | 33 | 28.2 |
| 27 | Qwen1.5-14B-Chat | 32k | 4 | 30 | 26 | 36 | 34 | 26.0 |
| 28 | Magicoder-S-DS-6.7B | 16k | 27 | 21 | 7 | 25 | 36 | 23.2 |
| 29 | Phi-3-mini-128k-instruct | 128k | 19 | 25 | 25 | 21 | 22 | 22.4 |
| 30 | Mistral-7B-Instruct-v0.1 | 32k | 10 | 9 | 10 | 11 | 15 | 11.0 |
| 31 | deepseek-coder-6.7b-instruct | 16k | 11 | 21 | 2 | 3 | 16 | 10.6 |
| 32 | Qwen1.5-7B-Chat | 32k | 1 | 6 | 2 | 2 | 3 | 2.8 |
| 33 | codegemma-7b-it | 8k | 3 | 2 | 1 | 1 | 4 | 2.2 |

**榜单观察**

- **前三名同为 90.6，但排序不同**：官方首页把 `gpt-4o-2024-05-13` 列第一，论文则把 `claude-3-opus-20240229` 列第一。三者的 Avg 完全一致，差异只出现在子项（例如 TS 列 gpt-4o 为 97、claude-3-opus 为 94），因此"谁是第一"取决于排序口径，而非分数差异。
- **榜单与论文 Table 2 有 4 处 1 分之差**，均为榜单比论文高 1 分：
  - `Phi-3-medium-128k-instruct` 的 `.py`：榜单 57 / 论文 56
  - `CodeQwen1.5-7B-Chat` 的 `.rs`：榜单 57 / 论文 56
  - `Mistral-7B-Instruct-v0.3` 的 `.cpp`：榜单 57 / 论文 56
  - `CodeLlama-13b-Instruct-hf` 的 `.ts`：榜单 57 / 论文 56
  引用分数时应注明来源（homepage 或论文），不要把两个版本混用。
- **第 22 位为并列**：`DeepSeek-V2-Lite-Chat` 与 `CodeLlama-34b-Instruct-hf` 的 Avg 均为 41.6，因此 33 行数据只对应 32 个名次段。
- **榜单自 2024 年下半年起未更新**，其中没有 2025 年发布的新模型；需要新模型的成绩只能自行运行评测。

## 7. 论文主要结论

论文给出三条 takeaway（原文）：

> "(i) there is still a small gap between the best open and proprietary models; (ii) different models are good at different languages; and (iii) models may understand code better without comments."

围绕这三条，可以归纳出以下结论：

- **第一梯队几乎全是闭源模型**：Avg > 90% 的四款模型（`gpt-4o-2024-05-13` 90.6、`gemini-1.5-pro-latest` 90.6、`claude-3-opus-20240229` 90.6、`gemini-1.5-flash-latest` 90.0）均为闭源 API 模型。
- **开源模型已逼近但仍有差距**：最强开源模型 `DeepSeek-V2-Chat` 得 83.4、`Meta-Llama-3-70B-Instruct` 得 82.2，二者已略超 `claude-3-haiku-20240307`（81.8）与 `gpt-4-turbo-2024-04-09`（76.4），但仍未进入 90% 梯队。
- **语言难度梯度**：Java、TypeScript 最易 → Python → C++ → Rust 最难。作者推测这与训练语料中各语言代码量的多少相关。以三款模型的分项为例（顺序为 Py / C++ / Rust / Java / TS）：`gpt-4o-2024-05-13` 为 95 / 80 / 85 / 96 / 97，`gpt-4-turbo-2024-04-09` 为 84 / 79 / 75 / 89 / 55，`codegemma-7b-it` 为 3 / 2 / 1 / 1 / 4。即便是同一个模型，C++/Rust 也普遍低于 Java/TS；而弱模型在 Rust 上的下滑往往最剧烈（如 `deepseek-coder-33b-instruct` 的 Rust 只有 23，而 Py 有 59）。
- **注释消融**：除 Gemini 系以外，多数模型在"无注释"模式下反而提升，其中 `gpt-4o` 从 90.6 提升到 **93.2**，幅度最明显。这说明注释既可能是线索也可能是干扰。
- **典型失败模式**：`Gemini-1.5-Flash` 的典型失败是"忘掉任务、转而去数行号"——模型没有输出函数，而是开始列举行号，属于指令跟随失效而非检索能力不足。
- **短上下文模型的免训练解锁**：对上下文窗口只有 8k / 16k 的模型，用 dynamic RoPE scaling 或直接改写最大长度并取最优配置，即可在不做任何训练的情况下跑 16K 设置；作者据此认为"Llama-3-Instruct 家族其实是被隐藏的长上下文模型"。
- **作者自陈的局限**（来自官方 FAQ，论文本身没有 Limitations 节）：函数描述被刻意写得啰嗦，以避免一段描述同时匹配多个函数；这与真实场景中开发者常用短查询的习惯存在分布差异，因此 SNF 分数不能直接等同于真实代码检索体验。
- **Future work**：扩展 SNF（更多任务形态）、构造更复杂的任务、覆盖更多语言与模型。

## 8. 常见误传与澄清

| 传闻 | 核实结果 |
|------|----------|
| arXiv 编号是 2406.06036 | ❌ 2406.06036 是一篇无关的数学论文；正确编号为 **2406.06025** |
| 评测覆盖 2K–2M tokens 的变长上下文 | ❌ 论文与官网只有 **16K 固定窗口**，没有变长评测 |
| 存在 Go 语言子集，以及 `repoqa-2024-06-23` / `repoqa-2024-06-24` 等数据集版本 | ❌ README、arXiv 全文、HuggingFace 上的 evalplus 组织页均无任何依据；HF 上只有第三方（如 `Nutanix/RepoQA-neo4j` 等）镜像，非官方发布 |
| README 里就有 leaderboard 表格 | ❌ README 只有安装、用法与引用信息；榜单在 homepage（`evalplus.github.io/repoqa.html`） |
| 判分使用 LLM-as-judge | ❌ 判定为 tree-sitter 解析 + 平滑 BLEU ≥ 0.8 且排序第一，全程没有 LLM 评审 |

## 9. 维护状态

| 指标 | 数值 |
|------|------|
| Stars | 137 |
| Forks | 10 |
| Watchers | 5 |
| Releases | 7 个，最新为 v0.1.2 |
| 最新 commit | `ae876de`，时间 2024-10-29，信息 `refactor(hf): use stop_strings over external lib` |
| 许可证 | Apache-2.0 |
| 数据集获取方式 | 不在 HuggingFace 上托管，通过仓库内的 `repoqa_release` 加载 |

仓库的最新 commit 停留在 2024-10-29，榜单也未增补 2025 年的模型，可视为进入低频维护状态。若要用于新模型的横向对比，需要按 [第 5 节](#5-使用方式cli) 的 CLI 自行运行评测，并注意保持 `--code-context-size` 与 tokenizer 设置与官方一致，否则分数不可比。

## 10. 参考链接

| 资源 | 链接 |
|------|------|
| 论文摘要页（arXiv abs） | <https://arxiv.org/abs/2406.06025> |
| 论文全文（arXiv HTML，v1） | <https://arxiv.org/html/2406.06025v1> |
| GitHub 仓库 | <https://github.com/evalplus/repoqa> |
| 官方榜单页 | <https://evalplus.github.io/repoqa.html> |
| EvalPlus 组织页 | <https://github.com/evalplus> |
| HF evalplus 组织页 | <https://huggingface.co/evalplus>（**无** RepoQA 数据集） |

表中 arXiv HTML 链接指向 v1 的 HTML 版本；该 HTML 页面的可访问性本次**未逐次核实**，若链接失效，请以摘要页提供的 PDF 为准。

## 11. 信息核实说明（含未获证实的常见误传）

**采集时间与方式**

- 采集时间：2026-11-11。
- 采集方式：通过真实 Chrome 直连打开官方一手页面取证，共两轮；榜单、论文摘要、README 均以渲染后的页面文本为准，未依赖二手博客或搜索摘要。
- 证据截图：
  - 榜单：`/Users/ydl/.yagent/data/browser/5315c972-6383-407d-a5d6-55f0b07c/repoqa-leaderboard-16k.png`
  - 论文摘要：`/Users/ydl/.yagent/data/browser/5315c972-6383-407d-a5d6-55f0b07c/repoqa-arxiv-abstract.png`
  - 说明：原始记录里该目录名被写作 `...55f0b07c7de`，磁盘上的真实目录名为 `...55f0b07c`（无 `7de` 后缀），此处按实际存在的路径给出。

**网络说明**

`github.com`、`arxiv.org`、`evalplus.github.io` 均可直连成功，未使用代理；仅搜索引擎路径不可用（Bing 国内版结果被污染、DuckDuckGo 触发 CAPTCHA），因此采用"已知权威 URL 直接打开"的取证策略，而不是先搜索再点击。

**未核实项**

- **2K–2M 变长上下文数据**：论文与官网均只有 16K 固定窗口，未发现任何变长评测的证据。
- **needle 深度分层性能曲线**：数据集按 10%–100% 深度植入针函数，但论文与官网都没有给出"分数随深度变化"的曲线；本仓库中也未找到该数据的公开出处。
- **Go 语言成绩**：RepoQA 的 5 种语言中不含 Go，任何 Go 子集成绩均无一手依据。
- **2025 年后的新模型成绩**：榜单未更新，官方未提供相应数据，需自行运行评测。
- **"Repository Question Answering" 作为正式全称**：论文标题与摘要只写 RepoQA，未做全称展开；该说法属社区按字面理解，非官方定义。

**关于"常见误传"的进一步说明**

[第 8 节](#8-常见误传与澄清)列出的五条传闻，均是在检索过程中反复出现、但无法在一手页面中找到依据的说法。其中最容易被误引的是 arXiv 编号（`2406.06036` 与 `2406.06025` 仅差一位）与"变长上下文"——后者很可能是把其他长上下文基准的设定张冠李戴到了 RepoQA 上。引用本文档中的任何分数时，请同时注明来源是 homepage 榜单还是论文 Table 2，因为两者存在 4 处 1 分之差。
