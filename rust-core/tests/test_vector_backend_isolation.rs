//! 两个嵌入后端在同一项目上互不干扰（P4 闸门）。
//!
//! 覆盖设计文档 §三 的三处隔离：LanceDB 表名、增量哈希文件名、嵌入缓存 key。
//! 其中「增量哈希按后端分桶」是最容易被漏掉的一条 —— 它不报错，只是让新后端
//! 一个文件都不处理，产出一个看着成功的空索引。
//!
//! 不联网、不加载模型：表名/哈希名/缓存 key 都是纯函数，LanceDB 用 tempdir。

use codeseek::config::{EmbeddingBackend, LocalOnnxEmbedding, RemoteEmbedding};
use codeseek::services::embedding_service::{EmbeddingProvider, embedding_cache_key};
use codeseek::services::{EmbeddingService};
use async_trait::async_trait;

struct DummyProvider;

#[async_trait]
impl EmbeddingProvider for DummyProvider {
    fn model(&self) -> String {
        "dummy-1024".to_string()
    }
    async fn get_embedding(&self, _t: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        Ok(vec![0.5; 1024])
    }
}

fn remote(backend_dims: usize) -> EmbeddingBackend {
    EmbeddingBackend::Remote(RemoteEmbedding {
        api_token: "sk-not-used".to_string(),
        dimensions: backend_dims,
        ..Default::default()
    })
}

/// 表名必须同时含项目 hash 前缀与后端指纹。
#[test]
fn table_names_are_isolated_per_backend() {
    let project_hash = "abcdef1234567890";
    let r = remote(2560).code_table_name(project_hash);
    let l = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default()).code_table_name(project_hash);

    // 表名取 project hash 的前 8 位 + '_' + 后端指纹
    assert!(r.starts_with("codeseek_abcdef12_"), "{r}");
    assert!(l.starts_with("codeseek_abcdef12_"), "{l}");
    assert_ne!(r, l, "远程与本地必须落在不同的表上");
    // LanceDB 表名只允许字母数字与下划线
    for name in [&r, &l] {
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "非法表名字符: {name}"
        );
    }
}

/// 同一后端换个模型也要分开（否则 1024 维的 bge 会撞上 1024 维的 harrier）。
#[test]
fn table_names_differ_across_models_of_the_same_dimension() {
    let a = remote(1024).code_table_name("proj1234");
    let mut b = RemoteEmbedding::default();
    b.dimensions = 1024;
    b.model = "BAAI/bge-m3".to_string();
    assert_ne!(a, EmbeddingBackend::Remote(b).code_table_name("proj1234"));
}

/// 增量哈希文件按后端分桶 —— 这条防的是「切后端后所有文件被当成未变更而跳过」。
#[test]
fn incremental_hash_files_are_per_backend() {
    let r = remote(2560).hashes_file_name();
    let l = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default()).hashes_file_name();
    assert_ne!(r, l);
    assert!(r.starts_with("embedding_hashes_") && r.ends_with(".json"), "{r}");

    // 同后端改 max_length → 截断点变 → 哈希文件也要换
    let m1 = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding { max_length: 2048, ..Default::default() });
    let m2 = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding { max_length: 1024, ..Default::default() });
    assert_ne!(m1.hashes_file_name(), m2.hashes_file_name());
}

/// 缓存 key 含后端指纹：换后端/换截断长度都读不到旧向量。
/// 旧实现只含模型名，改 max_length 时会命中过期向量。
#[test]
fn cache_key_is_scoped_to_the_backend() {
    let code = "fn main() {}";
    let r = remote(2560);
    let l = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default());
    let shorter = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding {
        max_length: 1024,
        ..Default::default()
    });

    let kr = embedding_cache_key(&r.fingerprint(), code);
    let kl = embedding_cache_key(&l.fingerprint(), code);
    let ks = embedding_cache_key(&shorter.fingerprint(), code);
    assert_ne!(kr, kl, "换后端必须换 key");
    assert_ne!(kl, ks, "换 max_length 必须换 key");

    // 同一后端同一文本 → 同一个 key（缓存才能命中）
    assert_eq!(kr, embedding_cache_key(&r.fingerprint(), code));
    // 不同文本 → 不同 key
    assert_ne!(kr, embedding_cache_key(&r.fingerprint(), "fn other() {}"));
}

/// 两张不同指纹的表可以在同一个 LanceDB 目录里共存；drop 只影响自己那张。
#[tokio::test]
async fn coexisting_tables_and_scoped_drop() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().to_str().unwrap();

    let remote_table = remote(2560).code_table_name("projhash12");
    let local_table = EmbeddingBackend::LocalOnnx(LocalOnnxEmbedding::default())
        .code_table_name("projhash12");

    // 两个 provider 都返回 1024 维假向量；表名不同即可验证共存
    let svc_a =
        EmbeddingService::new_with_provider(db, remote_table.clone(), Box::new(DummyProvider))
            .await
            .unwrap();
    let svc_b =
        EmbeddingService::new_with_provider(db, local_table.clone(), Box::new(DummyProvider))
            .await
            .unwrap();

    // new_with_provider 的 schema 固定 2560 维，这里只为验证表名隔离与 drop 作用域
    svc_a.ensure_collection().await.unwrap();
    svc_b.ensure_collection().await.unwrap();

    svc_a.drop_table().await.unwrap();

    let rest: Vec<String> = {
        let conn = lancedb::connect(db).execute().await.unwrap();
        conn.table_names().execute().await.unwrap()
    };
    assert!(!rest.contains(&remote_table), "被 drop 的表不该还在: {rest:?}");
    assert!(rest.contains(&local_table), "另一张表不该被波及: {rest:?}");
}
