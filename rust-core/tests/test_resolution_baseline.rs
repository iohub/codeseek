//! Phase P0：真实 parser 链路的调用边**基线测试** + 消歧 fixture。
//!
//! ## 这些测试断言的是什么
//! 断言的是**当前真实行为**（包含错误行为），用来锁死基线、量化 P2 改造后的 precision 差值。
//! 每个缺陷点都用 `// KNOWN-DEFECT(P2 修复): 期望 …，当前 …` 标注，并且**同时断言"当前实际等于该错误值"**，
//! 所以本文件里的用例现在必须是全绿的（红线：`cargo test` 0 failed）。
//!
//! ## 为什么放集成测试而不是 `parser.rs` 内联测试
//! `src/lib.rs` 已 `pub mod codegraph;`，`CodeParser` / `PetCodeGraph` / `TreeSitterParser`
//! 都是公开 API，因此可以像真实 CLI 那样从 crate 外驱动 `build_petgraph_code_graph`，
//! 也不需要把测试塞进 2000 行的 `parser.rs` 里；同时避免与既有 `tests/parsers/` 那套
//! （`eq_symbols` 大面积失效的）参考 JSON 对比基建纠缠。
//!
//! ## 为什么每条断言都只数"数量 / 集合成员 / 确定性优先级"，不锁具体文件
//! `_build_function_name_index`（`HashMap` + `or_insert`）与
//! `PetCodeGraph::find_functions_by_name`（`Vec` first-wins）的取值顺序取决于
//! `HashMap` 的**每进程随机哈希种子**，同一个 fixture 在不同进程里可能选到不同候选。
//! 因此"猜中了哪一个"绝不能写进断言（否则测试本身就是 flaky 的）；
//! 基线锁的是：候选个数、边条数、`is_resolved` 标记、callee 名字、以及
//! "本文件优先 / 行区间 first-wins"这类**与迭代顺序无关**的确定性规则。
//!
//! ## 稳定性
//! 所有用例都通过 `CodeParser::set_force_reindex(true)` 建图（P0 交付物 2 的逃生口），
//! 保证每次都是干净的从零解析，不受 `~/.codeseek/projects/` 下的历史缓存影响；
//! 用例结束时会顺手清掉本次写到 home 的 MD5 哈希目录。

use std::path::{Path, PathBuf};

use codeseek::codegraph::parser::CodeParser;
use codeseek::codegraph::treesitter::{SymbolType, TreeSitterParser};
use codeseek::codegraph::types::{FunctionInfo, PetCodeGraph};

/// fixture 根目录（仓库内，便于直接肉眼审阅）
const FIXTURE_ROOT: &str = "tests/fixtures/resolution";

/// 一条调用边的规范化视图（`Uuid` 一律翻译成"文件名 + 函数名"，方便断言与报错）。
#[derive(Debug, Clone)]
struct EdgeView {
    /// caller 节点（未解析边的 caller 一定存在）
    caller: Option<FunctionInfo>,
    /// callee 节点；未解析边的 callee 是 parser 临时造的"幽灵节点"（namespace = "unresolved"）
    callee: Option<FunctionInfo>,
    callee_name: String,
    line: usize,
    is_resolved: bool,
}

impl EdgeView {
    fn caller_file(&self) -> String {
        self.caller.as_ref().map(file_name_of).unwrap_or_default()
    }

    fn callee_file(&self) -> String {
        self.callee.as_ref().map(file_name_of).unwrap_or_default()
    }
}

/// AST 符号的一行摘要（用来证明"原料层"到底丢掉了什么信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct AstRow {
    symbol_type: String,
    name: String,
    namespace: String,
    line: usize,
}

/// 一个 fixture case 的建图结果。持有 `TempDir` 以保证用例期间目录存活。
struct Fixture {
    _temp: tempfile::TempDir,
    project_dir: PathBuf,
    graph: PetCodeGraph,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // 清理 `build_petgraph_code_graph` 第 8 步写到 home 的增量哈希缓存，
        // 避免在 ~/.codeseek/projects 下留下一次性目录（只删本用例自己产生的项目 id）。
        let id_by_md5 = format!("{:x}", md5::compute(self.project_dir.to_string_lossy().as_bytes()));
        cleanup_codeseek_project(&id_by_md5);
        if let Some(name) = self.project_dir.file_name() {
            cleanup_codeseek_project(&name.to_string_lossy());
        }
    }
}

fn cleanup_codeseek_project(project_id: &str) {
    let Some(home) = dirs_home() else { return };
    let dir = home.join(".codeseek").join("projects").join(project_id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl Fixture {
    /// 把 fixture case 复制进一个带唯一后缀的临时目录，然后走**真实 parser 全量链路**建图。
    ///
    /// 复制（而不是原地扫描仓库目录）有两个好处：
    /// 1. 目录名唯一，绝不会与用户真实项目的 project_id 撞名；
    /// 2. 不污染仓库目录（`build_petgraph_code_graph` 只读，但仍避免任何写风险）。
    fn build(case: &str) -> Self {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_ROOT).join(case);
        assert!(src.is_dir(), "fixture case 不存在: {}", src.display());

        let temp = tempfile::TempDir::new().expect("创建临时目录失败");
        let project_dir = temp
            .path()
            .join(format!("p0baseline_{}_{}", case, std::process::id()));
        copy_tree(&src, &project_dir);

        let mut parser = CodeParser::new();
        // 交付物 2 的逃生口：跳过一切缓存，保证"每次都是干净的全量解析"
        parser.set_force_reindex(true);
        let graph = parser
            .build_petgraph_code_graph(&project_dir)
            .unwrap_or_else(|e| panic!("case `{case}` 建图失败: {e}"));

        Self {
            _temp: temp,
            project_dir,
            graph,
        }
    }

    /// 本 case 内的某个源文件的绝对路径（按文件名在临时目录里递归查）。
    fn source_path(&self, file_name: &str) -> PathBuf {
        let mut found = Vec::new();
        collect_sources(&self.project_dir, &mut found);
        found
            .into_iter()
            .find(|path| path.file_name().map(|n| n.to_string_lossy() == file_name).unwrap_or(false))
            .unwrap_or_else(|| panic!("fixture 里找不到文件 {file_name}"))
    }

    /// 直接走 AST 层（`TreeSitterParser`），拿到 parser 改造前/后都需要的"原料"视图。
    fn ast_rows(&self, file_name: &str) -> Vec<AstRow> {
        let path = self.source_path(file_name);
        let parser = TreeSitterParser::new();
        let symbols = parser
            .parse_file(&path)
            .unwrap_or_else(|e| panic!("解析 {} 失败: {:?}", path.display(), e));

        let mut rows: Vec<AstRow> = symbols
            .iter()
            .map(|symbol| {
                let guard = symbol.read();
                let s = guard.as_ref();
                AstRow {
                    symbol_type: format!("{:?}", s.symbol_type()),
                    name: s.name().to_string(),
                    namespace: s.namespace().to_string(),
                    line: s.full_range().start_point.row + 1,
                }
            })
            .collect();
        rows.sort_by(|a, b| (&a.symbol_type, &a.name, a.line).cmp(&(&b.symbol_type, &b.name, b.line)));
        rows
    }

    fn find_row(rows: &[AstRow], symbol_type: SymbolType, line: usize) -> Option<AstRow> {
        let wanted = format!("{:?}", symbol_type);
        rows.iter().find(|row| row.symbol_type == wanted && row.line == line).cloned()
    }

    /// 图里全部函数节点（按 文件名 + 行号 排序，稳定）。
    fn all_functions(&self) -> Vec<FunctionInfo> {
        let mut rows: Vec<FunctionInfo> = self.graph.get_all_functions().into_iter().cloned().collect();
        rows.sort_by(|a, b| {
            file_name_of(a)
                .cmp(&file_name_of(b))
                .then(a.line_start.cmp(&b.line_start))
                .then_with(|| a.name.cmp(&b.name))
        });
        rows
    }

    /// 按名字取函数节点（`function_names` 索引里同名候选的全集 —— 歧义的直接度量）。
    fn candidates(&self, name: &str) -> Vec<FunctionInfo> {
        let mut rows: Vec<FunctionInfo> = self
            .graph
            .function_names
            .get(name)
            .map(|ids| ids.iter().filter_map(|id| self.graph.get_function_by_id(id)).cloned().collect())
            .unwrap_or_default();
        rows.sort_by(|a, b| file_name_of(a).cmp(&file_name_of(b)).then(a.line_start.cmp(&b.line_start)));
        rows
    }

    /// 某个文件里的函数节点，顺序 = `file_functions` 里的写入顺序
    /// （即 AST 符号产出顺序）—— 用来复现 caller 归属的 first-wins 语义。
    fn functions_of_file_in_graph_order(&self, file_name: &str) -> Vec<FunctionInfo> {
        let key = self
            .graph
            .file_functions
            .keys()
            .find(|path| path.file_name().map(|n| n.to_string_lossy() == file_name).unwrap_or(false))
            .cloned();
        match key {
            Some(key) => self
                .graph
                .file_functions
                .get(&key)
                .map(|ids| ids.iter().filter_map(|id| self.graph.get_function_by_id(id)).cloned().collect())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// 全部边（排序稳定）。
    fn edges(&self) -> Vec<EdgeView> {
        let mut rows: Vec<EdgeView> = self
            .graph
            .graph
            .edge_references()
            .map(|edge| {
                let rel = edge.weight();
                EdgeView {
                    caller: self.graph.get_function_by_id(&rel.caller_id).cloned(),
                    callee: self.graph.get_function_by_id(&rel.callee_id).cloned(),
                    callee_name: rel.callee_name.clone(),
                    line: rel.line_number,
                    is_resolved: rel.is_resolved,
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            caller_key(a)
                .cmp(&caller_key(b))
                .then(a.line.cmp(&b.line))
                .then_with(|| a.callee_name.cmp(&b.callee_name))
        });
        rows
    }

    /// 指定 caller 函数名发出的所有边。
    fn edges_from(&self, caller_name: &str) -> Vec<EdgeView> {
        self.edges().into_iter().filter(|edge| edge.caller.as_ref().map(|f| f.name == caller_name).unwrap_or(false)).collect()
    }

    /// 行区间覆盖 `line` 的本文件函数个数 —— 大于 1 就说明 caller 归属存在歧义。
    fn covering_functions(&self, file_name: &str, line: usize) -> Vec<FunctionInfo> {
        self.functions_of_file_in_graph_order(file_name)
            .into_iter()
            .filter(|f| line >= f.line_start && line <= f.line_end)
            .collect()
    }

    /// `_find_caller_function_by_line` 的语义复现：按图内写入顺序取**第一个**覆盖该行的函数。
    /// 这是"first-wins（不是最内层优先）"的确定性定义，P2 若改成最内层优先，本函数与基线断言都要同步更新。
    fn first_covering(&self, file_name: &str, line: usize) -> Option<FunctionInfo> {
        self.covering_functions(file_name, line).into_iter().next()
    }
}

/// 排序键：(caller 文件名, caller 函数名)。
fn caller_key(edge: &EdgeView) -> (String, String) {
    match &edge.caller {
        Some(caller) => (file_name_of(caller), caller.name.clone()),
        None => (String::new(), String::new()),
    }
}

fn file_name_of(function: &FunctionInfo) -> String {
    function
        .file_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| function.file_path.to_string_lossy().to_string())
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("创建目标目录失败");
    for entry in std::fs::read_dir(src).expect("读取 fixture 目录失败").flatten() {
        let target = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("复制 fixture 文件失败");
        }
    }
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("读取目录失败").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs") | Some("py") | Some("js") | Some("ts") | Some("java") | Some("go") | Some("c") | Some("cpp")
        ) {
            out.push(path);
        }
    }
}

// ============================================================================================
// 场景 1：跨文件同名函数 —— `helper()` 有两个同名定义，调用点所在文件自己没有定义
// ============================================================================================

#[test]
fn test_baseline_cross_file_same_name_is_first_wins_and_declared_resolved() {
    let fixture = Fixture::build("cross_file_same_name");

    // --- 结构事实：两份同名定义都在图里，且都带着自己的文件归属 ---
    let helper_candidates = fixture.candidates("helper");
    assert_eq!(
        helper_candidates.len(),
        2,
        "helper 应当有 alpha.rs / beta.rs 两份同名定义（当前图里确实都保留了候选）"
    );
    let mut helper_files: Vec<String> = helper_candidates.iter().map(file_name_of).collect();
    helper_files.sort();
    assert_eq!(helper_files, vec!["alpha.rs".to_string(), "beta.rs".to_string()]);

    // --- 确定性对照组：本文件有同名定义时走"第 1 级：本文件 first-wins"，结果稳定 ---
    let alpha_edges = fixture.edges_from("local_helper_call");
    assert_eq!(alpha_edges.len(), 1, "local_helper_call 只有 1 个调用点");
    assert_eq!(alpha_edges[0].callee_file(), "alpha.rs", "本文件优先应当命中 alpha.rs::helper");
    assert!(alpha_edges[0].is_resolved);

    let beta_edges = fixture.edges_from("other_beta_work");
    assert_eq!(beta_edges.len(), 1);
    assert_eq!(beta_edges[0].callee_file(), "beta.rs", "本文件优先应当命中 beta.rs::helper");
    assert!(beta_edges[0].is_resolved);

    // --- KNOWN-DEFECT ---
    // KNOWN-DEFECT(P2 修复): 期望 `gamma.rs::call_helper -> helper` 保留 **2 个候选**
    //   （alpha.rs::helper / beta.rs::helper）并标记为"歧义"；
    //   当前实际是**只产出 1 条边、callee 取 HashMap 迭代顺序里的第一个、且 is_resolved=true**
    //   （等于把"猜的"宣称为已解析）。
    //   注意：这里刻意不断言具体命中哪个文件 —— 那取决于每进程随机的 HashMap 种子，
    //   写死就会变成 flaky 测试；锁"候选数 2 / 边数 1 / 已解析标记为真"这三件事即可量化改造收益。
    let gamma_edges = fixture.edges_from("call_helper");
    assert_eq!(
        gamma_edges.len(),
        1,
        "当前实现只会从 2 个同名候选里挑 1 个（歧义被压成单边）：实测边 = {gamma_edges:?}"
    );
    let edge = &gamma_edges[0];
    assert_eq!(edge.callee_name, "helper");
    assert!(edge.is_resolved, "跨文件同名歧义目前被标记成 is_resolved=true（P2 后应不再是 true 或应保留多候选）");
    assert_eq!(edge.caller_file(), "gamma.rs");
    assert!(
        edge.callee_file() == "alpha.rs" || edge.callee_file() == "beta.rs",
        "callee 必须是两个同名候选之一，实际 = {}",
        edge.callee_file()
    );
    assert_ne!(edge.callee_file(), "gamma.rs", "gamma.rs 本身没有 helper 定义，这条边必然是跨文件的");

    // --- 原料层证据：调用符号连"来自哪个文件/哪个作用域"都没有 ---
    let rows = fixture.ast_rows("gamma.rs");
    let call = Fixture::find_row(&rows, SymbolType::FunctionCall, 6).expect("gamma.rs 第 6 行应有 FunctionCall 符号");
    assert_eq!(call.name, "helper");
    assert_eq!(
        call.namespace, "",
        "FunctionCall 的 namespace 为空 —— 裸名调用没有任何作用域/导入信息可供消歧"
    );

    // --- 量化基线：3 个调用点、3 条边、全部声称已解析 ---
    let stats = fixture.graph.get_stats();
    assert_eq!(fixture.edges().len(), 3);
    assert_eq!(stats.resolved_calls, 3, "整个 case 的 3 条边全被标记为已解析，包括那个真正的歧义点");
    assert_eq!(stats.unresolved_calls, 0);
}

// ============================================================================================
// 场景 2：Python import 别名 —— `from mod_x import func as alias` 后调用 `alias()`
// ============================================================================================

#[test]
fn test_baseline_python_import_alias_is_not_consumed() {
    let fixture = Fixture::build("python_import_alias");

    // --- 原料层：ImportDeclaration 符号确实存在（P2 就是用它做消歧），但当前链路完全不消费它 ---
    let rows = fixture.ast_rows("consumer.py");
    let import = Fixture::find_row(&rows, SymbolType::ImportDeclaration, 6)
        .expect("`from mod_x import func as alias` 必须产出一条 ImportDeclaration 符号");
    assert_eq!(
        import.name, "func",
        "AstSymbolInstance trait 只暴露通用 getter：能看到被导入的名字，但 alias / path_components / \
         import_type / filepath_ref 都拿不到（要靠 downcast），这条断言同时锁住\"改造要补的接口缺口\""
    );
    let call = Fixture::find_row(&rows, SymbolType::FunctionCall, 10).expect("第 10 行应有 FunctionCall 符号");
    assert_eq!(call.name, "alias", "调用符号里只剩别名，与\"真实函数叫 func\"之间没有任何可查的链接");

    // --- 函数节点：真实定义在图里，别名不是函数 ---
    let func_candidates = fixture.candidates("func");
    assert_eq!(func_candidates.len(), 1);
    assert_eq!(file_name_of(&func_candidates[0]), "mod_x.py");
    assert_eq!(
        fixture.candidates("alias").len(),
        1,
        "图里那个 `alias` 节点是未解析调用临时造的幽灵节点，不是真实定义"
    );
    let ghost = &fixture.candidates("alias")[0];
    assert_eq!(ghost.namespace, "unresolved", "未解析调用的临时节点用 namespace=\"unresolved\" 标记");
    assert_eq!((ghost.line_start, ghost.line_end), (10, 10), "临时节点的行区间被压成调用行本身");

    // --- KNOWN-DEFECT ---
    // KNOWN-DEFECT(P2 修复): 期望 `consumer.py::call_alias -> mod_x.py::func`（沿 import + 别名解析，
    //   is_resolved=true，callee_file=mod_x.py）；
    //   当前实际是 `call_alias --alias (is_resolved=false)--> 本文件里的临时幽灵节点 alias`，
    //   并且 `mod_x.py::func` **一条 caller 都没有**。
    let edges = fixture.edges();
    assert_eq!(edges.len(), 1, "本 case 只有 1 个调用点，只产出 1 条边：实测 = {edges:?}");
    let edge = &edges[0];
    assert_eq!(edge.caller_file(), "consumer.py");
    assert_eq!(edge.callee_name, "alias");
    assert!(!edge.is_resolved, "别名解析失败 → 未解析边（当前行为，必须锁住）");
    assert_eq!(edge.callee_file(), "consumer.py", "幽灵节点被造在调用点所在文件");

    let resolved_into_func = fixture
        .edges()
        .into_iter()
        .filter(|e| e.is_resolved && e.callee.as_ref().map(|c| c.name == "func").unwrap_or(false))
        .count();
    assert_eq!(resolved_into_func, 0, "mod_x.py::func 目前没有任何 caller —— 这就是缺陷的可量化口径");

    let stats = fixture.graph.get_stats();
    assert_eq!(stats.resolved_calls, 0);
    assert_eq!(stats.unresolved_calls, 1);
}

// ============================================================================================
// 场景 3：Rust 限定名 `mod_a::pick(x)` —— AST 层就丢掉限定前缀，退化得与裸名一模一样
// ============================================================================================

#[test]
fn test_baseline_rust_qualified_path_degrades_to_bare_name() {
    let fixture = Fixture::build("rust_qualified_path");

    // --- 原料层：`mod_a::pick(x)` 的 FunctionCall 名字是裸名 `pick`，namespace 为空 ---
    let rows = fixture.ast_rows("qualifier.rs");
    let call = Fixture::find_row(&rows, SymbolType::FunctionCall, 9)
        .expect("`mod_a::pick(x)` 第 9 行应产出 FunctionCall 符号");
    assert_eq!(
        call.name, "pick",
        "`::` 限定名被切成最后一段（rust.rs 的 scoped_identifier 分支），前缀 `mod_a` 不在 name 里"
    );
    assert_eq!(
        call.namespace, "",
        "本应存放限定前缀的 namespace 是空的：`_resolve_qualified_function_name` 只能按 `.` 切分，\
         对 Rust 的 `::` 完全无能为力，而这里连前缀信息都没传下来"
    );

    // --- 两个同名候选都在图里 ---
    let pick_candidates = fixture.candidates("pick");
    assert_eq!(pick_candidates.len(), 2, "mod_a.rs::pick 与 mod_b.rs::pick 是两份互不相同的定义");
    let mut pick_files: Vec<String> = pick_candidates.iter().map(file_name_of).collect();
    pick_files.sort();
    assert_eq!(pick_files, vec!["mod_a.rs".to_string(), "mod_b.rs".to_string()]);

    // --- 确定性对照组：本文件内调用 `pick(x)` 走"本文件优先"，稳定命中 mod_b.rs::pick ---
    let local = fixture.edges_from("pick_in_this_file");
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].callee_file(), "mod_b.rs");
    assert!(local[0].is_resolved);

    // --- KNOWN-DEFECT ---
    // KNOWN-DEFECT(P2 修复): 期望 `qualifier.rs::call_qualified -> mod_a.rs::pick`
    //   （限定前缀参与作用域查找）；
    //   当前实际是：前缀在 AST 层丢失 → 与裸名同名调用走同一套 first-wins，
    //   只产出 1 条 is_resolved=true 的边，命中 mod_a.rs 还是 mod_b.rs 取决于 HashMap 迭代顺序。
    let qualified = fixture.edges_from("call_qualified");
    assert_eq!(qualified.len(), 1, "限定名调用同样被压成单边：实测 = {qualified:?}");
    let edge = &qualified[0];
    assert_eq!(edge.callee_name, "pick", "边里记的是裸名，限定信息已经彻底丢失");
    assert!(edge.is_resolved, "当前把\"猜的\"标成已解析（P2 后应基于前缀确定，或保留 2 个候选）");
    assert!(
        edge.callee_file() == "mod_a.rs" || edge.callee_file() == "mod_b.rs",
        "callee 必须是两个同名候选之一，实际 = {}",
        edge.callee_file()
    );

    let stats = fixture.graph.get_stats();
    assert_eq!(fixture.edges().len(), 2);
    assert_eq!(stats.resolved_calls, 2);
    assert_eq!(stats.unresolved_calls, 0);
}

// ============================================================================================
// 场景 4：Python 点号限定名 `mod_y.helper(value)` —— 前缀只降级成一个 VariableUsage
// ============================================================================================

#[test]
fn test_baseline_python_dotted_qualified_name_ignores_prefix() {
    let fixture = Fixture::build("python_dotted_qualified");

    // --- 原料层：调用符号 name=helper（裸名），前缀 mod_y 变成一个变量使用符号 ---
    let rows = fixture.ast_rows("dotted_consumer.py");
    let call = Fixture::find_row(&rows, SymbolType::FunctionCall, 10).expect("第 10 行应有 FunctionCall 符号");
    assert_eq!(call.name, "helper");
    assert_eq!(call.namespace, "", "点号前的模块名没有进入 namespace");
    assert!(
        Fixture::find_row(&rows, SymbolType::VariableUsage, 10).is_some(),
        "receiver/前缀在 AST 层只剩一个 VariableUsage 符号（与参数 value 同级），解析链路完全不读它"
    );
    assert!(
        Fixture::find_row(&rows, SymbolType::ImportDeclaration, 5).is_some()
            && Fixture::find_row(&rows, SymbolType::ImportDeclaration, 6).is_some(),
        "`import mod_y` / `import other_mod` 两条 ImportDeclaration 都产出了，但没有被消费"
    );

    // --- 两个同名候选 ---
    let helper_candidates = fixture.candidates("helper");
    assert_eq!(helper_candidates.len(), 2, "mod_y.py::helper 与 other_mod.py::helper 同名");
    let mut helper_files: Vec<String> = helper_candidates.iter().map(file_name_of).collect();
    helper_files.sort();
    assert_eq!(helper_files, vec!["mod_y.py".to_string(), "other_mod.py".to_string()]);

    // --- KNOWN-DEFECT ---
    // KNOWN-DEFECT(P2 修复): 期望 `dotted_consumer.py::call_dotted -> mod_y.py::helper`
    //   （由 import + 前缀唯一确定）；
    //   当前实际是：前缀不参与判定，两个候选里 first-wins 取一个并标 is_resolved=true。
    //   `_resolve_qualified_function_name` 的 `func.namespace.contains(prefix) || func.name == method_name`
    //   里后半条恒真，即使走到第 4 级也等于没校验 —— 这里用"候选数 2 / 单边 / 已解析"锁住该事实。
    let dotted = fixture.edges_from("call_dotted");
    assert_eq!(dotted.len(), 1, "实测 = {dotted:?}");
    let edge = &dotted[0];
    assert_eq!(edge.callee_name, "helper", "记录的是裸名 helper 而不是 mod_y.helper");
    assert!(edge.is_resolved);
    assert!(
        edge.callee_file() == "mod_y.py" || edge.callee_file() == "other_mod.py",
        "callee 必须是两个同名候选之一，实际 = {}",
        edge.callee_file()
    );

    let stats = fixture.graph.get_stats();
    assert_eq!(fixture.edges().len(), 1);
    assert_eq!(stats.resolved_calls, 1);
    assert_eq!(stats.unresolved_calls, 0);
}

// ============================================================================================
// 场景 5：方法调用的 receiver 被丢弃 —— `calc.add(x)` 与自由函数 `add(x)` 无法区分
// ============================================================================================

#[test]
fn test_baseline_method_receiver_is_dropped_before_resolution() {
    let fixture = Fixture::build("method_receiver");

    // --- 原料层：`calc.add(x)` 的调用符号只剩裸名 `add`；receiver 变成一个 VariableUsage + caller_guid ---
    let rows = fixture.ast_rows("receiver_user.rs");
    let call = Fixture::find_row(&rows, SymbolType::FunctionCall, 11).expect("第 11 行应有 FunctionCall 符号");
    assert_eq!(call.name, "add", "rust.rs 的 field_expression 分支只取字段名当调用名");
    assert_eq!(call.namespace, "", "namespace 只有 scoped_identifier 分支才会写；方法调用这里恒为空");
    let receiver_usage = Fixture::find_row(&rows, SymbolType::VariableUsage, 11)
        .expect("receiver 应当以 VariableUsage 的形式存在于 AST 里（源码注释自称 dirty hack）");
    assert_eq!(
        receiver_usage.name, "calc",
        "receiver 信息其实**还在 AST 里**，只是解析链路完全没读它 —— P2 可以从这里取回"
    );
    assert!(
        Fixture::find_row(&rows, SymbolType::ImportDeclaration, 8).is_some(),
        "`use crate::calc::Calc;` 产出了 ImportDeclaration（其 name 为空串，说明 trait 层还没有 import 专用 getter）"
    );

    // --- 两个同名候选：Calc 的方法与自由函数 ---
    let add_candidates = fixture.candidates("add");
    assert_eq!(add_candidates.len(), 2, "calc.rs::add（方法）与 misc.rs::add（自由函数）同名");
    let mut add_files: Vec<String> = add_candidates.iter().map(file_name_of).collect();
    add_files.sort();
    assert_eq!(add_files, vec!["calc.rs".to_string(), "misc.rs".to_string()]);

    // --- KNOWN-DEFECT ---
    // KNOWN-DEFECT(P2 修复): 期望 `receiver_user.rs::use_method -> calc.rs::add`（按 receiver 类型消歧）；
    //   当前实际是：调用符号没有 receiver 信息，两个同名候选 first-wins 取一个并标 is_resolved=true；
    //   第 4 级限定名解析对 Rust 根本不生效（name 里既没有 `.` 也没有 `::`）。
    let method_edges = fixture.edges_from("use_method");
    assert_eq!(method_edges.len(), 1, "实测 = {method_edges:?}");
    let edge = &method_edges[0];
    assert_eq!(edge.callee_name, "add", "边里记的是裸名 add，看不出这是一次方法调用");
    assert!(edge.is_resolved);
    assert!(
        edge.callee_file() == "calc.rs" || edge.callee_file() == "misc.rs",
        "callee 必须是两个同名候选之一，实际 = {}",
        edge.callee_file()
    );

    // `impl Calc` 自身被解析成两份 StructDeclaration，但 ClassInfo 不进调用图，
    // 所以 `add` 的 namespace 也不含类型名（这里只锁住"图里没有类型信息可用于消歧"这一事实）。
    let stats = fixture.graph.get_stats();
    assert_eq!(fixture.edges().len(), 1);
    assert_eq!(stats.resolved_calls, 1);
    assert_eq!(stats.unresolved_calls, 0);
}

// ============================================================================================
// 场景 6：嵌套函数 / 闭包的 caller 归属
// ============================================================================================

#[test]
fn test_baseline_nested_function_caller_uses_line_range_first_wins() {
    let fixture = Fixture::build("nested_function_caller");

    // --- 行区间重叠：第 12 行的调用同时落在 inner 和 outer 两个函数里 ---
    let covering = fixture.covering_functions("nested.rs", 12);
    let mut covering_names: Vec<String> = covering.iter().map(|f| f.name.clone()).collect();
    covering_names.sort();
    assert_eq!(
        covering_names,
        vec!["inner".to_string(), "outer".to_string()],
        "caller 归属存在真实歧义：inner(11..13) 与 outer(10..16) 都覆盖第 12 行"
    );

    // --- KNOWN-DEFECT（语义锁）---
    // KNOWN-DEFECT(P2 修复): 期望 caller = **最内层**包含该行的函数（inner）；
    //   当前实现 `_find_caller_function_by_line` 是"按本文件函数列表顺序，第一个行区间覆盖即返回"，
    //   即 first-wins，完全没有"取最小覆盖区间"的概念。
    //   本 fixture 里实测 caller 恰好是 inner —— 那是因为 tree-sitter 先产出内层符号、
    //   后产出外层符号（`symbols.extend(children)` 再 `push(decl)`），**属于巧合而不是保证**；
    //   下面这条 first_covering 断言才是把"first-wins 语义"钉死的地方：
    //   P2 改成最内层优先后，这条断言必须以"改成 innermost 判定"的方式更新，而不是删掉。
    let leaf_edges = fixture.edges_from("inner");
    assert_eq!(leaf_edges.len(), 1, "实测 = {leaf_edges:?}");
    let expected_caller = fixture
        .first_covering("nested.rs", 12)
        .expect("第 12 行必然被至少一个函数覆盖");
    assert_eq!(
        expected_caller.name, "inner",
        "first-wins 的取值就是 inner（顺序巧合）；若改成最内层优先，这里应与实际 caller 一致但推导方式不同"
    );
    let edge = &leaf_edges[0];
    assert_eq!(edge.line, 12);
    assert_eq!(edge.callee_name, "leaf");
    assert!(edge.is_resolved);
    assert_eq!(edge.callee_file(), "nested.rs");

    // --- 对照组：outer 调用 inner 是"本文件同名 first-wins"，结果稳定正确 ---
    let inner_edges = fixture.edges_from("outer");
    assert_eq!(inner_edges.len(), 1);
    assert_eq!(inner_edges[0].callee_name, "inner");
    assert!(inner_edges[0].is_resolved);

    // --- KNOWN-DEFECT（闭包）---
    // KNOWN-DEFECT(P2 修复): 期望 `closure_user -> 该闭包`（闭包是可调用实体，至少应挂到定义它的外层函数）；
    //   当前实际是：`let doubled = |y| y * 2;` 只产出 VariableDefinition 符号，**不是** FunctionDeclaration，
    //   于是 `doubled(x)` 解析失败，parser 造出一个 namespace="unresolved"、
    //   行区间被压成调用行(20..20) 的**幽灵函数节点** + 一条 is_resolved=false 的边。
    let closure_edges = fixture.edges_from("closure_user");
    assert_eq!(closure_edges.len(), 1, "实测 = {closure_edges:?}");
    let closure_edge = &closure_edges[0];
    assert_eq!(closure_edge.callee_name, "doubled");
    assert!(!closure_edge.is_resolved, "闭包调用目前是未解析边");
    assert_eq!(closure_edge.line, 20);
    let doubled_candidates = fixture.candidates("doubled");
    assert_eq!(doubled_candidates.len(), 1, "图里名为 doubled 的只有那个幽灵节点");
    assert_eq!(doubled_candidates[0].namespace, "unresolved");
    assert_eq!((doubled_candidates[0].line_start, doubled_candidates[0].line_end), (20, 20));

    // --- 量化基线：4 个真实函数 + 1 个幽灵节点，3 条边（2 已解析 / 1 未解析） ---
    assert_eq!(fixture.all_functions().len(), 5);
    assert_eq!(fixture.edges().len(), 3);
    let stats = fixture.graph.get_stats();
    assert_eq!(stats.resolved_calls, 2);
    assert_eq!(stats.unresolved_calls, 1);
}

// ============================================================================================
// 场景 7：同名但应判为歧义 —— trait 声明 + 两个 impl + 两个 inherent 同名方法
// ============================================================================================

#[test]
fn test_baseline_same_name_trait_impls_are_collapsed_into_one_edge() {
    let fixture = Fixture::build("ambiguous_trait_impl");

    // --- 5 个同名 `run` 候选（trait 声明 / Fast / Slow / Loud / Loudish），图里都留着 ---
    let run_candidates = fixture.candidates("run");
    assert_eq!(run_candidates.len(), 5, "run 有 5 份互不相同的定义");
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for candidate in &run_candidates {
        *counts.entry(file_name_of(candidate)).or_insert(0) += 1;
    }
    let per_file: Vec<(String, usize)> = counts.into_iter().collect();
    assert_eq!(
        per_file,
        vec![("driver.rs".to_string(), 1usize), ("runners.rs".to_string(), 4usize)],
        "runners.rs 里是 trait 声明 + 2 个 trait impl + 1 个 inherent impl；driver.rs 里是 Loudish 的 inherent impl"
    );

    // --- KNOWN-DEFECT（确定性错误，可直接精确断言）---
    // KNOWN-DEFECT(P2 修复): `drive(runner: &dyn Runner)` 里的 `runner.run()` 正确结果应是
    //   "候选 = runners.rs 里的 trait/impl 版本"（至少排除 driver.rs 里那份 Loudish::run）；
    //   当前实际是"本文件同名 first-wins"→ 解析到 **driver.rs::run**（一个与 dyn Runner 无关的类型的方法），
    //   并且 is_resolved=true。这个错指在本 fixture 里是**稳定可复现**的（不依赖 HashMap 顺序），
    //   所以可以精确断言 —— 它正是 P2 分层精度解析要消灭的典型误连边。
    let dyn_edges = fixture.edges_from("drive");
    assert_eq!(dyn_edges.len(), 1, "实测 = {dyn_edges:?}");
    let dyn_edge = &dyn_edges[0];
    assert_eq!(dyn_edge.callee_name, "run");
    assert!(dyn_edge.is_resolved, "误连边同样被标成已解析");
    assert_eq!(
        dyn_edge.callee_file(),
        "driver.rs",
        "当前把 dyn Runner 的调用错接到了调用点所在文件里的 Loudish::run（期望应在 P2 后变成 runners.rs 的候选集）"
    );

    // --- 对照组：drive_loud 调 loud.run()，本文件 first-wins 这次是对的 ---
    let loud_edges = fixture.edges_from("drive_loud");
    assert_eq!(loud_edges.len(), 1);
    assert_eq!(loud_edges[0].callee_file(), "driver.rs");
    assert!(loud_edges[0].is_resolved);

    // --- 量化基线：5 个候选被压成 2 条边，全部 is_resolved=true，0 条歧义标记 ---
    assert_eq!(fixture.edges().len(), 2);
    let stats = fixture.graph.get_stats();
    assert_eq!(stats.resolved_calls, 2);
    assert_eq!(stats.unresolved_calls, 0);
    let drive_edge_count = fixture.edges_from("drive").len();
    let run_candidate_count = run_candidates.len();
    assert!(
        run_candidate_count > drive_edge_count,
        "候选数({run_candidate_count}) > 实际产出的边数({drive_edge_count}) —— \
         多余候选被静默丢弃，这就是 P2 要保留的多候选信息量"
    );
}

// ============================================================================================
// 汇总指标：把整个 `tests/fixtures/resolution/` 目录一次性全量建图，统计可对比的口径
// ============================================================================================

#[test]
fn test_baseline_aggregate_metrics_over_all_resolution_fixtures() {
    // case 传空串 = 直接以 fixture 根目录为项目根（README.md 会被 scan_directory 过滤掉）
    let fixture = Fixture::build("");
    let edges = fixture.edges();
    let functions = fixture.all_functions();

    let total_edges = edges.len();
    let resolved_edges = edges.iter().filter(|edge| edge.is_resolved).count();
    let unresolved_edges = total_edges - resolved_edges;
    let cross_file_edges = edges
        .iter()
        .filter(|edge| match (&edge.caller, &edge.callee) {
            (Some(caller), Some(callee)) => caller.file_path != callee.file_path,
            _ => false,
        })
        .count();
    // 同名歧义调用点：这条边的 callee_name 在图里有 >1 个候选函数
    let ambiguous_call_sites = edges
        .iter()
        .filter(|edge| fixture.candidates(&edge.callee_name).len() > 1)
        .count();
    // 幽灵节点：未解析调用被 parser 临时造的 namespace="unresolved" 假函数
    let ghost_nodes = functions.iter().filter(|f| f.namespace == "unresolved").count();
    // 候选总数（所有同名多份定义的候选数之和）与"实际产出的已解析边数"的差 = 被丢弃的候选信息量
    let total_candidates_of_ambiguous_names: usize = ["helper", "pick", "add", "run"]
        .iter()
        .map(|name| fixture.candidates(name).len())
        .sum();

    println!("===== Phase P0 调用边解析基线（真实 parser 全量建图）=====");
    println!("函数节点总数            = {}", functions.len());
    println!("调用边总数              = {total_edges}");
    println!("is_resolved=true 边数   = {resolved_edges}");
    println!("is_resolved=false 边数  = {unresolved_edges}");
    println!("跨文件边数              = {cross_file_edges}");
    println!("同名歧义调用点数        = {ambiguous_call_sites}");
    println!("幽灵(unresolved)节点数  = {ghost_nodes}");
    println!("4 组同名歧义名的候选总数 = {total_candidates_of_ambiguous_names}");
    println!("stats.resolved/unresolved = {}/{}", fixture.graph.get_stats().resolved_calls, fixture.graph.get_stats().unresolved_calls);
    println!("=============================================================");

    // 下界断言：只锁"量级"，不锁精确值（精确值全部打印在上面，P2 之后逐条对比即可）。
    assert!(functions.len() >= 30, "fixture 应至少产出 30 个函数节点，实际 {}", functions.len());
    assert!(total_edges >= 13, "调用边总数下界，实际 {total_edges}");
    assert!(resolved_edges >= 11, "当前实现把绝大多数调用都标成已解析（含误连），实际 {resolved_edges}");
    assert!(unresolved_edges >= 2, "至少 import 别名与闭包两处解析失败，实际 {unresolved_edges}");
    assert!(cross_file_edges >= 4, "跨文件边（合法 + 误连的混合口径），实际 {cross_file_edges}");
    assert!(ambiguous_call_sites >= 9, "落在同名多候选上的调用点数，实际 {ambiguous_call_sites}");
    assert_eq!(ghost_nodes, 2, "目前恰好两处产生幽灵节点：python 别名 alias 与 Rust 闭包 doubled");
    assert!(
        total_candidates_of_ambiguous_names > resolved_edges,
        "候选总数({total_candidates_of_ambiguous_names}) 应大于已解析边数({resolved_edges}) —— \
         差值就是当前被静默丢弃的候选信息量，也是 P2 保留多候选后的收益上限"
    );
}

// ============================================================================================
// 交付物 2 的逃生口：`--force-reindex` 语义（跳过图缓存 + 跳过文件 MD5 哈希，强制全量重解析）
// ============================================================================================

#[test]
fn test_force_reindex_escape_hatch_bypasses_all_caches() {
    // 用一个小 case 做三轮建图：force → 非 force → force，比较节点数量即可证明开关生效。
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_ROOT)
        .join("cross_file_same_name");
    let temp = tempfile::TempDir::new().expect("创建临时目录失败");
    let project_dir = temp.path().join(format!("p0forcereindex_{}", std::process::id()));
    copy_tree(&src, &project_dir);

    let md5_id = format!("{:x}", md5::compute(project_dir.to_string_lossy().as_bytes()));
    let name_id = project_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    let build = |force: bool| -> usize {
        let mut parser = CodeParser::new();
        parser.set_force_reindex(force);
        parser
            .build_petgraph_code_graph(&project_dir)
            .expect("全量建图应成功")
            .get_all_functions()
            .len()
    };

    // 第 1 轮：force 置位 —— 一定是从零解析，拿到完整函数集（5 个：2 helper + 2 调用者 + 1 call_helper）
    let first = build(true);
    assert_eq!(first, 5, "--force-reindex 第一次应完整解析 5 个函数，实际 {first}");

    // 第 2 轮：**不** force。此时 home 下已有第 1 轮写出的 MD5 哈希，
    // `_should_skip_file` 会跳过全部文件 → `file_functions` 为空 → 图里 0 个节点。
    // （这正是"增量跳过"的既有行为，也说明缓存确实写成功了。）
    let incremental = build(false);
    assert_eq!(
        incremental, 0,
        "非 force 且哈希未变化时应走增量跳过（本轮没有新解析函数），实际 {incremental}"
    );

    // 第 3 轮：再次 force —— 必须无视缓存重新解析，回到 5 个节点。这就是逃生口的价值。
    let forced_again = build(true);
    assert_eq!(
        forced_again, first,
        "--force-reindex 必须无视文件哈希缓存重新解析全部文件，实际 {forced_again}"
    );

    // 顺带确认开关本身可读（CLI 接线时按它分支）
    let mut parser = CodeParser::new();
    assert!(!parser.force_reindex_enabled(), "默认必须是 false，保证既有行为不变");
    parser.set_force_reindex(true);
    assert!(parser.force_reindex_enabled());

    // 清理本用例写到 home 的哈希目录，并关闭临时目录
    cleanup_codeseek_project(&md5_id);
    cleanup_codeseek_project(&name_id);
    let _ = temp.close();
}

