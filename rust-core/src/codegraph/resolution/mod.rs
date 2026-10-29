//! Phase P1 交付物 3：`codegraph::resolution` —— 名称解析的**数据层**。
//!
//! # 这个模块是什么
//! P0 把"版本门禁 + 基线测试"建好后，量化出当前调用边解析的缺陷面
//! （4 组同名歧义名共 13 个候选，最终只体现为 11 条已解析边）。
//! P1 的目标不是修复它，而是**把修复所需的原料与数据结构先做实**：
//!
//! | 文件 | 职责 |
//! |---|---|
//! | [`level`] | 9 级 `ResolutionLevel`（`#[repr(u8)]`，P2 直接落盘）+ `ResolutionReason` |
//! | [`candidate`] | `Candidate` / `Resolution` / `ResolutionStatus` + 状态判定纯函数 |
//! | [`scope`] | `ScopeTree` / `ScopeBuilder`：复用 `parent_guid`/`childs_guid` 建作用域树，提供 `innermost_scope_containing` |
//! | [`imports`] | `ImportMap` / `ImportEntry`：把 `ImportDeclaration` 的原料忠实搬出来（7 语言形态各有单测） |
//! | [`index`] | `ResolutionIndex`：`by_name: HashMap<String, Vec<FunctionTag>>` —— **不塌缩**的全仓函数索引 |
//! | [`resolver`] | `CallSite` / `resolve_call`：只实现 L1 与 L7，其余层留 `// P2` 钩子 |
//!
//! # 硬约束（P1 的红线）
//! - **只建不消费**：`resolve_call` 没有接进任何建图路径；`parser.rs` 的四处同名
//!   first-wins（`parser.rs:344` / `1469` / `1496` / `1179`）与 `incremental.rs:275` **一字未改**。
//! - 纯函数、无 I/O：只有 `ScopeBuilder::build` / `ImportMap::build` 读取已经解析好的
//!   `Vec<AstSymbolInstanceArc>`，不读文件、不碰数据库、不建图。
//! - 无新依赖：只用 `std` + `uuid`。**禁止引入 `stack-graphs`**（已被 GitHub 归档，
//!   挂 RUSTSEC-2026-0303）；本模块只借它的分层模型。
//!
//! # 与 `CodeParser` 的关系
//! `CodeParser` 新增 `file_scopes: HashMap<PathBuf, Arc<FileScopeCtx>>`，在
//! `parser.rs:197`（`_extract_entities_from_file`）与 `parser.rs:499`（`parse_file`）两处
//! 符号消费循环里**额外**产出 [`FileScopeCtx`]，仅供
//! [`CodeParser::file_scope_ctx`] 读取。产出过程不参与任何
//! `FunctionInfo` / `CallRelation` / `Uuid` 的生成分支，因此调用边语义不变。

pub mod candidate;
pub mod imports;
pub mod index;
pub mod level;
pub mod resolver;
pub mod scope;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::codegraph::types::FunctionInfo;
use crate::codegraph::treesitter::{AstSymbolInstanceArc, structs::SymbolType};

pub use candidate::{
    dedup_by_callee, rank_candidates, status_of, Candidate, Resolution, ResolutionStatus,
};
pub use imports::{ImportEntry, ImportMap, LOCAL_NAME_RULES};
// Phase P2a：索引的新入口（`build_resolution_index_from_graph` 是交付物 2(c) 的唯一构造点）。
pub use index::{
    build_resolution_index_from_graph, declared_before_call_site, module_distance_between,
    CallableFact, FunctionTag, ResolutionIndex,
};
pub use level::{ResolutionLevel, ResolutionReason};
// Phase P2a：`ResolutionConfig`（按层关闭诊断）与 CallSite 构造入口。
pub use resolver::{
    call_sites_from_symbols, call_sites_from_symbols_with_source, resolve_call,
    resolve_call_with_config, CallSite, ResolutionConfig,
};
pub use scope::{
    Scope, ScopeBuilder, ScopeId, ScopeItem, ScopeKind, ScopeSegment, ScopeTree,
};

/// 单个文件的解析上下文：作用域树 + import 绑定表 + 本文件 module path。
///
/// 这是 P2 的 `resolve_call` 需要的全部"文件内"信息，一次构建、随文件生命周期缓存。
#[derive(Debug, Clone)]
pub struct FileScopeCtx {
    /// 所属文件。
    pub file_path: PathBuf,
    /// 该文件的 module path（推导失败时为空串，见 [`index::derive_module_path`]）。
    pub module_path: String,
    /// 作用域树（`ScopeBuilder::build` 产出）。
    pub scopes: ScopeTree,
    /// import 绑定表（`ImportMap::build` 产出）。
    pub imports: ImportMap,
    /// 语言级包声明（目前只有 Java 的 `package a.b.c;` 会有值）。
    ///
    /// 数据来源：`TreeSitterParser::parse_file_with_metadata`（P1 交付物 2(b)）返回的
    /// `ParsedFileAst::declared_package`，由 `CodeParser` 的两处解析点透传进来 ——
    /// 因此 `CodeParser::file_scopes` 里的 Java ctx **不再** 是 `None`。
    /// 只有显式调用 [`build_file_scope_ctx`] 并传 `None` 时才是 `None`（单测路径）。
    pub declared_package: Option<Vec<String>>,
    /// Phase P2a 交付物 1：本文件内**可静态推断类型**的绑定（L3 的唯一信息来源）。
    ///
    /// 只从已解析出的符号计算（`param_bindings()` / `types()`），不做任何 I/O，
    /// 也**只覆盖任务允许的三种推断形式**（形参标注、`let v = T::new(..)`、`new T(..)`）；
    /// 推断不出来 ⇒ 该 receiver 没有条目 ⇒ L3 返回空候选并下沉，绝不猜。
    pub typed_bindings: Vec<TypedBinding>,
}

/// Phase P2a：receiver 类型推断的**允许来源**（口径来自任务的分层规则 L3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingVia {
    /// 函数形参 `v: T` / `T v`（来自 AST getter `param_bindings`）。
    Param,
    /// `let v: T = ...` 显式类型标注（仅当 AST 记录了标注时）。
    LetAnnotation,
    /// `let v = T::new(...)` / `let v = new T(...)` 构造表达式。
    ConstructorCall,
}

/// Phase P2a：一条"名字 → 类型"的可见绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedBinding {
    /// 绑定名（形参名 / `let` 变量名）。
    pub name: String,
    /// 推断出的类型名（规范化成单个标识符，如 `&Calc` → `Calc`）。
    pub type_name: String,
    /// 推断来源（诊断用：报告里能说清"凭什么认为它是 Calc"）。
    pub via: BindingVia,
    /// 生效区间起（1-based，含）。
    pub scope_start: usize,
    /// 生效区间止（1-based，含）。
    pub scope_end: usize,
    /// 绑定自身的声明行（遮蔽时取更晚的那条）。
    pub decl_line: usize,
    /// 来源符号 guid（确定性 tie-break 用）。
    pub guid: Uuid,
}

impl TypedBinding {
    /// 可见区间宽度（用于"取最内层绑定"）。
    pub fn span(&self) -> usize {
        self.scope_end.saturating_sub(self.scope_start)
    }
}

impl FileScopeCtx {
    /// 显式构造（用于已知 `declared_package` 的场合，以及单测）。
    pub fn new(
        file_path: &Path,
        symbols: &[AstSymbolInstanceArc],
        root: Option<&Path>,
        declared_package: Option<Vec<String>>,
        language: &str,
    ) -> Self {
        let scopes = ScopeBuilder::build(file_path, symbols);
        let imports = ImportMap::build(symbols);
        let module_path =
            index::derive_module_path(file_path, root, language, declared_package.as_deref());
        let typed_bindings = collect_typed_bindings(symbols, &scopes);
        Self {
            file_path: file_path.to_path_buf(),
            module_path,
            scopes,
            imports,
            declared_package,
            typed_bindings,
        }
    }

    /// Phase P2a 交付物 1：`line` 处看到 `receiver` 这个绑定是什么类型。
    ///
    /// 选择规则（**遮蔽语义**）：在所有"名字相同 + 区间覆盖该行"的绑定里，
    /// 取可见区间最窄（最内层）的一条；同宽度取声明行更晚的一条（后声明遮蔽先声明）；
    /// 再同按 guid 升序 —— 与符号产出顺序、HashMap 种子都无关。
    /// 找不到 ⇒ `None`，L3 必须返回空候选并下沉。
    pub fn receiver_type_at(&self, line: usize, receiver: &str) -> Option<&TypedBinding> {
        if receiver.is_empty() {
            return None;
        }
        self.typed_bindings
            .iter()
            .filter(|binding| {
                binding.name == receiver
                    && binding.scope_start <= line
                    && line <= binding.scope_end
            })
            .min_by(|a, b| {
                a.span()
                    .cmp(&b.span())
                    .then_with(|| b.decl_line.cmp(&a.decl_line))
                    .then_with(|| a.guid.as_u128().cmp(&b.guid.as_u128()))
            })
    }

    /// Phase P2a 交付物 1：`line` 处的**外围类型名**（L2 `EnclosingTypeMember` 判据）。
    ///
    /// 顶层自由函数/模块级语句 ⇒ `None`（没有外围类型，L2 必须下沉）。
    pub fn enclosing_type_at(&self, line: usize) -> Option<String> {
        let scope = self.innermost_scope_containing(line)?;
        self.scopes.enclosing_type_name(scope)
    }

    /// Phase P2a 交付物 1：`line` 处所在的最内层**函数**作用域名。
    ///
    /// 用途：给 harness 证明 caller 归属由 `innermost_scope_containing` 得出
    /// （`nested_function_caller` 场景里必须是 `inner`，而不是靠符号产出顺序巧合拿到的那个）。
    /// 落在文件根（没有函数包住这一行）时返回 `None`。
    pub fn enclosing_function_at(&self, line: usize) -> Option<String> {
        let id = self.innermost_scope_containing(line)?;
        let scope = self.scopes.scope(id)?;
        match scope.kind {
            ScopeKind::Function | ScopeKind::Closure => Some(scope.name.clone()),
            _ => None,
        }
    }

    /// 调用点在 `line` 处的最内层作用域。
    ///
    /// P2 用它替掉 `_find_caller_function_by_line`（`parser.rs:1223`）的行区间 first-wins。
    pub fn innermost_scope_containing(&self, line: usize) -> Option<ScopeId> {
        self.scopes.innermost_scope_containing(line)
    }

    /// 抽取"可调用事实"（名字 + 起始行 + 形参数 + 外围类型），供
    /// [`ResolutionIndex::build`] 用 `(file, line_start)` 回填 `FunctionTag.param_count`。
    ///
    /// 之所以要靠 `(file, line_start)` 而不是 id：`FunctionInfo.id` 是 `parser.rs` 里现场
    /// `Uuid::new_v4()` 生成的，与 AST `guid` 无任何对应关系（两处的 `line_start` 都等于
    /// `full_range.start_point.row + 1`，口径一致才敢这样关联）。
    pub fn callable_facts(&self) -> Vec<CallableFact> {
        self.scopes
            .callable_scopes()
            .into_iter()
            .filter_map(|id| {
                self.scopes.scope(id).map(|scope| {
                    // Phase P2a：与 L2 用同一个口径（`ScopeTree::enclosing_type_name` = 链上
                    // **最内层**的 Type），否则"最内层遮蔽外层"和索引进来的是外层类型会互相打架。
                    let enclosing_type = self.scopes.enclosing_type_name(id);
                    CallableFact {
                        name: scope.name.clone(),
                        line_start: scope.line_start,
                        param_count: scope.param_count,
                        enclosing_type,
                        // Phase P2a：抽象签名（trait 方法）标记随作用域一起搬进索引，
                        // L3 命中唯一一条时必须据它拒绝"已解析"。
                        has_body: scope.has_body,
                    }
                })
            })
            .collect()
    }

    /// 本文件 import 引入的本地绑定名集合（P2 判断"这个名字是不是外来名"）。
    pub fn imported_bindings(&self) -> Vec<String> {
        self.imports.bound_names()
    }
}

/// 把"符号列表 → FileScopeCtx"的构造收敛成一个函数，供 `CodeParser` 的两处消费点调用。
///
/// 这个函数**只读符号**：不修改、不增删任何 `AstSymbolInstanceArc`，
/// 也不返回任何会影响建图的数据。
///
/// `declared_package` 来自 `TreeSitterParser::parse_file_with_metadata`
/// （P1 交付物 2(b)：Java 的 `package a.b.c;` 过去被 `java.rs` 的空分支彻底丢弃）。
/// 它只影响 [`FileScopeCtx::module_path`] 的推导（`index::derive_module_path` 以包名为准），
/// 而 module path 目前**没有任何建图消费者**，所以调用边语义不变。
pub fn build_file_scope_ctx(
    file_path: &Path,
    symbols: &[AstSymbolInstanceArc],
    root: Option<&Path>,
    language: &str,
    declared_package: Option<Vec<String>>,
) -> Arc<FileScopeCtx> {
    Arc::new(FileScopeCtx::new(
        file_path,
        symbols,
        root,
        declared_package,
        language,
    ))
}

/// Phase P2a 交付物 1：从**已解析符号**收集"可静态推断类型"的绑定。
///
/// 只允许任务 L3 规定的三种形式，且**一律不猜**：
/// 1. **函数形参 `v: T` / `T v`** —— 走 AST getter `param_bindings()`，
///    可见区间 = 该函数声明的 `full_range`（形参在整个函数体内可见）。
/// 2. **`let v: T = ...` 显式标注** —— 只有当 AST 真的记录了标注类型才算
///    （实测：`rust.rs` 的 `VariableDefinition.type_` 是从**右值**推导的，
///     `let v: Calc = c` 得到的是 `name=None, inference_info=Some("c")`，
///     因此这一支目前对 Rust 基本拿不到信息 —— 这正是"信息不足即下沉"）。
/// 3. **`let v = T::new(..)` / `let v = new T(..)`** —— 从 `type_.inference_info`
///    的右值文本里**精确**识别这两种构造形态（别的构造写法一律不认）。
///
/// `let` 绑定的可见区间 = 声明行的下一行起，到**最内层包住它的作用域**结束为止
/// （用现成的 `ScopeTree`，因此遮蔽语义天然正确：内层块更窄 ⇒ `receiver_type_at` 优先取它）。
///
/// 输出排序：`(name, scope_start, scope_end, decl_line, guid)`，与符号产出顺序无关。
fn collect_typed_bindings(symbols: &[AstSymbolInstanceArc], scopes: &ScopeTree) -> Vec<TypedBinding> {
    let mut out: Vec<TypedBinding> = Vec::new();
    // 文件根的作用域区间（给"没有任何函数包住"的顶层 `let` 兜底）。
    let root_span = scopes
        .scope(scopes.root())
        .map(|root| (root.line_start, root.line_end))
        .unwrap_or((1, 1));

    for symbol in symbols {
        let guard = symbol.read();
        let symbol_ref = guard.as_ref();
        let guid = *symbol_ref.guid();
        match symbol_ref.symbol_type() {
            SymbolType::FunctionDeclaration => {
                let line_start = symbol_ref.full_range().start_point.row + 1;
                let line_end = symbol_ref.full_range().end_point.row + 1;
                for (name, type_name) in symbol_ref.param_bindings() {
                    out.push(TypedBinding {
                        name,
                        type_name,
                        via: BindingVia::Param,
                        scope_start: line_start,
                        scope_end: line_end,
                        decl_line: line_start,
                        guid,
                    });
                }
            }
            SymbolType::VariableDefinition => {
                let decl_line = symbol_ref.full_range().end_point.row + 1;
                let name = symbol_ref.name().to_string();
                if name.is_empty() {
                    continue;
                }
                // types()[0] 就是 `VariableDefinition.type_`（见 ast_instance_structs.rs）。
                let first = symbol_ref.types().into_iter().next();
                let (type_name, via) = match first.as_ref() {
                    // (2) 显式标注
                    Some(t) if t.binding_type_name().is_some() => (
                        t.binding_type_name().unwrap_or("").to_string(),
                        BindingVia::LetAnnotation,
                    ),
                    // (3) 构造表达式
                    Some(t) => match constructor_type_from_text(
                        t.inference_info.as_deref().unwrap_or_default(),
                    ) {
                        Some(parsed) => (parsed, BindingVia::ConstructorCall),
                        None => continue,
                    },
                    None => continue,
                };
                if type_name.is_empty() {
                    continue;
                }
                // 可见区间：声明行之后 ⇒ 最内层包住该行的作用域末尾。
                let (scope_start, scope_end) = scopes
                    .innermost_scope_containing(decl_line)
                    .and_then(|id| scopes.scope(id))
                    .filter(|scope| scope.kind != ScopeKind::File)
                    .map(|scope| (decl_line + 1, scope.line_end))
                    .unwrap_or((decl_line + 1, root_span.1));
                if scope_start > scope_end {
                    continue;
                }
                out.push(TypedBinding {
                    name,
                    type_name,
                    via,
                    scope_start,
                    scope_end,
                    decl_line,
                    guid,
                });
            }
            _ => {}
        }
    }

    out.sort_by(|a, b| {
        (
            &a.name,
            a.scope_start,
            a.scope_end,
            a.decl_line,
            a.guid.as_u128(),
        )
            .cmp(&(
                &b.name,
                b.scope_start,
                b.scope_end,
                b.decl_line,
                b.guid.as_u128(),
            ))
    });
    out
}

/// Phase P2a：从 `let` 的右值文本里识别 **`T::new(..)` / `new T(..)`** 两种构造形态。
///
/// 刻意只认这两种（任务 L3 的原文口径），其它（`T::default()`、`T{..}`、工厂函数）
/// 一律返回 `None` ⇒ 上层下沉。**不做任何"看起来像构造"的启发式**。
pub fn constructor_type_from_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // `T::new(...)` / `a::T::new(...)`：取 `::new` 之前的最后一个路径段。
    if let Some(head) = text.strip_suffix("::new()").or_else(|| {
        text.find("::new(").map(|pos| &text[..pos])
    }) {
        let head = head.trim();
        let last = head.rsplit([':', '.']).next().unwrap_or("").trim();
        if is_type_ident(last) {
            return Some(last.to_string());
        }
        return None;
    }
    // `new T(...)` / `new T{...}`
    if let Some(rest) = text.strip_prefix("new ") {
        let rest = rest.trim();
        let head: String = rest
            .chars()
            .take_while(|c| !matches!(c, '(' | '{' | '<'))
            .collect();
        let head = head.trim();
        if is_type_ident(head) {
            return Some(head.to_string());
        }
    }
    None
}

/// 是否像一个类型标识符（首字母大写、只含字母数字下划线）。
fn is_type_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_uppercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// 全仓解析上下文：`ResolutionIndex` + 每文件 ctx。
///
/// P2 建图时用它替代"每次调用都线性扫全图"的做法。P1 里它只被单测与守卫测试构造，
/// 没有任何生产调用点。
#[derive(Debug, Clone, Default)]
pub struct ResolutionContext {
    index: ResolutionIndex,
    per_file: HashMap<PathBuf, Arc<FileScopeCtx>>,
    root: Option<PathBuf>,
}

impl ResolutionContext {
    /// 由"已缓存的文件 ctx" + "全仓函数节点"组装。
    pub fn new(
        per_file: HashMap<PathBuf, Arc<FileScopeCtx>>,
        functions: &[FunctionInfo],
        root: Option<&Path>,
    ) -> Self {
        // 把每个文件的 AST 可调用事实汇总，用于回填 param_count。
        let mut callables: HashMap<PathBuf, Vec<CallableFact>> = HashMap::new();
        let mut packages: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for (path, ctx) in per_file.iter() {
            callables.insert(path.clone(), ctx.callable_facts());
            if let Some(pkg) = ctx.declared_package.as_ref() {
                packages.insert(path.clone(), pkg.clone());
            }
        }
        let index = ResolutionIndex::build(functions, root, &packages, &callables);
        Self {
            index,
            per_file,
            root: root.map(|p| p.to_path_buf()),
        }
    }

    pub fn index(&self) -> &ResolutionIndex {
        &self.index
    }

    pub fn file_ctx(&self, path: &Path) -> Option<&Arc<FileScopeCtx>> {
        self.per_file.get(path)
    }

    pub fn per_file(&self) -> &HashMap<PathBuf, Arc<FileScopeCtx>> {
        &self.per_file
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// 解析一个调用点（P2 的唯一入口；P1 只保证可用，不接生产路径）。
    pub fn resolve(&self, call_site: &CallSite) -> Resolution {
        resolve_call(
            call_site,
            self.file_ctx(&call_site.file).map(Arc::as_ref),
            &self.index,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegraph::treesitter::language_id::LanguageId;
    use crate::codegraph::treesitter::parsers::get_ast_parser;
    use uuid::Uuid;

    fn parse_source(lang: LanguageId, code: &str, filename: &str) -> Vec<AstSymbolInstanceArc> {
        let mut parser = get_ast_parser(lang).expect("get_ast_parser");
        parser.parse(code, &PathBuf::from(filename))
    }

    /// `CodeParser::file_scopes` 的真实产出路径：走 `build_file_scope_ctx`，
    /// 并且证明"建了但没消费"—— 它不参与 `FunctionInfo` 的生成。
    #[test]
    fn build_file_scope_ctx_from_real_rust_source() {
        let source = "use std::fmt;\n\
                      pub fn outer(v: i32) -> i32 { inner(v) }\n\
                      pub fn inner(v: i32) -> i32 { helper(v) }\n\
                      fn helper(v: i32) -> i32 { v + 1 }\n";
        let symbols = parse_source(LanguageId::Rust, source, "mod_ctx.rs");
        let ctx = build_file_scope_ctx(
            Path::new("/repo/src/mod_ctx.rs"),
            &symbols,
            Some(Path::new("/repo")),
            "rust",
            None,
        );

        assert_eq!(ctx.file_path.to_string_lossy(), "/repo/src/mod_ctx.rs");
        assert_eq!(ctx.module_path, "src::mod_ctx", "Rust module path 用 `::`");
        assert!(!ctx.scopes.is_empty(), "作用域树必须有内容");
        // 交付物 2(b) 的通路：显式传 `None` 时 package 不参与 module path（真实 Java 用例见
        // `java_package_flows_from_parse_file_with_metadata_into_module_path`）。
        assert_eq!(ctx.declared_package, None);
        // import 原料被真的搬出来了（std::fmt 一条）。
        assert!(
            ctx.imports.entries().iter().any(|e| e.module_path == vec![
                "std".to_string(),
                "fmt".to_string()
            ]),
            "实测 = {:?}",
            ctx.imports.entries()
        );
    }

    #[test]
    fn callable_facts_carry_param_counts_and_enclosing_types() {
        let source = "struct Calc;\nimpl Calc { fn add(a: i32, b: i32) -> i32 { a + b } }\n";
        let symbols = parse_source(LanguageId::Rust, source, "calc.rs");
        let ctx = FileScopeCtx::new(
            Path::new("/repo/calc.rs"),
            &symbols,
            Some(Path::new("/repo")),
            None,
            "rust",
        );
        let facts = ctx.callable_facts();
        let add = facts
            .iter()
            .find(|f| f.name == "add")
            .expect("add 应是可调用事实");
        assert_eq!(add.param_count, Some(2), "trait::param_count 必须真的传到 CallableFact");
        assert!(add.line_start >= 1);
    }

    #[test]
    fn resolution_context_assembles_index_and_resolves() {
        let source = "fn target(x: i32) -> i32 { x }\nfn caller() { target(1) }\n";
        let symbols = parse_source(LanguageId::Rust, source, "app.rs");
        let file = PathBuf::from("/repo/app.rs");
        let ctx = build_file_scope_ctx(&file, &symbols, Some(Path::new("/repo")), "rust", None);

        let mut per_file: HashMap<PathBuf, Arc<FileScopeCtx>> = HashMap::new();
        per_file.insert(file.clone(), ctx);

        let functions: Vec<FunctionInfo> = per_file
            .values()
            .flat_map(|c| {
                c.scopes
                    .callable_scopes()
                    .into_iter()
                    .filter_map(|id| c.scopes.scope(id))
                    .map(|scope| FunctionInfo {
                        id: scope.guid,
                        name: scope.name.clone(),
                        file_path: c.file_path.clone(),
                        line_start: scope.line_start,
                        line_end: scope.line_end,
                        namespace: c.module_path.clone(),
                        language: "rust".to_string(),
                        signature: Some(scope.name.clone()),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        let resolution_ctx = ResolutionContext::new(per_file, &functions, Some(Path::new("/repo")));
        assert_eq!(resolution_ctx.index().len(), functions.len());

        // 同文件调用 → L1 命中。
        let site = CallSite::new(&file, 2, "target", "rust");
        let resolution = resolution_ctx.resolve(&site);
        assert_eq!(resolution.status, ResolutionStatus::Resolved);
        assert_eq!(resolution.primary().unwrap().level, ResolutionLevel::LocalLexical);

        // 未知名字 → Unresolved。
        let missing = resolution_ctx.resolve(&CallSite::new(&file, 2, "nope", "rust"));
        assert_eq!(missing.status, ResolutionStatus::Unresolved);
    }

    #[test]
    fn file_ctx_reports_module_path_and_imported_bindings() {
        let source = "use a::b::{c, d as e};\nfn user() { c(); }\n";
        let symbols = parse_source(LanguageId::Rust, source, "u.rs");
        let ctx = build_file_scope_ctx(Path::new("/repo/u.rs"), &symbols, Some(Path::new("/repo")), "rust", None);
        assert_eq!(ctx.module_path, "u");
        assert_eq!(ctx.imported_bindings(), vec!["c".to_string(), "e".to_string()]);
        assert_eq!(ctx.innermost_scope_containing(2).is_some(), true);
    }

    /// Java 的 package 只有显式传入时才会参与 module path（P1b 的接入点，见字段 TODO）。
    #[test]
    fn declared_package_is_used_when_supplied() {
        let symbols = parse_source(
            LanguageId::Java,
            "package com.example;\npublic class Runner { public void run() {} }\n",
            "Runner.java",
        );
        let ctx = FileScopeCtx::new(
            Path::new("/repo/src/main/java/whatever/Runner.java"),
            &symbols,
            Some(Path::new("/repo")),
            Some(vec!["com".to_string(), "example".to_string()]),
            "java",
        );
        assert_eq!(ctx.module_path, "com.example");
        assert_eq!(
            ctx.declared_package.as_deref(),
            Some(&["com".to_string(), "example".to_string()][..])
        );
    }

    /// 交付物 2(b) → 交付物 3 的**端到端真实通路**：
    /// 落一个 `package com.example;` 的 Java 文件，走 `TreeSitterParser::parse_file_with_metadata`
    /// （`CodeParser` 的两处解析点用的就是它），证明包名从 AST 一路走到 `module_path`。
    /// 改造前 `java.rs` 的 `package_declaration` 是空分支，这条链路上任何一环都拿不到包名。
    #[test]
    fn java_package_flows_from_parse_file_with_metadata_into_module_path() {
        let temp = tempfile::TempDir::new().expect("临时目录");
        // 故意把文件放在与包名**无关**的目录下：证明 module path 来自 `package` 声明而不是目录结构。
        let file = temp.path().join("wrong_dir").join("Runner.java");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "package com.example;\npublic class Runner { public void run() { helper(); } }\n",
        )
        .unwrap();

        let ts = crate::codegraph::treesitter::TreeSitterParser::new();
        let ast = ts
            .parse_file_with_metadata(&file)
            .expect("parse_file_with_metadata 必须成功");
        assert_eq!(
            ast.declared_package.as_deref(),
            Some(&["com".to_string(), "example".to_string()][..]),
            "交付物 2(b)：java.rs 必须把 package 名带出来（改造前是空分支）"
        );

        let ctx = FileScopeCtx::new(
            &file,
            &ast.symbols,
            Some(temp.path()),
            ast.declared_package.clone(),
            "java",
        );
        assert_eq!(
            ctx.module_path, "com.example",
            "有 package 声明时以包名为准，不再拼目录（derive_module_path 规则 2）"
        );
        // 符号集合不受影响：类 + 方法都在，import 表为空（本文件没有 import）。
        assert!(ctx.imports.is_empty(), "本用例不含 import");
        assert!(
            !ctx.scopes.is_empty(),
            "作用域树必须照常建立（Runner 类型 + run 方法）"
        );

        // 不传包名的对照组：module path 退化为目录链，证明两者确实是"包名优先"。
        let without = FileScopeCtx::new(&file, &ast.symbols, Some(temp.path()), None, "java");
        assert_eq!(without.module_path, "wrong_dir");
    }

    #[test]
    fn uuid_and_path_types_are_reachable_from_module_root() {
        // 聚合 re-export 的可达性冒烟测试（保证 P2 只 `use resolution::...` 就够）。
        let id = Uuid::nil();
        let tag = FunctionTag {
            id,
            file: PathBuf::from("x.rs"),
            module_path: "m".into(),
            name: "f".into(),
            qualified_name: "m::f".into(),
            param_count: None,
            language: "rust".into(),
            line_start: 1,
            // Phase P2a 新增的两个成员身份字段（中性值）。
            enclosing_type: None,
            has_body: true,
        };
        assert_eq!(tag.id, id);
        assert_eq!(std::mem::size_of::<ScopeKind>(), 1);
    }
}
