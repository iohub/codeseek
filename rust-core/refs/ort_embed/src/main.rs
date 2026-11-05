//! rembed —— 本地 ONNX 文本嵌入推理 CLI
//!
//! 模型：harrier-oss-v1-0.6b-ONNX（Qwen3 decoder-only 架构，训练成 embedding 模型）
//!
//! 关键事实（由 `probe` 实测得到，见 README）：
//! - 图输入只有两个：`input_ids` int64 [-1, -1]、`attention_mask` int64 [-1, -1]
//! - 图输出只有一个：`sentence_embedding` float32 [-1, 1024]
//!   → **last-token pooling + L2 归一化已经内建在 ONNX 图里**，Rust 侧不需要再做。
//! - 模型权重是 fp16，但图已经把输出转回 float32，所以可以直接读 f32。
//!
//! 为了通用性，`embedder` 里仍保留了「输出是 last_hidden_state 时自己做池化」的分支：
//! 如果将来换成没内建池化的 ONNX，本程序会自动切换到 Rust 侧 last-token pooling + L2 normalize。
//!
//! 模块划分：`cli`（参数）→ `embedder`（推理）→ `vectors`（数学）/ `report`（渲染）
//! / `text`（预览）/ `pooling`（池化）/ `selftest`（基准）。本文件只做入口编排。

mod cli;
mod embedder;
mod pooling;
mod report;
mod selftest;
mod text;
mod vectors;

use anyhow::Result;

use crate::cli::Args;
use crate::embedder::Embedder;
use crate::report::{print_embeddings, print_matrix};
use crate::vectors::similarity_matrix;

fn main() -> Result<()> {
    let args = Args::parse_validated()?;

    // 注意：这一行必须在模型加载之前打印（基线输出顺序依赖它）
    println!("[rembed] 模型目录 = {}", args.model_dir.display());
    let mut embedder = Embedder::load(&args.model_dir, args.max_length)?;

    if args.dump_signature {
        embedder.dump_signature();
        return Ok(());
    }

    if args.selftest {
        return selftest::run(&mut embedder);
    }

    run_embedding(&args, &mut embedder)
}

/// 普通嵌入路径：拼前缀 → 推理 → 打印向量与相似度矩阵
fn run_embedding(args: &Args, embedder: &mut Embedder) -> Result<()> {
    // 指令前缀：--instruction 优先于 --query
    let inputs = args.prefixed_texts();
    if let Some(p) = args.instruction_prefix() {
        println!("[rembed] 指令前缀 = {p:?}");
    }

    let vecs = embedder.embed(&inputs)?;

    println!("\n=== 嵌入结果 ===");
    // 注意：这里显示的是用户传入的**原始文本**（未加前缀），与重构前一致
    print_embeddings(&args.text, &vecs);

    if vecs.len() >= 2 {
        println!("\n=== 余弦相似度矩阵（×100）===");
        print_matrix(&similarity_matrix(&vecs));
    }

    Ok(())
}
