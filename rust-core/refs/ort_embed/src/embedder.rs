//! 模型加载、tokenize、ONNX 推理（池化见 `pooling` 模块）。
//!
//! `Embedder` 封装 session + tokenizer，并把 `embed()` 拆成
//! `tokenize` → `infer` → `pooling::Pooling::apply` 三个步骤，便于单独阅读与验证。
//! 三者之间的数据顺序、padding/truncation 参数、张量名、池化算法
//! 都必须与重构前完全一致，否则 `--selftest` 的数值会漂移。

use std::path::Path;

use anyhow::{bail, Context, Result};
use ort::session::Session;
use ort::value::{DynValue, Tensor};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};

use crate::pooling::Pooling;

/// ONNX 文件相对模型目录的路径（注意本地是 onnx/onnx 双重嵌套）
///
/// 只在本模块使用，故不 `pub`；`cli.rs` 的 help 文案里有同样的字面量，改动时需手工同步。
const ONNX_REL_PATH: &str = "onnx/onnx/model_fp16.onnx";
/// tokenizer 文件名
const TOKENIZER_FILE: &str = "tokenizer.json";
/// pad token：`<|endoftext|>`，与 tokenizer_config.json / config.json 的 pad_token_id 一致
const PAD_TOKEN_ID: u32 = 151643;
const PAD_TOKEN: &str = "<|endoftext|>";

/// tokenize 之后的 batch 视图：扁平化的 ids / attention_mask 加形状信息
struct BatchInput {
    batch: usize,
    seq_len: usize,
    ids: Vec<i64>,
    mask: Vec<i64>,
}

impl BatchInput {
    /// 两个输入张量共用的形状 `[batch, seq_len]`。
    ///
    /// 只读 `batch`/`seq_len` 字段，**不**从 `ids.len()` 反推——`infer` 会把
    /// `ids` take 走（`Vec` 变空），之后池化仍需要正确的形状信息。
    fn dims(&self) -> [i64; 2] {
        [self.batch as i64, self.seq_len as i64]
    }
}

/// 封装 session + tokenizer 的嵌入器
pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
    pooling: Pooling,
    /// 输出嵌入维度
    dim: usize,
    /// 使用的输出名
    output_name: String,
}

impl Embedder {
    /// 加载模型与 tokenizer
    pub fn load(model_dir: &Path, max_length: usize) -> Result<Self> {
        let onnx_path = model_dir.join(ONNX_REL_PATH);
        let tokenizer_path = model_dir.join(TOKENIZER_FILE);
        if !onnx_path.exists() {
            bail!("找不到 ONNX 模型: {}", onnx_path.display());
        }
        if !tokenizer_path.exists() {
            bail!("找不到 tokenizer: {}", tokenizer_path.display());
        }

        let session = Self::load_session(&onnx_path)?;
        let tokenizer = Self::load_tokenizer(&tokenizer_path, max_length)?;
        let (pooling, dim, output_name) = Self::detect_pooling(&session)?;

        Ok(Self {
            session,
            tokenizer,
            pooling,
            dim,
            output_name,
        })
    }

    /// 创建 ONNX Runtime 环境并加载模型。
    ///
    /// 一个进程只能创建一个 Environment，因此 `ort::init()` 必须只在
    /// 这条路径上调用一次（位置与重构前相同：文件存在性检查之后）。
    fn load_session(onnx_path: &Path) -> Result<Session> {
        let env = ort::init().with_name("rembed").build()?;
        let session = Session::builder(&env)?
            .commit_from_file(onnx_path)
            .with_context(|| format!("加载 ONNX 模型失败: {}", onnx_path.display()))?;
        Ok(session)
    }

    /// 加载 tokenizer：开 padding（batch 内最长）+ truncation（max_length）
    fn load_tokenizer(path: &Path, max_length: usize) -> Result<Tokenizer> {
        let mut tokenizer =
            Tokenizer::from_file(path).map_err(|e| anyhow::anyhow!("加载 tokenizer 失败: {e}"))?;
        let mut pad = tokenizer.get_padding().cloned().unwrap_or_default();
        pad.strategy = PaddingStrategy::BatchLongest;
        pad.pad_id = PAD_TOKEN_ID;
        pad.pad_token = PAD_TOKEN.to_string();
        tokenizer.with_padding(Some(PaddingParams { ..pad }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length,
                strategy: TruncationStrategy::LongestFirst,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("设置 truncation 失败: {e}"))?;
        Ok(tokenizer)
    }

    /// 依据输出签名决定池化策略
    fn detect_pooling(session: &Session) -> Result<(Pooling, usize, String)> {
        let out = session.outputs().first().context("模型没有任何输出")?;
        let output_name = out.name().to_string();
        match out.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => {
                let dims: Vec<i64> = shape.iter().copied().collect();
                let (pooling, dim) = Pooling::detect(&output_name, &dims)?;
                Ok((pooling, dim, output_name))
            }
            other => bail!("输出不是张量: {other:?}"),
        }
    }

    /// 打印模型 input / output 签名
    pub fn dump_signature(&self) {
        println!("=== 模型输入签名 ===");
        for (i, inp) in self.session.inputs().iter().enumerate() {
            println!("  input[{i}]  name={:<16} dtype={}", inp.name(), inp.dtype());
        }
        println!("=== 模型输出签名 ===");
        for (i, out) in self.session.outputs().iter().enumerate() {
            println!("  output[{i}] name={:<16} dtype={}", out.name(), out.dtype());
        }
        println!("=== 推断结论 ===");
        println!("  池化策略      = {:?}", self.pooling);
        println!("  嵌入维度      = {}", self.dim);
        println!("  使用的输出名  = {}", self.output_name);
    }

    /// 对一个 batch 的文本做嵌入，返回每条文本的 f32 向量
    pub fn embed<S: AsRef<str>>(&mut self, texts: &[S]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let refs: Vec<&str> = texts.iter().map(AsRef::as_ref).collect();
        let mut input = self.tokenize(&refs)?;
        let (shape, data) = self.infer(&mut input)?;
        // 3) 池化：按策略把图输出转成每条的嵌入向量（见 `pooling` 模块）
        self.pooling
            .apply(input.batch, self.dim, &shape, &input.mask, &data)
    }

    /// 1) tokenize：batch 内 padding 到最长 + truncation
    fn tokenize(&self, texts: &[&str]) -> Result<BatchInput> {
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenize 失败: {e}"))?;

        let batch = encodings.len();
        let seq_len = encodings[0].get_ids().len();
        if encodings.iter().any(|e| e.get_ids().len() != seq_len) {
            bail!("padding 后 batch 内序列长度不一致（不该发生）");
        }

        // tokenizers 用的是 u32，ONNX 侧要求 int64
        let mut ids: Vec<i64> = Vec::with_capacity(batch * seq_len);
        let mut mask: Vec<i64> = Vec::with_capacity(batch * seq_len);
        for enc in &encodings {
            ids.extend(enc.get_ids().iter().map(|&v| v as i64));
            mask.extend(enc.get_attention_mask().iter().map(|&v| v as i64));
        }

        Ok(BatchInput {
            batch,
            seq_len,
            ids,
            mask,
        })
    }

    /// 2) 推理：跑一次 session，取出输出张量并转成 (shape, f32 数据)
    ///
    /// `input_ids` 用 `mem::take` 零拷贝取走（`pool` 只读 `mask`，从不读 `ids`）；
    /// `attention_mask` 必须留给 `pool` 找 last-token，所以仍需 clone 一次。
    fn infer(&mut self, input: &mut BatchInput) -> Result<(Vec<i64>, Vec<f32>)> {
        let dims = input.dims();
        let input_ids = Tensor::from_array((dims, std::mem::take(&mut input.ids)))?;
        let attention_mask = Tensor::from_array((dims, input.mask.clone()))?;

        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
            ])
            .context("推理失败")?;

        let value = outputs
            .get(self.output_name.as_str())
            .with_context(|| format!("输出里找不到 {}", self.output_name))?;
        extract_f32(value)
    }
}

/// 从输出张量里取出 (shape, f32 数据)，自动兼容 f16 / f32
fn extract_f32(value: &DynValue) -> Result<(Vec<i64>, Vec<f32>)> {
    if let Ok((shape, data)) = value.try_extract_tensor::<f32>() {
        return Ok((shape.iter().copied().collect(), data.to_vec()));
    }
    if let Ok((shape, data)) = value.try_extract_tensor::<half::f16>() {
        // fp16 输出：先转 f32 再用
        let v: Vec<f32> = data.iter().map(|x| x.to_f32()).collect();
        return Ok((shape.iter().copied().collect(), v));
    }
    bail!("输出既不是 f32 也不是 f16 张量");
}
