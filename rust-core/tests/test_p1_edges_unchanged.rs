//! Phase P1 的**边语义不变性守卫**（不是新期望，是"禁止变化"的护栏）。
//!
//! ## 这条守卫在保护什么
//! P1 的全部改动（`ImportDeclaration` 的 trait getter、Rust receiver、Java package/通配符、
//! 新建 `codegraph::resolution` 数据层、`CodeParser::file_scopes` 只建不消费的接线）
//! **一个都不许改变调用边**。本文件把 P0 实测出来的 8 项基线指标逐字段钉死：
//!
//! ```text
//! 函数节点总数            = 32
//! 调用边总数              = 13
//! is_resolved=true 边数   = 11
//! is_resolved=false 边数  = 2
//! 跨文件边数              = 4
//! 同名歧义调用点数        = 9
//! 幽灵(unresolved)节点数  = 2
//! 4 组同名歧义名的候选总数 = 13
//! ```
//!
//! 对照表出处：`rust-core/tests/fixtures/resolution/README.md`（7 个场景各自的
//! "当前解析结果 / 正确解析结果"），以及 `tests/test_resolution_baseline.rs`
//! 的 `test_baseline_aggregate_metrics_over_all_resolution_fixtures`
//! （那里只做了 `>=` 下界断言，本文件收紧成**精确相等**）。
//!
//! ## 为什么 P2/P3 也必须看这条测试
//! P2 把四处 first-wins 收敛成单一 resolver 时，这些数字**必然**改变（歧义候选被保留、
//! 误连边消失）。届时正确的做法是：改这张表并在 commit message 里逐项说明差异来源，
//! 而不是删掉本文件。测试名里的 `guard` 就是这个意思。
//!
//! ## 稳定性
//! 一律用 `CodeParser::set_force_reindex(true)` 从零全量解析（P0 交付物 2 的逃生口），
//! 因此不受 `~/.codeseek/projects/` 下的历史缓存影响；用例结束时清掉自己写进 home 的目录。

use std::path::{Path, PathBuf};

use codeseek::cli::args::ResolverMode;
use codeseek::codegraph::parser::CodeParser;
use codeseek::codegraph::types::{FunctionInfo, PetCodeGraph};

/// 7 个消歧场景所在目录（整目录一次性建图 = P0 基线口径）
const RESOLUTION_ROOT: &str = "tests/fixtures/resolution";
/// 4 个整仓小项目（跨语言回归面，P0 之前就有）
const REPOS_ROOT: &str = "tests/test_repos";

/// 一次全量建图的可比较指标。**字段顺序即 P0 基线报告的顺序**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Metrics {
    /// 图里的函数节点总数（含未解析调用造出的幽灵节点）
    function_nodes: usize,
    /// 调用边总数
    edges: usize,
    /// `is_resolved = true` 的边数
    resolved: usize,
    /// `is_resolved = false` 的边数
    unresolved: usize,
    /// caller 与 callee 落在不同文件里的边数
    cross_file_edges: usize,
    /// callee 名字在图里有 >1 个候选的边数（同名歧义调用点）
    ambiguous_call_sites: usize,
    /// `namespace == "unresolved"` 的幽灵节点数
    ghost_nodes: usize,
}

impl Metrics {
    /// 与 P0 完全一致（缺一项就把差异打全）
    fn assert_equals(&self, expected: &Metrics, label: &str) {
        let actual = format!("{:?}", self);
        let want = format!("{:?}", expected);
        assert_eq!(
            *self,
            *expected,
            "[{label}] 调用边语义发生变化！\n  P0/基线 = {want}\n  本次     = {actual}\n\
             若这是 P2/P3 的预期改动，请连同 tests/fixtures/resolution/README.md 一起更新本文件的表并说明原因。"
        );
    }

    /// 逐字段比较，但**跳过 `cross_file_edges`**，返回第一个不一致的字段名。
    ///
    /// 为什么不锁跨文件边数（仅限逐目录用例）：callee 命中哪一个同名候选取决于
    /// `_build_function_name_index` 的 `HashMap` 迭代顺序与 `or_insert` 的 first-wins
    /// （P0 基线文档里写明"取决于每进程随机哈希种子"），因此**同一份 fixture 在不同进程里
    /// 实测到的 cross_file_edges 会抖**（`simple_ts_project` 8↔13、`simple_js_project` 31↔35）。
    /// 锁它只会产出 flaky 测试。聚合口径（守卫 1）里的 4 之所以能锁：那 7 个场景的歧义名
    /// 候选无论选中哪一个都同样是跨文件边，计数不变 —— 这一点由守卫 1 的重复运行验证。
    fn first_difference_ignoring_cross_file(&self, expected: &Metrics) -> Option<&'static str> {
        if self.function_nodes != expected.function_nodes {
            return Some("function_nodes");
        }
        if self.edges != expected.edges {
            return Some("edges");
        }
        if self.resolved != expected.resolved {
            return Some("resolved");
        }
        if self.unresolved != expected.unresolved {
            return Some("unresolved");
        }
        if self.ambiguous_call_sites != expected.ambiguous_call_sites {
            return Some("ambiguous_call_sites");
        }
        if self.ghost_nodes != expected.ghost_nodes {
            return Some("ghost_nodes");
        }
        None
    }
}

/// 一次建图的产物：图 + 建图用的 parser（`file_scopes` 要从 parser 上读）。
struct Built {
    _temp: tempfile::TempDir,
    project_dir: PathBuf,
    graph: PetCodeGraph,
    parser: CodeParser,
}

impl Drop for Built {
    fn drop(&mut self) {
        // 清掉本次写进 ~/.codeseek/projects 的增量哈希目录（project_id 有两种形态，都试）。
        let id_by_md5 = format!(
            "{:x}",
            md5::compute(self.project_dir.to_string_lossy().as_bytes())
        );
        cleanup_codeseek_project(&id_by_md5);
        if let Some(name) = self.project_dir.file_name() {
            cleanup_codeseek_project(&name.to_string_lossy());
        }
    }
}

fn cleanup_codeseek_project(project_id: &str) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let dir = home.join(".codeseek").join("projects").join(project_id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

impl Built {
    /// 把 `src` 整目录复制进唯一命名的临时项目目录，然后走**真实 parser 全量链路**建图。
    fn build(label: &str, rel_src: &str) -> Self {
        Self::build_with(label, rel_src, None)
    }

    /// Phase P2a 交付物 3 的验证入口：同一条全量建图链路，但把 `--resolver` /
    /// `--max-candidates` / `--levels` 三枚 flag 注入 `CodeParser`。
    ///
    /// ⚠ **P2a 里这三个开关不参与产边**（任务红线 2）。因此带 `flags` 与不带的两次建图，
    /// 守卫表的 8 项指标**必须逐字段相同**，全部调用边的多重集也必须相同。
    /// 见 [`p2a_guard_tables_unchanged_under_both_resolver_modes`]。
    fn build_with(label: &str, rel_src: &str, flags: Option<ResolverFlags>) -> Self {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_src);
        assert!(src.is_dir(), "fixture 目录不存在: {}", src.display());

        let temp = tempfile::TempDir::new().expect("创建临时目录失败");
        let project_dir = temp
            .path()
            .join(format!("p1guard_{}_{}", label, std::process::id()));
        copy_tree(&src, &project_dir);

        let mut parser = CodeParser::new();
        parser.set_force_reindex(true);
        if let Some(flags) = flags {
            flags.apply(&mut parser);
        }
        let graph = parser
            .build_petgraph_code_graph(&project_dir)
            .unwrap_or_else(|e| panic!("[{label}] 建图失败: {e}"));

        Self {
            _temp: temp,
            project_dir,
            graph,
            parser,
        }
    }

    fn all_functions(&self) -> Vec<FunctionInfo> {
        self.graph
            .get_all_functions()
            .into_iter()
            .cloned()
            .collect()
    }

    /// `function_names` 索引里某个名字的候选个数（P0 的"候选总数"口径就是这个索引）。
    fn candidate_count(&self, name: &str) -> usize {
        self.graph.function_names.get(name).map(|v| v.len()).unwrap_or(0)
    }

    fn edges(&self) -> Vec<(Option<FunctionInfo>, Option<FunctionInfo>, String, bool)> {
        self.graph
            .graph
            .edge_references()
            .map(|edge| {
                let rel = edge.weight();
                (
                    self.graph.get_function_by_id(&rel.caller_id).cloned(),
                    self.graph.get_function_by_id(&rel.callee_id).cloned(),
                    rel.callee_name.clone(),
                    rel.is_resolved,
                )
            })
            .collect()
    }

    /// 算出全部守卫指标（口径与 `test_resolution_baseline.rs` 的汇总测试**逐行一致**）。
    fn metrics(&self) -> Metrics {
        let functions = self.all_functions();
        let edges = self.edges();
        let resolved = edges.iter().filter(|(_, _, _, is_resolved)| *is_resolved).count();
        Metrics {
            function_nodes: functions.len(),
            edges: edges.len(),
            resolved,
            unresolved: edges.len() - resolved,
            cross_file_edges: edges
                .iter()
                .filter(|(caller, callee, _, _)| match (caller, callee) {
                    (Some(caller), Some(callee)) => caller.file_path != callee.file_path,
                    _ => false,
                })
                .count(),
            ambiguous_call_sites: edges
                .iter()
                .filter(|(_, _, callee_name, _)| self.candidate_count(callee_name) > 1)
                .count(),
            ghost_nodes: functions.iter().filter(|f| f.namespace == "unresolved").count(),
        }
    }

    /// 4 组同名歧义名（helper/pick/add/run）的候选总数。
    fn ambiguous_name_candidates(&self) -> usize {
        ["helper", "pick", "add", "run"]
            .iter()
            .map(|name| self.candidate_count(name))
            .sum()
    }

    /// 全部调用边的**规范化多重集**（不含 uuid，只留源码事实）：
    /// `(caller 相对文件, caller 名, caller 定义行, callee 相对文件, callee 名, callee 定义行, 调用行, is_resolved)`。
    ///
    /// 用途：证明 `--resolver=layered` 在 P2a 对图**零影响** —— 比 8 项聚合指标强得多
    /// （聚合指标相同仍可能"换了一条边"）。
    fn normalized_edge_multiset(&self) -> Vec<(String, String, usize, String, String, usize, usize, bool)> {
        let root = self.project_dir.clone();
        let rel = |p: &Path| -> String {
            p.strip_prefix(&root).unwrap_or(p).to_string_lossy().replace('\\', "/")
        };
        let mut out: Vec<(String, String, usize, String, String, usize, usize, bool)> = self
            .graph
            .graph
            .edge_references()
            .map(|edge| {
                let rel_edge = edge.weight();
                let caller = self.graph.get_function_by_id(&rel_edge.caller_id);
                let callee = self.graph.get_function_by_id(&rel_edge.callee_id);
                (
                    caller.as_ref().map(|f| rel(&f.file_path)).unwrap_or_default(),
                    caller.as_ref().map(|f| f.name.clone()).unwrap_or_default(),
                    caller.as_ref().map(|f| f.line_start).unwrap_or(0),
                    callee.as_ref().map(|f| rel(&f.file_path)).unwrap_or_default(),
                    callee.as_ref().map(|f| f.name.clone()).unwrap_or_default(),
                    callee.as_ref().map(|f| f.line_start).unwrap_or(0),
                    rel_edge.line_number,
                    rel_edge.is_resolved,
                )
            })
            .collect();
        out.sort();
        out
    }

    /// [`Built::normalized_edge_multiset`] 去掉 **callee 文件** 那一维的版本。
    ///
    /// 只保留"caller 是谁、在第几行调用了哪个同名函数、以及那条边是否被 legacy 声称已解析"，
    /// 因为"两份同名候选里选哪一份"正是 legacy first-wins 的随机面（见守卫 4(b) 的注释）。
    fn normalized_edge_multiset_without_callee_file(
        &self,
    ) -> Vec<(String, String, usize, String, usize, bool)> {
        self.normalized_edge_multiset()
            .into_iter()
            .map(|(cf, cn, cl, _callee_file, cn2, _cl2, line, resolved)| {
                (cf, cn, cl, cn2, line, resolved)
            })
            .collect()
    }
}

/// Phase P2a 交付物 3：CLI 三枚 flag 的测试侧载体（`--resolver` / `--max-candidates` / `--levels`）。
#[derive(Debug, Clone, Copy)]
struct ResolverFlags {
    resolver: ResolverMode,
    max_candidates: usize,
    levels: Option<&'static str>,
}

impl ResolverFlags {
    /// 与 `services/analyzer.rs::configure_resolver`（`--force-reindex` 的同一条注入链）同口径。
    fn apply(&self, parser: &mut CodeParser) {
        parser.set_resolver_mode(self.resolver);
        parser.set_max_candidates(self.max_candidates);
        parser.set_levels_spec(self.levels.map(|s| s.to_string()));
    }

    /// `--resolver=layered --max-candidates=8`（即"不带任何 legacy 之外的行为，但把开关拨过去"）。
    const LAYERED: ResolverFlags = ResolverFlags {
        resolver: ResolverMode::Layered,
        max_candidates: 8,
        levels: None,
    };

    /// 最激进的一组：layered + 上限 1 + 只开 L5 —— 若哪天有人把它接进产边，这条守卫会立刻响。
    const LAYERED_STRICT: ResolverFlags = ResolverFlags {
        resolver: ResolverMode::Layered,
        max_candidates: 1,
        levels: Some("l5"),
    };
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("创建目标目录");
    for entry in std::fs::read_dir(src).expect("读取 fixture 目录") {
        let entry = entry.expect("目录项");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("复制文件");
        }
    }
}

// ============================================================================================
// 守卫 1：P0 汇总基线 —— 8 项指标逐字段精确相等（32/13/11/2/4/9/2/13）
// ============================================================================================

/// P0 实测的**汇总**基线（空 case = 以 `tests/fixtures/resolution` 整目录为项目根，
/// `README.md` 会被 `scan_directory` 过滤掉）。
const EXPECTED_AGGREGATE: Metrics = Metrics {
    function_nodes: 32,
    edges: 13,
    resolved: 11,
    unresolved: 2,
    cross_file_edges: 4,
    ambiguous_call_sites: 9,
    ghost_nodes: 2,
};
const EXPECTED_AGGREGATE_CANDIDATES: usize = 13;

#[test]
fn p1_guard_p0_aggregate_baseline_is_byte_for_byte_unchanged() {
    let built = Built::build("aggregate", RESOLUTION_ROOT);
    let actual = built.metrics();
    let candidates = built.ambiguous_name_candidates();

    println!("===== Phase P1 边语义不变性守卫：tests/fixtures/resolution 全量建图 =====");
    println!("函数节点总数            = {}", actual.function_nodes);
    println!("调用边总数              = {}", actual.edges);
    println!("is_resolved=true 边数   = {}", actual.resolved);
    println!("is_resolved=false 边数  = {}", actual.unresolved);
    println!("跨文件边数              = {}", actual.cross_file_edges);
    println!("同名歧义调用点数        = {}", actual.ambiguous_call_sites);
    println!("幽灵(unresolved)节点数  = {}", actual.ghost_nodes);
    println!("4 组同名歧义名的候选总数 = {candidates}");
    println!(
        "stats.resolved/unresolved = {}/{}",
        built.graph.get_stats().resolved_calls,
        built.graph.get_stats().unresolved_calls
    );
    println!("P0 基线口径（必须完全一致）= 32/13/11/2/4/9/2/13");
    println!("==========================================================================");

    actual.assert_equals(&EXPECTED_AGGREGATE, "tests/fixtures/resolution 全量");
    assert_eq!(
        candidates, EXPECTED_AGGREGATE_CANDIDATES,
        "4 组同名歧义名（helper/pick/add/run）的候选总数必须仍是 {EXPECTED_AGGREGATE_CANDIDATES}"
    );
}

// ============================================================================================
// 守卫 2：逐目录基线 —— 7 个消歧场景 + 4 个整仓小项目，一个数字都不许动
// ============================================================================================

/// `(目录, 期望指标, 同名候选总数)`。
/// 期望值 = HEAD(`851f7fb`, P0 之后 / P1 之前) 的实测值，由本文件的
/// `p1_guard_per_case_metrics_are_unchanged` 用 `--nocapture` 打印后逐条抄录；
/// P1 的任何改动都不应该让任何一格发生变化。
///
/// ⚠ 表里 `cross_file_edges` 这一格**只记录、不断言**（原因见
/// [`Metrics::first_difference_ignoring_cross_file`]）；其余 6 项与候选总数逐字段断言。
const PER_CASE_BASELINE: &[(&str, Metrics, usize)] = &[
    // ---- 7 个消歧场景（tests/fixtures/resolution/）----
    // 与 `tests/test_resolution_baseline.rs` 的逐场景断言互相印证：
    //   cross_file_same_name 3 条边全 is_resolved=true（同名歧义被谎报为已解析）
    ("cross_file_same_name", Metrics { function_nodes: 5, edges: 3, resolved: 3, unresolved: 0, cross_file_edges: 1, ambiguous_call_sites: 3, ghost_nodes: 0 }, 2),
    //   python_import_alias 唯一那条边 is_resolved=false，callee 是本文件幽灵节点 `alias`
    ("python_import_alias", Metrics { function_nodes: 4, edges: 1, resolved: 0, unresolved: 1, cross_file_edges: 0, ambiguous_call_sites: 0, ghost_nodes: 1 }, 0),
    ("rust_qualified_path", Metrics { function_nodes: 4, edges: 2, resolved: 2, unresolved: 0, cross_file_edges: 1, ambiguous_call_sites: 2, ghost_nodes: 0 }, 2),
    ("python_dotted_qualified", Metrics { function_nodes: 3, edges: 1, resolved: 1, unresolved: 0, cross_file_edges: 1, ambiguous_call_sites: 1, ghost_nodes: 0 }, 2),
    ("method_receiver", Metrics { function_nodes: 4, edges: 1, resolved: 1, unresolved: 0, cross_file_edges: 1, ambiguous_call_sites: 1, ghost_nodes: 0 }, 2),
    //   nested_function_caller = 4 个真实函数 + 1 个闭包幽灵节点 `doubled`，3 条边（2 解析 / 1 未解析）
    ("nested_function_caller", Metrics { function_nodes: 5, edges: 3, resolved: 2, unresolved: 1, cross_file_edges: 0, ambiguous_call_sites: 0, ghost_nodes: 1 }, 0),
    //   ambiguous_trait_impl 的 5 个同名 `run` 候选被压成 2 条边
    ("ambiguous_trait_impl", Metrics { function_nodes: 7, edges: 2, resolved: 2, unresolved: 0, cross_file_edges: 0, ambiguous_call_sites: 2, ghost_nodes: 0 }, 5),
    // ---- 4 个整仓小项目（tests/test_repos/）----
    ("simple_rust_project", Metrics { function_nodes: 46, edges: 44, resolved: 34, unresolved: 10, cross_file_edges: 6, ambiguous_call_sites: 0, ghost_nodes: 10 }, 1),
    ("simple_python_project", Metrics { function_nodes: 45, edges: 48, resolved: 32, unresolved: 16, cross_file_edges: 10, ambiguous_call_sites: 0, ghost_nodes: 16 }, 0),
    ("simple_ts_project", Metrics { function_nodes: 92, edges: 101, resolved: 70, unresolved: 31, cross_file_edges: 8, ambiguous_call_sites: 4, ghost_nodes: 31 }, 1),
    ("simple_js_project", Metrics { function_nodes: 1577, edges: 746, resolved: 546, unresolved: 200, cross_file_edges: 31, ambiguous_call_sites: 187, ghost_nodes: 200 }, 3),
];

#[test]
fn p1_guard_per_case_metrics_are_unchanged() {
    let mut cases: Vec<(&str, &'static str)> = Vec::new();
    for name in [
        "cross_file_same_name",
        "python_import_alias",
        "rust_qualified_path",
        "python_dotted_qualified",
        "method_receiver",
        "nested_function_caller",
        "ambiguous_trait_impl",
    ] {
        cases.push((name, RESOLUTION_ROOT));
    }
    for name in [
        "simple_rust_project",
        "simple_python_project",
        "simple_ts_project",
        "simple_js_project",
    ] {
        cases.push((name, REPOS_ROOT));
    }

    let mut failures: Vec<String> = Vec::new();
    for (case, root) in cases {
        let rel = format!("{root}/{case}");
        let built = Built::build(case, &rel);
        let actual = built.metrics();
        let candidates = built.ambiguous_name_candidates();
        println!("CASE {case}\t{actual:?}\tcandidates={candidates}");

        match PER_CASE_BASELINE.iter().find(|(name, _, _)| *name == case) {
            Some((_, expected, expected_candidates)) => {
                // 逐目录口径只锁"与 HashMap 顺序无关"的 6 项 + 同名候选总数；
                // cross_file_edges 仍然打印出来供人工比对，但不参与断言（见上面的说明）。
                if let Some(field) = actual.first_difference_ignoring_cross_file(expected) {
                    failures.push(format!(
                        "[{case}] 字段 `{field}` 变化：基线 = {expected:?}，本次 = {actual:?}"
                    ));
                }
                if candidates != *expected_candidates {
                    failures.push(format!(
                        "[{case}] 同名候选总数变化：基线 = {expected_candidates}，本次 = {candidates}"
                    ));
                }
            }
            None => failures.push(format!("[{case}] 基线表里缺这一条用例（不许静默漏掉）")),
        }
    }

    assert!(
        failures.is_empty(),
        "P1 改动破坏了调用边语义：\n{}",
        failures.join("\n")
    );
}

// ============================================================================================
// 守卫 3：只建不消费的自检 —— file_scopes 必须存在，且不得影响任何边
// ============================================================================================

#[test]
fn p1_guard_resolution_context_is_built_but_consumes_nothing() {
    let built = Built::build("ctx", &format!("{}/method_receiver", RESOLUTION_ROOT));
    // 建了：每个被解析的文件都应当有 ctx（P1 交付物 3 的接线证据）。
    assert!(
        built.parser.file_scope_count() > 0,
        "`CodeParser::file_scopes` 必须在全量建图后被填上（P1 交付物 3 的接线证据）"
    );

    // 找到了：`receiver_user.rs` 的 ctx 里作用域树与 import 表都已就位（交付物 2/3 的证据）。
    let path = built.project_dir.join("receiver_user.rs");
    let ctx = built
        .parser
        .file_scope_ctx(&path)
        .unwrap_or_else(|| panic!("{} 的 FileScopeCtx 应存在", path.display()));
    assert!(
        !ctx.scopes.is_empty(),
        "method_receiver/receiver_user.rs 应有作用域树，module_path = {}",
        ctx.module_path
    );
    assert_eq!(
        ctx.module_path, "receiver_user",
        "module path 必须由 `CodeParser` 记录的仓库根推导出来"
    );
    // P2 需要的原料现在拿得到：调用点行号 → 最内层作用域。
    assert!(
        ctx.innermost_scope_containing(1).is_some(),
        "文件根必须覆盖第 1 行（innermost 判定不依赖符号产出顺序）"
    );
}

// ============================================================================================
// 辅助：打印逐条基线（只在需要重新抄表时读输出，本身不做任何断言）
// ============================================================================================

#[test]
fn p1_guard_print_every_case_table_for_transcription() {
    // `-- --nocapture` 查看；输出即 PER_CASE_BASELINE 的抄录来源。
    println!("(见 p1_guard_per_case_metrics_are_unchanged 的 CASE 行)");
}

// ============================================================================================
// 守卫 4（Phase P2a 交付物 3）：`--resolver=layered` 在 P2a **不得改变任何产出**
//
// 两条断言合起来才是"红线 2"的完整证据：
//   (a) 不带 flag 与带 `--resolver=layered --max-candidates=8` 两次建图，聚合守卫表
//       都必须是 `32/13/11/2/4/9/2/13`；
//   (b) 再加一组最激进的 `layered + --max-candidates=1 + --levels=l5`，
//       边的**规范化多重集**（源码事实，不含 uuid）必须与 legacy 逐条相同。
//
// ⚠⚠ **P2b 必须替换本节**：接线之后 layered 会保留多候选（口径 A：每候选一条边），
// 届时 (a)(b) 都会分叉。分叉基线的做法写在
// `tests/fixtures/resolution/README.md` 与 P2b 接手清单里，要点是：
//   * 这里断言"相等"的两处，改成"legacy==P0 基线；layered==新分叉表（每候选一条边）"；
//   * 聚合边数 13 会上升（歧义调用点按候选数展开），`is_resolved=false`/幽灵节点数不变；
//   * `GRAPH_SCHEMA_VERSION` 必须与 `CallRelation` 加字段在同一个改动里 1→2。
// ============================================================================================

#[test]
fn p2a_guard_tables_unchanged_under_both_resolver_modes() {
    let legacy = Built::build("p2a_legacy", RESOLUTION_ROOT);
    let layered = Built::build_with("p2a_layered", RESOLUTION_ROOT, Some(ResolverFlags::LAYERED));
    let strict = Built::build_with("p2a_strict", RESOLUTION_ROOT, Some(ResolverFlags::LAYERED_STRICT));

    // ---- (0) flag 确实被读进来了（否则后面对比的是同一份配置，断言就是空的）----
    assert_eq!(legacy.parser.resolver_mode(), ResolverMode::Legacy, "默认必须是 legacy（红线 2）");
    assert_eq!(layered.parser.resolver_mode(), ResolverMode::Layered);
    assert_eq!(strict.parser.resolver_mode(), ResolverMode::Layered);
    assert_eq!(strict.parser.max_candidates(), 1);
    assert_eq!(strict.parser.levels_spec(), Some("l5"));
    assert!(legacy.parser.levels_spec().is_none(), "不传 `--levels` 时必须是\"全层\"而不是某个串");

    // ---- (a) 三组配置的聚合守卫表都必须逐字段等于 P0 基线 ----
    for (label, built) in [
        ("--resolver 未置（legacy）", &legacy),
        ("--resolver=layered --max-candidates=8", &layered),
        ("--resolver=layered --max-candidates=1 --levels=l5", &strict),
    ] {
        let actual = built.metrics();
        let candidates = built.ambiguous_name_candidates();
        println!("P2A GUARD {label}\t{actual:?}\tcandidates={candidates}");
        actual.assert_equals(&EXPECTED_AGGREGATE, label);
        assert_eq!(
            candidates, EXPECTED_AGGREGATE_CANDIDATES,
            "[{label}] 4 组同名歧义名的候选总数必须仍是 {EXPECTED_AGGREGATE_CANDIDATES}"
        );
    }

    // ---- (b) 逐条比较："选中的函数身份"必须相同；**唯一豁免的是"选了哪个同名文件"** ----
    //
    // 为什么不把 callee 的文件也纳入断言：那正是本次要消灭的缺陷本身。
    // legacy 的同名 first-wins 取哪一个取决于 `HashMap` 的**每进程/每表随机种子**，
    // 实测两次 legacy 建图就会翻（本测试的真实输出）：
    //   `cross_file_same_name/gamma.rs:6 helper()` → `alpha.rs::helper` ↔ `other_mod.py::helper`
    //   `python_dotted_qualified/dotted_consumer.py:10` → `other_mod.py::helper` ↔ `alpha.rs::helper`
    // 聚合守卫表（32/13/11/2/4/9/2/13）之所以能锁住：无论选中哪一份同名候选，
    // 它**同样是跨文件边**，因此计数不变。
    //
    // layered 侧不存在这个问题（候选顺序与命中由源码事实 + (level,rank,uuid) 决定），
    // 由 `tests/test_resolution_engine.rs::p2a_resolution_is_deterministic_…` 断言。
    let key_legacy = legacy.normalized_edge_multiset_without_callee_file();
    let key_layered = layered.normalized_edge_multiset_without_callee_file();
    let key_strict = strict.normalized_edge_multiset_without_callee_file();
    assert_eq!(
        key_legacy, key_layered,
        "P2a 阶段 layered 尚未接线：除\"选中哪个同名文件\"外，调用边必须与 legacy **逐条相同**"
    );
    assert_eq!(
        key_legacy, key_strict,
        "P2a 阶段 `--max-candidates` / `--levels` 同样不得影响产边（它们只影响引擎配置）"
    );

    // ---- (b2) 把 legacy 自身的种子抖动**量化记录**（只打印，不锁死：锁死会 flaky）----
    let full_legacy = legacy.normalized_edge_multiset();
    let full_layered = layered.normalized_edge_multiset();
    let flips: Vec<String> = full_legacy
        .iter()
        .zip(full_layered.iter())
        .filter(|(a, b)| a != b)
        .map(|(a, b)| format!("legacy={a:?}  layered={b:?}"))
        .collect();
    println!(
        "legacy 同名候选的身份抖动（同一份 fixture 两次建图，差异条数 = {}）：",
        flips.len()
    );
    for flip in &flips {
        println!("  · {flip}");
    }
    assert_eq!(
        full_legacy.len(),
        full_layered.len(),
        "抖动只能发生在\"选中哪一份同名候选\"，边的条数必须不变"
    );

    // ---- (c) 本阶段未接线这一事实本身的断言 ----
    // `CodeParser` 里存的 mode/max/levels 只被只读诊断 API `resolution_config()` 消费；
    // 若有人把它们接进产边分支，`git grep` 会在下面这份"允许清单"之外找到读者。
    // 这里用可运行的行为证据代替纯文本扫描：layered 下 `file_scopes` 仍然建了、
    // 但图的规模与 legacy 完全一致 ⇒ 引擎数据存在却没被用来决定任何一条边。
    assert_eq!(
        legacy.graph.get_all_functions().len(),
        layered.graph.get_all_functions().len(),
        "P2a 未接线：函数节点数不得随 `--resolver` 变化"
    );
    assert!(
        layered.parser.file_scope_count() > 0,
        "P2a 的 ctx 必须在两种模式下都建立（否则 P2b 无从消费）"
    );
}

/// **P2a 未接线**的源码级证明（与守卫 4(c) 的行为证明互补）。
///
/// 判据：分层引擎的三个入口符号
/// （`resolve_call` / `resolve_call_with_config` / `call_sites_from_symbols*` /
/// `build_resolution_index_from_graph`）在 `src/` 里**只允许**出现在
/// `src/codegraph/resolution/` 内部；出现在任何别处都说明有人把它接进了建图/查询管线，
/// 那在 P2a 是红线违规。P2b 接线时本测试**必须连同白名单一起改**（见报告"P2b 接手清单"）。
#[test]
fn p2a_engine_entry_points_are_not_referenced_outside_the_resolution_module() {
    let forbidden = [
        "resolve_call_with_config",
        "resolve_call(",
        "call_sites_from_symbols",
        "build_resolution_index_from_graph",
    ];
    let mut offenders: Vec<String> = Vec::new();

    let mut stack = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // 白名单：引擎本体。`src/codegraph/parser.rs` 里的**注释与文档**不算引用，
            // 因此这里只统计"真实调用/导入"形态。
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let is_engine = path
                .to_string_lossy()
                .replace('\\', "/")
                .contains("src/codegraph/resolution/");
            if is_engine {
                continue;
            }
            for needle in forbidden {
                for (idx, line) in text.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//")
                        || trimmed.starts_with("///")
                        || trimmed.starts_with("//!")
                    {
                        continue; // 注释里的名字不是引用
                    }
                    if line.contains(needle) {
                        offenders.push(format!(
                            "{}:{} 引用了 {needle}",
                            path.to_string_lossy(),
                            idx + 1
                        ));
                    }
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "P2a 的分层引擎必须\"只建不消费\"，但它已被建图/查询管线引用：\n{}",
        offenders.join("\n")
    );
    println!(
        "未接线证明：{} 个入口符号在 src/ 中（排除 src/codegraph/resolution/）零引用",
        forbidden.len()
    );
}
