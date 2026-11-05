//! ONNX Runtime 环境单例与模型签名探测。
//!
//! `environment()` 是整个 crate 唯一的 `ort::init()` 调用点：ORT 的进程级状态由
//! `OnceLock<Weak<EnvironmentInner>>` 管理（`ort/src/environment.rs:70`），重复初始化没有
//! 意义且会掩盖真实错误，所以这里用静态 `OnceLock` 收敛成一次。

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Outlet;

/// 获取（并按需初始化）进程唯一的 ORT Environment。
///
/// 失败原因一并缓存、不重试：`OnceLock::get_or_try_init` 在 rustc 1.94.1 仍是 unstable
/// （feature `once_cell_try`），而对本后端来说初始化失败是终态，重试只会推迟报错。
pub(crate) fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_name("codeseek-local-embed")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| anyhow::anyhow!("failed to initialize ONNX Runtime: {e}"))
}

/// 模型图签名：输入/输出的 `name: dtype` 文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSignature {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

fn format_outlets(outlets: &[Outlet]) -> Vec<String> {
    outlets
        .iter()
        .map(|o| format!("{}: {}", o.name(), o.dtype()))
        .collect()
}

/// 加载 ONNX 图并返回其签名。用于配置校验与 `--dump-signature` 诊断。
pub fn probe_signature(onnx_path: &Path) -> Result<ModelSignature> {
    let session = open_session(onnx_path, 0)?;
    Ok(ModelSignature {
        inputs: format_outlets(session.inputs()),
        outputs: format_outlets(session.outputs()),
    })
}

/// 创建 session。`intra_threads == 0` 时交给 ONNX Runtime 自行决定线程数。
pub(crate) fn open_session(onnx_path: &Path, intra_threads: usize) -> Result<Session> {
    let env = environment()?;
    let mut builder = Session::builder(env)
        .with_context(|| format!("failed to create ONNX Runtime session builder: {}", onnx_path.display()))?;
    if intra_threads > 0 {
        builder = builder
            .with_intra_threads(intra_threads)
            .map_err(|e| anyhow::anyhow!("failed to set intra-op threads: {e}"))?;
    }
    builder
        .commit_from_file(onnx_path)
        .with_context(|| format!("failed to load ONNX model: {}", onnx_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 探针用的模型路径；未配置时跳过依赖模型的测试（CI 上没有 1.2GB 权重）。
    pub(crate) fn model_path_from_env() -> Option<std::path::PathBuf> {
        let dir = std::env::var("CODESEEK_EMBED_MODEL_DIR").ok()?;
        let path = std::path::Path::new(&dir).join("onnx/onnx/model_fp16.onnx");
        path.exists().then_some(path)
    }

    #[test]
    fn probe_signature_reports_harrier_graph() {
        let Some(path) = model_path_from_env() else {
            eprintln!("skipping: set CODESEEK_EMBED_MODEL_DIR to the harrier model dir");
            return;
        };
        let sig = probe_signature(&path).expect("probe must succeed");
        assert_eq!(
            sig.inputs,
            vec![
                "input_ids: Tensor<i64>(batch_size, sequence_length)",
                "attention_mask: Tensor<i64>(batch_size, total_sequence_length)"
            ],
            "harrier 图只吃两个 int64 输入"
        );
        assert_eq!(
            sig.outputs,
            vec!["sentence_embedding: Tensor<f32>(batch_size, 1024)"],
            "池化与 L2 归一化已内建在图里，输出是 f32 [batch, 1024]"
        );
    }
}
