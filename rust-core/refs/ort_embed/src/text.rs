//! 文本预览：按**字符**（而不是字节）截断。
//!
//! 这里集中放预览相关的常量与函数，供 `report` 与 `selftest` 两处共用，
//! 避免两处各写一遍「截断 + 省略号」的逻辑而出现行为漂移。

/// 预览里保留的字符数
const PREVIEW_CHARS: usize = 60;

/// 按**字符**（而不是字节）截断字符串，返回 (前 `max_chars` 个字符, 是否发生了截断)。
///
/// 绝不使用 `&s[..n]` 这种按字节切片：中文 / emoji 是多字节 UTF-8，
/// 若 `n` 恰好落在字符内部，Rust 会直接 panic（"byte index N is not a char boundary"）。
/// 这里用 `char_indices().nth(max_chars)` 拿到第 `max_chars` 个字符的合法字节偏移，
/// 用该偏移切片天然落在 char 边界上，安全。
fn truncate_chars(s: &str, max_chars: usize) -> (String, bool) {
    match s.char_indices().nth(max_chars) {
        Some((cut, _)) => (s[..cut].to_string(), true),
        None => (s.to_string(), false),
    }
}

/// 预览 + 省略号标记：`(前 PREVIEW_CHARS 个字符, "…" 或 "")`。
///
/// 调用方自行决定省略号的位置（`print_vector` 把它放进引号内，
/// `selftest` 把它放在 `{:?}` 转义结果的外面），因此这里不做拼接。
pub fn preview_and_ellipsis(s: &str) -> (String, &'static str) {
    let (preview, truncated) = truncate_chars(s, PREVIEW_CHARS);
    (preview, if truncated { "…" } else { "" })
}

// ---------------------------------------------------------------------------
// 回归测试：字符串截断必须是 char 安全的
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::truncate_chars;

    /// 复现旧的脆弱实现：直接按字节切片。
    /// 只在测试里存在，用来证明这类写法确实会 panic。
    fn old_byte_slice_preview(s: &str) -> &str {
        &s[..60.min(s.len())]
    }

    /// 在静音 panic hook 的前提下执行 f，返回是否发生了 panic。
    /// hook 可能打印到 stderr，影响测试输出可读性，所以临时替换掉。
    fn panics<F: FnOnce() + std::panic::UnwindSafe>(f: F) -> bool {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(f);
        std::panic::set_hook(prev);
        r.is_err()
    }

    /// 关键回归：中英混排时第 60 个字节绝不是 char 边界 → 旧实现 panic，新实现正常。
    ///
    /// 注意：纯中文（每字 3 字节）或纯 emoji（每个 4 字节）时，60 恰好是 3 和 4 的公倍数，
    /// 第 60 字节**碰巧**落在字符边界上，不会 panic——这正是这个 bug 难以发现的原因。
    /// 只要前缀掺入 1 个 ASCII 字符，偏移立刻错位。
    #[test]
    fn mixed_text_old_impl_panics_new_impl_safe() {
        // 80 个汉字（3 字节/字）+ 1 个 ASCII 前缀 = 81 字符 > 60，必然触发截断
        let s = format!("a{}", "今天天气很好适合去公园散步股票市场出现了大幅波动".repeat(4));
        assert!(s.chars().count() > 60, "文本需超过 60 字符才能验证截断");
        assert!(!s.is_char_boundary(60), "本例中 60 必须不是 char 边界");
        assert!(panics(|| { old_byte_slice_preview(&s); }), "旧实现应 panic");

        let (preview, truncated) = truncate_chars(&s, 60);
        assert!(truncated, "超过 60 字符应标记截断");
        assert_eq!(preview.chars().count(), 60);
        assert!(s.starts_with(&preview), "截断结果应是原串的前缀");
        println!("中文 preview = {preview:?}");
    }

    /// emoji（4 字节字符）同理：加一个 ASCII 前缀就会错位
    #[test]
    fn emoji_old_impl_panics_new_impl_safe() {
        // 70 个 emoji（4 字节/个）+ 1 个 ASCII 前缀 = 71 字符 > 60
        let s = format!("a{}", "🧪".repeat(70));
        assert!(s.chars().count() > 60, "文本需超过 60 字符才能验证截断");
        assert!(!s.is_char_boundary(60), "本例中 60 必须不是 char 边界");
        assert!(panics(|| { old_byte_slice_preview(&s); }), "旧实现应 panic");

        let (preview, truncated) = truncate_chars(&s, 60);
        assert!(truncated);
        assert_eq!(preview.chars().count(), 60);
        println!("emoji preview = {preview:?}");
    }

    /// 短文本不截断、不补省略号
    #[test]
    fn short_text_is_not_truncated() {
        let (p, t) = truncate_chars("hi 你好", 60);
        assert!(!t);
        assert_eq!(p, "hi 你好");
    }

    /// 边界：恰好等于 max_chars 时不视为截断
    #[test]
    fn exact_boundary_is_not_truncated() {
        let s = "一二三四五";
        let (p, t) = truncate_chars(s, 5);
        assert!(!t);
        assert_eq!(p, s);
    }

    /// 空串不能 panic
    #[test]
    fn empty_text_is_safe() {
        let (p, t) = truncate_chars("", 60);
        assert!(!t);
        assert_eq!(p, "");
    }
}
