//! P0 链接/运行探针 CLI：加载 ONNX 图并打印签名。
//!
//! 用法：`ort_link_probe [onnx_path]`，缺省取 harrier 的本地路径。

use std::path::PathBuf;

use anyhow::{bail, Result};

use local_embed::probe_signature;

const DEFAULT_ONNX: &str =
    "/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX/onnx/onnx/model_fp16.onnx";

fn main() -> Result<()> {
    let path = PathBuf::from(std::env::args().nth(1).as_deref().unwrap_or(DEFAULT_ONNX));
    if !path.exists() {
        bail!("ONNX model not found: {}", path.display());
    }

    let sig = probe_signature(&path)?;

    println!("model = {}", path.display());
    println!("=== inputs ===");
    for (i, inp) in sig.inputs.iter().enumerate() {
        println!("  input[{i}]  {inp}");
    }
    println!("=== outputs ===");
    for (i, out) in sig.outputs.iter().enumerate() {
        println!("  output[{i}] {out}");
    }
    println!("PROBE OK");
    Ok(())
}
