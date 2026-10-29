use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use std::io;
use uuid::Uuid;
use tracing::{info, debug};
use md5;
use chrono::Utc;

use crate::codegraph::types::{
    FileMetadata, FileIndex, SnippetIndex, EntityGraph, PetCodeGraph,
    FunctionInfo, ClassInfo
};
use crate::codegraph::treesitter::TreeSitterParser;
// Phase P2b 交付物 2（接线点④）：增量链路也要能走分层解析器。
// Phase P3 交付物 1：产边/提实现在本文件**不再有副本** —— 四个共用件全部来自
// `codegraph::parser`（`CodeParser` 的刷新链路用的是同一份），因此
// "全量 vs 增量结果一致"（交付物 4 harness）是结构性的，而不是靠两份实现人肉对齐。
use crate::cli::args::ResolverMode;
use crate::codegraph::parser::{
    emit_layered_call_edges_for_file, emit_legacy_call_edges_for_file, extract_file_entities,
};
use crate::codegraph::resolution::resolver::ResolutionConfig;
use crate::codegraph::resolution::build_file_scope_ctx;

/// 增量更新管理器
pub struct IncrementalManager {
    /// 文件元数据存储
    file_metadata: HashMap<PathBuf, FileMetadata>,
    /// 文件索引
    file_index: FileIndex,
    /// 代码片段索引
    snippet_index: SnippetIndex,
    /// TreeSitter解析器
    ts_parser: TreeSitterParser,
    /// Phase P2a/P2b：`--resolver` 的取值（**默认 `Legacy`**）。
    ///
    /// P2a 里这个字段还不存在；P2b 起它与 `CodeParser::resolver_mode` 同口径，
    /// 唯一的差别是本结构体不缓存 `FileScopeCtx`，layered 分支**现场**用
    /// `parse_file_with_metadata` + `build_file_scope_ctx` 造 ctx（口径与
    /// `CodeParser::ensure_file_scope_ctx` 逐参数一致）。
    resolver_mode: ResolverMode,
    /// Phase P2b：本轮的仓库根，用于 module path 推导（`None` ⇒ module path 降级为空串）。
    resolution_root: Option<PathBuf>,
    /// Phase P2b 收口（任务口径 A）：文件级解析上下文缓存，与 `CodeParser::file_scopes` 同语义。
    ///
    /// 改造前 P2b 主体在 `_analyze_file_calls_layered` 里**每次都重新解析一遍文件**来造 ctx
    /// （本结构体当时没有 ctx 缓存），于是一次 `refresh_file` 对同一文件解析两次：
    /// `_extract_entities_from_file` 一次、产边再一次。口径 A 要求"解析/重建决策"与
    /// "索引供给"解耦，并且**不许为了喂索引而重复解析** —— 所以这里补上与 `CodeParser`
    /// 同一份生命周期的 ctx 缓存：
    ///  - 写入点：`_extract_entities_from_file`（与 `file_functions`/`ctx` 同批产出）；
    ///  - 失效点：`_remove_file_entities`（整文件失效）；
    ///  - 读者：`_analyze_file_calls_layered`（拿不到才按需补建一次，并缓存起来）。
    /// `Legacy` 分支完全不读这个字段 ⇒ 默认路径产边逐字节不变（红线 1）。
    file_scopes: HashMap<PathBuf, std::sync::Arc<crate::codegraph::resolution::FileScopeCtx>>,
}

impl IncrementalManager {
    pub fn new() -> Self {
        Self {
            file_metadata: HashMap::new(),
            file_index: FileIndex::default(),
            snippet_index: SnippetIndex::default(),
            ts_parser: TreeSitterParser::new(),
            // Phase P2b：默认 legacy —— 不置 flag 时增量产边与 P1 完全一致（红线 1）。
            resolver_mode: ResolverMode::Legacy,
            resolution_root: None,
            file_scopes: HashMap::new(),
        }
    }

    /// Phase P2b：设置解析引擎模式（与 `CodeParser::set_resolver_mode` 同名同义）。
    pub fn set_resolver_mode(&mut self, mode: ResolverMode) {
        self.resolver_mode = mode;
    }

    /// Phase P2b：当前解析引擎模式（默认 `Legacy`）。
    pub fn resolver_mode(&self) -> ResolverMode {
        self.resolver_mode.clone()
    }

    /// Phase P2b：设置 module path 推导用的仓库根。
    pub fn set_resolution_root(&mut self, root: Option<PathBuf>) {
        self.resolution_root = root;
    }

    /// 计算文件的MD5哈希值
    pub fn compute_file_md5(&self, file_path: &Path) -> Result<String, io::Error> {
        let content = fs::read(file_path)?;
        let hash = md5::compute(&content);
        Ok(format!("{:x}", hash))
    }

    /// 检查文件是否需要更新
    pub fn needs_update(&self, file_path: &Path) -> Result<bool, io::Error> {
        let current_md5 = self.compute_file_md5(file_path)?;
        
        if let Some(metadata) = self.file_metadata.get(file_path) {
            Ok(metadata.md5 != current_md5)
        } else {
            // 新文件，需要更新
            Ok(true)
        }
    }

    /// 获取文件元数据
    pub fn get_file_metadata(&self, file_path: &Path) -> Option<&FileMetadata> {
        self.file_metadata.get(file_path)
    }

    /// 更新单个文件
    pub fn refresh_file(
        &mut self,
        file_path: &PathBuf,
        entity_graph: &mut EntityGraph,
        call_graph: &mut PetCodeGraph,
    ) -> Result<(), String> {
        info!("Refreshing file: {}", file_path.display());

        // 检查文件是否存在
        if !file_path.exists() {
            // 文件被删除，清理相关索引
            self._remove_file_entities(file_path, entity_graph, call_graph);
            return Ok(());
        }

        // 计算当前MD5
        let current_md5 = self.compute_file_md5(file_path)
            .map_err(|e| format!("Failed to compute MD5 for {}: {}", file_path.display(), e))?;

        // 检查是否需要更新
        if let Some(metadata) = self.file_metadata.get(file_path) {
            if metadata.md5 == current_md5 {
                debug!("File {} unchanged, skipping", file_path.display());
                return Ok(());
            }
        }

        // 文件需要更新，执行增量更新
        self._update_file(file_path, &current_md5, entity_graph, call_graph)?;

        Ok(())
    }

    /// 更新文件元数据
    fn _update_file(
        &mut self,
        file_path: &PathBuf,
        current_md5: &str,
        entity_graph: &mut EntityGraph,
        call_graph: &mut PetCodeGraph,
    ) -> Result<(), String> {
        // 1. 移除旧的实体和函数
        self._remove_file_entities(file_path, entity_graph, call_graph);

        // 2. 解析文件，提取新的实体和函数
        let (classes, functions) = self._extract_entities_from_file(file_path)?;

        // 3. 添加到图中
        let class_ids: Vec<Uuid> = classes.iter().map(|c| c.id).collect();
        let function_ids: Vec<Uuid> = functions.iter().map(|f| f.id).collect();

        for class in classes {
            entity_graph.add_class(class);
        }

        for function in functions {
            call_graph.add_function(function);
        }

        // 4. 分析调用关系
        self._analyze_file_calls(file_path, &function_ids, call_graph)?;

        // 5. 更新索引
        self.file_index.rebuild_for_file(file_path, class_ids.clone(), function_ids.clone());

        // 6. 更新代码片段索引
        self._update_snippet_index(file_path, &class_ids, &function_ids)?;

        // 7. 更新文件元数据
        let metadata = FileMetadata {
            path: file_path.clone(),
            md5: current_md5.to_string(),
            last_updated: Utc::now(),
            file_size: fs::metadata(file_path)
                .map(|m| m.len())
                .unwrap_or(0),
            language: self._detect_language(file_path),
        };
        self.file_metadata.insert(file_path.clone(), metadata);

        info!("Successfully updated file: {}", file_path.display());
        Ok(())
    }

    /// 从文件提取实体
    ///
    /// Phase P3 交付物 1：实现本体已上提到
    /// [`crate::codegraph::parser::extract_file_entities`]（与 `CodeParser` 刷新链路
    /// **同一份**代码），这里只保留"把 ctx 装进本 manager 的 `file_scopes` 缓存"这一段
    /// 属于 `IncrementalManager` 自己的记账。改造前这里是 `CodeParser` 那份的逐行副本，
    /// 正是 P0 记录在案的"改一处漏一处"缺陷的复发温床。
    ///
    /// ⚠ `language`/`namespace` 仍由本 manager 自己的 `_detect_language`/`_extract_namespace`
    /// 供给（与改造前逐字节一致）：增量档历史上不识别 `.js/.jsx`，把它并到 `CodeParser`
    /// 的版本会改变增量 legacy 的节点字段，超出本轮范围，P3 报告已单独登记该差异。
    fn _extract_entities_from_file(&mut self, file_path: &PathBuf) -> Result<(Vec<ClassInfo>, Vec<FunctionInfo>), String> {
        let language = self._detect_language(file_path);
        let namespace = self._extract_namespace(file_path);
        let (classes, functions, ctx) = extract_file_entities(
            &self.ts_parser,
            file_path,
            self.resolution_root.as_deref(),
            &language,
            &namespace,
        )?;
        self.file_scopes.insert(file_path.clone(), ctx);
        Ok((classes, functions))
    }

    /// 分析文件的函数调用
    ///
    /// Phase P2b 交付物 2（接线点④）：按 `--resolver` 分叉。
    /// Phase P3 交付物 1：两档的**判定与产边代码**都上提到 `codegraph::parser` 的共享件
    /// （[`emit_legacy_call_edges_for_file`] / [`emit_layered_call_edges_for_file`]），
    /// 与 `CodeParser` 的刷新链路共用同一份实现 ⇒ 同一个调用点在"全量"与"增量"两条
    /// 链路上必然得到相同的边（含 `resolution_level`/`confidence`/`rank`/`candidate_count`
    /// 四个标量），这正是交付物 4 一致性 harness 能成立的结构性保证。
    fn _analyze_file_calls(
        &mut self,
        file_path: &PathBuf,
        function_ids: &[Uuid],
        call_graph: &mut PetCodeGraph,
    ) -> Result<(), String> {
        // 本文件的函数列表：增量链路以**图**为准（`_update_file` 刚把这批 id `add_function` 进去）。
        // 顺序 = `function_ids` 顺序 = `_extract_entities_from_file` 的产出顺序，
        // 因此下面 first-wins 的命中结果与改造前逐个 id 查图完全一致（红线 1）。
        let funcs: Vec<FunctionInfo> = function_ids
            .iter()
            .filter_map(|id| call_graph.get_function_by_id(id))
            .cloned()
            .collect();

        if self.resolver_mode == ResolverMode::Layered {
            // ctx：优先取缓存（`_extract_entities_from_file` 本轮已解析并顺手建好）；
            // 只有缓存里没有时才现场按需补建一次并回填，绝不为"喂索引"而重复解析该文件。
            let ctx = self.ensure_file_scope_ctx(file_path)?;
            let config = ResolutionConfig::all();
            return emit_layered_call_edges_for_file(
                &self.ts_parser,
                file_path,
                &ctx,
                &funcs,
                call_graph,
                self.resolution_root.as_deref(),
                &config,
            );
        }

        emit_legacy_call_edges_for_file(&self.ts_parser, file_path, &funcs, call_graph)
    }

    /// Phase P3：按需补建某个文件的 `FileScopeCtx`（命中缓存则直接复用）。
    ///
    /// 口径与 `CodeParser::ensure_file_scope_ctx` **逐参数一致**
    /// （`parse_file_with_metadata` → `_detect_language` → `build_file_scope_ctx`）。
    /// ctx 建不出来 ⇒ 上抛错误，**绝不静默产 0 条边**（与 legacy 里 `parse_file` 失败
    /// 直接 `Err` 的口径一致）。
    fn ensure_file_scope_ctx(
        &mut self,
        file_path: &PathBuf,
    ) -> Result<std::sync::Arc<crate::codegraph::resolution::FileScopeCtx>, String> {
        if let Some(cached) = self.file_scopes.get(file_path) {
            return Ok(cached.clone());
        }
        let ast = self
            .ts_parser
            .parse_file_with_metadata(file_path)
            .map_err(|e| format!("Failed to parse file for call analysis: {:?}", e))?;
        let language = self._detect_language(file_path);
        let built = build_file_scope_ctx(
            file_path,
            &ast.symbols,
            self.resolution_root.as_deref(),
            &language,
            ast.declared_package,
        );
        self.file_scopes.insert(file_path.clone(), built.clone());
        Ok(built)
    }

    /// 移除文件相关的所有实体
    fn _remove_file_entities(
        &mut self,
        file_path: &PathBuf,
        entity_graph: &mut EntityGraph,
        call_graph: &mut PetCodeGraph,
    ) {
        // 获取文件的所有实体ID
        let entity_ids = self.file_index.get_all_entity_ids(file_path);
        let function_ids = self.file_index.get_all_function_ids(file_path);
        let _class_ids = self.file_index.get_all_class_ids(file_path);

        // 从图中移除
        for entity_id in entity_ids {
            entity_graph.remove_entity(&entity_id);
        }

        for function_id in function_ids {
            // 必须走索引安全的删除原语：直接 remove_node 会让 NodeIndex 映射因 swap 搬移而失效
            call_graph.remove_function(&function_id);
        }

        // Phase P2b 收口（真缺陷 1 的同源修复，见 `CodeParser::_remove_file_entities` 的说明）：
        // `file_index` 只记录**本 manager 自己刷新过**的文件。若图里的节点是别的链路写进去的
        // （`CodeParser::parse_file` / 从 `graph.bin` 读回来的增量轮），上面的循环遍历的是空列表
        // ⇒ 旧节点一个都不删 ⇒ 紧接着 `add_function` 用新 Uuid 再插一份 ⇒ 同一文件两套函数节点
        // ⇒ 同一个调用点叠出两条一模一样的边。兜底以图自身的按文件反向索引为准（幂等：
        // 已删除的 id 在 `remove_function` 里查不到映射，直接返回 false）。
        call_graph.remove_functions_by_file(file_path);

        // 清理索引
        self.file_index.remove_file(file_path);
        self.snippet_index.clear_file_cache(file_path);
        // Phase P2b 收口（口径 A）：ctx 与实体同生命周期失效，
        // 残留会让下一个刷新轮读到已删除文件的旧作用域树/旧 import 绑定。
        self.file_scopes.remove(file_path);

        // 从元数据中移除
        self.file_metadata.remove(file_path);
    }

    /// 更新代码片段索引
    fn _update_snippet_index(
        &mut self,
        file_path: &PathBuf,
        class_ids: &[Uuid],
        function_ids: &[Uuid],
    ) -> Result<(), String> {
        // 读取文件内容
        let content = fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file for snippet indexing: {}", e))?;

        let _lines: Vec<&str> = content.lines().collect();

        // 为类添加代码片段
        for &class_id in class_ids {
            if let Some(entity) = self.snippet_index.get_snippet_info(&class_id) {
                let snippet_info = crate::codegraph::types::SnippetInfo {
                    file_path: file_path.clone(),
                    line_start: entity.line_start,
                    line_end: entity.line_end,
                    cached_content: None,
                };
                self.snippet_index.add_snippet(class_id, snippet_info);
            }
        }

        // 为函数添加代码片段
        for &function_id in function_ids {
            if let Some(entity) = self.snippet_index.get_snippet_info(&function_id) {
                let snippet_info = crate::codegraph::types::SnippetInfo {
                    file_path: file_path.clone(),
                    line_start: entity.line_start,
                    line_end: entity.line_end,
                    cached_content: None,
                };
                self.snippet_index.add_snippet(function_id, snippet_info);
            }
        }

        Ok(())
    }

    /// 检测文件语言
    fn _detect_language(&self, file_path: &Path) -> String {
        if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "rs" => "rust".to_string(),
                "py" | "py3" | "pyx" => "python".to_string(),
                "ts" | "tsx" => "typescript".to_string(),
                "java" => "java".to_string(),
                "cpp" | "cc" | "cxx" | "c++" | "c" | "h" | "hpp" | "hxx" | "hh" => "cpp".to_string(),
                "go" => "go".to_string(),
                _ => "unknown".to_string(),
            }
        } else {
            "unknown".to_string()
        }
    }

    /// 提取命名空间
    fn _extract_namespace(&self, file_path: &Path) -> String {
        // 从文件内容解析命名空间
        if let Ok(content) = fs::read_to_string(file_path) {
            return self._extract_namespace_from_content(&content, file_path);
        }
        "global".to_string()
    }

    /// 从文件内容提取命名空间
    fn _extract_namespace_from_content(&self, content: &str, file_path: &Path) -> String {
        let language = self._detect_language(file_path);
        
        match language.as_str() {
            "rust" => {
                // 查找mod声明
                for line in content.lines() {
                    if line.trim().starts_with("mod ") {
                        if let Some(name) = line.trim().split_whitespace().nth(1) {
                            return name.to_string();
                        }
                    }
                }
                "crate".to_string()
            },
            "python" => {
                // 查找包名或模块名
                for line in content.lines() {
                    if line.trim().starts_with("__package__") {
                        if let Some(name) = line.split('=').nth(1) {
                            return name.trim().trim_matches('"').trim_matches('\'').to_string();
                        }
                    }
                }
                "global".to_string()
            },
            "java" => {
                // 查找package声明
                for line in content.lines() {
                    if line.trim().starts_with("package ") {
                        if let Some(package) = line.trim().split_whitespace().nth(1) {
                            return package.trim_end_matches(';').to_string();
                        }
                    }
                }
                "default".to_string()
            },
            "cpp" => {
                // 查找namespace声明
                for line in content.lines() {
                    if line.trim().starts_with("namespace ") {
                        if let Some(name) = line.trim().split_whitespace().nth(1) {
                            return name.to_string();
                        }
                    }
                }
                "global".to_string()
            },
            "go" => {
                // 查找package声明
                for line in content.lines() {
                    if line.trim().starts_with("package ") {
                        if let Some(package) = line.trim().split_whitespace().nth(1) {
                            return package.trim_end_matches(';').to_string();
                        }
                    }
                }
                "global".to_string()
            },
            _ => "global".to_string(),
        }
    }

    /// 获取文件索引
    pub fn get_file_index(&self) -> &FileIndex {
        &self.file_index
    }

    /// 获取代码片段索引
    pub fn get_snippet_index(&self) -> &SnippetIndex {
        &self.snippet_index
    }

    /// 获取所有文件元数据
    pub fn get_all_file_metadata(&self) -> &HashMap<PathBuf, FileMetadata> {
        &self.file_metadata
    }

    /// 保存状态到文件
    pub fn save_state(&self, path: &Path) -> Result<(), String> {
        let state = serde_json::json!({
            "file_metadata": self.file_metadata,
            "file_index": self.file_index,
            "snippet_index": self.snippet_index,
        });

        fs::write(path, serde_json::to_string_pretty(&state)
            .map_err(|e| format!("Failed to serialize state: {}", e))?)
            .map_err(|e| format!("Failed to save state: {}", e))?;

        Ok(())
    }

    /// 从文件加载状态
    pub fn load_state(&mut self, path: &Path) -> Result<(), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read state file: {}", e))?;

        let state: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse state: {}", e))?;

        if let Some(metadata) = state.get("file_metadata") {
            self.file_metadata = serde_json::from_value(metadata.clone())
                .map_err(|e| format!("Failed to deserialize file metadata: {}", e))?;
        }

        if let Some(index) = state.get("file_index") {
            self.file_index = serde_json::from_value(index.clone())
                .map_err(|e| format!("Failed to deserialize file index: {}", e))?;
        }

        if let Some(snippets) = state.get("snippet_index") {
            self.snippet_index = serde_json::from_value(snippets.clone())
                .map_err(|e| format!("Failed to deserialize snippet index: {}", e))?;
        }

        Ok(())
    }
}

impl Default for IncrementalManager {
    fn default() -> Self {
        Self::new()
    }
} 

impl crate::storage::traits::IncrementalUpdater for IncrementalManager {
    fn compute_file_md5(&self, file_path: &std::path::Path) -> Result<String, std::io::Error> {
        Self::compute_file_md5(self, file_path)
    }

    fn needs_update(&self, file_path: &std::path::Path) -> Result<bool, std::io::Error> {
        Self::needs_update(self, file_path)
    }

    fn refresh_file(
        &mut self,
        file_path: &std::path::PathBuf,
        entity_graph: &mut EntityGraph,
        call_graph: &mut PetCodeGraph,
    ) -> Result<(), String> {
        Self::refresh_file(self, file_path, entity_graph, call_graph)
    }

    fn get_file_index(&self) -> &FileIndex { Self::get_file_index(self) }
    fn get_snippet_index(&self) -> &SnippetIndex { Self::get_snippet_index(self) }
    fn get_all_file_metadata(&self) -> &HashMap<std::path::PathBuf, FileMetadata> { Self::get_all_file_metadata(self) }

    fn save_state(&self, path: &std::path::Path) -> Result<(), String> { Self::save_state(self, path) }
    fn load_state(&mut self, path: &std::path::Path) -> Result<(), String> { Self::load_state(self, path) }
} 