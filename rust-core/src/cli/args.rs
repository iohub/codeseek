use clap::{Parser, Subcommand, ValueEnum};

/// Storage format for persisted call graphs
#[derive(Debug, Clone, ValueEnum)]
pub enum StorageMode {
    /// JSON only
    Json,
    /// Binary (bincode) only
    Binary,
    /// Both JSON and binary
    Both,
}

impl Default for StorageMode {
    fn default() -> Self {
        StorageMode::Binary
    }
}

/// Phase P2a 交付物 3：调用边 callee 的解析引擎（`--resolver`）。
///
/// - `Legacy`（**默认**）：完全维持现状 —— 五处同名 first-wins
///   （`codegraph/parser.rs` 的 `_find_callee_function` / `_resolve_callee_function` /
///   `_resolve_qualified_function_name` / `_build_function_name_index` 与
///   `storage/incremental.rs` 的 `_find_callee_function`）一字未改。
/// - `Layered`：分层精度解析（L1 词法 → L2 外围类型 → L3 receiver 类型 → L4 import 绑定
///   → L5 限定名 → L6 同模块 → L7 全仓唯一 → L8 启发式）。
///
/// ⚠ **P2a 阶段这个开关不改变任何产边结果**：分层引擎已经实现但没有接进建图管线
/// （只有 `tests/test_resolution_engine.rs` 与 `CodeParser::resolution_config` 消费它）。
/// 真正生效在 P2b（同时给 `CallRelation` 加解析元信息并把 `GRAPH_SCHEMA_VERSION` 1→2）。
/// 之所以现在就加 flag：CLI 链路（`cli → main.rs → services/analyzer.rs → CodeParser`）
/// 必须先在 legacy 下被验证是通的，P2b 才只需改一处分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ResolverMode {
    /// 现状：名字相等即命中、多候选时取第一个。
    Legacy,
    /// 分层精度解析（P2b 起生效）。
    Layered,
}

impl Default for ResolverMode {
    /// Phase P3 起默认 **layered**（分层精度解析）。`legacy` 保留为回滚逃生口，
    /// 行为与 P1 逐字节一致，但**不再是默认档**。
    fn default() -> Self {
        ResolverMode::Layered
    }
}

impl ResolverMode {
    /// 落盘用的**稳定**档位名（`graph.json` / `graph.bin` 的解析档位戳就存这个字符串）。
    ///
    /// 为什么必须有这个方法而不是 `format!("{:?}", mode).to_lowercase()`：
    /// 戳字段一旦写进缓存文件就成了**持久化契约**，绝不能跟着 Rust 变体的 Debug 打印
    /// 或重命名而漂移。取值域固定为 `"legacy"` / `"layered"`，与 `--resolver` 的
    /// ValueEnum 名称一致；空串 `""` 在戳的语义里表示"未知/旧文件"（见
    /// `storage::petgraph_storage::PetGraphStorage::resolver_mode`）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ResolverMode::Legacy => "legacy",
            ResolverMode::Layered => "layered",
        }
    }
}

#[derive(Parser, Debug)]
#[clap(name = "codeseek", author, version, about = "Code intelligence CLI tool", long_about = None)]
pub struct Cli {
    /// Verbose mode
    #[clap(short, long, action)]
    pub verbose: bool,

    /// 忽略所有已持久化的缓存（图缓存 + 文件 MD5 哈希），强制全量重新分析后重新落盘。
    ///
    /// 这是图缓存 schema 版本门禁的**逃生口**：当缓存被判定为失效、或怀疑缓存与源码
    /// 不一致时，用 `codeseek --force-reindex init` 保证一次干净的从零构建。
    #[clap(long, action)]
    pub force_reindex: bool,

    /// Phase P2a/P2b/P3：callee 解析引擎。**Phase P3 起默认 `layered`**。
    ///
    /// - `layered`（默认）= 分层精度解析（L1 词法 → L2 外围类型 → L3 receiver 类型 →
    ///   L4 import 绑定 → L5 限定名 → L6 同模块 → L7 全仓唯一 → L8 启发式），
    ///   歧义调用点**每个候选一条边**，并把层级/置信度/候选总数写进边；
    /// - `legacy` = 旧的同名匹配（名字相等就命中、多候选取第一个），**仅作回滚逃生口**：
    ///   怀疑分层引擎把某仓产边弄坏时，`--resolver=legacy` 一键退回与 P1 逐字节一致的行为。
    ///
    /// 默认值在 Phase P3 由 `legacy` 切到 `layered`（P2 阶段逐字节验证已完成，红线解除）。
    #[clap(
        long,
        value_enum,
        default_value = "layered",
        help = "调用边解析引擎：layered（默认）= 分层精度解析（每候选一条边，带层级/置信度）；legacy = 旧的同名匹配（命中第一个），仅作回滚逃生口"
    )]
    pub resolver: ResolverMode,

    /// Phase P2a/P2b：单个调用点最多落盘多少个候选。
    ///
    /// 只**截断落盘条数**：`candidate_count` 仍写截断前的真实候选数，
    /// `is_resolved` 也仍按真实数判定 ⇒ `--max-candidates=1` 不会把歧义伪装成唯一。
    #[clap(
        long,
        default_value = "8",
        help = "单个调用点最多落盘的候选条数（超出丢弃但仍计入 candidate_count，且不改变 is_resolved 判定）"
    )]
    pub max_candidates: usize,

    /// Phase P2a：分层诊断开关，只启用/禁用指定的层（**空 = 全开**）。
    ///
    /// 语法见 `codegraph::resolution::resolver::ResolutionConfig::from_levels_spec`：
    /// `all` / `p1` / `none` / `l1,l5` / `l2-l6` / `all,-l6` / `max=4`，逗号分隔、大小写不敏感。
    /// 非法取值会**直接报错退出**，绝不静默退回全开 —— 否则"关掉某层"的诊断结论不可信。
    #[clap(
        long,
        default_value = "",
        help = "只启用指定解析层（空=全开）：all/p1/none/l1..l8/l2-l6/all,-l6；仅对 --resolver=layered 生效"
    )]
    pub levels: String,

    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Build or update the code index (full on first run, MD5-incremental thereafter)
    Init {
        /// Interactive configuration wizard
        #[clap(short = 'i', long, action)]
        interactive: bool,
    },
    /// Show index statistics (functions, files, last update)
    Status {
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Semantic code search (vector + BM25 + RRF fusion)
    Search {
        /// Search query text
        query: String,
        /// Maximum results to return
        #[clap(short, long, default_value = "10")]
        limit: usize,
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Find functions that call the given symbol
    Callers {
        /// Function or symbol name
        symbol: String,
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Find functions called by the given symbol
    Callees {
        /// Function or symbol name
        symbol: String,
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Query function call graph with depth (bi-directional)
    Callgraph {
        /// Function name to query as center node
        symbol: String,
        /// Query depth — layers of callers and callees to include (1-3)
        #[arg(short = 'd', long = "depth", default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=3))]
        depth: u32,
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Delete the current project's index data
    Uninit {
        /// Skip confirmation prompt
        #[clap(long, action)]
        force: bool,
    },
    /// List all indexed projects
    List {
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Start MCP server (for Claude Code / Codex integration)
    Serve {
        /// Run in MCP stdio mode
        #[clap(long, action)]
        mcp: bool,
    },
    /// Register codeseek as MCP tools in Claude Code / Codex
    Install,
    /// Remove codeseek MCP integration from Claude Code / Codex
    Uninstall,
    /// Show the code skeleton (function signatures without implementation) for one or more files
    Skeleton {
        /// File paths to show skeletons for (absolute or relative to project root)
        #[arg(required = true)]
        file_paths: Vec<String>,
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Show the code snippet for a specific function
    Snippet {
        /// Function name to look up
        function_name: String,
        /// File path to disambiguate if multiple functions share the same name
        #[arg(short, long = "file-path")]
        file_path: Option<String>,
        /// Output as JSON
        #[clap(long, action)]
        json: bool,
    },
    /// Install git hooks (post-commit, post-merge → codeseek init)
    InstallHooks,
    /// Manage the persistent knowledge base (add/search/list/delete)
    Knowledge {
        #[clap(subcommand)]
        action: KnowledgeCommand,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum KnowledgeCommand {
    /// Add a knowledge record to the knowledge base
    Add {
        /// Title (≤30 chars)
        #[arg(long)]
        title: String,
        /// Content (≤500 chars)
        #[arg(long)]
        content: String,
        /// Knowledge type: repo_retrieval | coding_modification
        #[arg(long)]
        r#type: String,
        /// Tags, comma-separated
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Related file paths, comma-separated
        #[arg(long, value_delimiter = ',')]
        related_files: Vec<String>,
        /// Source agent: repo_agent | coding_agent
        #[arg(long)]
        source_agent: String,
        /// Task ID
        #[arg(long)]
        task_id: String,
        /// Confidence 0.0-1.0
        #[arg(long, default_value_t = 0.8)]
        confidence: f32,
    },
    /// Search the knowledge base
    Search {
        /// Query text
        #[arg(long)]
        query: String,
        /// Max results
        #[arg(long, default_value_t = 8)]
        limit: usize,
        /// Enable reranking (currently a no-op placeholder)
        #[arg(long, action)]
        rerank: bool,
        /// Domain filter: repo, coding (comma-separated; empty = all domains)
        #[arg(long, value_delimiter = ',')]
        domains: Vec<String>,
    },
    /// List knowledge records
    List {
        /// Max results
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Filter by type
        #[arg(long)]
        r#type: Option<String>,
        /// Filter by tag (match any)
        #[arg(long)]
        tag: Option<String>,
    },
    /// Delete a knowledge record by ID
    Delete {
        /// Record ID (kn_*)
        #[arg(long)]
        id: String,
    },
}
