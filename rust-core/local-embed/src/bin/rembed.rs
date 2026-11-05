//! rembed —— 本地 ONNX 文本嵌入诊断 CLI（`local-embed` 的数值基线工具）
//!
//! 由 `refs/ort_embed/src/main.rs` 提升而来。保留它不是为了当产品，而是因为
//! `--selftest` 是官方 model card 基准矩阵的复现闸门：任何一次池化 / attention_mask
//! 处理的漂移都会在这里暴露，而不是等到 codeseek 检索质量下降才发现。
//!
//! 模块划分：`cli`（参数）→ `embedder`（推理）→ `vectors`（数学）/ `report`（渲染）
//! / `text`（预览）/ `pooling`（池化）/ `selftest`（基准）。本文件只做入口编排。

use anyhow::Result;

use local_embed::cli::Args;
use local_embed::embedder::Embedder;
use local_embed::report::{print_embeddings, print_matrix};
use local_embed::vectors::similarity_matrix;
use local_embed::DEFAULT_ONNX_REL_PATH;

fn main() -> Result<()> {
    let args = Args::parse_validated()?;

    // 注意：这一行必须在模型加载之前打印（基线输出顺序依赖它）
    println!("[rembed] 模型目录 = {}", args.model_dir.display());
    let mut embedder = Embedder::load(&args.model_dir, DEFAULT_ONNX_REL_PATH, args.max_length, 0)?;

    if args.dump_signature {
        embedder.dump_signature();
        return Ok(());
    }

    if args.selftest {
        return local_embed::selftest::run(&mut embedder);
    }

    run_embedding(&args, &mut embedder)
}

/// 普通嵌入路径：拼前缀 → 推理 → 打印向量与相似度矩阵
fn run_embedding(args: &Args, embedder: &mut Embedder) -> Result<()> {
    let inputs = args.prefixed_texts();
    if let Some(p) = args.instruction_prefix() {
        println!("[rembed] 指令前缀 = {p:?}");
    }

    let vecs = embedder.embed(&inputs)?;

    println!("\n=== 嵌入结果 ===");
    // 显示的是用户传入的**原始文本**（未加前缀）
    print_embeddings(&args.text, &vecs);

    if vecs.len() >= 2 {
        println!("\n=== 余弦相似度矩阵（×100）===");
        print_matrix(&similarity_matrix(&vecs));
    }

    Ok(())
}
