# CodeSeek

**面向 Claude Code 的代码智能 CLI 工具。** 基于 AST 的调用图分析 + 语义搜索 — 从终端直接使用。

## 快速开始

```bash
# npm 安装（自动处理配置向导 + 二进制下载）
npm install -g codeseek

# 首次运行 — 交互式配置向导，填写嵌入模型信息
codeseek

# 构建索引
codeseek init

# 按符号名称搜索
codeseek search main --limit 10

# 查询调用关系
codeseek callers main
codeseek callees process_data

# 注册为 Claude Code / Codex MCP 工具
codeseek install

# 查看状态
codeseek status

# 安装 git hooks（提交时自动增量索引）
codeseek install-hooks
```

## 安装

### npm

```bash
npm install -g codeseek
```

npm 包包含一个轻量的 JS wrapper，负责：

| 步骤 | 说明 |
|------|------|
| **首次配置向导** | 交互式 CLI 引导填写 API token、模型名称、API 地址 |
| **二进制下载** | 自动从 GitHub Releases 拉取对应平台的 Rust 二进制 |
| **命令透传** | 所有命令（`init`、`search`、`callers` 等）转发给原生二进制 |

支持的平台：

| 平台 | 架构 |
|------|------|
| macOS | arm64（Apple Silicon）、x64（Intel） |
| Linux | x64 |

### Homebrew

```bash
brew tap CodeBendKit/codeseek git@github.com:CodeBendKit/codeseek.git
brew install codeseek
```

### 从源码编译

```bash
git clone https://github.com/CodeBendKit/codeseek.git
cd codeseek
./build.sh --release
```

`build.sh` 会同时编译 TypeScript wrapper（`dist/`）和 Rust 二进制，然后安装到 `~/.codeseek/bin/`。

## 命令列表

| 命令 | 说明 |
|------|------|
| `codeseek` | 首次配置向导（交互式配置嵌入模型） |
| `codeseek init` | 构建/更新代码索引（首次全量，后续 MD5 增量） |
| `codeseek status` | 索引统计：函数数、文件数、最后更新时间 |
| `codeseek search <关键词>` | 符号名称搜索（向量 → 图谱名称匹配自动回退） |
| `codeseek callers <符号>` | 查询调用该符号的函数 |
| `codeseek callees <符号>` | 查询该符号调用的函数 |
| `codeseek list` | 列出所有已索引的项目及路径 |
| `codeseek install` | 注册 codeseek 为 Claude Code / Codex 的 MCP 工具 |
| `codeseek uninstall` | 移除 MCP 集成 |
| `codeseek uninit` | 删除当前项目的索引数据 |
| `codeseek install-hooks` | 安装 git hooks（提交/合并时自动 `codeseek init`） |
| `codeseek serve --mcp` | 启动 MCP 服务器（stdio JSON-RPC，由 Claude Code 内部调用） |

所有查询命令支持 `--json` 输出机器可读格式。

## Claude Code / Codex 集成

```bash
codeseek install
```

自动写入 MCP 配置到：

| Agent | 配置文件 |
|-------|---------|
| **Claude Code** | `~/.claude.json`（全局，所有项目）或 `./.mcp.json`（项目局部） |
| **Codex CLI** | `~/.codex/config.toml` |

重启后 Claude Code 自动识别以下工具：

| 工具名 | 功能 |
|--------|------|
| `codeseek_search` | 按名称查找符号 |
| `codeseek_callers` | 追踪上游调用者 |
| `codeseek_callees` | 追踪下游被调用者 |
| `codeseek_status` | 检查索引健康状态 |

移除集成：

```bash
codeseek uninstall
```

## 工作原理

### 构建索引 (`codeseek init`)

```
源码文件
  → Tree-sitter AST 解析（7 种语言）
  → 提取函数 / 类 / 方法
  → 批量嵌入 API 调用（每次 20 个文本，SQLite 缓存）
  → LanceDB 向量存储
  → Tantivy BM25 索引
  → 序列化调用图 (PetCodeGraph)
  → 保存到 ~/.codeseek/<project_hash>/
```

**幂等**：首次全量构建，后续 MD5 对比仅处理变更文件。用 `codeseek install-hooks` 可在 commit/merge 时自动增量索引。

### 混合搜索管线 (`codeseek search`)

```
                        ┌─────────────────────┐
用户查询 ────────────────→│  嵌入模型 (Embedding)  │──→ 查询向量
                        └─────────────────────┘
                                  │
          ┌───────────────────────┼───────────────────────┐
          ▼                       ▼                       ▼
   ┌─────────────┐       ┌─────────────┐       ┌─────────────┐
   │  密集搜索    │       │  稀疏搜索    │       │  图谱搜索    │
   │ (LanceDB)   │       │ (Tantivy)   │       │(PetCodeGraph)│
   └──────┬───────┘       └──────┬──────┘       └──────┬──────┘
          │                      │                      │
          └──────────────────────┼──────────────────────┘
                                 ▼
                        ┌─────────────────┐
                        │   RRF 融合排序   │  ← Reciprocal Rank Fusion
                        │   (Top-20 候选)  │
                        └────────┬────────┘
                                 │
                                 ▼
                        ┌─────────────────┐
                        │   重排序 (Reranker)│  ← Cross-Encoder 精细打分
                        │ (Qwen3-Reranker) │    逐对 (查询, 代码) 评分
                        └────────┬────────┘
                                 │
                                 ▼
                        ┌─────────────────┐
                        │    最终结果      │  ← Top-5（或 Top-N）
                        └─────────────────┘
```

| 阶段 | 技术 | 作用 | 速度 |
|-------|------|------|:---:|
| **密集搜索** | LanceDB + 嵌入模型 | 语义向量相似度 | 快 |
| **稀疏搜索** | Tantivy BM25 | 关键词 & Token 匹配 | 快 |
| **RRF 融合** | Reciprocal Rank Fusion | 公平融合异构分数 | 即时 |
| **重排序** | Cross-Encoder (Qwen3-Reranker-4B) | 全交互精准评分 | ~1-2s |
| **兜底** | PetCodeGraph | 基于图谱的名称搜索 | 即时 |

嵌入 / Reranker 不可用时自动回退到图谱名称搜索。

### 存储

- **配置**：`~/.codeseek/config.json`（全局，所有项目共享）
- **索引**：`~/.codeseek/<md5(project_root)>/`
  - `project.json` — 项目元数据
  - `graph.bin` — 序列化调用图
  - `embeddings.lance/` — LanceDB 向量数据
  - `tantivy_bm25/` — BM25 全文索引
  - `file_hashes.json` — MD5 增量追踪

无守护进程、无 HTTP 服务器。每个命令都是独立的单次进程。

## 支持的语言

| 语言 | 函数 | 结构体/类 | 调用图 |
|------|:---:|:---:|:---:|
| Rust | ✅ | ✅ | ✅ |
| Python | ✅ | ✅ | ✅ |
| JavaScript | ✅ | ✅ | ✅ |
| TypeScript | ✅ | ✅ | ✅ |
| Go | ✅ | ✅ | ✅ |
| C/C++ | ✅ | ✅ | ✅ |
| Java | ✅ | ✅ | ✅ |

## 配置

`~/.codeseek/config.json`：

```json
{
  "embedding": {
    "provider": "openai-compatible",
    "model": "Qwen/Qwen3-Embedding-4B",
    "api_token": "sk-...",
    "api_base_url": "https://api.siliconflow.cn/v1",
    "dimensions": 2560,
    "instruct": "Given a code search query, retrieve relevant functions or code snippets that answer the query."
  },
  "index": {
    "min_code_block_length": 16,
    "enable_reranker": true,
    "hybrid": {
      "enable_bm25": true,
      "bm25_top_k": 20,
      "vector_top_k": 20,
      "rrf_k": 60,
      "rrf_top_k": 20
    },
    "reranker": {
      "enabled": true,
      "model": "Qwen/Qwen3-Reranker-4B",
      "api_token": "sk-...",
      "api_base_url": "https://api.siliconflow.cn/v1/rerank",
      "top_n": 5,
      "candidate_multiplier": 5,
      "timeout_secs": 60
    }
  },
  "installed_hooks": {}
}
```

> `embedding.instruct` —— 查询侧指令（非对称检索）。只有**查询**会被加上官方
> 前缀（`Instruct:` 后有一个空格）；入库的代码保持裸文本，已有向量索引不受影响。
> 省略该字段即使用上面这条默认指令；配置为 `""` 则完全关闭前缀注入。
> 模板字面量由后端固定：`openai-compatible` 用 `Instruct: {指令}\nQuery:{查询}`
> （`Query:` 后**无**空格），`local-onnx` 用 `Instruct: {指令}\nQuery: {查询}`
> （`Query:` 后**有**一个空格），各自对齐所属模型族的官方写法。

### 本地（离线）向量化

`provider` 决定嵌入后端，两者在结构上互斥 —— `local-onnx` 的配置块里不可能带着一个
会悄悄生效的 `api_token`。

```json
{
  "embedding": {
    "provider": "local-onnx",
    "model_dir": "/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX",
    "onnx_rel_path": "onnx/onnx/model_fp16.onnx",
    "max_length": 2048,
    "intra_threads": 0,
    "instruct": "Given a code search query, retrieve relevant functions or code snippets that answer the query."
  }
}
```

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `model_dir` | `/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX` | 存放 `tokenizer.json` 与 ONNX 图的目录 |
| `onnx_rel_path` | `onnx/onnx/model_fp16.onnx` | 双层 `onnx/onnx/` 是导出工具的目录结构，不是笔误 |
| `max_length` | `2048` | tokenizer 截断长度；模型本身支持到 32768 |
| `intra_threads` | `0` | `0` 交给 ONNX Runtime 决定；单次推理内部已用满多核 |
| `instruct` | 与远程同一默认 | 三态语义一致（缺省 / `""` / 自定义） |

`local-onnx` **故意没有 `dimensions` 字段**：输出维度由 ONNX 图签名探测。harrier 的图
输出是 `sentence_embedding: float32[batch, 1024]`，last-token 池化与 L2 归一化都已内建在
图里，所以 Rust 侧不需要再做，也不该被配置项覆盖。

切换后端不会迁移数据。后端身份（模型 + 维度 + 截断长度）会被哈希成一个指纹，并同时决定
LanceDB 表名（`codeseek_<项目>_<指纹>`）、增量哈希文件名（`embedding_hashes_<指纹>.json`）
以及每条嵌入缓存的 key —— 因此两个后端各自拥有一套索引，维度错配不可能静默发生。
切换后重新执行 `codeseek init` 即可；若要丢弃当前后端的索引并全量重嵌：

```bash
codeseek init --rebuild --yes
```

`--rebuild` 只删 `config.json` 里当前后端那一张表；之后切回另一个后端时它的索引仍然完整。
因为会删除索引数据，必须与 `--yes` 同时给出。

构建说明（macOS arm64）：ONNX Runtime 静态库引用了 clang 的
`___isPlatformVersionAtLeast`，而 macOS SDK 与 Rust 的 `compiler_builtins` 都不提供它。
`rust-core/local-embed/build.rs` 通过 `clang -print-resource-dir` 定位并链接
`libclang_rt.osx.a`，因此 `lto = "fat"` 可以保留，也不需要任何 `RUSTFLAGS`。
完整设计与实测数据见 `rust-core/docs/local-embedding-backend.md`。

### 模型角色

| 模型 | 作用 | 时机 |
|------|------|------|
| `Qwen/Qwen3-Embedding-4B` | 将代码转为向量，用于密集搜索 | 构建索引时 |
| `harrier-oss-v1-0.6b`（ONNX，本地） | 同上，走 ONNX Runtime 完全离线 | 构建索引时 |
| `Qwen/Qwen3-Reranker-4B` | 为 (查询, 代码) 配对精确打分 | 搜索时 |

首次运行交互式向导自动填写，也可以手动创建。

## 开发

```bash
cd rust-core

# 编译
cargo build

# 编译 + 安装到 ~/.codeseek/bin/
cd .. && ./build.sh --release

# 运行测试
cargo test

# 编译 TypeScript wrapper
npm run build
```

## 许可证

MIT

构建于：Tree-sitter · Petgraph · LanceDB · Tantivy · Tokio · Clap
