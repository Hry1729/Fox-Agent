//! 13.38 B5-1：Windows 批处理（.bat / .cmd）的换行与编码。
//!
//! cmd.exe 按系统 ANSI 代码页（中文系统 936/GBK）逐行读批处理，而且要求 CRLF。
//! Fox 写入或编辑 .bat / .cmd 时：
//! * 统一成 CRLF；
//! * 用 `GetACP()` 的代码页编码；
//! * 这个代码页编不了的字符（比如中文系统里的 emoji、日文假名以外的少数字符），整份改用
//!   UTF-8 无 BOM，并在第一行插入 `@chcp 65001 >nul`，让 cmd 先切到 UTF-8 再读后面的行。
//!
//! 读取时反过来：UTF-8 原样；否则按同一个代码页解码；CRLF 显示成 LF（写回时再转）。
//! Fox 自己插入的 chcp 行在写回时先去掉再重新判断，所以改到能用 ANSI 编码时不会留下
//! 一行把 cmd 切到 65001 的残留。非 Windows 平台不做任何转换（`batch_code_page()` 为 None）。

use std::path::Path;

/// Fox 在 UTF-8 回退时插入的第一行（不含换行）。
pub const UTF8_CHCP_LINE: &str = "@chcp 65001 >nul";

/// 这个路径是不是 cmd 批处理（扩展名不区分大小写）。
pub fn is_batch_script(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd"))
}

/// 需要转换时返回系统 ANSI 代码页；非 Windows 返回 None（行为不变）。
pub fn batch_code_page() -> Option<u32> {
    #[cfg(windows)]
    {
        let code_page = unsafe { windows::Win32::Globalization::GetACP() };
        Some(code_page)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 这个路径在本机是否要走批处理编码。
pub fn batch_code_page_for(path: &Path) -> Option<u32> {
    if is_batch_script(path) { batch_code_page() } else { None }
}

/// Windows ANSI 代码页 → encoding_rs 编码。不认识的返回 None（只能走 UTF-8 回退）。
fn encoding_for_code_page(code_page: u32) -> Option<&'static encoding_rs::Encoding> {
    use encoding_rs::*;
    Some(match code_page {
        936 => GBK,
        54936 => GB18030,
        950 => BIG5,
        932 => SHIFT_JIS,
        949 => EUC_KR,
        874 => WINDOWS_874,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1252 => WINDOWS_1252,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        65001 => UTF_8,
        _ => return None,
    })
}

/// 任意换行 → CRLF。
fn to_crlf(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized.replace('\n', "\r\n")
}

/// 去掉 Fox 自己插入的第一行 chcp（大小写、首尾空白不敏感）。
fn strip_fox_chcp(text: &str) -> &str {
    let (first, rest) = match text.find('\n') {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    if first.trim_end_matches('\r').trim().eq_ignore_ascii_case(UTF8_CHCP_LINE) {
        rest
    } else {
        text
    }
}

/// 写入时用的编码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedBatch {
    pub bytes: Vec<u8>,
    /// `cp936`、`utf-8+chcp65001` 等，进工具结果的 details 方便排查。
    pub encoding: String,
}

/// 把要写进 .bat / .cmd 的文本编码成 cmd 能直接执行的字节。
pub fn encode_batch(text: &str, code_page: u32) -> EncodedBatch {
    let body = to_crlf(strip_fox_chcp(text));
    if body.is_ascii() {
        return EncodedBatch { bytes: body.into_bytes(), encoding: format!("cp{code_page}") };
    }
    if code_page == 65001 {
        return EncodedBatch { bytes: body.into_bytes(), encoding: "utf-8".into() };
    }
    if let Some(encoding) = encoding_for_code_page(code_page) {
        let (bytes, _, unmappable) = encoding.encode(&body);
        if !unmappable {
            return EncodedBatch { bytes: bytes.into_owned(), encoding: format!("cp{code_page}") };
        }
    }
    let mut bytes = Vec::with_capacity(body.len() + UTF8_CHCP_LINE.len() + 2);
    bytes.extend_from_slice(UTF8_CHCP_LINE.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(body.as_bytes());
    EncodedBatch { bytes, encoding: "utf-8+chcp65001".into() }
}

/// 读回 .bat / .cmd：返回 (文本, 编码名, 是否有解不了的字节)。文本里的换行统一成 LF。
pub fn decode_batch(bytes: &[u8], code_page: u32) -> (String, String, bool) {
    let (text, encoding, lossy) = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let (text, lossy) = encoding_rs::UTF_8.decode_without_bom_handling(rest);
        (text.into_owned(), "utf-8-bom".to_owned(), lossy)
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        (text.to_owned(), "utf-8".to_owned(), false)
    } else if let Some(encoding) = encoding_for_code_page(code_page) {
        let (text, lossy) = encoding.decode_without_bom_handling(bytes);
        (text.into_owned(), format!("cp{code_page}"), lossy)
    } else {
        (String::from_utf8_lossy(bytes).into_owned(), format!("cp{code_page}-lossy"), true)
    };
    (text.replace("\r\n", "\n"), encoding, lossy)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GBK: u32 = 936;

    #[test]
    fn batch_extensions_are_case_insensitive() {
        assert!(is_batch_script(Path::new("测试.bat")));
        assert!(is_batch_script(Path::new("D:/x/RUN.CMD")));
        assert!(!is_batch_script(Path::new("a.ps1")));
        assert!(!is_batch_script(Path::new("bat")));
    }

    /// 实测 B5-1 的脚本：中文文件名 + 中文内容 + %1 参数。
    #[test]
    fn chinese_batch_with_parameter_becomes_gbk_crlf() {
        let script = "@echo off\necho 参数是 %1\n";
        let encoded = encode_batch(script, GBK);
        assert_eq!(encoded.encoding, "cp936");
        let expected = encoding_rs::GBK.encode("@echo off\r\necho 参数是 %1\r\n").0.into_owned();
        assert_eq!(encoded.bytes, expected);
        assert!(!encoded.bytes.windows(2).any(|pair| pair[0] != b'\r' && pair[1] == b'\n'), "每个 LF 前都有 CR");
        assert!(std::str::from_utf8(&encoded.bytes).is_err(), "不再是 UTF-8 字节");
        let (text, encoding, lossy) = decode_batch(&encoded.bytes, GBK);
        assert_eq!((text.as_str(), encoding.as_str(), lossy), (script, "cp936", false), "读回原文");
    }

    #[test]
    fn unmappable_characters_fall_back_to_utf8_with_chcp() {
        let script = "echo 你好 😀 %1\n";
        let encoded = encode_batch(script, GBK);
        assert_eq!(encoded.encoding, "utf-8+chcp65001");
        assert_eq!(encoded.bytes, "@chcp 65001 >nul\r\necho 你好 😀 %1\r\n".as_bytes());
        assert!(!encoded.bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "无 BOM");
        let (text, encoding, _) = decode_batch(&encoded.bytes, GBK);
        assert_eq!(encoding, "utf-8");
        assert_eq!(text, "@chcp 65001 >nul\necho 你好 😀 %1\n");
        // 编辑后再写：chcp 行不会重复插入。
        let again = encode_batch(&text, GBK);
        assert_eq!(again.bytes, encoded.bytes);
    }

    #[test]
    fn fox_chcp_line_is_dropped_when_ansi_can_encode_again() {
        let edited = "@chcp 65001 >nul\necho 你好 %1\n";
        let encoded = encode_batch(edited, GBK);
        assert_eq!(encoded.encoding, "cp936");
        assert_eq!(encoded.bytes, encoding_rs::GBK.encode("echo 你好 %1\r\n").0.into_owned());
    }

    #[test]
    fn ascii_and_mixed_line_endings_become_crlf() {
        let encoded = encode_batch("@echo off\r\necho a\recho b\n", GBK);
        assert_eq!(encoded.bytes, b"@echo off\r\necho a\r\necho b\r\n");
        assert_eq!(encode_batch("echo %1", GBK).bytes, b"echo %1", "没有换行就不加");
    }

    #[test]
    fn other_code_pages_and_unknown_ones() {
        let latin = encode_batch("echo café\n", 1252);
        assert_eq!(latin.encoding, "cp1252");
        assert_eq!(latin.bytes, b"echo caf\xe9\r\n");
        assert_eq!(decode_batch(&latin.bytes, 1252).0, "echo café\n");
        // 系统本身是 UTF-8（Beta：使用 Unicode UTF-8）：不需要 chcp。
        assert_eq!(encode_batch("echo 你好\n", 65001).bytes, "echo 你好\r\n".as_bytes());
        // 不认识的代码页：非 ASCII 只能 UTF-8 + chcp。
        assert_eq!(encode_batch("echo 你好\n", 12345).encoding, "utf-8+chcp65001");
    }

    #[test]
    fn existing_utf8_and_bom_files_still_read() {
        assert_eq!(decode_batch("echo 你好\r\n".as_bytes(), GBK), ("echo 你好\n".to_owned(), "utf-8".to_owned(), false));
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("echo 你好".as_bytes());
        assert_eq!(decode_batch(&bom, GBK).1, "utf-8-bom");
    }
}
