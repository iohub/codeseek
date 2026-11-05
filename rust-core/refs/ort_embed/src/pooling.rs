//! 池化层：由输出签名决定策略，并把图输出转成每条的嵌入向量。
//!
//! 这里刻意不碰 session / tokenizer，只吃「batch 大小 + 维度 + 图输出的形状与扁平
//! f32 数据 + attention_mask」，因此可以脱开模型单独阅读。
//! 算法细节（last-token 位置、L2 归一化、切片顺序）必须与重构前完全一致，
//! 否则 `--selftest` 的数值会漂移。

use anyhow::{bail, Context, Result};

use crate::vectors::l2_normalize;

/// 池化策略：由模型输出签名决定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pooling {
    /// 图内已内建 last-token pooling + L2 normalize（harrier 的 model_fp16.onnx 就属于这种）
    BuiltIn,
    /// 图只输出 last_hidden_state，需要 Rust 侧自己做 last-token pooling + L2 normalize
    LastToken,
}

impl Pooling {
    /// 依据输出名与输出形状推断池化策略与嵌入维度。
    ///
    /// 判定顺序与重构前一致：先看「名字是 sentence_embedding 或 rank==2」→ BuiltIn，
    /// 再看 rank==3 → LastToken，其余报错。
    pub(crate) fn detect(output_name: &str, dims: &[i64]) -> Result<(Self, usize)> {
        let (strategy, what) = if output_name == "sentence_embedding" || dims.len() == 2 {
            (Self::BuiltIn, "嵌入")
        } else if dims.len() == 3 {
            (Self::LastToken, "隐藏")
        } else {
            bail!("不认识的输出签名: name={output_name} shape={dims:?}");
        };

        let d = *dims.last().unwrap_or(&0);
        if d <= 0 {
            bail!("无法从输出签名推断{what}维度: {dims:?}");
        }
        Ok((strategy, d as usize))
    }

    /// 按策略把图输出转成每条的嵌入向量。
    ///
    /// - `batch`：条数。取自 tokenize 而非 `shape`，两者不一致时由 `last_token` 报错。
    /// - `dim`：期望的嵌入维度
    /// - `shape` / `data`：图输出的形状与扁平 f32 数据
    /// - `mask`：长度 `batch * seq_len` 的 attention_mask
    pub(crate) fn apply(
        &self,
        batch: usize,
        dim: usize,
        shape: &[i64],
        mask: &[i64],
        data: &[f32],
    ) -> Result<Vec<Vec<f32>>> {
        match self {
            Self::BuiltIn => builtin(data, batch, dim),
            Self::LastToken => last_token(batch, dim, shape, mask, data),
        }
    }
}

/// 图内已完成 last-token pooling + L2 normalize，直接按 `dim` 切片
fn builtin(data: &[f32], batch: usize, dim: usize) -> Result<Vec<Vec<f32>>> {
    if data.len() != batch * dim {
        bail!(
            "输出元素个数 {} 与期望 {}x{} = {} 不符",
            data.len(),
            batch,
            dim,
            batch * dim
        );
    }
    Ok(data.chunks_exact(dim).map(|c| c.to_vec()).collect())
}

/// 图只给 last_hidden_state：用 attention_mask 找每条文本的最后一个非 pad 位置
fn last_token(
    batch: usize,
    dim: usize,
    shape: &[i64],
    mask: &[i64],
    data: &[f32],
) -> Result<Vec<Vec<f32>>> {
    let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
    if dims.len() != 3 || dims[0] != batch || dims[2] != dim {
        bail!("last_hidden_state 形状异常: {dims:?}");
    }
    let (seq, hidden) = (dims[1], dims[2]);
    (0..batch)
        .map(|b| {
            let mask_row = &mask[b * seq..(b + 1) * seq];
            let t = last_token_index(mask_row)
                .with_context(|| format!("第 {b} 条文本 attention_mask 全为 0"))?;
            let start = (b * seq + t) * hidden;
            let mut v = data[start..start + hidden].to_vec();
            l2_normalize(&mut v);
            Ok(v)
        })
        .collect()
}

/// 一行 attention_mask 里最后一个 `1` 的下标（即 last-token 位置）
fn last_token_index(mask_row: &[i64]) -> Option<usize> {
    mask_row.iter().rposition(|&m| m == 1)
}
