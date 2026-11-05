//! 命令行接口：clap derive 的参数定义、默认值与输入归一化。
//!
//! 参数集合与原手写解析器完全等价（`--model-dir` / `--text` / `--query` /
//! `--instruction` / `--max-length` / `--dump-signature` / `--selftest` / `-h, --help`），
//! 唯一有意的变化是 `--help` 文本改由 clap 渲染。无参数调用的错误文案仍走
//! `validate()`，保持与重构前逐字一致。

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser};

/// 本地模型目录默认值（只在本模块的 `#[arg]` 里用，故不 `pub`）
const DEFAULT_MODEL_DIR: &str = "/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX";
/// 默认截断长度（同上，只在本模块使用）
const DEFAULT_MAX_LENGTH: usize = 2048;
/// 查询侧默认指令前缀（web search 场景，来自官方 model card）
pub const WEB_SEARCH_INSTRUCTION: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ";

/// 解析后的命令行参数
#[derive(Debug, Clone, Parser)]
#[command(
    name = "rembed",
    about = "rembed —— 本地 ONNX 文本嵌入推理",
    // 关掉自动 help，改由 `parse_validated()` 注册一个带中文说明的 -h/--help
    disable_help_flag = true,
    after_help = "示例:\n  # 普通文档嵌入\n  rembed --text \"hello world\"\n\n  # 查询侧加指令前缀\n  rembed --query --text \"how much protein should a female eat\"\n\n  # 复现 README 基准矩阵\n  rembed --selftest"
)]
pub struct Args {
    // 注意：下面 doc 注释与 arg 的 help 里各有一处 `onnx/onnx/model_fp16.onnx` 字面量。
    // clap 的 `help` 属性只接受字面量、不支持 const 插值，所以无法与
    // `embedder::ONNX_REL_PATH` 做编译期联动——改那边的常量时必须手工同步这里。
    /// 模型目录（内部自动拼 onnx/onnx/model_fp16.onnx 与 tokenizer.json）
    #[arg(
        long,
        value_name = "dir",
        default_value = DEFAULT_MODEL_DIR,
        help = "模型目录，内部自动拼 onnx/onnx/model_fp16.onnx 与 tokenizer.json"
    )]
    pub model_dir: PathBuf,

    /// 待编码文本，可重复出现；多条会一起 encode 成一个 batch
    #[arg(
        long = "text",
        value_name = "string",
        help = "待编码文本，可重复出现；多条会一起 encode 成一个 batch"
    )]
    pub text: Vec<String>,

    /// 给每条 --text 加上默认 web-search 指令前缀
    #[arg(long, help = "给每条 --text 加上默认 web-search 指令前缀")]
    pub query: bool,

    /// 自定义指令前缀（优先于 --query）
    #[arg(
        long,
        value_name = "string",
        help = "自定义指令前缀（优先于 --query）"
    )]
    pub instruction: Option<String>,

    /// 截断长度
    #[arg(
        long,
        value_name = "usize",
        default_value_t = DEFAULT_MAX_LENGTH,
        help = "截断长度"
    )]
    pub max_length: usize,

    /// 只打印模型 input/output 签名后退出
    #[arg(long, help = "只打印模型 input/output 签名后退出")]
    pub dump_signature: bool,

    /// 跑 README 的官方基准用例
    #[arg(long, help = "跑 README 的官方基准用例（2 query × 2 doc 分数矩阵）")]
    pub selftest: bool,
}

impl Args {
    /// 解析命令行并对业务约束做校验（clap 依赖不外泄给 `main`）。
    ///
    /// `-h/--help` 与参数格式错误由 clap 自行打印并退出（退出码 0 / 2），
    /// 这里只负责映射成中文帮助文案与保留原有的业务错误文案。
    pub fn parse_validated() -> Result<Self> {
        let cmd = Self::command()
            // 自动 help 被关掉，这里补一个语义完全等价的 -h/--help，并给中文说明
            .arg(
                Arg::new("help")
                    .short('h')
                    .long("help")
                    .action(ArgAction::Help)
                    .help("打印本帮助"),
            );
        let matches = cmd.get_matches();
        // `get_matches` 已保证格式合法；理论上不会走到 exit 分支。
        let args = Self::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
        args.validate()?;
        Ok(args)
    }

    /// 业务层校验：保留与重构前逐字一致的中文错误文案
    fn validate(&self) -> Result<()> {
        if !self.dump_signature && !self.selftest && self.text.is_empty() {
            bail!("没有输入文本：请用 --text 至少给一条，或用 --selftest / --dump-signature");
        }
        Ok(())
    }

    /// 生效的指令前缀：`--instruction` 优先于 `--query`（返回借用，不 clone）
    pub fn instruction_prefix(&self) -> Option<&str> {
        self.instruction
            .as_deref()
            .or_else(|| self.query.then_some(WEB_SEARCH_INSTRUCTION))
    }

    /// 按指令前缀拼出真正送进 tokenizer 的文本，顺序与 `--text` 一致
    pub fn prefixed_texts(&self) -> Vec<String> {
        let prefix = self.instruction_prefix();
        self.text
            .iter()
            .map(|t| match prefix {
                Some(p) => format!("{p}{t}"),
                None => t.clone(),
            })
            .collect()
    }
}
