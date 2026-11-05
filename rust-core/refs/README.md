# refs/

这里的 `ort_embed`（rembed CLI）已提升为 workspace member `rust-core/local-embed`：
同一套推理实现，`embedder` / `pooling` / `vectors` / `text` 作为库被 codeseek 依赖，
`rembed --selftest` 保留为官方基准的数值闸门。

设计与其历史实测见 `rust-core/docs/local-embedding-backend.md`。
