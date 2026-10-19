//! 内置的暴雪游戏已知文件名清单。
//!
//! MPQ 归档的文件名只存在于可选的 `(listfile)` 中；受保护地图和部分
//! 受损归档没有它，此时条目只能显示为 `<unnamed #N>`。与内置了
//! war3.txt / war3x.txt 清单的第三方工具（Ladik's MPQ Editor、
//! MPQMaster 等）相同，[`crate::Archive`] 在缺失 listfile 时会把这些
//! 名字逐个送入 hash 表试探 —— 命中（name1 / name2 完全匹配）即为
//! 真实文件名，误报概率约为 2⁻⁶⁴。
//!
//! [`BUNDLED_LISTFILE`] 是嵌入的主清单：MPQMaster 所带五个魔兽
//! 清单（Warcraft III / TFT / ROC / ROC Beta / 完整魔兽数据列表）
//! 合并去重后的 zlib 压缩文本（首次使用时解压一次）。此外保留一份
//! 精炼的核心清单 [`WAR3_FILES`]，覆盖嵌入清单可能滞后缺收的
//! 新格式文件（如 Reforged 的 `war3map.lua`）。
//!
//! Warcraft III 地图还有一条额外恢复途径：世界编辑器写入的
//! `war3map.imp` 导入清单，逐条列出地图导入文件的路径，见
//! [`parse_imp`]。

use std::io::Read;
use std::sync::LazyLock;

use flate2::read::ZlibDecoder;

/// 合并清单的 zlib 压缩文本（77 067 个名字，CRLF 分隔，ASCII）。
static BUNDLED_LISTFILE_ZLIB: &[u8] = include_bytes!("data/war3_listfile.zlib");

/// 嵌入的主清单：合并去重后的魔兽文件名全集（首次访问时解压）。
pub(crate) static BUNDLED_LISTFILE: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut text = String::new();
    let _ = ZlibDecoder::new(BUNDLED_LISTFILE_ZLIB).read_to_string(&mut text);
    text.lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
});

/// Warcraft III 地图 / 战役文件（StormLib war3.txt 清单的核心部分）。
pub(crate) const WAR3_FILES: &[&str] = &[
    // -- 世界编辑器地图文件 --------------------------------------------
    "war3map.w3e", // 地形环境
    "war3map.w3i", // 地图信息
    "war3map.w3r", // 区域
    "war3map.w3c", // 摄像机
    "war3map.w3s", // 声音定义
    "war3map.w3a", // 自定义技能
    "war3map.w3q", // 自定义升级
    "war3map.w3u", // 自定义单位
    "war3map.w3t", // 自定义物品
    "war3map.w3b", // 自定义光环/效果
    "war3map.w3d", // 自定义可破坏物
    "war3map.w3h", // 自定义物品技能
    "war3map.w3f", // 自定义雾
    "war3map.wtg", // 触发器（GUI）
    "war3map.wct", // 触发器自定义文本
    "war3map.wts", // 字符串表
    "war3map.j",   // JASS 脚本
    "war3map.lua", // Lua 脚本（Reforged）
    "war3map.ai",  // AI 脚本
    "war3map.doo", // 装饰物
    "war3mapUnits.doo", // 单位放置
    "war3map.shd", // 阴影
    "war3map.mmp", // 小地图标记
    "war3map.wpm", // 路径图
    "war3map.imp", // 导入清单（也用于恢复导入文件名）
    "war3map.blp",
    "war3mapMap.blp", // 加载画面
    "war3mapMap.tga",
    "war3mapMap.jpg",
    "war3mapMap.pcx",
    "war3mapPreview.blp", // 小地图预览
    "war3mapPreview.tga",
    "war3mapPreview.jpg",
    "war3mapPreview.pcx",
    "war3mapMisc.txt",
    "war3mapExtra.txt",
    // -- 战役文件 --------------------------------------------------------
    "war3campaign.w3f",
    "war3campaign.j",
    "war3campaignLib.j",
    // -- 公共脚本库（Scripts\ 前缀） --------------------------------
    "Scripts\\war3map.j",
    "Scripts\\war3map.lua",
    "Scripts\\Blizzard.j",
    "Scripts\\common.j",
    "Scripts\\common.ai",
    "Scripts\\AprFunctions.j",
];

/// 地图常用来替换原版游戏数据的文件（WE 平衡性修改、汉化包等）。
pub(crate) const WAR3_DATA_FILES: &[&str] = &[
    "Units\\CommandFunc.txt",
    "Units\\CommandStrings.txt",
    "Units\\ItemFunc.txt",
    "Units\\ItemStrings.txt",
    "Units\\MiscGame.txt",
    "Units\\MiscGameStrings.txt",
    "UI\\MiscData.txt",
    "UI\\WorldEditStrings.txt",
    "Units\\HumanUnitFunc.txt",
    "Units\\HumanUnitStrings.txt",
    "Units\\HumanAbilityFunc.txt",
    "Units\\HumanAbilityStrings.txt",
    "Units\\HumanUpgradeFunc.txt",
    "Units\\HumanUpgradeStrings.txt",
    "Units\\HumanBuildingFunc.txt",
    "Units\\HumanBuildingStrings.txt",
    "Units\\OrcUnitFunc.txt",
    "Units\\OrcUnitStrings.txt",
    "Units\\OrcAbilityFunc.txt",
    "Units\\OrcAbilityStrings.txt",
    "Units\\OrcUpgradeFunc.txt",
    "Units\\OrcUpgradeStrings.txt",
    "Units\\OrcBuildingFunc.txt",
    "Units\\OrcBuildingStrings.txt",
    "Units\\NightElfUnitFunc.txt",
    "Units\\NightElfUnitStrings.txt",
    "Units\\NightElfAbilityFunc.txt",
    "Units\\NightElfAbilityStrings.txt",
    "Units\\NightElfUpgradeFunc.txt",
    "Units\\NightElfUpgradeStrings.txt",
    "Units\\NightElfBuildingFunc.txt",
    "Units\\NightElfBuildingStrings.txt",
    "Units\\UndeadUnitFunc.txt",
    "Units\\UndeadUnitStrings.txt",
    "Units\\UndeadAbilityFunc.txt",
    "Units\\UndeadAbilityStrings.txt",
    "Units\\UndeadUpgradeFunc.txt",
    "Units\\UndeadUpgradeStrings.txt",
    "Units\\UndeadBuildingFunc.txt",
    "Units\\UndeadBuildingStrings.txt",
    "Units\\NeutralUnitFunc.txt",
    "Units\\NeutralUnitStrings.txt",
    "Units\\NeutralAbilityFunc.txt",
    "Units\\NeutralAbilityStrings.txt",
    "Units\\NeutralUpgradeFunc.txt",
    "Units\\NeutralUpgradeStrings.txt",
    "Units\\NeutralBuildingFunc.txt",
    "Units\\NeutralBuildingStrings.txt",
];

/// 其他暴雪游戏中最常见的核心文件。
pub(crate) const MISC_GAME_FILES: &[&str] = &[
    "staredit\\scenario.chk", // StarCraft 地图本体
];

// -- 未命名条目的内容类型探测 -----------------------------------------------
//
// 仍未命名的条目（无任何引用线索）可以读其内容、按文件头（魔法
// 字节）识别实际类型，把 `<unnamed #N>` 显示为 `<unnamed #N>.mdx`
// 等 —— 后缀仅提示类型，不是原文件名。加密的未命名条目无法解密
// （storm 加密的 key 由名字派生），只能保持无后缀。

/// 按文件头 / 结构特征识别资源类型，返回建议的扩展名（小写）。
/// 覆盖 Warcraft III 地图常见的导入资源格式。
pub(crate) fn detect_ext(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"MDLX") {
        return Some("mdx"); // Blizzard MDX 压缩模型
    }
    if data.starts_with(b"BLP") && data.len() > 3 && data[3].is_ascii_digit() {
        return Some("blp"); // BLP0/1/2 纹理（War3 为 BLP1）
    }
    if data.starts_with(b"\x89PNG") {
        return Some("png");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if data.starts_with(b"GIF8") {
        return Some("gif");
    }
    if data.starts_with(b"DDS ") {
        return Some("dds");
    }
    if data.starts_with(b"OggS") {
        return Some("ogg");
    }
    if data.starts_with(b"fLaC") {
        return Some("flac");
    }
    if data.starts_with(b"MThd") {
        return Some("mid");
    }
    if data.starts_with(b"ID3") {
        return Some("mp3");
    }
    // 无 ID3 头的裸 MPEG 音频帧同步（误报仅影响显示后缀，无害）。
    if data.len() >= 4
        && data[0] == 0xFF
        && data[1] & 0xE0 == 0xE0
        && data[1] & 0x06 != 0 // layer 位不全 0
        && data[2] & 0xF0 != 0xF0 // bitrate index 保留值
        && data[2] & 0x0C != 0x0C // 采样率保留值
    {
        return Some("mp3");
    }
    if data.starts_with(b"RIFF") && data.len() >= 12 {
        return match &data[8..12] {
            b"WAVE" => Some("wav"),
            b"AVI " => Some("avi"),
            _ => None,
        };
    }
    // TGA 无文件头；可选 footer 位于最后 18 字节。
    if data.len() >= 18 && &data[data.len() - 18..] == b"TRUEVISION-XFILE.\0" {
        return Some("tga");
    }
    if data.starts_with(b"ID;") {
        return Some("slk"); // Firegalaxy SLK 数据表
    }
    None
}

// -- 从已命名文件中扫描资源路径 --------------------------------------------
//
// 导入文件（war3mapImported\... 等）的名字不出现在任何游戏清单里，
// 但会被地图自身的文件引用：
// * `war3map.j` / `.lua` / `.ai` 脚本：字符串常量里的特效 / 预载路径，
//   JASS 源码中反斜杠以 `\\` 转义、也可能写作 `/`；
// * 对象数据（`.w3u/.w3t/.w3b/.w3d/.w3a/.w3h/.w3q`）：自定义单位 /
//   物品的模型、图标、投射物等字段直接内嵌 UTF-8 路径；
// * 声音定义（`.w3s`）、字符串表（`.wts`）、触发器（`.wtg/.wct`）等。

/// 值得扫描资源路径的文件扩展名（小写；不含扩展名的文件不扫描）。
/// MDX 不在此列 —— 它走精确的 [`mdx_texture_paths`]（TEXS chunk），
/// 避免对大模型的顶点 / 动画数据做无意义的 token 扫描。
const SCANNABLE_EXTS: &[&str] = &[
    "j", "lua", "ai", "txt", "slk", "fdf", "wts", "wtg", "wct", "w3i", "w3s", "w3u", "w3t",
    "w3b", "w3d", "w3a", "w3h", "w3q", "mdl",
];

/// 解析 MDX 模型的 TEXS chunk，提取内嵌的贴图路径。MDX 为 chunk
/// 结构（`[四字符标签][u32 大小][内容]`）；TEXS 的每条记录**定长
/// 268 字节**：`replaceableId (u32) + path (char[256]，NUL 填充) +
/// flags (u32) + 保留 (u32)`。replaceableId 非 0 的记录是
/// ReplaceableTextures 替换槽，路径为空、自然被跳过。
pub fn mdx_texture_paths(data: &[u8]) -> Vec<String> {
    /// 单条 TEXS 记录的字节数。
    const RECORD: usize = 4 + 256 + 4 + 4;

    let mut out = Vec::new();
    if !data.starts_with(b"MDLX") {
        return out;
    }
    let mut pos = 4usize;
    while pos + 8 <= data.len() {
        let tag = &data[pos..pos + 4];
        let size =
            u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(data.len());
        if tag == b"TEXS" {
            let mut p = body;
            while p + RECORD <= end {
                let field = &data[p + 4..p + 4 + 256];
                let nul = field.iter().position(|&b| b == 0).unwrap_or(256);
                let path = &field[..nul];
                if !path.is_empty() && std::str::from_utf8(path).is_ok() {
                    let norm: String = path
                        .iter()
                        .map(|&b| if b == b'/' { b'\\' } else { b })
                        .map(|b| b as char)
                        .collect();
                    out.push(norm);
                }
                p += RECORD;
            }
        }
        pos = end;
    }
    out
}

/// 候选资源路径的目标扩展名（小写）。
const PATH_EXTS: &[&str] = &[
    "mdx", "mdl", "blp", "tga", "jpg", "jpeg", "pcx", "png", "gif", "wav", "mp3", "mid", "txt",
    "slk", "fdf",
];

/// 文件名（按小写扩展名）是否值得扫描资源路径。
pub(crate) fn is_scannable(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    if dot + 1 >= name.len() {
        return false;
    }
    let ext = name[dot + 1..].to_ascii_lowercase();
    SCANNABLE_EXTS.contains(&ext.as_str())
}

/// 路径 token 的字符集：字母数字、空格与常见文件名字符、两种
/// 分隔符，以及一切 >= 0x80 的字节（UTF-8 中文等非 ASCII 文件名，
/// 具体合法性在 [`consider_token`] 做 UTF-8 校验时把关）。引号
/// （JASS 字符串边界）、括号（函数调用）等其余字符都会终止 token。
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b >= 0x80 || b" _-.\\[]!+#/".contains(&b)
}

/// 从任意文件内容（脚本文本、对象数据二进制等）中提取候选资源
/// 路径：按 [`is_token_byte`] 分词，把 `\\` 折叠为 `\`、`/` 改写为
/// `\`，保留扩展名属于 [`PATH_EXTS`] 的 token。误报候选无害 ——
/// 试探不命中即被丢弃。
pub(crate) fn extract_candidate_paths(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &b) in data.iter().enumerate() {
        if is_token_byte(b) {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            consider_token(&data[s..i], &mut out);
        }
    }
    if let Some(s) = start {
        consider_token(&data[s..], &mut out);
    }
    out
}

fn consider_token(token: &[u8], out: &mut Vec<String>) {
    if token.len() < 4 || token.len() > 260 {
        return;
    }
    // 非 ASCII 字节（中文文件名等）经 UTF-8 校验后放行。
    let Ok(t) = std::str::from_utf8(token) else {
        return;
    };

    // JASS 转义 `\\` 折叠、`/` 归一为 `\`。
    let mut norm = String::with_capacity(t.len());
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                norm.push('\\');
            }
            '/' => norm.push('\\'),
            _ => norm.push(c),
        }
    }

    let n = norm.trim_matches(|c: char| c == ' ' || c == '.');
    let Some(dot) = n.rfind('.') else { return };
    let ext = n[dot + 1..].to_ascii_lowercase();
    if !PATH_EXTS.contains(&ext.as_str()) {
        return;
    }
    // 扩展名本身至少 2 个字符且不含分隔符。
    if ext.len() < 2 || ext.contains('\\') {
        return;
    }
    out.push(n.to_string());
}

/// 解析 `war3map.imp`（世界编辑器导入清单）：每条记录为 1 字节标志
/// （0x0D = 自定义路径，0x0A = 原版路径）加 NUL 结尾的路径字符串。
/// 遇到畸形数据（无 NUL 终止、不可打印字符）即静默停止。
pub(crate) fn parse_imp(data: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        pos += 1; // 标志字节
        let end = match data[pos..].iter().position(|&b| b == 0) {
            Some(e) => pos + e,
            None => break,
        };
        let raw = &data[pos..end];
        pos = end + 1;
        if raw.is_empty() || raw.len() > 260 {
            break;
        }
        if !raw.iter().all(|&b| (0x20..0x7F).contains(&b) || b >= 0x80) {
            break;
        }
        paths.push(String::from_utf8_lossy(raw).into_owned());
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flagged_nul_terminated_paths() {
        let mut data = vec![0x0D];
        data.extend_from_slice(b"war3mapImported\\icon.blp\0");
        data.push(0x0A);
        data.extend_from_slice(b"Units\\CommandFunc.txt\0");
        assert_eq!(
            parse_imp(&data),
            vec![
                "war3mapImported\\icon.blp".to_string(),
                "Units\\CommandFunc.txt".to_string()
            ]
        );
    }

    #[test]
    fn stops_on_missing_terminator() {
        assert_eq!(parse_imp(&[0x0D, b'a', b'b']), Vec::<String>::new());
    }

    #[test]
    fn extracts_jass_escaped_and_slash_paths() {
        let src = br#"
call AddSpecialEffect("war3mapImported\\fx.mdx", x, y)
call Preload("Sound\\Custom\\bgm.mp3")
call PlayMusic("war3mapImported/intro.mid")
call BJDebugMsg("hello, world")
set count = 100.
"#;
        let paths = extract_candidate_paths(src);
        assert!(paths.contains(&"war3mapImported\\fx.mdx".to_string()));
        assert!(paths.contains(&"Sound\\Custom\\bgm.mp3".to_string()));
        assert!(paths.contains(&"war3mapImported\\intro.mid".to_string()));
        // 普通字符串与数字字面量不得成为候选。
        assert!(!paths.iter().any(|p| p.contains("hello")));
        assert!(!paths.iter().any(|p| p.contains("100")));
    }

    #[test]
    fn extracts_paths_embedded_in_binary_object_data() {
        let mut data = vec![0x00, 0x01, 0xFF, 0x10];
        data.extend_from_slice(b"war3mapImported\\heroModel.mdx\0");
        data.extend_from_slice(b"ReplaceableTextures\\Custom\\icon.blp\0");
        data.extend_from_slice(&[0u8; 4]);
        let paths = extract_candidate_paths(&data);
        assert!(paths.contains(&"war3mapImported\\heroModel.mdx".to_string()));
        assert!(paths
            .contains(&"ReplaceableTextures\\Custom\\icon.blp".to_string()));
    }

    #[test]
    fn rejects_unrelated_extensions_and_noise() {
        let paths = extract_candidate_paths(b"see https://example.com/a.jpg note foo.bar baz.mdx.exe");
        // `//` 折叠后 https: 冒号断段 —— `example.com\a.jpg` 有可能
        // 被提取，但 `baz.mdx.exe`（多级扩展名取最后一段）不属于
        // 候选集合之外的 .exe …… 实际会以 .exe 结尾被拒绝：
        assert!(!paths.iter().any(|p| p.ends_with(".exe")));
        assert!(!paths.iter().any(|p| p.contains("foo.bar")));
    }

    #[test]
    fn scannable_by_extension() {
        assert!(is_scannable("war3map.j"));
        assert!(is_scannable("war3map.W3U")); // 大小写不敏感
        assert!(is_scannable("Scripts\\common.ai"));
        assert!(!is_scannable("war3map.doo"));
        assert!(!is_scannable("war3map"));
        assert!(!is_scannable("(listfile)"));
        assert!(!is_scannable("hero.mdx")); // 走 TEXS 精确解析
    }

    #[test]
    fn parses_mdx_texs_texture_paths() {
        /// TEXS 记录定长 268：id(4) + path[256] + flags(4) + 保留(4)。
        fn tex_record(path: &[u8]) -> Vec<u8> {
            let mut rec = vec![0u8; 268];
            rec[4..4 + path.len()].copy_from_slice(path);
            rec
        }

        let mut mdx = Vec::new();
        mdx.extend_from_slice(b"MDLX");
        // VERS chunk：size=4，内容 u32 版本。
        mdx.extend_from_slice(b"VERS");
        mdx.extend_from_slice(&4u32.to_le_bytes());
        mdx.extend_from_slice(&4u32.to_le_bytes());
        // TEXS chunk：两条命名贴图 + 一条 ReplaceableTextures 空槽。
        let mut texs = Vec::new();
        texs.extend_from_slice(&tex_record(b"war3mapImported\\hero.blp"));
        texs.extend_from_slice(&tex_record(b"ReplaceableTextures/Custom/t.tga"));
        // 替换槽：id 非 0、路径为空（写在最后一条记录的开头）。
        let last_rec = texs.len() - 268;
        texs[last_rec..last_rec + 4].copy_from_slice(&1u32.to_le_bytes());
        texs.extend_from_slice(&tex_record(b"Textures\\Glow.blp"));
        mdx.extend_from_slice(b"TEXS");
        mdx.extend_from_slice(&(texs.len() as u32).to_le_bytes());
        mdx.extend_from_slice(&texs);
        // GEOS chunk 头（内容为任意二进制）——遍历应正确跳过。
        mdx.extend_from_slice(b"GEOS");
        mdx.extend_from_slice(&8u32.to_le_bytes());
        mdx.extend_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);

        assert_eq!(
            mdx_texture_paths(&mdx),
            vec![
                "war3mapImported\\hero.blp".to_string(),
                "ReplaceableTextures\\Custom\\t.tga".to_string(),
                "Textures\\Glow.blp".to_string(),
            ]
        );
        // 非 MDX 数据安全返回空。
        assert!(mdx_texture_paths(b"BLP1xxxx").is_empty());
    }

    #[test]
    fn detects_resource_magic_bytes() {
        assert_eq!(detect_ext(b"MDLX\x04\x00\x00\x00"), Some("mdx"));
        assert_eq!(detect_ext(b"BLP1\x01"), Some("blp"));
        assert_eq!(detect_ext(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(detect_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(detect_ext(b"GIF89a"), Some("gif"));
        assert_eq!(detect_ext(b"DDS |\x00\x00\x00"), Some("dds"));
        assert_eq!(detect_ext(b"OggS\x00"), Some("ogg"));
        assert_eq!(detect_ext(b"fLaC"), Some("flac"));
        assert_eq!(detect_ext(b"MThd\x00\x00"), Some("mid"));
        assert_eq!(detect_ext(b"ID3\x04\x00"), Some("mp3"));
        // 裸 MPEG 音频帧。
        assert_eq!(detect_ext(&[0xFF, 0xFB, 0x90, 0x00]), Some("mp3"));
        // RIFF/WAVE：magic 在偏移 8 处。
        let mut wav = b"RIFF\x24\x00\x00\x00WAVEfmt ".to_vec();
        assert_eq!(detect_ext(&wav), Some("wav"));
        wav[8..12].copy_from_slice(b"AVI ");
        assert_eq!(detect_ext(&wav), Some("avi"));
        // TGA footer（最后 18 字节）。
        let mut tga = vec![0u8; 64];
        tga[64 - 18..].copy_from_slice(b"TRUEVISION-XFILE.\0");
        assert_eq!(detect_ext(&tga), Some("tga"));
        // SLK 表头。
        assert_eq!(detect_ext(b"ID;PSCALAR"), Some("slk"));
        // 未知 / 文本内容不得误判。
        assert_eq!(detect_ext(b"hello world, this is text"), None);
        assert_eq!(detect_ext(b"W3E!\x11"), None);
    }
}
