use std::path::PathBuf;
use std::fs;
use std::io;
use dirs;
use std::collections::HashMap;
use crate::codegraph::types::PetCodeGraph;
use crate::storage::petgraph_storage::PetGraphStorageManager;
use crate::cli::args::StorageMode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

pub struct PersistenceManager {
    base_dir: PathBuf,
    storage_mode: StorageMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub project_id: String,
    pub project_dir: String,
    pub parsed_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectsRegistry {
    // key: project_id
    projects: HashMap<String, ProjectRecord>,
}

impl PersistenceManager {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let base_dir = home.join(".codeseek");
        Self::with_storage_mode(StorageMode::default(), base_dir)
    }

    pub fn with_storage_mode(storage_mode: StorageMode, base_dir: PathBuf) -> Self {
        // Create base directory if it doesn't exist
        if !base_dir.exists() {
            fs::create_dir_all(&base_dir).ok();
        }
        
        Self { base_dir, storage_mode }
    }

    pub fn set_storage_mode(&mut self, storage_mode: StorageMode) {
        self.storage_mode = storage_mode;
    }

    pub fn get_storage_mode(&self) -> &StorageMode {
        &self.storage_mode
    }

    /// 获取项目数据目录: ~/.codeseek/projects/<project_id>/
    fn project_dir(&self, project_id: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek").join("projects").join(project_id)
    }

    pub fn save_graph(&self, project_id: &str, graph: &PetCodeGraph) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        fs::create_dir_all(&project_dir)?;
        
        match self.storage_mode {
            StorageMode::Json => {
                self.save_graph_json(project_id, graph)?;
            },
            StorageMode::Binary => {
                self.save_graph_binary(project_id, graph)?;
            },
            StorageMode::Both => {
                self.save_graph_json(project_id, graph)?;
                self.save_graph_binary(project_id, graph)?;
            },
        }
        
        Ok(())
    }

    fn save_graph_json(&self, project_id: &str, graph: &PetCodeGraph) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        let graph_file = project_dir.join("graph.json");
        
        PetGraphStorageManager::save_to_file(graph, &graph_file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        
        Ok(())
    }

    fn save_graph_binary(&self, project_id: &str, graph: &PetCodeGraph) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        let graph_file = project_dir.join("graph.bin");
        
        PetGraphStorageManager::save_to_binary(graph, &graph_file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        
        Ok(())
    }

    /// `StorageMode::Both` 的缓存读取聚合逻辑（纯函数，便于脱离文件系统单测）。
    ///
    /// 原实现是：`match self.load_graph_binary(id) { Ok(graph) => Ok(graph), Err(_) => json }`，
    /// 有两个问题：
    /// 1. `Ok(graph)` 把 **`Ok(None)`（graph.bin 不存在）也当成命中直接返回**，
    ///    于是"只有 graph.json"的缓存永远读不到；
    /// 2. JSON 分支的 `Err` 原样上抛 —— 而 `GRAPH_SCHEMA_VERSION` 版本门禁正是以 `Err`
    ///    表达"缓存失效"，上抛会让 `init` 变成致命失败而不是重建。
    ///
    /// 现在的语义（缓存问题绝不是致命错误）：
    /// - binary 命中（`Ok(Some)`）→ 直接使用；
    /// - binary 缺失（`Ok(None)`）或不可用（`Err`）→ 回退 JSON；
    /// - JSON 也不可用 → **降级成 `Ok(None)`**（=缓存 miss，由
    ///   `parser::_load_existing_graph` 触发全量重建），错误只落 warn 日志。
    fn read_both_with_fallback(
        load_binary: impl FnOnce() -> io::Result<Option<PetCodeGraph>>,
        load_json: impl FnOnce() -> io::Result<Option<PetCodeGraph>>,
    ) -> io::Result<Option<PetCodeGraph>> {
        match load_binary() {
            Ok(Some(graph)) => Ok(Some(graph)),
            Ok(None) => load_json(),
            Err(binary_err) => {
                warn!("二进制图缓存不可用，回退 JSON 缓存：{}", binary_err);
                match load_json() {
                    Ok(graph) => Ok(graph),
                    Err(json_err) => {
                        warn!(
                            "JSON 图缓存同样不可用（{}），按缓存失效处理并触发全量重建；原始二进制错误：{}",
                            json_err, binary_err
                        );
                        Ok(None)
                    }
                }
            }
        }
    }

    pub fn load_graph(&self, project_id: &str) -> io::Result<Option<PetCodeGraph>> {
        match self.storage_mode {
            StorageMode::Json => self.load_graph_json(project_id),
            StorageMode::Binary => self.load_graph_binary(project_id),
            StorageMode::Both => Self::read_both_with_fallback(
                || self.load_graph_binary(project_id),
                || self.load_graph_json(project_id),
            ),
        }
    }

    fn load_graph_json(&self, project_id: &str) -> io::Result<Option<PetCodeGraph>> {
        let graph_file = self.project_dir(project_id).join("graph.json");
        
        if !graph_file.exists() {
            return Ok(None);
        }
        
        let graph = PetGraphStorageManager::load_from_file(&graph_file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        
        Ok(Some(graph))
    }

    fn load_graph_binary(&self, project_id: &str) -> io::Result<Option<PetCodeGraph>> {
        let graph_file = self.project_dir(project_id).join("graph.bin");
        
        if !graph_file.exists() {
            return Ok(None);
        }
        
        let graph = PetGraphStorageManager::load_from_binary(&graph_file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        
        Ok(Some(graph))
    }

    pub fn save_file_hash(&self, project_id: &str, file_path: &str, hash: &str) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        fs::create_dir_all(&project_dir)?;
        
        let hash_file = project_dir.join("file_hashes.json");
        let mut hashes: HashMap<String, String> = if hash_file.exists() {
            let content = fs::read_to_string(&hash_file)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        
        hashes.insert(file_path.to_string(), hash.to_string());
        let json = serde_json::to_string_pretty(&hashes)?;
        fs::write(hash_file, json)?;
        
        Ok(())
    }

    pub fn load_file_hashes(&self, project_id: &str) -> io::Result<HashMap<String, String>> {
        let hash_file = self.project_dir(project_id).join("file_hashes.json");
        
        if !hash_file.exists() {
            return Ok(HashMap::new());
        }
        
        let content = fs::read_to_string(hash_file)?;
        let hashes: HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        
        Ok(hashes)
    }

    pub fn delete_project(&self, project_id: &str) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        if project_dir.exists() {
            fs::remove_dir_all(project_dir)?;
        }
        // also remove from registry if present
        let mut registry = self.load_registry()?;
        registry.projects.remove(project_id);
        self.save_registry(&registry)?;
        Ok(())
    }

    pub fn list_projects(&self) -> io::Result<Vec<String>> {
        let projects_dir = crate::config::Config::projects_dir();
        let mut projects = Vec::new();
        
        if projects_dir.exists() {
            for entry in fs::read_dir(&projects_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        projects.push(name.to_string());
                    }
                }
            }
        }
        
        Ok(projects)
    }

    /// 获取已保存的文件信息
    pub fn get_saved_files_info(&self, project_id: &str) -> io::Result<Vec<String>> {
        let project_dir = self.project_dir(project_id);
        let mut files = Vec::new();
        
        if !project_dir.exists() {
            return Ok(files);
        }
        
        for entry in fs::read_dir(&project_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    let metadata = entry.metadata()?;
                    let size = metadata.len();
                    files.push(format!("{} ({} bytes)", name, size));
                }
            }
        }
        
        Ok(files)
    }

    // ---- Projects registry (for parsed projects) ----

    fn registry_path(&self) -> PathBuf {
        self.base_dir.join("projects.json")
    }

    fn load_registry(&self) -> io::Result<ProjectsRegistry> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(ProjectsRegistry::default());
        }
        let content = fs::read_to_string(path)?;
        let reg: ProjectsRegistry = serde_json::from_str(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(reg)
    }

    fn save_registry(&self, registry: &ProjectsRegistry) -> io::Result<()> {
        let path = self.registry_path();
        let json = serde_json::to_string_pretty(registry)?;
        fs::write(path, json)
    }

    pub fn register_project(&self, project_id: &str, project_dir: &str) -> io::Result<()> {
        let mut registry = self.load_registry()?;
        let record = ProjectRecord {
            project_id: project_id.to_string(),
            project_dir: project_dir.to_string(),
            parsed_at: Utc::now(),
        };
        registry.projects.insert(project_id.to_string(), record);
        self.save_registry(&registry)
    }

    pub fn is_project_parsed(&self, project_id: &str) -> io::Result<bool> {
        let registry = self.load_registry()?;
        Ok(registry.projects.contains_key(project_id))
    }

    pub fn find_project_by_dir(&self, project_dir: &str) -> io::Result<Option<String>> {
        let registry = self.load_registry()?;
        for (pid, rec) in registry.projects.iter() {
            if rec.project_dir == project_dir {
                return Ok(Some(pid.clone()));
            }
        }
        Ok(None)
    }

    pub fn list_parsed_projects(&self) -> io::Result<Vec<ProjectRecord>> {
        let registry = self.load_registry()?;
        Ok(registry.projects.values().cloned().collect())
    }
} 

impl crate::storage::traits::GraphPersistence for PersistenceManager {
    fn save_graph(&self, project_id: &str, graph: &PetCodeGraph) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        fs::create_dir_all(&project_dir)?;

        match self.storage_mode {
            StorageMode::Json => {
                self.save_graph_json(project_id, graph)?;
            },
            StorageMode::Binary => {
                self.save_graph_binary(project_id, graph)?;
            },
            StorageMode::Both => {
                self.save_graph_json(project_id, graph)?;
                self.save_graph_binary(project_id, graph)?;
            },
        }
        Ok(())
    }

    fn load_graph(&self, project_id: &str) -> io::Result<Option<PetCodeGraph>> {
        match self.storage_mode {
            StorageMode::Json => self.load_graph_json(project_id),
            StorageMode::Binary => self.load_graph_binary(project_id),
            StorageMode::Both => {
                // 与固有方法收敛到同一套回退语义，避免两份实现分叉
                Self::read_both_with_fallback(
                    || self.load_graph_binary(project_id),
                    || self.load_graph_json(project_id),
                )
            },
        }
    }

    fn save_file_hash(&self, project_id: &str, file_path: &str, hash: &str) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        fs::create_dir_all(&project_dir)?;

        let hashes_file = project_dir.join("file_hashes.json");
        let mut hashes: HashMap<String, String> = if hashes_file.exists() {
            let content = fs::read_to_string(&hashes_file)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        hashes.insert(file_path.to_string(), hash.to_string());

        let json = serde_json::to_string_pretty(&hashes)?;
        fs::write(&hashes_file, json)?;
        Ok(())
    }

    fn load_file_hashes(&self, project_id: &str) -> io::Result<HashMap<String, String>> {
        let hashes_file = self.project_dir(project_id).join("file_hashes.json");
        if !hashes_file.exists() {
            return Ok(HashMap::new());
        }
        let content = fs::read_to_string(&hashes_file)?;
        let hashes: HashMap<String, String> = serde_json::from_str(&content).unwrap_or_default();
        Ok(hashes)
    }

    fn delete_project(&self, project_id: &str) -> io::Result<()> {
        let project_dir = self.project_dir(project_id);
        if project_dir.exists() {
            fs::remove_dir_all(project_dir)?;
        }
        Ok(())
    }

    fn list_projects(&self) -> io::Result<Vec<String>> {
        Self::list_projects(self)
    }

    fn get_saved_files_info(&self, project_id: &str) -> io::Result<Vec<String>> {
        Self::get_saved_files_info(self, project_id)
    }

    fn register_project(&self, project_id: &str, project_dir: &str) -> io::Result<()> {
        Self::register_project(self, project_id, project_dir)
    }

    fn is_project_parsed(&self, project_id: &str) -> io::Result<bool> {
        Self::is_project_parsed(self, project_id)
    }

    fn find_project_by_dir(&self, project_dir: &str) -> io::Result<Option<String>> {
        Self::find_project_by_dir(self, project_dir)
    }

    fn list_parsed_projects(&self) -> io::Result<Vec<crate::storage::persistence::ProjectRecord>> {
        Self::list_parsed_projects(self)
    }
}

#[cfg(test)]
mod tests {
    //! `StorageMode::Both` 回退语义的单测（纯函数，不碰文件系统）。
    //!
    //! 守住两条红线：
    //! 1. graph.bin 缺失/不可用时必须回退 graph.json（原实现把 `Ok(None)` 当命中直接返回）；
    //! 2. 两个缓存都不可用时只能降级成 `Ok(None)`（缓存 miss），**绝不把错误上抛成致命失败** ——
    //!    `GRAPH_SCHEMA_VERSION` 版本门禁正是以 `Err` 表达"缓存失效"，全靠这条降级链生效。

    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn dummy_graph(functions: usize) -> PetCodeGraph {
        let mut graph = PetCodeGraph::new();
        for index in 0..functions {
            graph.add_function(crate::codegraph::types::FunctionInfo {
                id: uuid::Uuid::new_v4(),
                name: format!("fn_{index}"),
                file_path: PathBuf::from("src/dummy.rs"),
                line_start: index + 1,
                line_end: index + 2,
                namespace: "crate".to_string(),
                language: "rust".to_string(),
                signature: None,
            });
        }
        graph
    }

    fn io_err(msg: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, msg)
    }

    #[test]
    fn test_both_binary_hit_skips_json() {
        let json_called = Rc::new(Cell::new(false));
        let flag = Rc::clone(&json_called);

        let result = PersistenceManager::read_both_with_fallback(
            || Ok(Some(dummy_graph(2))),
            move || {
                flag.set(true);
                Ok(Some(dummy_graph(9)))
            },
        )
        .expect("binary 命中不应返回错误");

        assert!(matches!(result, Some(graph) if graph.graph.node_count() == 2));
        assert!(!json_called.get(), "binary 命中时不该再读 JSON 缓存");
    }

    #[test]
    fn test_both_falls_back_when_binary_missing() {
        // 这正是原实现的缺陷：graph.bin 不存在（Ok(None)）时旧代码会直接返回 None
        let json_called = Rc::new(Cell::new(false));
        let flag = Rc::clone(&json_called);

        let result = PersistenceManager::read_both_with_fallback(
            || Ok(None),
            move || {
                flag.set(true);
                Ok(Some(dummy_graph(3)))
            },
        )
        .expect("回退 JSON 命中不应返回错误");

        assert!(json_called.get(), "binary 缺失时必须回退 JSON 缓存");
        assert!(matches!(result, Some(graph) if graph.graph.node_count() == 3));
    }

    #[test]
    fn test_both_falls_back_when_binary_unusable() {
        // 版本门禁/损坏文件都表现为 Err：必须回退而不是上抛
        let result = PersistenceManager::read_both_with_fallback(
            || Err(io_err("schema 版本不匹配")),
            || Ok(Some(dummy_graph(1))),
        )
        .expect("binary Err + JSON 命中不应返回错误");

        assert!(matches!(result, Some(graph) if graph.graph.node_count() == 1));
    }

    #[test]
    fn test_both_missing_both_returns_none() {
        let result = PersistenceManager::read_both_with_fallback(|| Ok(None), || Ok(None))
            .expect("两个缓存都不存在时应为 Ok(None)（缓存 miss）");
        assert!(result.is_none());
    }

    #[test]
    fn test_both_degrades_json_error_to_cache_miss() {
        // 关键用例：两侧都是 Err（例如旧 bincode + 旧 JSON）也只能得到 Ok(None)，
        // 上层 `_load_existing_graph` 据此走全量重建，绝不 panic、绝不上抛致命错误。
        let result = PersistenceManager::read_both_with_fallback(
            || Err(io_err("schema 版本不匹配")),
            || Err(io_err("missing field `schema_version`")),
        );

        let graph = match result {
            Ok(graph) => {
                assert!(graph.is_none(), "两侧均不可用必须是 Ok(None)，而不是缓存数据");
                graph
            }
            Err(e) => panic!("缓存不可用绝不该上抛错误，实际得到 Err: {e}"),
        };
        assert!(graph.is_none());
    }
}
