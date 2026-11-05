//! 模型加载、tokenize、ONNX 推理（池化见 [`crate::pooling`]）。
//!
//! `Embedder` 封装 session + tokenizer，并把 `embed()` 拆成
//! `tokenize` → `infer` → `Pooling::apply` 三步。三者之间的数据顺序、
//! padding/truncation 参数、张量名与池化算法都必须与 `refs/ort_embed` 的原实现完全一致，
//! 否则 `rembed --selftest` 的数值基线会漂移。
//!
//! 与原实现的差别只有两处，都是为了作为库被复用：
//! - ONNX 相对路径与线程数由调用方传入（原来硬编码在常量里）；
//! - session 通过 [`crate::signature::open_session`] 创建，共享进程级 ORT Environment
//!   （原来每次 `load` 都调一次 `ort::init()`）。

use std::path::Path;

use anyhow::{bail, Context, Result};
use ort::session::Session;
use ort::value::{DynValue, Tensor};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};

use crate::pooling::Pooling;
use crate::signature::open_session;

/// tokenizer 文件名（相对模型目录）
const TOKENIZER_FILE: &str = "tokenizer.json";
/// pad token 的 id 与 token，取自 tokenizer_config.json 的 `pad_token`
/// （`tokenizer.json` 的 added_tokens 里 151643 的内容即该串）。
/// 与 config.json 的 `pad_token_id` 一致；换模型时必须一起改。
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
    /// 只读 `batch`/`seq_len` 字段，**不**从 `ids.len()` 反推——`infer` 会把 `ids`
    /// take 走（`Vec` 变空），之后池化仍需要正确的形状信息。
    fn dims(&self) -> [i64; 2] {
        [self.batch as i64, self.seq_len as i64]
    }
}

/// 封装 session + tokenizer 的嵌入器
pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
    pooling: Pooling,
    /// 输出嵌入维度，由图签名探测得出，不接受外部配置。
    dim: usize,
    output_name: String,
    /// tokenizer 截断长度，用于判断 `seq_len == max_length` 的截断信号
    max_length: usize,
    /// 上一次 `embed()` 是否有条文本撞上 `max_length`。
    ///
    /// 这是**启发式**：batch 内最长序列恰好等于 max_length 时也会置位（此时并未截断）。
    /// 宁可多警告一次，也不要长函数的尾部被静默吃掉而无人知晓。
    truncation_hit: bool,
}

impl Embedder {
    /// 加载模型与 tokenizer。
    ///
    /// - `model_dir`：模型目录
    /// - `onnx_rel_path`：ONNX 图相对 `model_dir` 的路径（harrier 本地是双层 `onnx/onnx/`）
    /// - `max_length`：tokenizer 截断长度
    /// - `intra_threads`：ORT intra-op 线程数，`0` = 由 ORT 决定
    pub fn load(
        model_dir: &Path,
        onnx_rel_path: &str,
        max_length: usize,
        intra_threads: usize,
    ) -> Result<Self> {
        let onnx_path = model_dir.join(onnx_rel_path);
        let tokenizer_path = model_dir.join(TOKENIZER_FILE);
        if !onnx_path.exists() {
            bail!("ONNX model not found: {}", onnx_path.display());
        }
        if !tokenizer_path.exists() {
            bail!("tokenizer.json not found: {}", tokenizer_path.display());
        }

        let session = open_session(&onnx_path, intra_threads)?;
        let tokenizer = Self::load_tokenizer(&tokenizer_path, max_length)?;
        let (pooling, dim, output_name) = Self::detect_pooling(&session)?;

        Ok(Self {
            session,
            tokenizer,
            pooling,
            dim,
            output_name,
            max_length,
            truncation_hit: false,
        })
    }

    /// 加载 tokenizer：开 padding（batch 内最长）+ truncation（max_length）
    fn load_tokenizer(path: &Path, max_length: usize) -> Result<Tokenizer> {
        let mut tokenizer =
            Tokenizer::from_file(path).map_err(|e| anyhow::anyhow!("failed to load tokenizer: {e}"))?;
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
            .map_err(|e| anyhow::anyhow!("failed to set truncation: {e}"))?;
        Ok(tokenizer)
    }

    /// 依据输出签名决定池化策略，并探测嵌入维度
    fn detect_pooling(session: &Session) -> Result<(Pooling, usize, String)> {
        let out = session.outputs().first().context("model has no outputs")?;
        let output_name = out.name().to_string();
        match out.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => {
                let dims: Vec<i64> = shape.iter().copied().collect();
                let (pooling, dim) = Pooling::detect(&output_name, &dims)?;
                Ok((pooling, dim, output_name))
            }
            other => bail!("output is not a tensor: {other:?}"),
        }
    }

    /// 探测出的嵌入维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 实际使用的输出张量名
    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    /// 生效的池化策略
    pub fn pooling(&self) -> Pooling {
        self.pooling
    }

    /// 上一次 `embed()` 是否有条文本撞上 `max_length`（见 `truncation_hit` 字段注释）
    pub fn truncation_hit(&self) -> bool {
        self.truncation_hit
    }

    fn set_truncation_hit(&mut self, hit: bool) {
        self.truncation_hit = hit;
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
        self.pooling
            .apply(input.batch, self.dim, &shape, &input.mask, &data)
    }

    /// 1) tokenize：batch 内 padding 到最长 + truncation
    fn tokenize(&mut self, texts: &[&str]) -> Result<BatchInput> {
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenize failed: {e}"))?;

        let batch = encodings.len();
        let seq_len = encodings[0].get_ids().len();
        if encodings.iter().any(|e| e.get_ids().len() != seq_len) {
            bail!("sequence lengths differ after padding (should be impossible)");
        }

        // tokenizers 用的是 u32，ONNX 侧要求 int64
        let mut ids: Vec<i64> = Vec::with_capacity(batch * seq_len);
        let mut mask: Vec<i64> = Vec::with_capacity(batch * seq_len);
        for enc in &encodings {
            ids.extend(enc.get_ids().iter().map(|&v| v as i64));
            mask.extend(enc.get_attention_mask().iter().map(|&v| v as i64));
        }

        // padding 到 batch 内最长，因此 seq_len == max_length 说明至少有一条撞上了上限
        self.set_truncation_hit(seq_len == self.max_length);

        Ok(BatchInput {
            batch,
            seq_len,
            ids,
            mask,
        })
    }

    /// 2) 推理：跑一次 session，取出输出张量并转成 (shape, f32 数据)
    ///
    /// `input_ids` 用 `mem::take` 零拷贝取走（池化只读 `mask`，从不读 `ids`）；
    /// `attention_mask` 必须留给池化找 last-token，所以仍需 clone 一次。
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
            .context("inference failed")?;

        let value = outputs
            .get(self.output_name.as_str())
            .with_context(|| format!("output {} missing from results", self.output_name))?;
        extract_f32(value)
    }
}

/// 从输出张量里取出 (shape, f32 数据)，自动兼容 f16 / f32
fn extract_f32(value: &DynValue) -> Result<(Vec<i64>, Vec<f32>)> {
    if let Ok((shape, data)) = value.try_extract_tensor::<f32>() {
        return Ok((shape.iter().copied().collect(), data.to_vec()));
    }
    if let Ok((shape, data)) = value.try_extract_tensor::<half::f16>() {
        let v: Vec<f32> = data.iter().map(|x| x.to_f32()).collect();
        return Ok((shape.iter().copied().collect(), v));
    }
    bail!("output is neither an f32 nor an f16 tensor")
}
