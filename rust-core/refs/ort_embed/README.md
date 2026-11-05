# rembed

本地 ONNX 文本嵌入推理 CLI（Rust）。针对 `microsoft/harrier-oss-v1-0.6b` 的 ONNX 导出
（`harrier-oss-v1-0.6b-ONNX`，MIT 许可）实现：Qwen3 decoder-only 架构，被对比学习训练成
**embedding 模型**，输出 1024 维、已 L2 归一化的句向量。

- 推理后端：`ort` 2.0.0-rc.15（ONNX Runtime 1.31，CPU Execution Provider，默认 `download-binaries`）
- Tokenizer：HuggingFace `tokenizers`，直接加载 `tokenizer.json`（Qwen2Tokenizer / BPE）
- 无 Python 依赖，单条 `cargo run` 即可跑通

## 一、快速开始

```bash
cd /Users/ydl/iohub/rembed

# 1) 单条文本
cargo run --release -- --text "hello world"

# 2) 查询侧加官方 web-search 指令前缀（文档侧不加）
cargo run --release -- --query --text "how much protein should a female eat"

# 3) 多条文本一起 encode（同一 batch），额外打印余弦相似度矩阵 ×100
cargo run --release -- --text "hello world" --text "你好，世界"

# 4) 复现官方 model card 的基准分数矩阵（Step E 验收）
cargo run --release -- --selftest

# 5) 只看模型 input/output 签名
cargo run --release -- --dump-signature
```

默认模型目录：`/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX`，程序会自动拼
`onnx/onnx/model_fp16.onnx`（注意本地是 **双层 `onnx/onnx/`** 目录）与 `tokenizer.json`。
用 `--model-dir <dir>` 可指向别处。

## 二、命令行参数

| 参数 | 说明 |
|---|---|
| `--model-dir <dir>` | 模型目录，默认 `/Users/ydl/modelhub/harrier-oss-v1-0.6b-ONNX` |
| `--text <string>` | 待编码文本，**可重复**；多条会一起 padding 成同一 batch |
| `--query` | 给每条 `--text` 加默认的 web-search 指令前缀 |
| `--instruction <string>` | 自定义指令前缀，优先于 `--query` |
| `--max-length <usize>` | 截断长度，默认 `512` |
| `--dump-signature` | 只打印模型 input/output 签名与推断结论后退出 |
| `--selftest` | 跑官方基准用例（2 query × 2 doc），打印分数矩阵并与官方值比对 |
| `-h, --help` | 帮助 |

## 三、实测模型签名（`--dump-signature` 的真实输出）

```
=== 模型输入签名 ===
  input[0]  name=input_ids        dtype=Tensor<i64>(batch_size, sequence_length)
  input[1]  name=attention_mask   dtype=Tensor<i64>(batch_size, total_sequence_length)
=== 模型输出签名 ===
  output[0] name=sentence_embedding dtype=Tensor<f32>(batch_size, 1024)
=== 推断结论 ===
  池化策略      = BuiltIn
  嵌入维度      = 1024
  使用的输出名  = sentence_embedding
```

**关键结论：`last-token pooling` + `L2 normalize` 已经内建在 ONNX 图内部**，Rust 侧不需要
自己再池化。同时虽然权重是 fp16，图已经把输出转回 **float32**，因此不用 `half` 做转换
（代码里仍保留 fp16 输出分支 + `last_hidden_state` 手动池化分支，便于换模型时自动适配）。

## 四、实现要点

1. **tokenize**：`tokenizer.encode_batch(texts, true)`，开
   `padding = BatchLongest`（pad 到 batch 内最长）与 `truncation = LongestFirst(max_length)`；
   pad token 用 `<|endoftext|>`（id `151643`，即 `pad_token_id`）。
   `attention_mask` 直接取 tokenizer 生成的（1 = 有效，0 = pad），转成 `i64`。
2. **推理**：只喂 `input_ids` / `attention_mask` 两个 int64 张量，shape `[batch, seq_len]`；
   该图没有 `position_ids` / `token_type_ids` 等额外必需输入。
3. **输出**：`sentence_embedding` shape `[batch, 1024]`，float32，L2 范数实测 `1.000000`。
4. **相似度**：完整余弦实现（对已归一化向量等价于点积），结果 ×100 打印。
5. **错误处理**：`anyhow` 传播，无 `unwrap()` 崩栈。

## 五、验收结果（`--selftest`）

官方 model card 用 **q4** 权重跑出的分数矩阵（queries × documents，余弦 ×100）：

```
[[66.65688, 28.08504],
 [29.10841, 70.66003]]
```

本工程用 **fp16** 权重的实测结果：

```
=== 分数矩阵（余弦相似度 ×100）===
  [   66.2748,    28.4627]
  [   29.7776,    69.9482]

  [0][0] got=   66.2748 want=   66.6569 diff=0.3820 OK
  [0][1] got=   28.4627 want=   28.0850 diff=0.3777 OK
  [1][0] got=   29.7776 want=   29.1084 diff=0.6692 OK
  [1][1] got=   69.9482 want=   70.6600 diff=0.7118 OK
对角线占优: 是
SELFTEST PASS
```

全部偏差 ≤ 0.72（验收要求 ±2 以内），且对角线严格占优 → 池化与 `attention_mask` 处理正确。

## 六、查询侧指令前缀

文档侧**不加**前缀；查询侧必须加，否则效果下降（model card FAQ 明确说明）。三个官方前缀：

```text
web search: "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: "
sts:        "Instruct: Retrieve semantically similar text\nQuery: "
bitext:     "Instruct: Retrieve parallel sentences\nQuery: "
```

`--query` 用的是第一个；其余可用 `--instruction` 传入。

## 七、构建注意事项（macOS arm64，已踩过的坑）

`ort` 默认 `download-binaries` 会下载 onnxruntime **静态库**，该静态库以 macOS **13.4** 为部署目标编译。
因此：

- 本仓库的 `.cargo/config.toml` 里已设 `MACOSX_DEPLOYMENT_TARGET = "13.4"`；
- **不要**在 `Cargo.toml` 里开启 `lto`。开启 LTO 后链接器会把 onnxruntime 的 CoreML EP
  目标文件一并拉入，链接报：
  ```
  Undefined symbols for architecture arm64:
    "___isPlatformVersionAtLeast", referenced from:
        onnxruntime::coreml::util::CoreMLVersion() in libort_sys-*.rlib
  ```

若目标机器无法下载 onnxruntime 预编译库，可改用本地动态库：

```bash
# 例如从 pip 安装的 onnxruntime 里拿到 dylib
python3 -m pip install onnxruntime
export ORT_DYLIB_PATH=$(python3 -c "import onnxruntime,os;print(os.path.join(os.path.dirname(onnxruntime.__file__),'capi','libonnxruntime.dylib'))")
```

并把 `Cargo.toml` 的 ort 依赖改为 `features = ["load-dynamic", "half"]`。

## 八、首次构建耗时

`ort-sys` 会编译整个 ONNX Runtime，首次 `cargo build --release` 约需数分钟，之后增量编译为秒级。
