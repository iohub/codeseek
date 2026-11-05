//! `local-embed` —— codeseek 的本地 ONNX 文本嵌入后端。
//!
//! 针对 `microsoft/harrier-oss-v1-0.6b` 的 ONNX 导出（Qwen3 decoder-only 架构，
//! 被对比学习训练成 embedding 模型，输出 1024 维、已 L2 归一化的句向量）。
//! 本 crate 由 `refs/ort_embed`（rembed CLI）提升而来，推理数值路径与原实现逐字一致，
//! `rembed --selftest` 是这条路径的数值黄金基线。
//!
//! 库消费者只需要三个东西：
//! - [`Embedder`]：加载 + `embed()`，维度由图签名探测（不接受外部配置）
//! - [`probe_signature`]：只做配置校验/诊断，不 tokenize 不推理
//! - [`Pooling`]：解释「图内已内建池化」与「Rust 侧自己做 last-token 池化」两种情况
//!
//! 诊断 CLI（`rembed`）在 `cli` feature 之后，codeseek 不依赖它。

pub mod embedder;
pub mod pooling;
pub mod signature;
pub mod text;
pub mod vectors;

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
pub mod report;
#[cfg(feature = "cli")]
pub mod selftest;

pub use embedder::Embedder;
pub use pooling::Pooling;
pub use signature::{probe_signature, ModelSignature};

/// ONNX 图相对模型目录的默认路径。
///
/// harrier 的本地目录是**双层 `onnx/onnx/`**（`onnx/onnx/model_fp16.onnx`），
/// 这是导出工具的产物，不是笔误。`cli.rs` 的 help 文案里有同样的字面量，改动时需手工同步
/// （clap 的 `help` 属性只接受字面量，不支持 const 插值）。
pub const DEFAULT_ONNX_REL_PATH: &str = "onnx/onnx/model_fp16.onnx";
