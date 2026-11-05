//! harrier 模型的数值闸门：维度探测、L2 归一化、确定性。
//!
//! 需要 1.2GB 权重在场，故由 `CODESEEK_EMBED_MODEL_DIR` 门控 —— 未设置时**跳过而不是失败**。
//! 这一条必须诚实：CI 上没有模型，这些断言就不会被执行，真正的兜底是
//! `rembed --selftest` 在本机跑通（见 docs/local-embedding-backend.md 实施记录）。

use std::path::{Path, PathBuf};

use local_embed::pooling::Pooling;
use local_embed::vectors::l2_norm;
use local_embed::{Embedder, DEFAULT_ONNX_REL_PATH};

fn model_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("CODESEEK_EMBED_MODEL_DIR").ok()?);
    dir.join(DEFAULT_ONNX_REL_PATH)
        .exists()
        .then_some(dir)
}

/// 跳过时打印原因，避免「测试通过」被误读成「断言被执行了」。
macro_rules! require_model {
    () => {
        match model_dir() {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: CODESEEK_EMBED_MODEL_DIR is not set or model missing");
                return;
            }
        }
    };
}

fn load(dir: &Path) -> Embedder {
    // max_length 用 512（与 refs 原基准一致），intra_threads 交给 ORT
    Embedder::load(dir, DEFAULT_ONNX_REL_PATH, 512, 0).expect("model must load")
}

#[test]
fn dim_and_pooling_are_probed_from_graph_signature() {
    let dir = require_model!();
    let embedder = load(&dir);
    assert_eq!(embedder.dim(), 1024, "harrier 输出是 [batch, 1024]");
    assert_eq!(
        embedder.pooling(),
        Pooling::BuiltIn,
        "last-token pooling + L2 normalize 已内建在 ONNX 图里"
    );
    assert_eq!(embedder.output_name(), "sentence_embedding");
}

#[test]
fn embeddings_are_l2_normalized() {
    let dir = require_model!();
    let mut embedder = load(&dir);
    let vecs = embedder.embed(&["hello world"]).expect("embed failed");
    assert_eq!(vecs.len(), 1);
    let norm = l2_norm(&vecs[0]);
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "图内已做 L2 归一化，实测范数 = {norm}"
    );
}

/// 同一段文本重复推理必须得到**完全相同**的向量。
/// 这是最便宜的回归锚：一旦 ORT 线程数/图优化改变导致数值漂移，这里先响。
#[test]
fn inference_is_deterministic_across_calls() {
    let dir = require_model!();
    let mut embedder = load(&dir);
    let a = embedder.embed(&["summit define"]).unwrap()[0].clone();
    let b = embedder.embed(&["summit define"]).unwrap()[0].clone();
    assert_eq!(a, b, "同一输入两次推理必须逐位一致");
}

/// batch 顺序必须与输入顺序一致（codeseek 的缓存回填按 zip 配对，依赖这条）。
#[test]
fn batch_order_matches_input_order() {
    let dir = require_model!();
    let mut embedder = load(&dir);
    let texts = ["how much protein should a female eat", "summit define", "你好，世界"];
    let batch = embedder.embed(&texts).unwrap();
    assert_eq!(batch.len(), texts.len());
    for (i, text) in texts.iter().enumerate() {
        let single = embedder.embed(&[*text]).unwrap();
        assert_eq!(
            batch[i], single[0],
            "batch 内第 {i} 条应与单独推理一致；不一致说明 padding/attention_mask 处理有误"
        );
    }
}
