//! README 官方基准（Step E 验收）：2 query × 2 doc 的分数矩阵。
//!
//! 期望值来自 README（q4 权重），fp16 权重允许 ±`TOLERANCE` 的差异；
//! 对角线占优是「池化 / attention_mask 处理是否正确」的最强信号。

use anyhow::{Context, Result};
use ndarray::Array2;

use crate::cli::WEB_SEARCH_INSTRUCTION;
use crate::embedder::Embedder;
use crate::report::{print_matrix, print_vector};
use crate::text::preview_and_ellipsis;
use crate::vectors::cross_similarity;

/// README 官方基准矩阵（q4 权重跑出来的，单位：余弦相似度 ×100）
const BENCH_EXPECTED: [[f32; 2]; 2] = [[66.65688, 28.08504], [29.10841, 70.66003]];
/// 允许的绝对误差（单位同 BENCH_EXPECTED）
const TOLERANCE: f32 = 2.0;

/// 基准用例里 queries 的原始文本（发送前会加 web-search 指令前缀）
const QUERIES: [&str; 2] = ["how much protein should a female eat", "summit define"];
/// 基准用例里 documents 的原始文本（不加指令前缀）
const DOCS: [&str; 2] = [
    "As a general guideline, the CDC's average requirement of protein for women ages 19 to 70 is 46 grams per day. But, as you can see from this chart, you'll need to increase that if you're expecting or training for a marathon. Check out the chart below to see how much protein you should be eating each day.",
    "Definition of summit for English Language Learners. : 1  the highest point of a mountain : the top of a mountain. : 2  the highest level. : 3  a meeting or series of meetings between the leaders of two or more governments.",
];

// 对角线占优的打印文案（`[0][0] > [0][1] 且 [1][1] > [1][0]`）按 2×2 硬编码，
// 用编译期断言把「基准矩阵 = 2 query × 2 doc」这个前提钉死：常量一改就编译失败，
// 而不是静默打印出与矩阵形状不符的索引。
const _: () = assert!(
    BENCH_EXPECTED.len() == 2 && BENCH_EXPECTED[0].len() == 2 && BENCH_EXPECTED[1].len() == 2,
    "对角线占优的打印格式按 2x2 硬编码"
);
const _: () = assert!(
    QUERIES.len() == BENCH_EXPECTED.len() && DOCS.len() == BENCH_EXPECTED[0].len(),
    "QUERIES/DOCS 的数量必须与 BENCH_EXPECTED 的行/列数一致"
);

/// 跑完整基准并打印结果
pub fn run(embedder: &mut Embedder) -> Result<()> {
    let queries: Vec<String> = QUERIES
        .iter()
        .map(|q| format!("{WEB_SEARCH_INSTRUCTION}{q}"))
        .collect();
    let docs: Vec<String> = DOCS.iter().map(|d| d.to_string()).collect();

    print_cases(&queries, &docs);

    let qv = embedder.embed(&queries).context("编码 queries 失败")?;
    let dv = embedder.embed(&docs).context("编码 documents 失败")?;

    println!("\n=== query 向量 ===");
    for (i, (t, v)) in queries.iter().zip(&qv).enumerate() {
        print_vector(&format!("query[{i}]"), t, v);
    }
    println!("=== document 向量 ===");
    for (i, (t, v)) in docs.iter().zip(&dv).enumerate() {
        print_vector(&format!("doc[{i}]  "), t, v);
    }

    println!("\n=== 分数矩阵（余弦相似度 ×100）===");
    let m = cross_similarity(&qv, &dv);
    print_matrix(&m);

    println!("\n=== 与 README 官方值对比（官方用 q4 权重，fp16 允许小差异）===");
    let (all_ok, diag_ok) = compare_with_expected(&m);
    print_verdict(&m, all_ok, diag_ok);
    Ok(())
}

/// 打印对角线对比与最终判定。
///
/// 2×2 的前提由文件顶部的 `const _` 断言锁住；这里只负责取值与打印，不重复校验。
fn print_verdict(m: &Array2<f32>, all_ok: bool, diag_ok: bool) {
    let (d00, d01, d10, d11) = (m[[0, 0]], m[[0, 1]], m[[1, 0]], m[[1, 1]]);
    // 参数顺序必须与文案一致（`[0][0] > [0][1]`、`[1][1] > [1][0]`），故 d11 排在 d10 前面
    println!(
        "\n对角线占优: {}（[0][0]={:.4} > [0][1]={:.4} 且 [1][1]={:.4} > [1][0]={:.4}）",
        if diag_ok { "是" } else { "否" },
        d00,
        d01,
        d11,
        d10
    );
    if all_ok && diag_ok {
        println!("SELFTEST PASS");
    } else if diag_ok {
        println!("SELFTEST PARTIAL (对角占优，但数值超出 ±2 容差)");
    } else {
        println!("SELFTEST FAIL (对角线不占优：池化或 attention_mask 处理有误)");
    }
}

/// 打印用例清单（queries 加前缀，documents 按字符截断预览）
fn print_cases(queries: &[String], docs: &[String]) {
    println!("=== README 基准用例 ===");
    println!("queries（加 web-search 指令前缀）:");
    for q in queries {
        println!("  - {q:?}");
    }
    println!("documents（不加指令前缀）:");
    for d in docs {
        // 必须按字符截断：原来的 `&d[..60.min(d.len())]` 是切片字节，
        // 一旦文档里有中文/emoji 且 60 不是 char 边界就会 panic。
        let (preview, ellipsis) = preview_and_ellipsis(d);
        println!("  - {preview:?}{ellipsis}");
    }
}

/// 逐格与官方值对比，返回 `(全部在容差内, 对角线是否占优)`。
///
/// 循环用 `BENCH_EXPECTED` 的形状驱动，不再硬编码 `0..2`；
/// 打印顺序仍是先行后列，格式与重构前逐字一致。
fn compare_with_expected(m: &Array2<f32>) -> (bool, bool) {
    let mut all_ok = true;
    for (i, row) in BENCH_EXPECTED.iter().enumerate() {
        for (j, &want) in row.iter().enumerate() {
            let got = m[[i, j]];
            let diff = (got - want).abs();
            let ok = diff <= TOLERANCE;
            all_ok &= ok;
            println!(
                "  [{i}][{j}] got={got:>10.4} want={want:>10.4} diff={diff:>6.4} {}",
                if ok { "OK" } else { "超出 ±2" }
            );
        }
    }

    (all_ok, diagonal_dominates(m))
}

/// 对角线占优检查：每一行里，对角线元素都应大于该行其它元素。
///
/// 对 2×2 而言完全等价于原来的
/// `m[[0,0]] > m[[0,1]] && m[[1,1]] > m[[1,0]]`。
fn diagonal_dominates(m: &Array2<f32>) -> bool {
    BENCH_EXPECTED.iter().enumerate().all(|(i, row)| {
        row.iter()
            .enumerate()
            .all(|(j, _)| i == j || m[[i, i]] > m[[i, j]])
    })
}
