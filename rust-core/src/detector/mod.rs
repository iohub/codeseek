//! JavaScript 编译/混淆代码检测模块
//!
//! 采用多级短路混合启发式架构，由浅入深检测：
//! - L1: 零开销指纹匹配（SIMD 优化的字符串包含检查）
//! - L2: 结构密度分析（平均行长）
//! - L3: 标识符语义分析（短变量名占比）

use regex::Regex;
use std::sync::LazyLock;

// 预编译正则，在程序首次运行时编译，后续调用零开销
static IDENTIFIER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z_$][a-zA-Z0-9_$]*").unwrap()
});

/// 检测结果分类
#[derive(Debug, Clone, PartialEq)]
pub enum CodeType {
    /// 源码（正常代码）
    SourceCode,
    /// 编译/压缩/混淆代码
    CompiledCode,
    /// 空文件
    Empty,
}

/// 检测报告
#[derive(Debug, Clone)]
pub struct DetectionReport {
    pub code_type: CodeType,
    pub reason: String,
}

/// 主检测函数
///
/// 分析 JS 代码内容，返回检测报告。
/// 采用短路策略：L1 → L2 → L3，命中即返回。
pub fn analyze_js_code(code: &str) -> DetectionReport {
    // 0. 空文件处理
    if code.trim().is_empty() {
        return DetectionReport {
            code_type: CodeType::Empty,
            reason: "文件内容为空".to_string(),
        };
    }

    // 1. L1: 零开销指纹匹配
    // 使用 str::contains，Rust 底层使用 memchr 并利用 SIMD 指令集加速
    let fingerprints = [
        "__webpack_require__",
        "webpackChunk",
        "webpackJsonp",
        "_classCallCheck",
        "_interopRequireDefault",
        "//# sourceMappingURL=",
        "var _0x",           // OB混淆特征
        "function _0x",      // OB混淆函数定义
        "(function(_0x",     // OB混淆IIFE
        "while (!![]) {",    // 控制流平坦化特征
        "eval(function(p,a,c,k,e,d)",  // Dean Edwards packer
        "eval(function(p,a,c,k,e,r)",
        "[][(![]+",          // JSFuck 特征
    ];

    for fp in fingerprints.iter() {
        if code.contains(fp) {
            return DetectionReport {
                code_type: CodeType::CompiledCode,
                reason: format!("命中编译器/混淆器指纹: '{}'", fp),
            };
        }
    }

    // 2. L2: 结构密度分析（平均行长）
    let total_chars = code.len();
    let non_empty_lines: Vec<&str> = code.lines().filter(|l| !l.trim().is_empty()).collect();

    if non_empty_lines.is_empty() {
        return DetectionReport {
            code_type: CodeType::Empty,
            reason: "无有效代码行".to_string(),
        };
    }

    let line_count = non_empty_lines.len();
    let avg_line_length = total_chars as f64 / line_count as f64;

    // 经验阈值：平均行长 > 400 字符，大概率是压缩代码
    if avg_line_length > 400.0 {
        return DetectionReport {
            code_type: CodeType::CompiledCode,
            reason: format!("代码密度过高，平均行长: {:.0} 字符", avg_line_length),
        };
    }

    // 3. L3: 标识符语义分析
    let identifiers: Vec<&str> = IDENTIFIER_RE
        .find_iter(code)
        .map(|m| m.as_str())
        .collect();

    // 代码量太少不具备统计意义，直接放行
    if identifiers.len() < 50 {
        return DetectionReport {
            code_type: CodeType::SourceCode,
            reason: "代码量过少，跳过语义分析".to_string(),
        };
    }

    let short_identifiers_count = identifiers.iter().filter(|id| id.len() <= 2).count();
    let short_ratio = short_identifiers_count as f64 / identifiers.len() as f64;

    // 经验阈值：短标识符占比 > 55%，判定为混淆代码
    if short_ratio > 0.55 {
        return DetectionReport {
            code_type: CodeType::CompiledCode,
            reason: format!("短标识符比例过高: {:.2}%", short_ratio * 100.0),
        };
    }

    // 默认判定为源码
    DetectionReport {
        code_type: CodeType::SourceCode,
        reason: "符合源码特征".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_code() {
        let code = "function add(a, b) { return a + b; }\nlet result = add(1, 2);\nconsole.log(result);";
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::SourceCode);
    }

    #[test]
    fn test_webpack_code() {
        let code = "const __webpack_require__ = () => { /* ... */ };\n";
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }

    #[test]
    fn test_minified_code() {
        // 模拟极长的压缩代码（多行重复，每行都很长）
        let line = "function a(b,c){return b+c}function d(e,f){return e*f}let g=a(1,2);let h=d(3,4);";
        let code = line.repeat(100);
        let report = analyze_js_code(&code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }

    #[test]
    fn test_obfuscator_code() {
        let code = "var _0x1a2b = ['hello']; console.log(_0x1a2b[0]);";
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }

    #[test]
    fn test_empty_code() {
        let report = analyze_js_code("   \n  \n  ");
        assert_eq!(report.code_type, CodeType::Empty);
    }

    #[test]
    fn test_short_code_skipped() {
        let code = "let x = 1;";
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::SourceCode);
    }

    #[test]
    fn test_control_flow_flattening() {
        let code = r#"
function something() {
    while (!![]) {
        console.log("hello");
    }
}
"#;
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }

    #[test]
    fn test_source_map_url() {
        let code = "// Some code\n//# sourceMappingURL=app.js.map\n";
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }

    #[test]
    fn test_webpack_chunk() {
        let code = r#"webpackChunkapp = function() { console.log("chunk"); }"#;
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }

    #[test]
    fn test_dean_edwards_packer() {
        let code = "eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\\\b'+c.toString(a)+'\\\\b','g'),k[c]);return p}('...',1,1,'x'.split('|'),0,{}))";
        let report = analyze_js_code(code);
        assert_eq!(report.code_type, CodeType::CompiledCode);
    }
}
