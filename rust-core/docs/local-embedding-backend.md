# 本地 ONNX 嵌入后端设计（远程 API 与本地推理可配置共存）

状态：设计已定稿；P0（构建可行性）实测通过，待实施 P1
适用：`rust-core`（crate `codeseek`）
前置参考实现：`rust-core/refs/ort_embed`（可运行的 rembed CLI，selftest 已 PASS）

---

## 一、目标与非目标

### 目标

1. 向量化后端可在配置中二选一：`openai-compatible`（现状，远程 API）或 `local-onnx`（本地 ONNX 推理）。
2. 两个后端**互斥且结构上不可能配错**：不允许出现「provider 是 local 却带着 api_token 生效」这类半吊子状态。
3. 同一个项目可以在两个后端之间来回切换，切换后检索仍然正确（不产生维度/模型错配的脏向量）。
4. 本地后端不引入网络依赖、不引入 Python，`codeseek index` / `codeseek search` 全程离线可用。
5. 现有远程路径的行为**逐字节不变**：`Instruct:`/`Query:` 模板、维度默认值、批次大小、缓存语义、RRF 融合都不动。

### 非目标

- 不做本地 reranker。Cross-Encoder 精排（`BAAI/bge-reranker-v2-m3`）继续走 API；本地后端下若无 api_token，reranker 自动失效并降级到 RRF（现有降级路径已支持，见 `services/hybrid_search.rs:539-546`）。
- 不做旧索引迁移。已建好的 2560 维 LanceDB 表原地保留、不再被读取，也不自动删除。
- 不做多模型 ensemble / 量化推理调优（EP 选择、int8 等）。
- 不改 BM25 通道。Tantivy 索引与嵌入后端无关，两个后端共用同一份 `tantivy_bm25/`。

---

## 二、已核实的现状约束

以下每条都在本机实测或读源码确认过，是设计的前提，不是推测。

### 2.1 现有向量链路

| 事实 | 位置 |
|---|---|
| 向量化单位是「符号」，只索引 `FunctionDeclaration \| StructDeclaration` | `services/embedding_service.rs:609-627`、`codegraph/chunker.rs:37` |
| 短于 `min_code_block_length`(默认 16 字符) 的符号跳过 | `services/embedding_service.rs:635` |
| 远程调用：`POST {base}/embeddings`，默认 SiliconFlow + `Qwen/Qwen3-Embedding-4B` | `services/embedding_service.rs:191,233-243` |
| 维度 2560 硬默认，散布 5 处 | `config.rs:82`、`embedding_service.rs:337,396`、`knowledge/store.rs:74,173,186` |
| 输入上限 **64_000 字节**（非 token），char-boundary 安全截断 | `services/embedding_service.rs:36,43` |
| 查询侧模板 `Instruct: {task}\nQuery:{query}`，`Query:` 后**无空格**，有逐字节断言 | `services/embedding_service.rs:72`、测试 `:1091-1108` |
| 入库侧永远裸文本，两处显式禁止调用 `format_query` | `services/embedding_service.rs:703`、`knowledge/store.rs:165` |
| 缓存 key = `md5(model_name + code_block)`，全局共享 SQLite | `services/embedding_service.rs:656-658,329-331` |
| batch=20 条/次 API 调用；每 100 条 add 进 LanceDB | `services/embedding_service.rs:601,701,691` |
| LanceDB **无任何 ANN 索引**，`nearest_to` 是暴力 KNN | 全库无 `create_index` 调用（已 grep 确认） |
| 分数 = `1/(1 + _distance)`，`_distance` 是 L2 距离 | `services/embedding_service.rs:962` |
| `ensure_collection` 只判表是否存在，**不做 schema 迁移** | `services/embedding_service.rs:405-435` |
| 维度不符时 `upload_points` 只 `error!` + `continue`，**静默丢行** | `services/embedding_service.rs:810-813` |
| 增量：`embedding_hashes.json` 存 per-file md5，未变文件整体跳过 | `services/embedding_service.rs:531-564` |
| `KnowledgeStore` 每次 add/search 都**新建** provider（重读 config + 新 reqwest client） | `knowledge/store.rs:124`，调用点 `:161,431` |
| knowledge 表名硬编码 `"knowledge"` | `knowledge/store.rs:47` |
| 嵌入阶段的门控是 `!api_token.is_empty()`，共 3 处 | `main.rs:191,288`、`embedding_service.rs:343` |

### 2.2 ort / ONNX Runtime 侧

| 事实 | 位置 |
|---|---|
| `Session::run(&mut self, ...)` —— **需要可变借用** | `ort-2.0.0-rc.15/src/session/run.rs:48` |
| `run_async` 带 `#[cfg(all(target_family = "wasm", ...))]` —— **原生平台不可用** | `ort-2.0.0-rc.15/src/session/run.rs:187-188` |
| `Session` 是 `Send + Sync`（`unsafe impl`） | `ort-2.0.0-rc.15/src/session/mod.rs:367-368` |
| Environment 由 `OnceLock<Weak<EnvironmentInner>>` 管理，进程内单例 | `ort-2.0.0-rc.15/src/environment.rs:70` |
| onnxruntime 预编译静态库**已缓存本机**：80MB `libonnxruntime.a` | `~/Library/Caches/ort.pyke.io/dfbin/aarch64-apple-darwin/4033efca…/` |
| 下载源走 `dirs::cache_dir()` + `ort.pyke.io`；可用 `ORT_SKIP_DOWNLOAD`/`CARGO_NET_OFFLINE` 跳过下载 | `ort-sys-2.0.0-rc.15/build/main.rs:120`、`build/vars.rs:11` |
| 可用 `ORT_LIB_PATH`/`ORT_LIB_LOCATION` 指向本地库 | `ort-sys-2.0.0-rc.15/build/vars.rs:5` |

### 2.3 本机环境

- 物理内存 **16 GB**；`/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX` 已就位（图 287KB + `model_fp16.onnx_data` 1.2GB）。
- harrier `config.json`：`model_type = qwen3`、`hidden_size = 1024`、`max_position_embeddings = 32768`。
- **`github.com` 与 `huggingface.co` 均不可达**（curl 返回 000）。因此：ort 依赖本机已缓存的静态库；不得引入任何需要联网下载权重/二进制的方案。
- `rust-core/Cargo.toml` release profile：`lto = "fat"`、`opt-level = "z"`、`codegen-units = 1`、`strip = "symbols"`（`:68-71`）。
- 仓库根无 `Cargo.toml`；`rust-core/` 是单 package，`target/` 已有 14GB。
- `rust-core/` 下**没有** `.cargo/config.toml`（`refs/ort_embed/.cargo/config.toml` 里有 `MACOSX_DEPLOYMENT_TARGET = "13.4"`）。

---

## 三、设计核心：一个指纹驱动三处隔离

整个设计只有一个新抽象——**后端指纹（backend fingerprint, `fp`）**。它由「模型身份 + 维度 + 截断长度」确定性推导，然后同时驱动三件事：

```
                  ┌──────────────────────────────────────────────┐
   config.embedding ─▶  fp = short_hash(model_identity) + dims   │
                  └──────────────────────────────────────────────┘
                        │              │                │
             ┌──────────┘              │                └──────────┐
             ▼                         ▼                           ▼
   LanceDB 表名               增量哈希文件名              嵌入缓存 key
   codeseek_<h8>_<fp>   embedding_hashes_<fp>.json   md5(fp + code_block)
   knowledge_<fp>
```

**为什么这样设计**：现状最危险的不是「维度写错」，而是维度写错之后系统**不会报错**——`ensure_collection` 不迁移 schema（`:405`），`upload_points` 静默丢行（`:810`）。把维度与模型身份编码进表名，错配在结构上就无法发生：换模型 = 换表名 = 全新空表，不可能把 1024 维向量塞进 2560 维的 `FixedSizeList`。

同理，增量哈希文件必须带 `fp`：否则从远程切到本地后，`vectorize_directory` 会因为 `embedding_hashes.json` 里 file md5 未变而**整体跳过所有文件**（`:466-472`），索引看起来建好了、实际一条向量都没有。这是本设计发现的最隐蔽的一个坑。

缓存 key 带 `fp` 则天然覆盖了 `max_length`：截断长度一变，向量就变，缓存必须失效。现状 key 只含 `model + code_block`（`:657`），换后端时虽然会自动失效（模型名不同），但改 `max_length` 不会——这是现状的一个潜在错误来源，一并修掉。

### 3.1 指纹定义

```rust
/// 后端指纹：8 位十六进制，由模型身份 + 维度 + 截断长度确定性推导。
/// 同时用于 LanceDB 表名后缀、增量哈希文件名、嵌入缓存 key 前缀。
pub fn fingerprint(backend: &EmbeddingBackend) -> String {
    let identity = match backend {
        // 远程：base_url 不参与（同模型不同端点产出同分布向量）
        EmbeddingBackend::Remote(r) =>
            format!("remote|{}|{}|{}", r.model, r.dimensions, NO_TRUNCATION_TOKEN),
        // 本地：max_length 参与（截断点变了向量就变了）
        EmbeddingBackend::LocalOnnx(l) =>
            format!("local|{}|{}|{}", l.model_id, l.dimensions, l.max_length),
    };
    format!("{:x}", md5::compute(identity.as_bytes()))[..8].to_string()
}
```

- 远程后端的 `dimensions` 取自配置（API 不廉价地自报维度），`model_id` 即 `model` 字段。
- 本地后端的 `dimensions` **不取自配置，而是从 ONNX 输出签名探测**（`refs/ort_embed/src/pooling.rs:26-40` 的 `Pooling::detect` 已实现：输出名 `sentence_embedding` 或 rank==2 → 取最后一维）。harrier 探测结果恒为 1024。
- `model_id` 取模型目录下 `config.json` 的 `_name_or_path`／目录名，保证「换了权重目录 → 换了指纹 → 换了表」。

指纹只用于命名，不用于校验语义；语义校验见 §六。

---

## 四、配置模型：内部标签联合，向后兼容

### 4.1 结构

把 `EmbeddingConfig` 从「一层平铺 + provider 字符串」改成 **serde 内部标签联合**：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "kebab-case")]
pub enum EmbeddingBackend {
    /// 远程 OpenAI 兼容 /embeddings 端点（现状，默认）
    #[serde(rename = "openai-compatible")]
    Remote(RemoteEmbedding),
    /// 本地 ONNX 推理（ort + tokenizers，离线）
    #[serde(rename = "local-onnx")]
    LocalOnnx(LocalOnnxEmbedding),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEmbedding {
    pub model: String,            // default "Qwen/Qwen3-Embedding-4B"
    pub api_token: String,        // default ""（也回退到 SILICONFLOW_API_KEY）
    pub api_base_url: String,     // default "https://api.siliconflow.cn/v1"
    pub dimensions: usize,        // default 2560
    pub instruct: Option<String>, // 三态语义不变，见 config.rs:90-96
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOnnxEmbedding {
    /// 模型目录，内部拼 onnx_rel_path 与 tokenizer.json
    pub model_dir: PathBuf,       // default "/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX"
    /// ONNX 图相对路径（harrier 本地是双层 onnx/onnx/）
    pub onnx_rel_path: String,    // default "onnx/onnx/model_fp16.onnx"
    /// tokenizer 截断长度。harrier max_position_embeddings = 32768，2048 安全
    pub max_length: usize,        // default 2048
    /// 查询侧指令（task 文本）。缺省用 harrier 官方 web-search 指令
    pub instruct: Option<String>,
    /// ORT intra-op 线程数；0 = 交给 ONNX Runtime 自行决定
    pub intra_threads: usize,     // default 0
}
```

**注意：本地后端没有 `dimensions` 字段**——维度一律探测，不给用户配错的机会。

### 4.2 向后兼容论证

serde 的内部标签表示（internally tagged）与现有平铺 JSON **同形**：

```jsonc
// 现有 ~/.codeseek/config.json（不动也能解析）
{ "embedding": { "provider": "openai-compatible", "model": "...", "api_token": "...",
                 "api_base_url": "...", "dimensions": 2560 } }

// 切到本地
{ "embedding": { "provider": "local-onnx",
                 "model_dir": "/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX",
                 "max_length": 2048 } }
```

- `provider` 缺省时需 `#[serde(default)]` 兜到 `Remote`（现有配置里 `provider` 有默认值 `"openai-compatible"`，`config.rs:79`）。serde 内部标签枚举无法直接对「标签字段缺失」用 default，因此 `Config.embedding` 字段类型改为 `Option<EmbeddingBackend>` 并在访问器里 `unwrap_or(Remote::default())`，或实现自定义 `Deserialize`。**这是本设计唯一的实现难点，P2 第一步就用单测钉住**（用现存的 6 个 config 单测 `config.rs:290-358` 作为回归基线，它们必须全绿）。
- 不加 `deny_unknown_fields`：旧配置里多余的键被忽略，不报错。

### 4.3 查询侧模板按后端分化

两个模型族的官方模板**不同**，这是必须分化的字面量：

| 后端 | 模板 | 出处 |
|---|---|---|
| Remote (Qwen3-Embedding) | `Instruct: {task}\nQuery:{query}` —— `Query:` 后**无空格** | `services/embedding_service.rs:72` |
| LocalOnnx (harrier) | `Instruct: {task}\nQuery: {query}` —— `Query:` 后**有空格** | `refs/ort_embed/src/cli.rs:18-19` |

设计：`build_instruct_query` 从自由函数改成 `EmbeddingBackend` 的方法，模板由 variant 决定，可被配置覆盖（新增可选 `query_template`，缺省按上表）。现有 4 个逐字节断言测试（`:1091-1141`）继续覆盖 Remote 分支不动，LocalOnnx 分支补对称的 4 个测试。

**入库侧仍然永远裸文本**，两个后端一致。这条不变量由 §九 的闸门测试保护。

---

## 五、后端抽象与并发模型

### 5.1 trait 不动

`EmbeddingProvider`（`services/embedding_service.rs:155-177`）保持原签名：

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn get_embedding(&self, text: &str) -> Result<Vec<f32>, Box<dyn Error>>;
    async fn get_embeddings_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn Error>>;
    fn format_query(&self, query: &str) -> String;
    fn model(&self) -> String;
}
```

新增实现 `LocalOnnxEmbeddingProvider`。`EmbeddingService`、`HybridSearchService`、RRF、reranker、LanceDB 读写**全部不改**——这是 trait 接缝已经留对的红利。

### 5.2 为什么必须 `Mutex` + `spawn_blocking`

三个实测事实叠加，没有别的选择：

1. `Session::run` 要 `&mut self`（`run.rs:48`），而 trait 方法是 `&self` → 必须内部可变性。
2. `run_async` 是 wasm-only（`run.rs:187`）→ 原生平台只能调同步 `run`。
3. 一次 batch 推理是纯 CPU 密集、百毫秒级 → 直接在 async 上下文里调用会占死 tokio worker。

因此：

```rust
pub struct LocalOnnxEmbedder {
    session: Mutex<ort::session::Session>,   // run 需要 &mut self
    tokenizer: Tokenizer,                    // tokenizers::Tokenizer 自身 Send+Sync
    pooling: Pooling,
    dim: usize,
    output_name: String,
}

pub struct LocalOnnxEmbeddingProvider {
    inner: Arc<LocalOnnxEmbedder>,
    instruct: String,
    model_id: String,
}

#[async_trait]
impl EmbeddingProvider for LocalOnnxEmbeddingProvider {
    async fn get_embeddings_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn Error>> {
        let inner = Arc::clone(&self.inner);
        let owned: Vec<String> = texts.to_vec();
        tokio::task::spawn_blocking(move || inner.embed(&owned))
            .await
            .map_err(|e| format!("local embedding task panicked: {e}"))?
            .map_err(|e| e.into())
    }
    // get_embedding 委托给 get_embeddings_batch(&[text])，取第 0 条
}
```

**会话池大小固定为 1**，理由：16GB 内存下 1.2GB 权重只该有一份；ORT 的 intra-op 并行已经在单次 `run` 内吃满多核，多会话只会互相抢核并线性放大内存。`Mutex` 带来的串行化不是瓶颈——瓶颈是模型本身。

**懒加载**：`LocalOnnxEmbedder` 在 provider 首次被调用时才加载权重（`OnceLock`），而不是构造时。否则 `codeseek status` 这类不检索的命令也要付 1.2GB 加载成本。`ort::init()` 同样用 `OnceLock` 保护（`environment.rs:70` 已是进程内单例语义，但我们仍需保证只调一次以拿到明确的错误信息）。

### 5.3 修掉 `KnowledgeStore` 的 per-call provider

`knowledge/store.rs:124` 的 `build_provider()` 在每次 `add`/`search` 都新建 provider。远程后端下只是多一个 reqwest client（浪费但无害）；本地后端下等于**每次检索重新加载 1.2GB 权重**（致命）。

改造：provider 工厂上提为进程级单例。

```rust
// services/provider_factory.rs（新）
static PROVIDER: OnceLock<Arc<dyn EmbeddingProvider>> = OnceLock::new();

pub fn shared_provider(backend: &EmbeddingBackend) -> anyhow::Result<Arc<dyn EmbeddingProvider>>;
```

`EmbeddingService::new`（`:318`）与 `KnowledgeStore::build_provider`（`:124`）都改为从这里取。远程后端下这也顺带复用了 reqwest 连接池。

---

## 六、存储隔离与启动校验

### 6.1 命名

| 对象 | 现状 | 新方案 |
|---|---|---|
| 代码向量表 | `codeseek_<project_hash8>`（`main.rs:197,290`） | `codeseek_<project_hash8>_<fp>` |
| 知识向量表 | `knowledge`（`knowledge/store.rs:47`） | `knowledge_<fp>` |
| 增量哈希文件 | `embedding_hashes.json`（`:532,556`） | `embedding_hashes_<fp>.json` |
| 嵌入缓存 key | `md5(model + code_block)`（`:657`） | `md5(fp + code_block)` |
| BM25 索引目录 | `tantivy_bm25/`（`config.rs:212`） | **不变**（与后端无关，两后端共享） |

LanceDB 表名允许字母数字与下划线，`<fp>` 是 8 位十六进制，安全。

`load_hashes`/`save_hashes`（`:531,555`）是关联函数、签名里只有 `project_hash`，需要加 `fp` 参数；调用点 `main.rs:213,221`。

### 6.2 启动校验（补偿控制）

指纹让错配「结构上不可能」，但仍需一道运行时闸门，防手工建表／库损坏／探测失败：

```
ensure_collection()：
  1. 表不存在 → 按 provider 探测/配置的 dims 建表（现有逻辑，:405）
  2. 表存在 → 读 schema 的 vector 字段，取 FixedSizeList 长度
     - == dims → 通过
     - != dims → 立即 Err，文案（英文，交付件）：
       "vector table '<name>' has dimension <A> but backend '<fp>' produces <B>;
        run `codeseek index --rebuild` to recreate the index for this backend"
```

同时把 `upload_points` 的静默丢行（`:810-813`）改成**首批即失败**：维度不符说明上游校验被绕过，继续跑只会产出半个索引。这是本次设计里唯一一处「把宽松改严格」的行为变更，需要显式记录在实施记录里。

### 6.3 新增 `codeseek index --rebuild`

清空当前 `fp` 对应的表 + `embedding_hashes_<fp>.json` 后全量重建。**不动**其它指纹的表（远程/本地索引可以并存），也**不动** BM25（重建时会按文件重写）。

这是不可逆动作，遵循「显式闸门」：`--rebuild` 必须与 `--yes` 同时给出才执行删除，否则只打印将要删除的对象并退出非零。

---

## 七、检索链路影响面

### 7.1 分数语义（结论：混合检索天然免疫）

两个后端的输出向量都是 L2 归一化的（Qwen3-Embedding 官方输出归一化；harrier 图内已内建 L2 normalize，`refs/ort_embed/README.md` 第三节实测 `L2 = 1.000000`）。

LanceDB `nearest_to` 默认 L2 距离，`score = 1/(1+d)`（`:962`）。对归一化向量，L2 距离与余弦相似度单调等价（`d² = 2 - 2cos`），因此：

- **top-k 排序在两个后端下都与余弦一致** → RRF 只吃 rank，不吃绝对分数（`hybrid_search.rs:367-446`），融合结果不受后端切换影响。
- 绝对分数值跨后端**不可比**。唯一暴露绝对分数的地方是降级路径 `dense_only_candidates` 的 `final_score`（`hybrid_search.rs:313`），它只在单次查询内用于排序，不落盘、不跨查询比较 → 安全。
- 不设任何基于绝对分数的阈值（现在也没有）。若将来要加，必须按 `fp` 分别标定。

### 7.2 门控改造

`!api_token.is_empty()` 的 3 处门控换成后端感知的可用性判定：

```rust
impl EmbeddingBackend {
    /// 该后端是否可用于建索引/检索。
    pub fn is_available(&self) -> Result<(), String> {
        match self {
            Remote(r) if r.api_token.is_empty() && env::var("SILICONFLOW_API_KEY").is_err() =>
                Err("no API token configured".into()),
            LocalOnnx(l) if !l.model_dir.join(&l.onnx_rel_path).exists() =>
                Err(format!("ONNX model not found: {}", l.model_dir.join(&l.onnx_rel_path).display())),
            LocalOnnx(l) if !l.model_dir.join("tokenizer.json").exists() =>
                Err("tokenizer.json not found in model_dir".into()),
            _ => Ok(()),
        }
    }
}
```

改造点：`main.rs:191`（index Phase 4）、`main.rs:288`（search）、`embedding_service.rs:343`（provider 构造）。

`main.rs:188` 的注释「only if API token configured」与 `:241` 的 `suffix` 文案需要同步改成后端中性表述；进度条 phase 文案（`:193` "Building vector embeddings..."）保持英文不变。

### 7.3 降级阶梯不变

`HybridSearchService::search`（`hybrid_search.rs:217-302`）的三档降级完全复用：

- 本地模型文件缺失/加载失败 → dense 通道 `Err` → 自动 sparse-only（BM25）→ BM25 也不可用 → `Err` → 调用方回退 PetCodeGraph 名称搜索（`main.rs` 的 `use_graph_fallback`）。
- 这条链路已经过测试覆盖（`hybrid_search.rs:685-704`），本地后端不需要新增降级代码。

**新增要求**：本地后端加载失败时，错误必须带上模型路径与「检查 model_dir 配置」的提示，否则用户只看到「退回图搜索」而不知道是模型没找到。

---

## 八、长输入与截断

### 8.1 决策

`max_length` 默认 **2048**（harrier `max_position_embeddings = 32768`，2048 远在安全区内），tokenizer 侧 `TruncationStrategy::LongestFirst`（与 `refs/ort_embed/src/embedder.rs:103-108` 一致）。超出部分截断，不切块。

### 8.2 两个后端的截断语义不同，必须分别实现

| | Remote | LocalOnnx |
|---|---|---|
| 截断单位 | 字节（64_000） | token（2048） |
| 实现 | `truncate_to_byte_limit`（`:43`）不动 | tokenizer 内部截断 |
| 预保护 | 无 | 先按 `max_length * 8` 字节粗截，避免超长文本 tokenize 本身耗时/耗内存 |

字节预截断必须 char-boundary 安全 —— 直接复用现有的 `truncate_to_byte_limit`（`:43-52`，已有 8 个单测覆盖中文/emoji/零长边界，`:1263-1366`）。

### 8.3 截断要可见

现状远程路径的字节截断是静默的。本地路径改为：**每文件首次发生截断时 `warn!` 一次**（带符号名与原始 token 数），并用 per-file 标志避免逐符号刷屏——这个「只警告一次」的模式在 `:941,968-974` 已有先例（`warned_missing_line_start`），照抄。

理由：2048 token 大约对应 6-8KB 源码，仓库里超长函数并不罕见；静默截断会让「向量为什么召回不到函数尾部逻辑」变成无法排查的问题。

---

## 九、crate 与 workspace 布局

### 9.1 布局

把 `refs/ort_embed` **提升为 workspace member 并改造成 lib + bin**，而不是新写一个 crate 复制它的 400 行：

```
rust-core/
├── Cargo.toml            # [package] codeseek  +  [workspace] members = ["local-embed"]
├── .cargo/config.toml    # 新增：MACOSX_DEPLOYMENT_TARGET = "13.4"
├── local-embed/          # 由 refs/ort_embed 改造而来
│   ├── Cargo.toml        # ort / tokenizers / ndarray / half / anyhow / clap
│   └── src/
│       ├── lib.rs        # 新：pub API（LocalOnnxEmbedder / Pooling / probe_signature）
│       ├── embedder.rs   # 搬自 refs，去掉 CLI 耦合
│       ├── pooling.rs    # 搬自 refs，原样
│       ├── vectors.rs    # 搬自 refs，原样
│       ├── text.rs       # 搬自 refs，原样（含 char 截断回归测试）
│       └── bin/rembed.rs # 保留 CLI：--dump-signature / --selftest 作为诊断与数值基线工具
└── refs/ort_embed/       # 删除，README 留一行指针指向 local-embed
```

**为什么保留 bin**：`rembed --selftest` 是唯一的数值黄金基线（README 第五节：官方 q4 基准 ±2 容差、对角线占优）。改造过程中任何一次数值漂移都能被它抓到，这个闸门不能丢。

**为什么提升而不是新建**：新建 crate 会把 `embedder.rs`/`pooling.rs` 复制一份，两份实现各自漂移——正是 `refs/` 目录当初要隔离的东西。

### 9.2 构建约束（P0 已实测，结论推翻了本节初稿的推断）

⚠️ **初稿的两处推断都被实测否证**，以下是 P0 的实测数据（详见文末实施记录）：

1. **`lto` 不是触发条件。** `refs/ort_embed/README.md` 第七节称「开启 LTO 会报
   `___isPlatformVersionAtLeast` 未定义」，但实测 `lto = "fat"` 与 `lto = "thin"` **都失败**，
   加上修复后 `lto = "fat"` **直接通过**。因此 `rust-core/Cargo.toml` 的 `[profile.release]`
   一行都不改，`opt-level = "z"` / `codegen-units = 1` / `strip` 保持原样。
2. **`MACOSX_DEPLOYMENT_TARGET` 不是本次失败的原因。** 设成 13.4 后链接行里确实带了
   `-mmacosx-version-min=13.4.0`，仍报同一个符号缺失。保留 `.cargo/config.toml` 只是为了与
   静态库的编译目标一致（避免另一类版本符号问题），它不是修复手段。

**真正的原因**：ort-sys 预编译静态库里 CoreML EP 的 `host_utils.mm.o` / `model.mm.o` 引用了
clang 的 availability helper `___isPlatformVersionAtLeast`。该符号：

- 不在 macOS SDK 的任何 `.tbd` 里（`libSystem` 不导出，实测 grep 为空）；
- 不在 Rust sysroot 的 `libcompiler_builtins-*.rlib` 里（`nm` 计数 0）；
- 只在 clang 自带归档 `<resource-dir>/lib/darwin/libclang_rt.osx.a` 里有定义（`nm -gU` 见 `T`）。

clang 编译链 C 程序会自动带上这个归档，rustc 不会（它用 `-nodefaultlibs`，只链 sysroot 的
compiler_builtins）。

**修复**：`local-embed/build.rs` 用 `clang -print-resource-dir` 可移植地探测归档位置，emit
`rustc-link-search` + `rustc-link-lib=static=clang_rt.osx`，**链接 Apple 自己的实现**，不手写
符号桩（手写桩要猜 availability 语义，CoreML EP 一旦启用就有偏差风险）。

已实测的传播性：下游 package 的二进制（P0 用临时 `linktest` crate 验证）**无需 RUSTFLAGS**
即可链接并运行，产物里 `CoreML` 相关字符串 525 处，证明归档确实被解析。即 P3 里 `codeseek`
依赖 `local-embed` 时不需要在主 package 重复这套配置。注意：只在**一级依赖**上验证过。

**体积**（P0 实测）：链接 ort 静态库的探针二进制 18MB（含 `strip = "symbols"`）。初稿
「数十 MB 膨胀」的估计量级偏高，实际可控。

离线保障：本机已缓存 80MB `libonnxruntime.a`
（`~/Library/Caches/ort.pyke.io/dfbin/aarch64-apple-darwin/4033efca…/`），重建不联网。
缓存目录哈希由 (ort 版本, 平台, **feature 集合**) 决定，所以 `local-embed/Cargo.toml` 的 ort
依赖行必须与 `refs/ort_embed/Cargo.toml:12` **逐字一致**（已在该文件注释里钉住）。本机
`github.com` 不可达，feature 一改就可能去下载然后失败。兜底：`ORT_SKIP_DOWNLOAD` /
`ORT_LIB_LOCATION`（`ort-sys/build/vars.rs:5,11`）。

### 9.3 已知的 ort API 事实（P0/资料核对所得，影响 §5.2 的写法）

| 事实 | 位置 | 对设计的影响 |
|---|---|---|
| `Session::builder(&Environment) -> Result<SessionBuilder>`，`commit_from_file(&mut self) -> Result<Session>`（默认 `'static` 生命周期） | `ort/src/session/mod.rs:159`、`session/builder/impl_commit.rs:47` | Environment 不与 Session 生命周期绑定，可用 `OnceLock` 持有 |
| `Session::inputs()` / `outputs()` 返回 `&[Outlet]`；`Outlet` 是 `pub(crate) mod r#type` 里的项，只经 `ort::value::Outlet` 公开 | `session/mod.rs:150,154`、`value/mod.rs:36` | 写 `use ort::value::Outlet`，不要写 `ort::session::Outlet` |
| `Environment` **没有**在 crate 根 re-export（根只 `pub use` 了 `init` 与 error 类型） | `ort/src/lib.rs:70-73` | 类型路径是 `ort::environment::Environment` |
| `OnceLock::get_or_try_init` 在 rustc **1.94.1 仍是 unstable**（feature `once_cell_try`） | 实测 E0658 | §5.2 的 Environment 单例改用 `OnceLock<Result<T, String>>` + `get_or_init`，即**缓存失败原因、不重试** |

---

## 十、交付顺序与验证闸门

每步的闸门单独取 exit code，不看混合输出。

### P0 — 构建可行性（先证伪，再动手）

1. `rust-core/Cargo.toml` 加 `[workspace] members = ["local-embed"]`，`local-embed` 先只放一个空 lib。
2. 加 `ort` 依赖，`lto = "fat"` 下 `cargo build --release`；记录是否复现 `___isPlatformVersionAtLeast`。
3. 按结果定 profile；再加 `.cargo/config.toml`。
4. **闸门**：`cargo build --release; echo $?` == 0，且二进制体积记录在案。

### P1 — local-embed crate

1. 搬 `embedder.rs`/`pooling.rs`/`vectors.rs`/`text.rs`，新增 `lib.rs` 暴露：
   `LocalOnnxEmbedder::load(model_dir, onnx_rel_path, max_length, intra_threads)`、`embed(&[String]) -> Vec<Vec<f32>>`、`dim()`、`probe_signature()`。
2. 保留 `bin/rembed.rs`（`--selftest` / `--dump-signature`）。
3. 搬 `text.rs` 的 5 个 char 截断回归测试（`refs/ort_embed/src/text.rs:60-113`）。
4. **闸门 A**：`cargo test -p local-embed; echo $?` == 0。
5. **闸门 B（数值黄金基线）**：`cargo run -p local-embed --bin rembed --release -- --selftest` 输出 `SELFTEST PASS`，且四格 diff ≤ 0.72（README 第五节实测值）。
6. **闸门 C（L2 归一化）**：新增测试断言 `embed(&["hello world"])[0]` 的 L2 范数在 `1.0 ± 1e-5`。

> 注：闸门 B/C 需要 1.2GB 模型在场，CI 上不可用 → 用 `CODESEEK_EMBED_MODEL_DIR` 环境变量 gate，未设置时 `#[ignore]`。这一点必须诚实写进文档，不能假装 CI 覆盖了它。

### P2 — 配置联合类型

1. `EmbeddingBackend` 枚举 + 两个 variant struct + `fingerprint()` + `is_available()` + `format_query()`。
2. 处理 `provider` 缺省的 default 语义（§4.2 的实现难点）。
3. **闸门**：`config.rs` 现有 6 个单测（`:290-358`）全绿 + 新增：旧平铺 JSON 解析为 `Remote`、`provider` 缺省解析为 `Remote`、`local-onnx` 解析出 `max_length=2048` 默认值、指纹对 `(model, dims, max_length)` 任一变化都敏感、指纹对 `api_base_url` 变化**不**敏感。
4. **闸门**：`embedding_service.rs` 现有 4 个模板逐字节测试（`:1091-1141`）全绿，另补 LocalOnnx 的 4 个对称测试。

### P3 — provider 工厂与接入

1. `services/provider_factory.rs`：`OnceLock<Arc<dyn EmbeddingProvider>>` + 懒加载。
2. `LocalOnnxEmbeddingProvider` 实现 trait（`Mutex` + `spawn_blocking`，§5.2）。
3. `EmbeddingService::new`（`:318`）与 `KnowledgeStore::build_provider`（`:124`）改走工厂。
4. 3 处门控改 `is_available()`（`main.rs:191,288`、`embedding_service.rs:343`）。
5. **闸门**：`cargo test --lib; echo $?` == 0；新增测试证明 `LocalOnnxEmbeddingProvider::get_embedding` 在 tokio 单线程 runtime 下不阻塞（用 mock session 或 `#[tokio::test(flavor = "current_thread")]` + 计时断言）。

### P4 — 存储隔离

1. 表名 / 哈希文件名 / 缓存 key 三处接 `fp`（§6.1）。
2. `ensure_collection` 加维度校验；`upload_points` 静默丢行改首批失败（§6.2）。
3. `codeseek index --rebuild --yes`。
4. **闸门**：新增测试——同一项目先用 mock remote(2560) 建表、再切 local(1024)，断言生成两张不同名的表、且 1024 表里能查到向量；断言 `embedding_hashes_<fp>.json` 各自独立（切换后端后不会因哈希命中而跳过全部文件）。

### P5 — 端到端与文档

1. 真实跑一遍：`provider = local-onnx` → `codeseek index` → `codeseek search "..."`，确认离线可用（断网/无 api_token）。
2. 与远程后端跑同一组查询，记录 top-5 结果差异（不要求一致，要求「都合理」）。
3. 更新 `README.md` / `README_ZH.md` 的配置段（英文交付件 + 中文文档各一份）、`docs/ARCHITECTURE.md`。
4. 本文档尾部补「实施记录」小节：实际改动清单、闸门实测输出、与设计的偏差。

---

## 十一、风险与补偿控制

| 风险 | 影响 | 补偿控制 |
|---|---|---|
| ~~`lto = "fat"` 与 ort 链接冲突~~ | ~~构建失败~~ | **P0 已消解**：根因不是 lto，而是缺 `___isPlatformVersionAtLeast`；`local-embed/build.rs` 链上 clang 的 `libclang_rt.osx.a` 后 `lto = "fat"` 通过，profile 一行未改 |
| onnxruntime 静态库缓存失效（升级 ort 时 hash 变） | 需联网下载，本机 github 不可达 | 记录 `ORT_SKIP_DOWNLOAD` / `ORT_LIB_LOCATION` 兜底；锁死 ort 版本 `=2.0.0-rc.15` |
| 二进制体积暴涨数十 MB | 与 `opt-level=z` 取向冲突 | 已知并被接受；退路是 sidecar + `load-dynamic`，接口层不变 |
| 16GB 内存下 1.2GB 权重 + LanceDB + Tantivy 同时驻留 | OOM 风险 | 会话池固定 1；懒加载；不检索的命令不加载模型 |
| 切换后端后增量哈希命中导致「假成功」 | 索引为空但无报错 | 哈希文件名带 `fp`（§三）；P4 专门测这条 |
| 维度错配静默丢行 | 半个索引 | 表名带 `fp` + 启动校验 fail-fast + `upload_points` 改严格 |
| 2048 token 截断丢失函数尾部语义 | 召回质量下降 | 首次截断 warn；后续可评估符号内二次切块（本方案非目标） |
| 本地推理拖慢 `codeseek index` | 大仓库索引变慢 | 缓存 key 带 `fp` 保证增量有效；batch=20 复用现有分批；必要时下调 `max_length` |
| 远程路径行为被误改 | 现网检索质量退化 | Remote variant 的所有默认值/模板/维度逐字保留；P2 以现有 10 个单测为回归基线 |

---

## 十二、验收标准

1. `provider = "openai-compatible"` 时，`cargo test` 全绿，且现有 config/embedding 单测（10 个）无一修改断言值。
2. `provider = "local-onnx"` 时，在无网络、无 `SILICONFLOW_API_KEY` 的环境下 `codeseek index` 与 `codeseek search` 均成功返回结果。
3. `rembed --selftest` 输出 `SELFTEST PASS`（数值黄金基线未漂移）。
4. 同一项目在两个后端间来回切换各一次后，两次检索都能返回结果，且 LanceDB 下存在两张不同指纹的表。
5. 本地后端产出的向量 L2 范数 == 1.0（±1e-5）。
6. 二进制体积、构建耗时、`codeseek index` 在本仓库上的耗时，三项前后对比数据记录在实施记录里。

---

## 实施记录

### P0 —— 构建可行性（2026-11-05，macOS arm64 / rustc 1.94.1 / SDK 26.4）

#### 改动清单

| 文件 | 动作 |
|---|---|
| `rust-core/Cargo.toml` | 加 `[workspace] members = ["local-embed"]` + `exclude = ["refs/ort_embed"]`。`[profile.release]` **未改** |
| `rust-core/.cargo/config.toml` | 新建，`[env] MACOSX_DEPLOYMENT_TARGET = "13.4"` |
| `rust-core/local-embed/Cargo.toml` | 新建，ort 依赖行与 `refs/ort_embed/Cargo.toml:12` 逐字一致 |
| `rust-core/local-embed/build.rs` | 新建，macOS 上探测 clang resource dir 并链 `libclang_rt.osx.a` |
| `rust-core/local-embed/src/lib.rs` | 新建，P0 探针：`environment()` 单例 + `probe_signature()` |
| `rust-core/local-embed/src/bin/ort_link_probe.rs` | 新建，可执行探针 |
| 临时 `rust-core/linktest/` | 用于验证下游二进制继承链接参数，**已删除** |

#### 闸门实测（每条单独取 exit code）

| # | 条件 | 命令 | exit |
|---|---|---|---|
| 1 | 初版源码（`ort::session::Outlet` + `get_or_try_init`） | `cargo check -p local-embed` | 101（E0603 私有模块 / E0658 unstable / E0282） |
| 2 | 修正 import 与单例写法后 | `cargo check -p local-embed` | **0** |
| 3 | `lto = "fat"`，无链接修复 | `cargo build --release -p local-embed` | **101** |
| 4 | `lto = "thin"`，无链接修复 | `cargo build --release -p local-embed` | **101** |
| 5 | `lto = "fat"` + `RUSTFLAGS` 指向 `libclang_rt.osx.a` | `cargo build --release -p local-embed` | **0** |
| 6 | `lto = "fat"` + `build.rs`（清空 `RUSTFLAGS`） | `cargo build --release -p local-embed` | **0** |
| 7 | 同 6，运行探针（真实 1.2GB 模型） | `./target/release/ort_link_probe` | **0**，`PROBE OK` |
| 8 | 下游 package 二进制（无 `RUSTFLAGS`） | `cargo build --release -p linktest` + 运行 | **0 / 0**，`LINKTEST OK` |
| 9 | workspace 解析自检 | `cargo metadata --no-deps`（根 / refs 各一次） | 0 / 0，根 = `codeseek + local-embed`，refs 仍是独立 `rembed` |
| 10 | 主 package 全量 release 链接 | `cargo build --release -p codeseek` | 进行中（见下） |

#### 关键结论

1. **`lto` 与失败无关，§9.2 初稿的推断被否证。** 闸门 3/4 显示 fat 与 thin 同样失败，闸门 5 显示
   加上 `libclang_rt.osx.a` 后 **`lto = "fat"` 直接通过**。`[profile.release]` 因此一行未动。
2. **根因是缺失的 clang availability helper。** `___isPlatformVersionAtLeast` 不在 SDK 的任何 `.tbd`、
   不在 Rust sysroot 的 `compiler_builtins`，只在 `<clang resource dir>/lib/darwin/libclang_rt.osx.a`
   （`nm -gU` 见 `T` 定义）。对照实验：同一个 C 程序用 clang 直接编译链接 → exit 0，产物里内联了
   `___isPlatformOrVariantPlatformVersionAtLeast`，证明 rustc 的 `-nodefaultlibs` 是缺口来源。
3. **修复选用 Apple 的实现，不手写桩。** `build.rs` 通过 `clang -print-resource-dir` 探测路径，
   非 `apple-darwin` 目标直接返回；探测失败时只发 `cargo:warning`，让链接错误照常暴露而不是静默降级。
4. **下游传播已实测**：`linktest`（另一 package 的二进制）不带 `RUSTFLAGS` 就能链接并运行，产物含
   525 处 `CoreML` 字符串，说明归档确实被解析、且 CoreML EP 目标文件被拉进了最终二进制。
   仅验证一级依赖；P3 的 `codeseek → local-embed` 正是一级。
5. **`MACOSX_DEPLOYMENT_TARGET` 不是修复项**：闸门 3 的链接行里已带 `-mmacosx-version-min=13.4.0`，
   仍失败。config.toml 保留只为与静态库编译目标一致。
6. **体积**：探针二进制 18MB（`opt-level=z` + `lto=fat` + `strip=symbols`），远小于初稿担心的
   「数十 MB 膨胀」。
7. **模型签名与 README 逐字一致**：`input_ids: Tensor<i64>(batch_size, sequence_length)`、
   `attention_mask: Tensor<i64>(batch_size, total_sequence_length)`、
   `sentence_embedding: Tensor<f32>(batch_size, 1024)` → **1024 维确认**，§三 的指纹设计与 §4.1
   「本地不配 dimensions」的前提成立。

#### 与设计文档的偏差（需在 P1 落实）

| 偏差 | 影响章节 | 处置 |
|---|---|---|
| `ort::session` 未公开 `Outlet`；实际路径是 `ort::value::Outlet` | §5.2 代码样例 | 已在 9.3 表格里记录 |
| `OnceLock::get_or_try_init` 在 rustc 1.94.1 仍 unstable | §5.2「`OnceLock` 保护 `ort::init()`」 | 改为 `OnceLock<Result<Environment, String>>` + `get_or_init`，即**缓存失败原因、不重试**；P1 若要「失败可重试」需另加 `Mutex<Option<..>>` |
| `Environment` 不在 crate 根 re-export | 同上 | 用 `ort::environment::Environment` |

#### 闸门 10 结果

`cargo build --release -p codeseek` → **成功**（`Finished` 无 error），产物 **58MB**，
耗时 **18m33s**。耗时全部来自新增 `.cargo/config.toml` 的 `[env]` 使
`target/release` 全套依赖缓存失效（lance / lancedb / tantivy / aws-sdk 等重编），
**与 workspace 化本身无关**。这是一次性成本，记录为约束：改动 `[env]` 或
`[profile.*]` 都会触发全量重建。

结论：workspace 化没有破坏主 package 的构建。

---

### P1 —— local-embed 推理后端

#### 改动清单

| 文件 | 动作 |
|---|---|
| `local-embed/src/lib.rs` | 模块树 + `DEFAULT_ONNX_REL_PATH`；CLI 三模块挂 `cli` feature |
| `local-embed/src/signature.rs` | 新建：`environment()` 进程单例（`OnceLock<Result<..>>`）、`open_session()`、`probe_signature()` |
| `local-embed/src/embedder.rs` | 由 refs 搬入；`load(model_dir, onnx_rel_path, max_length, intra_threads)`；session 改走 `open_session` 共享 Environment；库内错误文案改英文 |
| `pooling.rs` / `vectors.rs` / `text.rs` / `report.rs` / `selftest.rs` | 搬入。除 `pooling` 可见性 `pub(crate)`→`pub` 与 `cli.rs` 默认值外**逐字未动** |
| `local-embed/src/bin/rembed.rs` | 由 refs 的 `main.rs` 改造，保留 `--selftest` / `--dump-signature` |
| `local-embed/tests/harrier_baseline.rs` | 新建：4 项真实模型数值闸门 |

#### 闸门实测

| 闸门 | 命令 | exit | 结果 |
|---|---|---|---|
| A | `cargo check -p local-embed --features cli --all-targets` | **0** | 一次通过 |
| B | `cargo test -p local-embed --features cli` | **0** | 6 个单元测试（含 refs 搬来的 5 个 char 截断回归）+ 4 个真实模型测试全绿 |
| C | `cargo run -p local-embed --features cli --bin rembed -- --selftest` | **0** | `SELFTEST PASS` |
| D | L2 归一化断言 | 含在 B | `dim=1024`、`L2=1.000000`、`Pooling::BuiltIn` |

数值基线**逐字复现** README 第五节（`max_length` 已按设计提到 2048，因两条文档都远短于
512 token，数值不受影响）：

```
[   66.2748,    28.4627]        diff 0.3820 / 0.3777
[   29.7776,    69.9482]        diff 0.6692 / 0.7118      对角线占优: 是
```

新增的两条数值断言值得单独记下：

- `inference_is_deterministic_across_calls`：同一文本两次推理必须**逐位一致**（ORT 线程/
  图优化变动会先在这里响）。
- `batch_order_matches_input_order`：batch 内每条向量与单独推理该条**逐位一致** ——
  这是 padding 到 batch 最长 + `attention_mask` 找 last-token 是否处理正确的强判据，
  比原 selftest 的分数矩阵更直接。

#### 与设计的偏差

- `PAD_TOKEN` / `PAD_TOKEN_ID` 在写入过程中被工具层反复转写（`<|...|>` 形态的特殊 token），
  最终由脚本从 `tokenizer.json` 的 `added_tokens` 读出真实串后写回。已核实：
  `tokenizer_config.json` 的 `pad_token` 与 `tokenizer.json` 中 id `151643` 的内容一致。
- CLI 的 `--max-length` 默认值从 512 改为 2048，与 `config` 的本地默认对齐，
  避免两处默认漂移。
- **截断警告的粒度**：§8.3 原写「每文件首次截断 warn 一次」，实际实现是**每 batch 一次**。
  信号来自 `Embedder::truncation_hit` —— tokenize 后 `seq_len == max_length` 即置位。
  这是**启发式**：batch 内最长序列恰好等于上限（并未截断）时也会警告一次，措辞相应写成
  「its tail *was* truncated」的前提略宽。选择在 tokenizer 长度上取巧而不是额外做一遍
  无截断编码，是因为后者要给每条文本多付一次 BPE pass。`refs` 里没有这个信号，属于新增。
- `local-embed` 的库内错误文案统一改英文（交付件语言约定），`rembed` CLI 的用户输出保持
  原中文（它是基线工具，与 README 的实测输出逐字对齐，改动会让基线不可比对）。

---

### P2 —— 后端配置判别式联合

`EmbeddingConfig` → `EmbeddingBackend::{Remote, LocalOnnx}`，`#[serde(tag = "provider")]`，
`Deserialize` 手写以兼容「缺 `provider` 键」的旧配置（§4.2 预告的唯一实现难点）。

新增 `identity()` / `fingerprint()` / `code_table_name()` / `hashes_file_name()` /
`check_available()` / `known_dimensions()` / `format_query()`；模板常量提为
`REMOTE_QUERY_TEMPLATE` / `LOCAL_QUERY_TEMPLATE` + `render_query()`，
`build_instruct_query` 改为委托，模板只留一处定义。

闸门：`cargo test -p codeseek --lib` 中 config 模块 **8 旧 + 10 新** 全绿。旧测试的
断言值一个没改，只有两处按枚举形态重写：

- 类型名 `EmbeddingConfig` → `EmbeddingBackend`；
- `config_without_embedding_section_...` 原来断言原始字段 `instruct == None`
  （derive(Default) 的产物），现在断言**生效值** `effective_instruct()` —— 因为
  `RemoteEmbedding::default()` 直接把 `Some(DEFAULT_QUERY_INSTRUCT)` 填进字段，
  两者行为等价，断言生效值更能代表契约。

#### P2 抓到的真 bug（值得留档）

第一版只在**私有** `BackendWire` 枚举上写了 `#[serde(rename = "openai-compatible")]`，
`EmbeddingBackend` 自身依赖 `rename_all = "kebab-case"` 序列化 → 写出
`"provider":"remote"`，而读入只认 `openai-compatible`。也就是说
`Config::save()` 之后再 `Config::load()` 会失败。是新写的
`instruct_field_survives_serialization_round_trip` 断言把它打下来的（236 passed / 1 failed），
补上变体的显式 rename 后修复。**结论：读写必须成对测试，只测「旧配置能读进来」不够。**

---

### P3 —— provider 工厂与本地后端接入

- `services/provider_factory.rs`（新）：`shared_handle(&backend) -> Arc<BackendHandle>`，
  `BackendHandle { provider, dimensions }`。
- **只有本地后端进缓存**。初稿把两种 provider 都按指纹缓存，实测中暴露出：`instruct`
  不在指纹里（它只影响查询前缀、不影响入库向量，不该进表名），所以缓存远程 handle 会让
  同进程内改配置的执行沿用旧前缀。远程构造只是几个字符串 + 一个 client，改为每次新建。
- `LocalOnnxEmbeddingProvider`：`Arc<Mutex<Embedder>>` + `spawn_blocking`（§5.2 的
  `run` 需要 `&mut self`、`run_async` 是 wasm-only 两条事实的直接后果）；锁中毒时
  `into_inner()` 继续用而非让整个 CLI 崩；送进 tokenizer 前有 `max_length * 8` 字节的
  预截断（复用 `truncate_to_byte_limit`，char-boundary 安全）。
- `KnowledgeStore::build_provider()` 删除，改 `handle()`；`ensure_table()` 增加
  `dimensions: Option<usize>` 参数 —— 删除/列表路径传 `None`，避免为了删一条记录去
  加载 1.2GB 模型；`add`/`search` 传真实维度以触发校验。
- `EmbeddingService::new` 走工厂；`embedding_provider` 由 `Box<dyn …>` 改 `Arc<dyn …>`。
- 门控：`main.rs` 的 index / search 两处 `!api_token.is_empty()` 改为
  `check_available()`，**不可用的原因打进 warn 日志**（本地后端下这必须是带模型路径的句子，
  否则用户只看到「退回图搜索」）。

---

### P4 —— 存储隔离与启动校验

| 对象 | 实现 |
|---|---|
| 表名 | `EmbeddingBackend::code_table_name(project_hash)` → `codeseek_<h8>_<fp>`；知识表 `knowledge_<fp>`（`KnowledgeStore::new` 计算） |
| 增量哈希 | `embedding_hashes_<fp>.json`，`load_hashes`/`save_hashes` 增加指纹参数 |
| 缓存 key | 提取为 `embedding_cache_key(fingerprint, code_block)` 以便单测；旧实现只含模型名，改 `max_length` 时不会失效 |
| 启动校验 | 新模块 `storage/vector_schema.rs`：`vector_dims()` + `require_dims_match()`，`ensure_collection`（代码表）与 `ensure_table`（知识表）都调用 |
| 静默丢行 | `upload_points` 的 `error! + continue` 改为首批维度不符直接 `Err`，错误里点名是哪个符号 |
| 重建 | `codeseek init --rebuild --yes`：只 drop 当前指纹的表 + 删该后端的哈希文件；缺 `--yes` 时**拒绝执行并返回非零**，不动任何数据 |

新集成测试 `tests/test_vector_backend_isolation.rs`（5 个，不联网不加载模型）覆盖设计要求的
两条：同项目两个后端落在不同表且能共存（含 drop 作用域验证）、以及
**「切后端不会因增量哈希命中而跳过全部文件」** —— 后者是本次改动里唯一「不报错但结果为空」
的失败形态。

#### P4 实测中修掉的一个缺陷

`--rebuild` 在表尚不存在时（首次索引就带 `--rebuild`）会让 `drop_table` 报错，
而 `main.rs` 把该错误当作「嵌入阶段失败」→ **整个嵌入被跳过**，用户看到的是一个
什么都没做的"重建"。`drop_table()` 已改为先查 `table_names()`、表不存在时视为成功。

---

### P5 —— 端到端验证

前置：`provider = "local-onnx"` 的配置写入 `~/.codeseek/config.json`（**备份后替换、
脚本 trap 无条件恢复并校验 JSON 可解析**），`unset SILICONFLOW_API_KEY`，配置里
**完全没有 `api_token` 字段** —— 即离线且无任何远程凭据。测试项目是 `/tmp` 下的独立
git 仓库（2 个源文件、3 个符号），索引目录在删除前用 `project.json` 里的
`project_root` 反查确认，避免误碰真实项目。

| 闸门 | 命令 | exit | 结果 |
|---|---|---|---|
| E2E-1 | `codeseek init` | **0** | 建出 `lancedb/codeseek_976ab12b_a5c462f7.lance` 与 `embedding_hashes_a5c462f7.json`，两处指纹一致 |
| E2E-2 | `codeseek search "merge two configuration maps with override" --json` | **0** | 首选 `merge_configs`，`source=Fused`（dense + BM25 双通道命中） |
| E2E-3 | `codeseek search "监听目录里文件的创建和修改" --json` | **0** | 首选 Python 的 `watch_directory`，`source=DenseOnly` —— 中文查询召回英文命名的函数，跨语言语义由本地模型完成 |
| E2E-4 | `codeseek init --rebuild`（缺 `--yes`） | **1** | 拒绝执行、给出确认提示，未删除任何数据 |
| E2E-5 | `codeseek init --rebuild --yes` | **0** | 删表 + 删该后端哈希 + 重建成功 |

向后兼容的另一条实测：用**现网真实配置**（`provider=openai-compatible` + 非空 token）
跑 `codeseek status` → exit 0，判别式联合在旧配置上解析正常。

#### 测试汇总（全部单独取 exit code）

| 套件 | exit | 数量 |
|---|---|---|
| `cargo test -p codeseek --lib` | **0** | 237 passed / 0 failed / 1 ignored |
| `cargo test -p codeseek`（全部集成测试目标） | **0** | 含既有 12 个测试文件全部 ok |
| `cargo test -p codeseek --test test_vector_backend_isolation` | **0** | 5 passed |
| `cargo test -p local-embed --features cli` | **0** | 6 + 4 passed |
| `codeseek` release 全量构建 | **0** | 见下 |

#### 过程中被测试抓到的两个真 bug

1. **`EmbeddingBackend` 的读/写标签不对称**：只有私有 `BackendWire` 带
   `#[serde(rename = "openai-compatible")]`，`EmbeddingBackend` 自身靠
   `rename_all = "kebab-case"` 序列化成 `"remote"` → `Config::save()` 后再 `load()` 直接失败。
   由新写的 `instruct_field_survives_serialization_round_trip` 抓出（236 passed / 1 failed）。
   **教训：只测「旧配置能读进来」不够，读写必须成对覆盖。**
2. **`--rebuild` 在不存在的表上失败会静默跳过整个嵌入阶段**（见 P4 小节）。
3. 另有一处是我自己写错的断言（表名前缀按 hash 前 8 位而非 6 位），代码无问题。

#### 遗留 / 已知边界

- **`refs/ort_embed` 已删除**，`rust-core/refs/README.md` 留指针。理由：与 `local-embed`
  是同一份实现，两份并存必然漂移。历史内容可通过 git 取回。
- 需要 1.2GB 模型的测试由 `CODESEEK_EMBED_MODEL_DIR` 门控，未设置时**打印跳过原因**而不是
  静默通过。CI 上没有该模型，因此那 4 项数值断言在 CI 不会被执行 —— 这是事实，不假装覆盖。
- `cli::run_vectorize`（`codeseek vectorize`）不在 `Commands` 里、无调用方，本次未动。
  它自己拼表名，若将来接线需要改成 `code_table_name()`。
- 本地推理目前**没有**做 batch 内二级并发（会话池=1），大仓库首次 `init` 的嵌入阶段会明显
  慢于远程 API。缓解手段是既有的 SQLite 缓存（key 含后端指纹）。
- reranker 仍是远程 API。`provider=local-onnx` 且无 reranker token 时它会自然失效，
  hybrid 检索退回纯 RRF（`hybrid_search.rs:539-546` 的既有降级路径）。

#### 交付件语言

`config` 字段、CLI 文案、库内错误串、日志均为英文；本文档与 `docs/` 下的说明为中文，
`README.md` 英文 / `README_ZH.md` 中文双份。

#### 对 CI / Release 的影响（`.github/workflows/release.yml`，本次未修改）

该工作流为 `x86_64-apple-darwin`、`aarch64-apple-darwin`、`x86_64-unknown-linux-gnu`
三个 target 跑 `cargo build --release`，引入 `local-embed` 后新增两条依赖：

1. **CI 必须能联网下载 ONNX Runtime 预编译库**。`ort-sys` 的 `download-binaries` 会去
   GitHub releases 取每个 target 的静态库；本机因为 github 不可达只能吃缓存，
   GitHub Actions 上没问题，但**自建 CI / 内网 runner 会直接失败**。
   兜底是 `ORT_LIB_LOCATION` 指向预置的库目录。
2. **产物体积会变大**（静态链入 onnxruntime）。本机实测：只链 ort 的探针二进制 18MB
   （`opt-level=z` + `lto=fat` + `strip`），`codeseek` 的 release 体积变化见上表。

`build.rs` 的链接修复对两个 macOS target 都成立 —— 已实测
`<clang resource dir>/lib/darwin/libclang_rt.osx.a` 是 fat 归档，
`lipo -thin x86_64` 与 `lipo -thin arm64` 后各自都有
`T ___isPlatformVersionAtLeast` 定义（x86_64 交叉编译那条腿尤其需要验证，
因为 CI 是在 arm64 runner 上产 x86_64 二进制）。Linux 分支在 `build.rs` 里
按 `TARGET` 直接返回，不做任何事。

CI 配置本身没有改动：需要联网这一点属于事实约束而非可选项，是否要为内网 runner 预置
`ORT_LIB_LOCATION`、以及是否接受体积增长，属于发布策略，留给项目决定。
