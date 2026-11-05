//! 向量数学：L2 范数 / 归一化 / 余弦相似度 / 相似度矩阵。
//!
//! 全部为纯函数，不含任何 I/O，方便单独验证。运算顺序与收尾的 `* 100.0`
//! 缩放位置都必须保持不变——`--selftest` 的数值输出依赖于此。

use ndarray::Array2;

/// 原地做 L2 归一化
pub fn l2_normalize(v: &mut [f32]) {
    let norm = l2_norm(v);
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// 向量的 L2 范数
pub fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// 两条向量的余弦相似度（只被 `cross_similarity` 使用，故不 `pub`）
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let (na, nb) = (l2_norm(a), l2_norm(b));
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// 计算 a×a 的自相似度矩阵（×100）
pub fn similarity_matrix(vecs: &[Vec<f32>]) -> Array2<f32> {
    cross_similarity(vecs, vecs)
}

/// 计算交叉相似度矩阵（×100）
pub fn cross_similarity(a: &[Vec<f32>], b: &[Vec<f32>]) -> Array2<f32> {
    let mut m = Array2::<f32>::zeros((a.len(), b.len()));
    for i in 0..a.len() {
        for j in 0..b.len() {
            m[[i, j]] = cosine(&a[i], &b[j]) * 100.0;
        }
    }
    m
}
