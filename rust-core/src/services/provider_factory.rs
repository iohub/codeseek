//! 进程级 embedding provider 工厂。
//!
//! 存在的理由：`KnowledgeStore` 过去每次 `add`/`search` 都新建一个 provider（原
//! `knowledge/store.rs::build_provider`）。远程后端下那只是多一个 reqwest client，
//! 本地后端下等于**每次检索重新加载 1.2GB 权重** —— 必须共享。
//!
//! 缓存按后端指纹分桶：同一进程内切换后端（测试、多项目）各自命中自己的 handle。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Result};

use crate::config::{EmbeddingBackend, LocalOnnxEmbedding, RemoteEmbedding};
use crate::services::embedding_service::{
    EmbeddingProvider, LocalOnnxEmbeddingProvider, OpenAICompatibleEmbeddingProvider,
};

/// provider + 它的输出维度。
///
/// 维度对远程后端来自配置，对本地后端来自 ONNX 图签名探测 —— 两者都在工厂里定死，
/// 下游（LanceDB schema、维度校验）只消费这一个数。
pub struct BackendHandle {
    pub provider: Arc<dyn EmbeddingProvider>,
    pub dimensions: usize,
}

fn cache() -> &'static Mutex<HashMap<String, Arc<BackendHandle>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<BackendHandle>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 取（或建）该后端的共享 provider。
///
/// 只有本地后端进缓存：加载 1.2GB 权重是唯一值得复用的成本。远程 provider
/// 就是几个字符串加一个 reqwest client，构造可以忽略，因此**每次新建** ——
/// 若把它也缓存起来，同进程内改变配置（测试、多项目）就会拿到旧的后端实例，
/// 而指纹里并不包含 `instruct`，查询前缀会静默沿用旧值。
pub async fn shared_handle(backend: &EmbeddingBackend) -> Result<Arc<BackendHandle>> {
    if let EmbeddingBackend::Remote(_) = backend {
        return Ok(Arc::new(build_handle(backend).await?));
    }

    let fp = backend.fingerprint();

    // guard 必须在 await 之前离开作用域，否则 future 不再是 Send
    let cached = {
        let guard = cache().lock().unwrap_or_else(|e| e.into_inner());
        guard.get(&fp).cloned()
    };
    if let Some(hit) = cached {
        return Ok(hit);
    }

    let handle = Arc::new(build_handle(backend).await?);
    {
        // 两个并发调用者可能都未命中并各自构建一份。对单进程 CLI 来说这个代价可接受
        // （最贵的是本地权重加载）；真出现双份常驻再上 singleflight。
        let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(fp, Arc::clone(&handle));
    }
    Ok(handle)
}

async fn build_handle(backend: &EmbeddingBackend) -> Result<BackendHandle> {
    match backend {
        EmbeddingBackend::Remote(r) => {
            // instruct 的三态解析只在 `EmbeddingBackend::effective_instruct` 一处定义，
            // 这里直接把结果传进去，避免工厂里再实现一遍而与 config 漂移。
            let provider = build_remote(r, &backend.effective_instruct())?;
            Ok(BackendHandle {
                dimensions: r.dimensions,
                provider: Arc::new(provider),
            })
        }
        EmbeddingBackend::LocalOnnx(_) => {
            let provider = LocalOnnxEmbeddingProvider::build(backend)
                .await
                .map_err(|e| anyhow!("{e}"))?;
            let dimensions = provider.dimensions();
            Ok(BackendHandle {
                dimensions,
                provider: Arc::new(provider),
            })
        }
    }
}

fn build_remote(
    r: &RemoteEmbedding,
    instruct: &str,
) -> Result<OpenAICompatibleEmbeddingProvider> {
    // 错误文案与重构前逐字一致（`API Key not found in config or environment`）
    let api_token = if r.api_token.is_empty() {
        std::env::var("SILICONFLOW_API_KEY").ok()
    } else {
        Some(r.api_token.clone())
    };
    let api_token = api_token.ok_or_else(|| anyhow!("API Key not found in config or environment"))?;

    let base_url = if r.api_base_url.is_empty() {
        None
    } else {
        Some(r.api_base_url.clone())
    };
    let model = if r.model.is_empty() {
        RemoteEmbedding::default().model
    } else {
        r.model.clone()
    };

    Ok(OpenAICompatibleEmbeddingProvider::new(
        api_token,
        base_url,
        model,
    )
    .with_instruct(instruct.to_string()))
}

impl LocalOnnxEmbedding {
    /// 本地后端一次推理最多送进 tokenizer 的字节数。
    ///
    /// 粗略上限，只为防超长文本把 BPE 编码本身拖垮；真正的截断由 tokenizer 的
    /// `max_length` 完成。按 8 字节/token 取值：harrier 的 Qwen2 BPE 对源码大约
    /// 3-4 字节/token，留了一倍余量。
    pub fn input_byte_cap(&self) -> usize {
        self.max_length.saturating_mul(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_QUERY_INSTRUCT;

    /// 远程后端缺 token 时必须报出旧文案，且**不**去构造本地模型。
    #[test]
    fn remote_without_token_keeps_legacy_error_message() {
        let backend = EmbeddingBackend::Remote(RemoteEmbedding {
            api_token: String::new(),
            ..Default::default()
        });
        // 环境里可能有 SILICONFLOW_API_KEY，那就该视为可用；只在没有时断言
        if std::env::var("SILICONFLOW_API_KEY").is_ok() {
            return;
        }
        let err = build_remote(&backend.expect_remote(), DEFAULT_QUERY_INSTRUCT)
            .err()
            .expect("empty token must fail");
        assert_eq!(
            err.to_string(),
            "API Key not found in config or environment"
        );
    }

    /// 同端点、不同 token 的两个配置共享指纹 → 命中同一份 handle（缓存键正确性前提）。
    #[test]
    fn fingerprint_is_the_cache_key() {
        let a = EmbeddingBackend::Remote(RemoteEmbedding {
            api_token: "sk-a".into(),
            ..Default::default()
        });
        let b = EmbeddingBackend::Remote(RemoteEmbedding {
            api_token: "sk-b".into(),
            ..Default::default()
        });
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    /// 本地后端在模型缺失时 check_available 就报错，不该走到加载。
    #[tokio::test]
    async fn local_backend_without_model_reports_before_loading() {
        let backend = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding {
            model_dir: std::path::PathBuf::from("/nonexistent/dir"),
            ..Default::default()
        });
        let err = LocalOnnxEmbeddingProvider::build(&backend)
            .await
            .err()
            .expect("missing model must not load");
        assert!(err.to_string().contains("/nonexistent/dir"), "{err}");
    }

    impl EmbeddingBackend {
        fn expect_remote(&self) -> RemoteEmbedding {
            match self {
                Self::Remote(r) => r.clone(),
                other => panic!("expected Remote, got {other:?}"),
            }
        }
    }
}
