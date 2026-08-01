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

#[derive(Parser, Debug)]
#[clap(name = "codeseek", author, version, about = "Code intelligence CLI tool", long_about = None)]
pub struct Cli {
    /// Verbose mode
    #[clap(short, long, action)]
    pub verbose: bool,

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
