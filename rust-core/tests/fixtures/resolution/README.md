# 名称解析基线 fixture（Phase P0）

这些目录是**被 parser 真实读取的源码样本**，专门用来暴露"同名即命中、命中取第一个"的
调用边解析缺陷。每个 case 都不要求能被编译器编译通过（我们只用 tree-sitter 做语法解析）。

| case 目录 | 场景 | 当前解析结果 | 正确解析结果 |
|---|---|---|---|
| `cross_file_same_name/` | 跨文件同名函数 | `call_helper → helper` 的 callee 取决于 HashMap 迭代顺序（`alpha.rs` 或 `beta.rs`），且被标记为 `is_resolved=true` | 两个候选无法由语法信息区分 → 应保留 **2 个候选**（歧义），不能宣称已解析 |
| `python_import_alias/` | `from mod_x import func as alias` 后调用 `alias()` | `alias` 不是任何函数名 → 产生 `is_resolved=false` 的未解析边，`mod_x.func` 永远没有 caller | 应沿 import 解析到 `mod_x.func` |
| `rust_qualified_path/` | `mod_a::pick(x)` | AST 层就把 name 切成裸名 `pick`、限定前缀丢失 → 与"裸名同名"同一套 first-wins | 应解析到 `mod_a::pick`（前缀参与作用域查找） |
| `python_dotted_qualified/` | `mod_y.helper(value)` | 前缀要么在 AST 丢失、要么走 `namespace.contains(prefix) \|\| name == method` 的伪校验 → 仍然 first-wins | 应解析到 `mod_y.helper` |
| `method_receiver/` | `calc.add(x)`，同时存在自由函数 `add` | receiver 在 AST 层退化成一个 `VariableUsage`，调用符号的 name 只剩 `add` → 与自由函数 `add` 无法区分 | 应按 receiver 类型解析到 `Calc::add` |
| `nested_function_caller/` | 外层函数里定义嵌套函数，嵌套函数体内发起调用；文件里另有一个闭包调用 | 第 12 行同时落在 `inner(11..13)` 与 `outer(10..16)` 两个函数区间内，caller 由"本文件函数列表行区间 first-wins"决定 —— 实测取到 `inner`，但那是 tree-sitter 先产出内层符号的**顺序巧合**，不是"最内层优先"的保证；另外 `let doubled = \|y\| y*2;` 只产出 `VariableDefinition` 而非 `FunctionDeclaration`，于是 `doubled(x)` 解析失败并造出 `namespace="unresolved"` 的幽灵节点 | caller 应记给**最内层**包含该行的函数；闭包应作为可调用实体挂到定义它的外层函数 |
| `ambiguous_trait_impl/` | `run` 有 5 份同名定义（trait 声明 + 2 个 trait impl + 2 个 inherent impl），`drive(runner: &dyn Runner)` 里调用 `runner.run()` | "本文件同名 first-wins"把它**稳定地**错接到 `driver.rs::run`（`Loudish` 的 inherent 方法，与 `dyn Runner` 无关），且 `is_resolved=true`；`runners.rs` 里的 4 份候选一条 caller 都没有 | 应按参数类型 `dyn Runner` 解析到 `runners.rs` 的 trait/impl 候选集（无法唯一确定时保留多候选），绝不该指向 `driver.rs::run` |

配套的基线测试见 `rust-core/tests/test_resolution_baseline.rs`；
每个用例都用 `// KNOWN-DEFECT(P2 修复): ...` 注释锁定"当前实际行为"，改造后需同步更新断言。
