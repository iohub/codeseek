//! 临时探针：打印 layered 模式下的守卫表（抄表用，跑完即删）。
use std::path::{Path, PathBuf};

use codeseek::cli::args::ResolverMode;
use codeseek::codegraph::parser::CodeParser;
use codeseek::codegraph::types::{FunctionInfo, PetCodeGraph};

const RESOLUTION_ROOT: &str = "tests/fixtures/resolution";
const REPOS_ROOT: &str = "tests/test_repos";

struct Metrics {
    function_nodes: usize,
    edges: usize,
    resolved: usize,
    unresolved: usize,
    cross_file_edges: usize,
    ambiguous_call_sites: usize,
    ghost_nodes: usize,
    /// 口径 B：有边的调用点数（caller, line）去重
    call_sites_with_edges: usize,
    layered_level0_edges: usize,
    layered_candidate_sum: usize,
}

fn build(rel_src: &str, label: &str, layered: bool) -> (PetCodeGraph, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_src);
    let temp = tempfile::TempDir::new().expect("tmp");
    let project_dir = temp.path().join(format!("p2bprobe_{}", std::process::id()));
    copy_tree(&src, &project_dir);
    let mut parser = CodeParser::new();
    parser.set_force_reindex(true);
    parser.set_resolver_mode(if layered { ResolverMode::Layered } else { ResolverMode::Legacy });
    parser.set_max_candidates(8);
    let graph = parser
        .build_petgraph_code_graph(&project_dir)
        .unwrap_or_else(|e| panic!("[{label}] 建图失败: {e}"));
    // temp 必须活到函数返回后再删：把 project_dir 复制出来，图是 owned 的
    let dir = project_dir.clone();
    std::mem::forget(temp);
    (graph, dir)
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readdir") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

fn metrics(graph: &PetCodeGraph, root: &Path) -> Metrics {
    let functions: Vec<&FunctionInfo> = graph.get_all_functions();
    let edges = &graph.graph;
    let rels: Vec<_> = edges.edge_references().map(|e| e.weight().clone()).collect();
    let node_of = |id: &uuid::Uuid| graph.get_function_by_id(id).cloned();
    let resolved = rels.iter().filter(|r| r.is_resolved).count();
    let cross = rels
        .iter()
        .filter(|r| {
            match (node_of(&r.caller_id), node_of(&r.callee_id)) {
                (Some(a), Some(b)) => a.file_path != b.file_path,
                _ => false,
            }
        })
        .count();
    let ambiguous = rels.iter().filter(|r| r.candidate_count >= 2).count();
    let ghosts = functions.iter().filter(|f| f.namespace == "unresolved").count();
    let sites: std::collections::HashSet<(String, usize, String)> = rels
        .iter()
        .map(|r| {
            let f = node_of(&r.caller_id);
            (
                f.as_ref()
                    .map(|x| x.file_path.strip_prefix(root).unwrap_or(&x.file_path).to_string_lossy().to_string())
                    .unwrap_or_default(),
                r.line_number,
                r.callee_name.clone(),
            )
        })
        .collect();
    Metrics {
        function_nodes: functions.len(),
        edges: rels.len(),
        resolved,
        unresolved: rels.len() - resolved,
        cross_file_edges: cross,
        ambiguous_call_sites: ambiguous,
        ghost_nodes: ghosts,
        call_sites_with_edges: sites.len(),
        layered_level0_edges: rels.iter().filter(|r| r.resolution_level == 0).count(),
        layered_candidate_sum: rels.iter().map(|r| r.candidate_count as usize).sum(),
    }
}

#[test]
fn probe_layered_tables() {
    for layered in [false, true] {
        let tag = if layered { "LAYERED" } else { "LEGACY" };
        println!("===== {tag} =====");
        let (g, root) = build(RESOLUTION_ROOT, "aggregate", layered);
        let m = metrics(&g, &root);
        println!(
            "aggregate nodes={} edges={} resolved={} unresolved={} cross={} ambiguous={} ghosts={} sites_with_edges={} level0_edges={} cand_sum={}",
            m.function_nodes, m.edges, m.resolved, m.unresolved, m.cross_file_edges,
            m.ambiguous_call_sites, m.ghost_nodes, m.call_sites_with_edges, m.layered_level0_edges, m.layered_candidate_sum
        );
    }
    let cases = [
        "cross_file_same_name",
        "python_import_alias",
        "rust_qualified_path",
        "python_dotted_qualified",
        "method_receiver",
        "nested_function_caller",
        "ambiguous_trait_impl",
    ];
    for case in cases {
        for layered in [false, true] {
            let tag = if layered { "LAYERED" } else { "LEGACY" };
            let (g, root) = build(&format!("{RESOLUTION_ROOT}/{case}"), case, layered);
            let m = metrics(&g, &root);
            println!(
                "{tag} CASE {case} nodes={} edges={} resolved={} unresolved={} cross={} ambiguous={} ghosts={} sites={} level0={} cand_sum={}",
                m.function_nodes, m.edges, m.resolved, m.unresolved, m.cross_file_edges,
                m.ambiguous_call_sites, m.ghost_nodes, m.call_sites_with_edges, m.layered_level0_edges, m.layered_candidate_sum
            );
            if layered {
                // 打印逐条边（相对路径）
                let mut rows: Vec<String> = g
                    .graph
                    .edge_references()
                    .map(|e| {
                        let r = e.weight();
                        let c = g.get_function_by_id(&r.caller_id);
                        let k = g.get_function_by_id(&r.callee_id);
                        format!(
                            "    {}:{}@{} -> {}:{} L{} c{} r{} n{} res={}",
                            c.as_ref().map(|x| x.name.clone()).unwrap_or_default(),
                            c.as_ref().map(|x| x.line_start).unwrap_or(0),
                            c.as_ref().map(|x| x.file_path.strip_prefix(&root).unwrap_or(&x.file_path).to_string_lossy().to_string()).unwrap_or_default(),
                            k.as_ref().map(|x| x.name.clone()).unwrap_or_default(),
                            k.as_ref().map(|x| x.line_start).unwrap_or(0),
                            r.resolution_level, r.confidence, r.rank, r.candidate_count, r.is_resolved,
                        )
                    })
                    .collect();
                rows.sort();
                for row in rows {
                    println!("{row}");
                }
            }
        }
    }
    for case in [
        "simple_rust_project",
        "simple_python_project",
        "simple_ts_project",
        "simple_js_project",
    ] {
        for layered in [false, true] {
            let tag = if layered { "LAYERED" } else { "LEGACY" };
            let (g, root) = build(&format!("{REPOS_ROOT}/{case}"), case, layered);
            let m = metrics(&g, &root);
            println!(
                "{tag} REPO {case} nodes={} edges={} resolved={} unresolved={} cross={} ambiguous={} ghosts={} sites={} level0={} cand_sum={}",
                m.function_nodes, m.edges, m.resolved, m.unresolved, m.cross_file_edges,
                m.ambiguous_call_sites, m.ghost_nodes, m.call_sites_with_edges, m.layered_level0_edges, m.layered_candidate_sum
            );
        }
    }
}
