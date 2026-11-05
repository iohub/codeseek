//! 渲染层：向量摘要与相似度矩阵的终端打印。
//!
//! 所有格式串、对齐宽度、缩进都必须逐字保留——这些字节是行为回归基线的一部分。

use ndarray::Array2;

use crate::text::preview_and_ellipsis;
use crate::vectors::l2_norm;

/// 向量摘要里展示的头部维度数（同时决定 `take(N)` 和打印标签 `head{HEAD_DIMS}=`）
const HEAD_DIMS: usize = 8;

/// 打印矩阵（保留 4 位小数）
pub fn print_matrix(m: &Array2<f32>) {
    for row in m.rows() {
        let cells: Vec<String> = row.iter().map(|x| format!("{x:>10.4}")).collect();
        println!("  [{}]", cells.join(", "));
    }
}

/// 打印单条文本的向量摘要
pub fn print_vector(tag: &str, text: &str, v: &[f32]) {
    let head: Vec<String> = v.iter().take(HEAD_DIMS).map(|x| format!("{x:.6}")).collect();
    // 长文本只显示前 60 个字符（按字符截断，中文/emoji 安全）
    let (preview, ellipsis) = preview_and_ellipsis(text);
    println!(
        "  {tag} dim={} L2={:.6} head{HEAD_DIMS}=[{}]",
        v.len(),
        l2_norm(v),
        head.join(", ")
    );
    println!("      text: \"{preview}{ellipsis}\"");
}

/// 打印一组成对（原文, 向量）的摘要，tag 依次为 `[0]`、`[1]`…
///
/// 注意：只有普通嵌入路径用它；selftest 的 tag 形如 `query[0]` / `doc[0]  `，
/// 由 selftest 自己拼，不能共用本函数。
pub fn print_embeddings(orig_texts: &[String], vecs: &[Vec<f32>]) {
    for (i, (orig, v)) in orig_texts.iter().zip(vecs).enumerate() {
        print_vector(&format!("[{i}]"), orig, v);
    }
}
