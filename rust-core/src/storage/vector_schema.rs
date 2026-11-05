//! LanceDB 向量表 schema 的读取与校验。
//!
//! 两个向量库共用（`EmbeddingService` 的代码符号表、`KnowledgeStore` 的知识表）。
//!
//! 为什么需要它：换嵌入后端时维度会变（远程 Qwen3-4B 是 2560，本地 harrier 是 1024），
//! 而 `create_empty_table` 只在表不存在时建表、**不做任何列迁移**。旧实现里维度不符时
//! `upload_points` 只是 `error!` + `continue`，结果是一个「命令报成功、表里却没几行」的
//! 半空索引。表名已经带后端指纹（结构上让错配不可能发生），这里再补一道运行时闸门，
//! 兜住手工建表 / 库损坏 / 探测失败三种情况。

use anyhow::{anyhow, Result};
use arrow::datatypes::{DataType, Schema};

/// 读某列的 `FixedSizeList` 长度；列不存在或类型不符时返回 `None`。
pub fn vector_dims(schema: &Schema, column: &str) -> Option<usize> {
    let field = schema.field_with_name(column).ok()?;
    match field.data_type() {
        DataType::FixedSizeList(_, len) => (*len).try_into().ok(),
        _ => None,
    }
}

/// 校验已有表的向量列长度与后端产出维度一致；不一致时返回带修复指引的英文错误。
pub fn require_dims_match(
    schema: &Schema,
    column: &str,
    expected: usize,
    table_name: &str,
) -> Result<()> {
    match vector_dims(schema, column) {
        None => Err(anyhow!(
            "vector table '{table_name}' has no FixedSizeList column '{column}'; \
             run `codeseek index --rebuild --yes` to recreate the index for this backend"
        )),
        Some(found) if found != expected => Err(anyhow!(
            "vector table '{table_name}' has dimension {found} but the configured embedding \
             backend produces {expected}; run `codeseek index --rebuild --yes` to recreate the \
             index for this backend"
        )),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::Field;
    use std::sync::Arc;

    fn schema_with(len: i32) -> Schema {
        Schema::new(vec![Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), len),
            false,
        )])
    }

    #[test]
    fn reads_fixed_size_list_length() {
        assert_eq!(vector_dims(&schema_with(1024), "vector"), Some(1024));
        assert_eq!(vector_dims(&schema_with(1024), "missing"), None);
    }

    #[test]
    fn non_list_column_is_reported_as_absent() {
        let s = Schema::new(vec![Field::new("vector", DataType::Float32, false)]);
        assert_eq!(vector_dims(&s, "vector"), None);
        let err = require_dims_match(&s, "vector", 1024, "t").unwrap_err();
        assert!(err.to_string().contains("no FixedSizeList column"));
    }

    /// 维度不符必须指名道姓地报出 found/expected 与修复命令 —— 静默丢行是最坏的结果。
    #[test]
    fn mismatch_reports_both_numbers_and_the_rebuild_command() {
        let err = require_dims_match(&schema_with(2560), "vector", 1024, "codeseek_abc_12345678")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("dimension 2560"), "{msg}");
        assert!(msg.contains("produces 1024"), "{msg}");
        assert!(msg.contains("index --rebuild --yes"), "{msg}");
        assert!(msg.contains("codeseek_abc_12345678"), "{msg}");
    }

    #[test]
    fn match_passes() {
        assert!(require_dims_match(&schema_with(1024), "vector", 1024, "t").is_ok());
    }
}
