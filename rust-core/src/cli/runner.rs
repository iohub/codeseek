use tracing::info;
use crate::config::Config;

use super::args::{Cli, Commands};

pub struct CodeSeekRunner;

impl CodeSeekRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(cli: Cli, _config: Option<Config>) -> Result<(), Box<dyn std::error::Error>> {
        // `--force-reindex` 定义在 `Cli`（全局 flag）上，而下面的 `match cli.command`
        // 会移动 `cli.command`，所以必须在 match 之前先把这个 bool 取出来。
        let force_reindex = cli.force_reindex;

        match cli.command {
            Commands::Init { interactive: _ } => {
                // 真正的索引逻辑在 `src/main.rs` 的 `Commands::Init` 分支：
                // 该 flag 经 `CodeAnalyzer::set_force_reindex` → `CodeParser::set_force_reindex`
                // 让三个缓存读取点一律返回"缓存 miss"。
                info!(
                    "init command - to be implemented in Phase 2 (force_reindex: {})",
                    force_reindex
                );
                Ok(())
            }
            Commands::Status { json: _ } => {
                info!("status command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::Search { query: _, limit: _, json: _ } => {
                info!("search command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::Callers { symbol: _, json: _ } => {
                info!("callers command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::Callees { symbol: _, json: _ } => {
                info!("callees command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::Callgraph { symbol: _, depth: _, json: _ } => {
                info!("callgraph command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::Uninit { force: _ } => {
                info!("uninit command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::List { json: _ } => {
                info!("list command - please use codeseek directly");
                Ok(())
            }
            Commands::Serve { mcp: _ } => {
                info!("serve command - please use codeseek directly");
                Ok(())
            }
            Commands::Install => {
                info!("install command - please use codeseek directly");
                Ok(())
            }
            Commands::Uninstall => {
                info!("uninstall command - please use codeseek directly");
                Ok(())
            }
            Commands::Skeleton { file_paths: _, json: _ } => {
                info!("skeleton command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::Snippet { function_name: _, file_path: _, json: _ } => {
                info!("snippet command - to be implemented in Phase 2");
                Ok(())
            }
            Commands::InstallHooks => {
                info!("install-hooks command - please use codeseek directly");
                Ok(())
            }
            Commands::Knowledge { action } => {
                crate::knowledge::cli::handle_knowledge_command(action).await?;
                Ok(())
            }
        }
    }
}
