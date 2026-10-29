use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use std::io;
use uuid::Uuid;
use tracing::{info, warn, debug};
use md5;
use chrono::Utc;

use crate::codegraph::types::{
    FileMetadata, FileIndex, SnippetIndex, EntityGraph, PetCodeGraph,
    FunctionInfo, ClassInfo, CallRelation
};
use crate::codegraph::treesitter::TreeSitterParser;
// Phase P2b 交付物 2（接线点④）：增量链路也要能走分层解析器。
// 与 `codegraph::parser` 共用 `resolution` 里的同一套 caller 归属与产边代码，
// 这是"全量 vs 增量结果一致"（交付物 4 harness）成立的前提。
use crate::cli::args::ResolverMode;
use crate::codegraph::resolution::resolver::{
    call_sites_from_symbols_with_source, resolve_call_with_config, ResolutionConfig,
};
use crate::codegraph::resolution::{
    build_file_scope_ctx, build_resolution_index_from_graph,
    emit_layered_resolution as resolution_emit, layered_caller_node as resolution_caller_node,
};

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
    /// Phase P2b 收口（口径 A）：签名 `&self` → `&mut self`，因为这里要顺手把
    /// `FileScopeCtx` 缓存进 `self.file_scopes`（与 `CodeParser::_extract_entities_from_file`
    /// 完全同口径），让 layered 产边**不必为了拿 ctx 而重复解析同一个文件**。
    /// 解析入口改用 `parse_file_with_metadata`：它内部就是原来的 `parse_file`
    /// （`parse_file` = `parse_file_with_metadata(..)?.symbols` 的转发），
    /// 符号列表逐字节相同，只是额外带出 Java 的 package 声明 ⇒ 产出的实体不变（红线 1）。
    fn _extract_entities_from_file(&mut self, file_path: &PathBuf) -> Result<(Vec<ClassInfo>, Vec<FunctionInfo>), String> {
        let mut classes = Vec::new();
        let mut functions = Vec::new();

        // 使用TreeSitter解析器解析文件
        let ast = self.ts_parser.parse_file_with_metadata(file_path)
            .map_err(|e| format!("Failed to parse file {}: {:?}", file_path.display(), e))?;
        let symbols = ast.symbols;

        let language = self._detect_language(file_path);
        let namespace = self._extract_namespace(file_path);

        // Phase P2b 收口（口径 A）：与上面同一次解析顺手建 ctx 并缓存。
        // legacy 分支不读它 ⇒ 默认路径产边逐字节不变。
        let ctx = build_file_scope_ctx(
            file_path.as_path(),
            &symbols,
            self.resolution_root.as_deref(),
            &language,
            ast.declared_package,
        );
        self.file_scopes.insert(file_path.clone(), ctx);

        for symbol in symbols {
            let symbol_guard = symbol.read();
            let symbol_ref = symbol_guard.as_ref();

            match symbol_ref.symbol_type() {
                crate::codegraph::treesitter::structs::SymbolType::FunctionDeclaration => {
                    let function = FunctionInfo::legacy(
                        Uuid::new_v4(),
                        symbol_ref.name().to_string(),
                        file_path.clone(),
                        symbol_ref.full_range().start_point.row + 1,
                        symbol_ref.full_range().end_point.row + 1,
                        namespace.clone(),
                        language.clone(),
                        Some(symbol_ref.name().to_string()),
                    );
                    functions.push(function);
                },
                crate::codegraph::treesitter::structs::SymbolType::StructDeclaration => {
                    let class = ClassInfo {
                        id: Uuid::new_v4(),
                        name: symbol_ref.name().to_string(),
                        file_path: file_path.clone(),
                        line_start: symbol_ref.full_range().start_point.row + 1,
                        line_end: symbol_ref.full_range().end_point.row + 1,
                        namespace: namespace.clone(),
                        language: language.clone(),
                        class_type: crate::codegraph::types::ClassType::Struct,
                        parent_class: None,
                        implemented_interfaces: vec![],
                        member_functions: vec![],
                        member_variables: vec![],
                    };
                    classes.push(class);
                },
                _ => {}
            }
        }

        Ok((classes, functions))
    }

    /// 分析文件的函数调用
    ///
    /// Phase P2b 交付物 2（接线点④）：按 `--resolver` 分叉。
    /// - `Legacy`（默认）：下面原有的"本文件同名 first-wins + 全局 first-wins"**一字不改**；
    /// - `Layered`：交给 [`IncrementalManager::_analyze_file_calls_layered`]。
    fn _analyze_file_calls(
        &mut self,
        file_path: &PathBuf,
        function_ids: &[Uuid],
        call_graph: &mut PetCodeGraph,
    ) -> Result<(), String> {
        if self.resolver_mode == ResolverMode::Layered {
            return self._analyze_file_calls_layered(file_path, function_ids, call_graph);
        }

        let symbols = self.ts_parser.parse_file(file_path)
            .map_err(|e| format!("Failed to parse file for call analysis: {:?}", e))?;

        for symbol in symbols {
            let symbol_guard = symbol.read();
            let symbol_ref = symbol_guard.as_ref();

            if symbol_ref.symbol_type() == crate::codegraph::treesitter::structs::SymbolType::FunctionCall {
                let call_name = symbol_ref.name();
                let call_line = symbol_ref.full_range().start_point.row + 1;

                // 查找调用者函数
                if let Some(caller_id) = self._find_caller_function(file_path, call_line, function_ids, call_graph) {
                    // 查找被调用函数（先在本文件，再全局）
                    if let Some(callee_id) = self._find_callee_function(call_name, function_ids, call_graph) {
                        let relation = CallRelation::legacy(
                            *caller_id,
                            callee_id,
                            "".to_string(), // 会在add_call_relation中填充
                            call_name.to_string(),
                            file_path.clone(),
                            file_path.clone(),
                            call_line,
                            true,
                        );
                        // Phase P2b 收口（口径 B）：与 `CodeParser` 的刷新链路共用**同一个**
                        // 边写入入口 `PetCodeGraph::upsert_call_site_edges`（先删同调用点旧边
                        // 再写），因此全量/增量、legacy/layered 四条路径只有一套去重逻辑。
                        if call_graph
                            .upsert_call_site_edges(caller_id, call_line, call_name, vec![relation])
                            == 0
                        {
                            warn!("Failed to add call relation: {}", call_name);
                        }
                    } else {
                        // 未解析的调用
                        self._handle_unresolved_call(caller_id, call_name, file_path, call_line, call_graph);
                    }
                }
            }
        }

        Ok(())
    }

    /// Phase P2b 交付物 2（接线点④的 layered 分支）：增量刷新单个文件时按分层引擎产边。
    ///
    /// 与 `CodeParser::_analyze_file_calls_layered` 共用
    /// `resolution::{layered_caller_node, emit_layered_resolution}`，因此同一个调用点在
    /// "全量建图"与"增量刷新"两条链路上得到**完全相同**的边（含 4 个新标量）——
    /// 这正是交付物 4 的全量/增量一致性 harness 能成立的前提。
    ///
    /// Phase P2b 收口（口径 A）：ctx **优先取缓存**（`_extract_entities_from_file` 本轮已经
    /// 解析过该文件并顺手建好 ctx）；只有缓存里没有时才现场按需补建一次并回填缓存，
    /// 绝不为"喂索引"而重复解析同一个文件。改造前是"每次产边都重新解析 + 重建 ctx"，
    /// 于是一次 `refresh_file` 对同一个文件解析两遍，并且把哈希跳过的文件也拖回解析队列。
    /// 补建口径与 `CodeParser::ensure_file_scope_ctx` 逐参数一致
    /// （`parse_file_with_metadata` → `_detect_language` → `build_file_scope_ctx`）。
    fn _analyze_file_calls_layered(
        &mut self,
        file_path: &PathBuf,
        function_ids: &[Uuid],
        call_graph: &mut PetCodeGraph,
    ) -> Result<(), String> {
        // (1) ctx：缓存命中直接复用；未命中才补建一次并回填。
        let ctx = match self.file_scopes.get(file_path) {
            Some(cached) => cached.clone(),
            None => {
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
                built
            }
        };
        // (2) 调用点符号：只取 symbols（`parse_file` 就是 `parse_file_with_metadata(..)?.symbols`
        // 的转发，符号列表逐字节相同），不再重建 ctx。
        let symbols = self
            .ts_parser
            .parse_file(file_path)
            .map_err(|e| format!("Failed to parse file for call analysis: {:?}", e))?;

        // 本文件的函数节点（从图里按 id 取 —— 图是增量链路的唯一真相）。
        let funcs: Vec<FunctionInfo> = function_ids
            .iter()
            .filter_map(|id| call_graph.get_function_by_id(id))
            .cloned()
            .collect();

        let mut per_file: HashMap<PathBuf, std::sync::Arc<_>> = HashMap::new();
        per_file.insert(file_path.clone(), ctx.clone());
        let config = ResolutionConfig::all();
        let max_candidates = config.max_candidates.max(1) as usize;
        let index =
            build_resolution_index_from_graph(call_graph, &per_file, self.resolution_root.as_deref());

        let source = std::fs::read_to_string(file_path).ok();
        // P2a 口径①：生产路径必须走 `_with_source` 变体
        // （AST 的 `namespace` 对调用符号恒空，限定前缀只能从源码文本还原）。
        let sites = call_sites_from_symbols_with_source(&symbols, ctx.as_ref(), source.as_deref());

        for site in sites {
            let Some(caller) = resolution_caller_node(&site, Some(ctx.as_ref()), &funcs) else {
                continue;
            };
            let resolution = resolve_call_with_config(&site, Some(ctx.as_ref()), &index, &config);
            resolution_emit(&site, &caller, &resolution, call_graph, max_candidates);
        }

        Ok(())
    }

    /// 查找调用者函数
    fn _find_caller_function<'a>(&self, file_path: &PathBuf, call_line: usize, function_ids: &'a [Uuid], call_graph: &PetCodeGraph) -> Option<&'a Uuid> {
        // 根据行号范围查找包含调用行的函数
        for function_id in function_ids {
            if let Some(function) = call_graph.get_function_by_id(function_id) {
                if function.file_path == *file_path && 
                   call_line >= function.line_start && 
                   call_line <= function.line_end {
                    return Some(function_id);
                }
            }
        }
        None
    }

    /// 查找被调用函数
    ///
    /// Phase P2b 交付物 2（接线点④的第二段）：`layered` 下**不再**返回
    /// `find_functions_by_name().first()` 的同名塌缩结果，而是问分层引擎要最优候选；
    /// `legacy` 分支一字未改（红线 1）。
    ///
    /// ⚠ 完整产边并不经过这里：layered 的 `_analyze_file_calls` 已在函数入口分叉到
    /// [`IncrementalManager::_analyze_file_calls_layered`]（每候选一条边）。本方法只是
    /// 保留给"只能返回一个 id"的旧签名，防止有人绕过发射器直接调它时又落回 first-wins。
    fn _find_callee_function(&self, call_name: &str, function_ids: &[Uuid], call_graph: &PetCodeGraph) -> Option<Uuid> {
        if self.resolver_mode == ResolverMode::Layered {
            // 与 `CodeParser::_layered_callee_first` 同口径：索引一律来自
            // `build_resolution_index_from_graph`（不塌缩），这里**没有 ctx**
            // （本结构体不缓存），因此引擎自然下沉到不依赖 ctx 的层 —— 更保守，不是更错。
            let funcs: Vec<FunctionInfo> = function_ids
                .iter()
                .filter_map(|id| call_graph.get_function_by_id(id))
                .cloned()
                .collect();
            let file = funcs
                .first()
                .map(|f| f.file_path.clone())
                .unwrap_or_default();
            let language = funcs.first().map(|f| f.language.clone()).unwrap_or_default();
            let line = funcs.first().map(|f| f.line_start).unwrap_or(1);
            let per_file: HashMap<PathBuf, std::sync::Arc<_>> = HashMap::new();
            let index =
                build_resolution_index_from_graph(call_graph, &per_file, self.resolution_root.as_deref());
            let site = crate::codegraph::resolution::CallSite::new(&file, line, call_name, &language);
            let config = ResolutionConfig::all();
            let resolution = resolve_call_with_config(&site, None, &index, &config);
            return resolution
                .primary()
                .filter(|candidate| candidate.callee_id != uuid::Uuid::nil())
                .map(|candidate| candidate.callee_id);
        }

        // 先在本文件查找
        for &func_id in function_ids {
            if let Some(func) = call_graph.get_function_by_id(&func_id) {
                if func.name == call_name {
                    return Some(func_id);
                }
            }
        }

        // 再全局查找
        let global_functions = call_graph.find_functions_by_name(call_name);
        global_functions.first().map(|f| f.id)
    }

    /// 处理未解析的调用
    fn _handle_unresolved_call(
        &self,
        caller_id: &Uuid,
        call_name: &str,
        file_path: &PathBuf,
        call_line: usize,
        call_graph: &mut PetCodeGraph,
    ) {
        let relation = CallRelation::legacy(
            *caller_id,
            Uuid::new_v4(), // 临时ID
            "".to_string(),
            call_name.to_string(),
            file_path.clone(),
            file_path.clone(),
            call_line,
            false,
        );

        // Phase P2b 收口（口径 B）：幽灵边同样走唯一的边写入入口。
        if call_graph
            .upsert_call_site_edges(caller_id, call_line, call_name, vec![relation])
            == 0
        {
            warn!("Failed to add unresolved call relation: {}", call_name);
        }
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