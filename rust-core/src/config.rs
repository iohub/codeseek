use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::info;

/// 全文检索配置
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HybridSearchConfig {
    #[serde(default = "default_true")]
    pub enable_bm25: bool,
    #[serde(default = "default_100")]
    pub bm25_top_k: usize,
    #[serde(default = "default_100")]
    pub vector_top_k: usize,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f64,
    #[serde(default = "default_20")]
    pub rrf_top_k: usize,
    #[serde(default = "default_short_code_threshold")]
    pub short_code_threshold: usize,
    #[serde(default = "default_short_code_penalty")]
    pub short_code_penalty: f64,
}

fn default_true() -> bool { true }
fn default_100() -> usize { 100 }
fn default_20() -> usize { 20 }
fn default_rrf_k() -> f64 { 60.0 }
fn default_short_code_threshold() -> usize { 30 }
fn default_short_code_penalty() -> f64 { 0.5 }

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            enable_bm25: default_true(),
            bm25_top_k: default_100(),
            vector_top_k: default_100(),
            rrf_k: default_rrf_k(),
            rrf_top_k: default_20(),
            short_code_threshold: default_short_code_threshold(),
            short_code_penalty: default_short_code_penalty(),
        }
    }
}

/// Qwen3-Embedding 官方查询侧指令（instruction-aware，非对称检索）。
///
/// 官方规范：只有 **query** 侧需要加 `Instruct: {task}` + `Query:{query}` 前缀；
/// document / passage（本项目即入库的代码块、知识记录）**绝对不加**任何指令，
/// 否则已有 LanceDB 向量索引与新查询向量处于不同分布，检索质量退化。
/// 官方另建议：指令用英文书写，每个 query 配一句任务描述。
///
/// 任务描述文本对远程/本地两个后端通用；**模板字面量按后端分化**（见
/// [`EmbeddingBackend::format_query`]），因为两家官方写法不同。
pub const DEFAULT_QUERY_INSTRUCT: &str =
    "Given a code search query, retrieve relevant functions or code snippets that answer the query.";

/// 远程 Qwen3-Embedding 官方模板：`Query:` 冒号后**没有空格**。
///
/// `pub` 是为了让 `services::embedding_service::build_instruct_query` 复用同一个字面量——
/// 模板只许有一处定义，否则 Remote 的逐字节基线测试与后端实现会各自漂移。
pub const REMOTE_QUERY_TEMPLATE: &str = "Instruct: {task}\nQuery:{query}";
/// harrier 官方模板：`Query:` 冒号后**有一个空格**。
pub const LOCAL_QUERY_TEMPLATE: &str = "Instruct: {task}\nQuery: {query}";

/// 按模板渲染查询前缀。`instruct` 为空或 `query` 为空白时原样返回 query
/// （保持「空输入报错」的既有行为，不会只把指令本身去 embedding）。
pub fn render_query(template: &str, instruct: &str, query: &str) -> String {
    if instruct.is_empty() || query.trim().is_empty() {
        return query.to_string();
    }
    template.replace("{task}", instruct).replace("{query}", query)
}

/// 嵌入后端：远程 API 与本地 ONNX 二选一。
///
/// 用 serde 内部标签（`provider` 字段）表达，与旧版平铺 JSON **同形**：
/// `{"provider":"openai-compatible","model":...,"api_token":...}` 直接命中 `Remote`。
/// 但「标签字段缺失」不会走 `#[serde(default)]`，所以 `Deserialize` 是手写的
/// （见 [`EmbeddingBackend`] 的 impl）：缺 `provider` 视为 `openai-compatible`。
///
/// 结构上不可能出现「provider 是 local 却带着生效的 api_token」这类半吊子配置。
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum EmbeddingBackend {
    /// 远程 OpenAI 兼容 `/embeddings` 端点（现状默认）
    #[serde(rename = "openai-compatible")]
    Remote(RemoteEmbedding),
    /// 本地 ONNX 推理（ort + HuggingFace tokenizers，离线）
    #[serde(rename = "local-onnx")]
    LocalOnnx(LocalOnnxEmbedding),
}

/// 远程后端配置。字段语义与旧 `EmbeddingConfig` 逐一对应，默认值未变。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RemoteEmbedding {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default = "default_api_base_url")]
    pub api_base_url: String,
    #[serde(default = "default_dimensions")]
    pub dimensions: usize,
    /// 查询侧指令前缀中的任务描述。**仅作用于查询，不作用于入库**。
    ///
    /// 三态语义（与旧实现一致）：
    /// - 字段缺省 → [`DEFAULT_QUERY_INSTRUCT`]
    /// - 显式 `""` 或纯空白 → **关闭** instruct 注入，查询保持裸文本
    /// - 其它文本 → 使用该文本（首尾空白裁掉）
    #[serde(default = "default_instruct")]
    pub instruct: Option<String>,
}

/// 本地 ONNX 后端配置。
///
/// 注意**没有 `dimensions` 字段**：本地维度一律由 ONNX 图输出签名探测
/// （`local_embed::Embedder::load` → `Pooling::detect`），不给配错的机会。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LocalOnnxEmbedding {
    /// 模型目录，内部拼 `onnx_rel_path` 与 `tokenizer.json`
    #[serde(default = "default_model_dir")]
    pub model_dir: PathBuf,
    /// ONNX 图相对模型目录的路径（harrier 本地是双层 `onnx/onnx/`）
    #[serde(default = "default_onnx_rel_path")]
    pub onnx_rel_path: String,
    /// tokenizer 截断长度。harrier 的 `max_position_embeddings = 32768`，2048 安全
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    /// ORT intra-op 线程数；`0` = 交给 ONNX Runtime 决定
    #[serde(default)]
    pub intra_threads: usize,
    /// 与 [`RemoteEmbedding::instruct`] 同样的三态语义
    #[serde(default = "default_instruct")]
    pub instruct: Option<String>,
}

fn default_model() -> String { "Qwen/Qwen3-Embedding-4B".to_string() }
fn default_api_base_url() -> String { "https://api.siliconflow.cn/v1".to_string() }
fn default_dimensions() -> usize { 2560 }
fn default_instruct() -> Option<String> { Some(DEFAULT_QUERY_INSTRUCT.to_string()) }
fn default_model_dir() -> PathBuf {
    PathBuf::from("/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX")
}
fn default_onnx_rel_path() -> String { local_embed::DEFAULT_ONNX_REL_PATH.to_string() }
fn default_max_length() -> usize { 2048 }

impl Default for RemoteEmbedding {
    fn default() -> Self {
        Self {
            model: default_model(),
            api_token: String::new(),
            api_base_url: default_api_base_url(),
            dimensions: default_dimensions(),
            instruct: default_instruct(),
        }
    }
}

impl Default for LocalOnnxEmbedding {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            onnx_rel_path: default_onnx_rel_path(),
            max_length: default_max_length(),
            intra_threads: 0,
            instruct: default_instruct(),
        }
    }
}

impl Default for EmbeddingBackend {
    /// 缺省后端 = 远程，保持旧配置（整个 `embedding` 段缺失）的行为不变。
    fn default() -> Self {
        Self::Remote(RemoteEmbedding::default())
    }
}

/// 手写反序列化用的同形私有枚举：`EmbeddingBackend` 自己不能 derive。
#[derive(Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
enum BackendWire {
    #[serde(rename = "openai-compatible")]
    Remote(RemoteEmbedding),
    #[serde(rename = "local-onnx")]
    LocalOnnx(LocalOnnxEmbedding),
}

impl From<BackendWire> for EmbeddingBackend {
    fn from(w: BackendWire) -> Self {
        match w {
            BackendWire::Remote(r) => Self::Remote(r),
            BackendWire::LocalOnnx(l) => Self::LocalOnnx(l),
        }
    }
}

impl<'de> Deserialize<'de> for EmbeddingBackend {
    /// 反序列化，兼容三种输入：
    /// 1. 有 `"provider"` 标签 → 按标签分派（未知标签直接报错，不静默回退）
    /// 2. **没有** `"provider"` → 旧配置，按 `openai-compatible` 解析
    /// 3. `null` → 用 [`EmbeddingBackend::default`]
    ///
    /// 借助 `serde_json::Value` 做一遍预读来判断标签有无，因此本 impl 与实际
    /// 配置格式（JSON）耦合；`Config::load` 本来就用 serde_json。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.as_object() {
            None => Ok(Self::default()),
            Some(obj) => {
                let has_tag = obj
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .is_some();
                if has_tag {
                    serde_json::from_value::<BackendWire>(value)
                        .map(Self::from)
                        .map_err(de::Error::custom)
                } else {
                    serde_json::from_value::<RemoteEmbedding>(value)
                        .map(Self::Remote)
                        .map_err(de::Error::custom)
                }
            }
        }
    }
}

/// `instruct` 三态的公共实现：`None` → 默认指令，空白 → 关闭，其余 → trim 后使用。
fn resolve_instruct(instruct: Option<&str>) -> &str {
    match instruct {
        None => DEFAULT_QUERY_INSTRUCT,
        Some(s) if s.trim().is_empty() => "",
        Some(s) => s.trim(),
    }
}

impl EmbeddingBackend {
    fn instruct_field(&self) -> Option<&str> {
        match self {
            Self::Remote(r) => r.instruct.as_deref(),
            Self::LocalOnnx(l) => l.instruct.as_deref(),
        }
    }

    /// 实际生效的查询侧任务描述。返回**空串**表示关闭 instruct 注入。
    ///
    /// `None`（字段缺省）与 `Some("")`（显式关闭）走不同分支，
    /// 因此调用方不能用 `unwrap_or_default()` 抹平这个区别。
    pub fn effective_instruct(&self) -> &str {
        resolve_instruct(self.instruct_field())
    }

    /// 查询侧指令前缀模板（按后端分化）。
    fn query_template(&self) -> &'static str {
        match self {
            Self::Remote(_) => REMOTE_QUERY_TEMPLATE,
            Self::LocalOnnx(_) => LOCAL_QUERY_TEMPLATE,
        }
    }

    /// **查询专用**：按该后端的官方模板拼指令前缀。
    ///
    /// 入库/文档路径必须继续发送裸文本，否则已建好的向量索引与新查询向量
    /// 落在不同分布。
    ///
    /// - 指令为空（配置显式关闭）→ 原样返回 query
    /// - query 为空白 → 原样返回，保持「空输入报错」的既有行为，不把指令单独 embedding
    pub fn format_query(&self, query: &str) -> String {
        render_query(self.query_template(), self.effective_instruct(), query)
    }

    /// 该后端是否为「同一个索引」。
    ///
    /// 远程不含 `api_base_url`：同模型不同端点产出同分布向量，换端点不该换索引。
    /// 本地含 `max_length`：截断点变了向量就变了。
    pub fn identity(&self) -> String {
        match self {
            Self::Remote(r) => format!("remote|{}|{}", r.model, r.dimensions),
            Self::LocalOnnx(l) => {
                let dir = l
                    .model_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| l.model_dir.to_string_lossy().into_owned());
                format!("local|{}|{}|{}", dir, l.onnx_rel_path, l.max_length)
            }
        }
    }

    /// 后端指纹（8 位十六进制），同时驱动 LanceDB 表名、增量哈希文件名与缓存 key。
    ///
    /// 把「模型身份 + 维度 + 截断长度」编进名字，维度/模型错配在结构上就无法发生：
    /// 换后端等于换一套独立的索引，不会把 1024 维向量塞进 2560 维的 FixedSizeList，
    /// 也不会因为增量哈希命中而在新后端下跳过全部文件（产生「看着成功、实际空」的索引）。
    pub fn fingerprint(&self) -> String {
        format!("{:x}", md5::compute(self.identity().as_bytes()))[..8].to_string()
    }

    /// 模型标识，用于日志与 provider 上报。
    pub fn model_id(&self) -> String {
        match self {
            Self::Remote(r) => r.model.clone(),
            Self::LocalOnnx(l) => l
                .model_dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "local-onnx".to_string()),
        }
    }

    /// 配置里已知的维度。本地后端返回 `None`——维度由图签名探测，加载后才知道。
    pub fn known_dimensions(&self) -> Option<usize> {
        match self {
            Self::Remote(r) => Some(r.dimensions),
            Self::LocalOnnx(_) => None,
        }
    }

    /// 代码符号向量表名：项目 hash + 后端指纹。
    ///
    /// 指纹进表名后，「把 1024 维向量写进 2560 维表」在结构上就不可能发生：
    /// 换后端等于换一张全新的空表，旧表原地保留但不被读写。
    pub fn code_table_name(&self, project_hash: &str) -> String {
        let h = &project_hash[..project_hash.len().min(8)];
        format!("codeseek_{h}_{}", self.fingerprint())
    }

    /// 增量文件哈希的持久化文件名（同样按指纹分桶）。
    ///
    /// 这条不是审美选择：旧实现用单一的 `embedding_hashes.json`，从远程切到本地后
    /// 会因为 file md5 未变而**跳过所有文件**，产出一个「命令报成功、表里没向量」的
    /// 空索引。
    pub fn hashes_file_name(&self) -> String {
        format!("embedding_hashes_{}.json", self.fingerprint())
    }

    /// 该后端当前是否可用于建索引/检索。Err 里是给用户的英文原因。
    pub fn check_available(&self) -> Result<(), String> {
        match self {
            Self::Remote(r) => {
                if !r.api_token.is_empty()
                    || std::env::var("SILICONFLOW_API_KEY").is_ok()
                {
                    Ok(())
                } else {
                    Err("no embedding API token configured (set embedding.api_token or \
                         the SILICONFLOW_API_KEY environment variable)"
                        .to_string())
                }
            }
            Self::LocalOnnx(l) => {
                let onnx = l.model_dir.join(&l.onnx_rel_path);
                if !onnx.exists() {
                    return Err(format!(
                        "ONNX model not found: {} (check embedding.model_dir / onnx_rel_path)",
                        onnx.display()
                    ));
                }
                let tokenizer = l.model_dir.join("tokenizer.json");
                if !tokenizer.exists() {
                    return Err(format!("tokenizer.json not found: {}", tokenizer.display()));
                }
                Ok(())
            }
        }
    }
}

/// 索引配置
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IndexConfig {
    #[serde(default = "default_min_code_block_length")]
    pub min_code_block_length: usize,
    #[serde(default)]
    pub enable_reranker: bool,
    #[serde(default)]
    pub hybrid: HybridSearchConfig,
    #[serde(default)]
    pub reranker: RerankerConfig,
}

fn default_min_code_block_length() -> usize { 16 }

/// 重排序配置（保留向后兼容）
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct RerankerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_reranker_model")]
    pub model: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default = "default_api_base_url")]
    pub api_base_url: String,
    #[serde(default = "default_10")]
    pub top_n: usize,
    #[serde(default = "default_5")]
    pub candidate_multiplier: usize,
    #[serde(default = "default_30")]
    pub timeout_secs: u64,
}

fn default_reranker_model() -> String { "BAAI/bge-reranker-v2-m3".to_string() }
fn default_10() -> usize { 10 }
fn default_5() -> usize { 5 }
fn default_30() -> u64 { 30 }

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            min_code_block_length: default_min_code_block_length(),
            enable_reranker: false,
            hybrid: HybridSearchConfig::default(),
            reranker: RerankerConfig {
                enabled: false,
                model: default_reranker_model(),
                api_token: String::new(),
                api_base_url: default_api_base_url(),
                top_n: default_10(),
                candidate_multiplier: default_5(),
                timeout_secs: default_30(),
            },
        }
    }
}

/// 全局配置（~/.codeseek/config.json）
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub embedding: EmbeddingBackend,
    #[serde(default)]
    pub index: IndexConfig,
    /// 已安装 git hooks 的项目 { project_path → [hook_names] }
    #[serde(default)]
    pub installed_hooks: HashMap<String, Vec<String>>,
}

impl Config {
    /// 全局配置文件路径
    pub fn global_config_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek").join("config.json")
    }

    /// 二进制存储目录
    pub fn bin_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek").join("bin")
    }

    /// 项目索引目录: ~/.codeseek/projects/<project_hash>/
    pub fn project_index_dir(project_hash: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek").join("projects").join(project_hash)
    }

    /// 全局共享缓存目录: ~/.codeseek/cache/
    pub fn cache_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek").join("cache")
    }

    /// 项目数据根目录: ~/.codeseek/projects/
    pub fn projects_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek").join("projects")
    }

    /// CodeSeek 全局配置根目录: ~/.codeseek/
    pub fn codeseek_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".codeseek")
    }

    /// 项目 LanceDB 向量数据库目录: ~/.codeseek/projects/<project_hash>/lancedb/
    pub fn lancedb_dir(project_hash: &str) -> PathBuf {
        Self::project_index_dir(project_hash).join("lancedb")
    }

    /// 项目 BM25 全文索引目录: ~/.codeseek/projects/<project_hash>/tantivy_bm25/
    pub fn bm25_dir(project_hash: &str) -> PathBuf {
        Self::project_index_dir(project_hash).join("tantivy_bm25")
    }

    /// 项目知识向量库目录: ~/.codeseek/projects/<project_hash>/knowledge/
    pub fn knowledge_dir(project_hash: &str) -> PathBuf {
        Self::project_index_dir(project_hash).join("knowledge")
    }

    /// 项目知识 BM25 全文索引目录: ~/.codeseek/projects/<project_hash>/knowledge_index/
    pub fn knowledge_bm25_dir(project_hash: &str) -> PathBuf {
        Self::project_index_dir(project_hash).join("knowledge_index")
    }

    /// 加载全局配置
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = Self::global_config_path();
        info!("Loading configuration from: {:?}", config_path);

        if !config_path.exists() {
            return Err(format!(
                "Config file not found at {:?}. Run 'codeseek' to set up.",
                config_path
            ).into());
        }

        let contents = fs::read_to_string(&config_path).map_err(|e| {
            format!("Failed to read config file at {:?}: {}", config_path, e)
        })?;

        let config: Config = serde_json::from_str(&contents).map_err(|e| {
            format!("Failed to parse config file: {}", e)
        })?;

        Ok(config)
    }

    /// 保存全局配置
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = Self::global_config_path();

        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)?;
        fs::write(&config_path, contents)?;

        info!("Configuration saved to: {:?}", config_path);
        Ok(())
    }

    /// 从当前目录检测项目根（向上找第一个 .git/ 目录）
    pub fn detect_project_root() -> Option<PathBuf> {
        let mut current = std::env::current_dir().ok()?;
        loop {
            if current.join(".git").exists() {
                return Some(current);
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// 计算项目 hash
    pub fn compute_project_hash(project_root: &PathBuf) -> String {
        format!("{:x}", md5::compute(project_root.to_string_lossy().as_bytes()))
    }


}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧配置文件（`provider` 在场、没有 `instruct` 字段）必须能正常反序列化，并取到默认指令。
    #[test]
    fn embedding_config_without_instruct_field_uses_default() {
        let json = r#"{
            "provider": "openai-compatible",
            "model": "Qwen/Qwen3-Embedding-4B",
            "api_token": "sk-test",
            "api_base_url": "https://api.siliconflow.cn/v1",
            "dimensions": 2560
        }"#;
        let cfg: EmbeddingBackend = serde_json::from_str(json).expect("legacy config must deserialize");
        assert!(matches!(cfg, EmbeddingBackend::Remote(_)));
        assert_eq!(cfg.instruct_field(), Some(DEFAULT_QUERY_INSTRUCT));
        assert_eq!(cfg.effective_instruct(), DEFAULT_QUERY_INSTRUCT);
    }

    /// 显式配置为空字符串 → 字段保持空串（而不是被默认值覆盖），并且表示关闭注入。
    #[test]
    fn embedding_config_explicit_empty_instruct_stays_empty() {
        let cfg: EmbeddingBackend = serde_json::from_str(r#"{"instruct": ""}"#).unwrap();
        assert_eq!(cfg.instruct_field(), Some(""));
        assert_eq!(cfg.effective_instruct(), "");
    }

    /// 纯空白同样视为关闭（避免拼出 "Instruct: \nQuery:xxx" 这种污染查询）。
    #[test]
    fn embedding_config_whitespace_only_instruct_disables_injection() {
        let cfg: EmbeddingBackend = serde_json::from_str(r#"{"instruct": "   "}"#).unwrap();
        assert_eq!(cfg.effective_instruct(), "");
    }

    /// 自定义指令：原样使用（首尾空白裁掉）。
    #[test]
    fn embedding_config_custom_instruct_is_used_verbatim_after_trim() {
        let cfg: EmbeddingBackend =
            serde_json::from_str(r#"{"instruct": "  Given a question, retrieve matching documents  "}"#)
                .unwrap();
        assert_eq!(
            cfg.effective_instruct(),
            "Given a question, retrieve matching documents"
        );
    }

    /// 整个 embedding 段缺失 → `EmbeddingBackend::default()` = Remote，仍是默认指令。
    ///
    /// 注意：旧实现里 `EmbeddingConfig` 由 derive(Default) 得到 `instruct == None`，
    /// 新实现的 `RemoteEmbedding::default()` 直接给 `Some(DEFAULT_QUERY_INSTRUCT)`。
    /// 两者 `effective_instruct()` 结果相同，因此这里断言的是**生效值**而不是原始字段。
    #[test]
    fn config_without_embedding_section_falls_back_to_default_instruct() {
        let cfg: Config = serde_json::from_str(r#"{}"#).expect("empty config object must deserialize");
        assert!(matches!(cfg.embedding, EmbeddingBackend::Remote(_)));
        assert_eq!(cfg.embedding.effective_instruct(), DEFAULT_QUERY_INSTRUCT);
        assert_eq!(
            DEFAULT_QUERY_INSTRUCT,
            "Given a code search query, retrieve relevant functions or code snippets that answer the query."
        );
    }

    /// 保存/加载往返：自定义值与「显式关闭」都能无损写回 JSON（标签字段一并写回）。
    #[test]
    fn instruct_field_survives_serialization_round_trip() {
        let remote = EmbeddingBackend::Remote(RemoteEmbedding {
            instruct: Some("custom instruct".to_string()),
            ..Default::default()
        });
        let json = serde_json::to_string(&remote).unwrap();
        assert!(json.contains("\"provider\":\"openai-compatible\""), "{json}");
        let back: EmbeddingBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back.effective_instruct(), "custom instruct");

        let off = EmbeddingBackend::Remote(RemoteEmbedding {
            instruct: Some(String::new()),
            ..Default::default()
        });
        let back: EmbeddingBackend = serde_json::from_str(&serde_json::to_string(&off).unwrap()).unwrap();
        assert_eq!(back.instruct_field(), Some(""));
        assert_eq!(back.effective_instruct(), "");
    }

    // ---- 后端判别：标签缺失 / 未知标签 / 本地标签 ----

    /// 完全没有 `provider` 键的旧配置 → 按 openai-compatible 解析（这是手写 Deserialize 的全部意义）。
    #[test]
    fn backend_without_provider_tag_deserializes_as_remote() {
        let cfg: EmbeddingBackend = serde_json::from_str(
            r#"{"model":"Qwen/Qwen3-Embedding-4B","api_token":"sk-x","dimensions":2560}"#,
        )
        .expect("legacy config without a provider tag must still parse");
        match cfg {
            EmbeddingBackend::Remote(r) => {
                assert_eq!(r.model, "Qwen/Qwen3-Embedding-4B");
                assert_eq!(r.api_token, "sk-x");
                assert_eq!(r.dimensions, 2560);
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    /// 未知 provider 必须**报错**，不能静默回退到远程（否则拼错后端的配置会悄悄用错模型）。
    #[test]
    fn unknown_provider_tag_is_an_error() {
        let err = serde_json::from_str::<EmbeddingBackend>(r#"{"provider":"local-torch"}"#)
            .expect_err("unknown tag must not fall back silently");
        assert!(err.to_string().contains("local-torch"), "{err}");
    }

    /// local-onnx 的默认值：模型目录 / 双层 onnx 路径 / 2048 截断 / 线程数交给 ORT。
    #[test]
    fn local_onnx_backend_fills_documented_defaults() {
        let cfg: EmbeddingBackend =
            serde_json::from_str(r#"{"provider":"local-onnx"}"#).expect("bare local tag must parse");
        match cfg {
            EmbeddingBackend::LocalOnnx(l) => {
                assert_eq!(
                    l.model_dir.to_str().unwrap(),
                    "/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX"
                );
                assert_eq!(l.onnx_rel_path, local_embed::DEFAULT_ONNX_REL_PATH);
                assert_eq!(l.max_length, 2048);
                assert_eq!(l.intra_threads, 0);
                assert_eq!(l.instruct.as_deref(), Some(DEFAULT_QUERY_INSTRUCT));
            }
            other => panic!("expected LocalOnnx, got {other:?}"),
        }
    }

    /// 本地后端**没有** dimensions 字段：维度只能由图签名探测，配了也不该被读。
    #[test]
    fn local_onnx_has_no_configurable_dimensions() {
        let cfg: EmbeddingBackend =
            serde_json::from_str(r#"{"provider":"local-onnx","dimensions":7}"#).unwrap();
        assert_eq!(cfg.known_dimensions(), None);
        // 远程后端则照实报告配置值
        let remote: EmbeddingBackend =
            serde_json::from_str(r#"{"provider":"openai-compatible","dimensions":7}"#).unwrap();
        assert_eq!(remote.known_dimensions(), Some(7));
    }

    // ---- 指纹：换模型/换截断长度必须换索引，换端点不能换索引 ----

    #[test]
    fn fingerprint_tracks_model_identity_and_truncation() {
        let base = EmbeddingBackend::Remote(RemoteEmbedding::default());
        let other_model = EmbeddingBackend::Remote(RemoteEmbedding {
            model: "BAAI/bge-m3".to_string(),
            ..Default::default()
        });
        let other_dims = EmbeddingBackend::Remote(RemoteEmbedding {
            dimensions: 1024,
            ..Default::default()
        });
        assert_ne!(base.fingerprint(), other_model.fingerprint());
        assert_ne!(base.fingerprint(), other_dims.fingerprint());
        assert_eq!(base.fingerprint().len(), 8);

        // 换 API 端点不改变向量分布 → 不该换索引
        let other_url = EmbeddingBackend::Remote(RemoteEmbedding {
            api_base_url: "https://other.example.com/v1".to_string(),
            ..Default::default()
        });
        assert_eq!(base.fingerprint(), other_url.fingerprint());

        // 本地：max_length 改变截断点 → 向量变 → 必须换索引
        let l0 = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default());
        let l1 = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding {
            max_length: 1024,
            ..Default::default()
        });
        assert_ne!(l0.fingerprint(), l1.fingerprint());
        // 远程 1024 维与本地 harrier 1024 维是不同分布 → 指纹必然不同
        assert_ne!(other_dims.fingerprint(), l0.fingerprint());
        // 指纹是确定性的（同一配置每次运行得到同一套表名）
        assert_eq!(l0.fingerprint(), l0.fingerprint());
    }

    // ---- 查询模板按后端分化 ----

    /// 远程模板 `Query:` 后**无空格**（与 Qwen3-Embedding 官方 README 逐字节一致）。
    #[test]
    fn remote_template_has_no_space_after_query_colon() {
        let cfg = EmbeddingBackend::Remote(RemoteEmbedding::default());
        let out = cfg.format_query("how to parse toml");
        assert_eq!(
            out.as_bytes(),
            format!("Instruct: {DEFAULT_QUERY_INSTRUCT}\nQuery:how to parse toml").as_bytes()
        );
        assert!(out.contains("\nQuery:h"), "{out}");
        assert!(!out.contains("Query: h"), "远程模板 Query: 之后不能有空格");
    }

    /// 本地模板 `Query:` 后**有一个空格**（harrier 官方写法）。
    #[test]
    fn local_template_has_one_space_after_query_colon() {
        let cfg = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default());
        let out = cfg.format_query("how to parse toml");
        assert_eq!(
            out.as_bytes(),
            format!("Instruct: {DEFAULT_QUERY_INSTRUCT}\nQuery: how to parse toml").as_bytes()
        );
        let (head, tail) = out.split_once('\n').expect("one newline separator");
        assert_eq!(head, format!("Instruct: {DEFAULT_QUERY_INSTRUCT}"));
        assert_eq!(tail, "Query: how to parse toml");
    }

    /// 两个后端共享「关闭注入」与「空白查询原样返回」的边界行为。
    #[test]
    fn both_backends_honour_disabled_and_blank_query() {
        for cfg in [
            EmbeddingBackend::Remote(RemoteEmbedding::default()),
            EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default()),
        ] {
            let off = match cfg {
                EmbeddingBackend::Remote(_) => EmbeddingBackend::Remote(RemoteEmbedding {
                    instruct: Some(String::new()),
                    ..Default::default()
                }),
                EmbeddingBackend::LocalOnnx(_) => EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding {
                    instruct: Some(String::new()),
                    ..Default::default()
                }),
            };
            assert_eq!(off.format_query("find login handler"), "find login handler");
            assert!(!off.format_query("q").contains("Instruct:"));
            // 空白查询不加前缀（保持「空输入报错」的既有行为）
            assert_eq!(cfg.format_query("   "), "   ");
        }
    }

    // ---- 可用性门控 ----

    /// 本地后端缺模型文件时必须给出**带路径**的错误，否则用户只看到「退回图搜索」。
    #[test]
    fn local_backend_reports_missing_model_path() {
        let cfg = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding {
            model_dir: PathBuf::from("/nonexistent/model/dir"),
            ..Default::default()
        });
        let err = cfg.check_available().expect_err("missing model must not be available");
        assert!(err.contains("/nonexistent/model/dir"), "{err}");
        assert!(err.contains("ONNX model not found"), "{err}");
    }

    /// 真实 harrier 目录在场时本地后端可用（未安装模型的开发机跳过）。
    #[test]
    fn local_backend_is_available_when_model_installed() {
        let cfg = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default());
        if cfg.check_available().is_err() {
            eprintln!("skipping: harrier model not installed at the default path");
            return;
        }
        assert!(cfg.check_available().is_ok());
    }
}
