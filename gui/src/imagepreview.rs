//! 归档内图片条目的预览：浏览器 `<img>` 原生支持的格式以 base64
//! data URL 原样传给前端；TGA 与 Warcraft III 的 BLP2 在此解码后
//! 转成 PNG。图片不走 256 KiB 文本截断，也不可编辑。

use base64::Engine as _;
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::Serialize;

/// 浏览器原生支持的扩展名 → MIME（直接透传，不解码）。
const NATIVE_MIME: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("jfif", "image/jpeg"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("dib", "image/bmp"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("cur", "image/x-icon"),
    ("svg", "image/svg+xml"),
];

/// 原生透传的大小上限：base64 后约放大 1/3，再大走 IPC 不划算。
const NATIVE_LIMIT: usize = 16 * 1024 * 1024;
/// 解码格式（TGA / BLP）的像素数上限，防止损坏的尺寸头分配超大内存。
const MAX_PIXELS: u64 = 4096 * 4096;

#[derive(Serialize)]
pub struct ImageDto {
    /// 完整 data URL（`data:<mime>;base64,…`），前端直接作为 `<img src>`。
    pub url: String,
    pub mime: String,
    /// 由本工具解码转 PNG 显示（TGA / BLP）；false = 原样透传。
    pub converted: bool,
}

/// 条目按扩展名是图片时返回 data URL；不是图片返回 `None`，
/// 由调用方继续走文本预览；数据损坏以 Err 返回。
pub fn build(name: &str, data: &[u8]) -> Result<Option<ImageDto>, String> {
    let ext = ext_of(name);
    if let Some((_, mime)) = NATIVE_MIME.iter().find(|(e, _)| *e == ext) {
        if data.len() > NATIVE_LIMIT {
            return Err(format!(
                "图片过大（{} MiB，上限 16 MiB），请提取后用看图软件查看",
                data.len() / 1024 / 1024
            ));
        }
        return Ok(Some(data_url(mime, data)));
    }
    match ext.as_str() {
        "tga" => decode_tga(data).map(Some),
        "blp" => decode_blp(data).map(Some),
        _ => Ok(None),
    }
}

fn ext_of(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.rfind('.') {
        Some(i) => lower[i + 1..].to_string(),
        None => String::new(),
    }
}

fn data_url(mime: &str, data: &[u8]) -> ImageDto {
    ImageDto {
        url: format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(data)
        ),
        mime: mime.to_string(),
        converted: false,
    }
}

/// RGBA 像素编码成 PNG 的 data URL（converted = true）。
fn png_url(img: RgbaImage) -> Result<ImageDto, String> {
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    let mut dto = data_url("image/png", &png);
    dto.converted = true;
    Ok(dto)
}

fn decode_tga(data: &[u8]) -> Result<ImageDto, String> {
    // 尺寸在固定偏移 12..16；先做像素数上限检查再交给 image 解码。
    if data.len() < 18 {
        return Err("TGA 文件头不完整".to_string());
    }
    let w = u16::from_le_bytes([data[12], data[13]]) as u64;
    let h = u16::from_le_bytes([data[14], data[15]]) as u64;
    if w * h > MAX_PIXELS {
        return Err(format!("TGA 尺寸异常（{w}×{h}）"));
    }
    let img = image::load_from_memory_with_format(data, ImageFormat::Tga)
        .map_err(|e| format!("TGA 解码失败：{e}"))?;
    png_url(img.to_rgba8())
}

/// 解码 BLP2（Warcraft III 的贴图格式）。只取 mip 0（最高清一级）：
/// - type 0：mip 数据是标准 JPEG，直接透传给浏览器；
/// - type 1 + 压缩 1：调色板索引 → RGBA；
/// - type 1 + 压缩 2：DXT1 / DXT3 / DXT5 块解码 → RGBA。
fn decode_blp(data: &[u8]) -> Result<ImageDto, String> {
    // 头 0x14 字节 + 两张 mip 表各 0x40 = 0x94；调色板变体在 0x94 处
    // 还有 1024 字节调色板（mip 数据因此从 0x494 起）。
    if data.len() < 0x94 {
        return Err("BLP 头不完整".to_string());
    }
    let u32at = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    match u32at(0) {
        0x3250_4C42 => {} // "BLP2"
        0x3150_4C42 => return Err("BLP1（旧格式）暂不支持预览".to_string()),
        _ => return Err("不是有效的 BLP 文件".to_string()),
    }
    let blp_type = u32at(4);
    let compression = data[8];
    let alpha_depth = data[9];
    let width = u32at(12);
    let height = u32at(16);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("BLP 尺寸异常（{width}×{height}）"));
    }
    let mip_off = u32at(0x14) as usize;
    let mip_len = u32at(0x54) as usize;
    let mip_end = mip_off
        .checked_add(mip_len)
        .filter(|&end| end <= data.len())
        .ok_or_else(|| "BLP mipmap 数据越界".to_string())?;
    let mip = &data[mip_off..mip_end];
    match (blp_type, compression) {
        (0, _) => Ok(data_url("image/jpeg", mip)),
        (1, 1) => blp_palette(data, mip, width, height, alpha_depth),
        (1, 2) => blp_dxt(mip, width, height, alpha_depth),
        _ => Err(format!("未知 BLP 类型 {blp_type}（压缩 {compression}）")),
    }
}

/// 调色板 BLP：mip 0 是每像素 1 字节的调色板索引（无 alpha 且超过
/// 65536 像素时为 2 字节、低字节有效——按数据量自校正识别）；
/// alphaDepth > 0 时索引数据之后还有 1/4/8 位的附加 alpha。
fn blp_palette(
    full: &[u8],
    mip: &[u8],
    width: u32,
    height: u32,
    alpha_depth: u8,
) -> Result<ImageDto, String> {
    if full.len() < 0x94 + 1024 {
        return Err("BLP 头不完整（缺调色板）".to_string());
    }
    let palette = &full[0x94..0x94 + 1024]; // 256 × BGRA
    let n = width as usize * height as usize;
    let idx2 = alpha_depth == 0 && mip.len() >= n * 2;
    let idx_len = if idx2 { n * 2 } else { n };
    if mip.len() < idx_len {
        return Err("BLP 像素数据不完整".to_string());
    }
    let alpha_len = match alpha_depth {
        0 => 0,
        1 => n.div_ceil(8),
        4 => n.div_ceil(2),
        8 => n,
        d => return Err(format!("未知 alphaDepth {d}")),
    };
    // 附加 alpha 缺失时按不透明处理，不让整张图预览失败。
    let has_alpha = mip.len() >= idx_len + alpha_len;
    let mut img = RgbaImage::new(width, height);
    for i in 0..n {
        let idx = (if idx2 { mip[i * 2] } else { mip[i] }) as usize;
        let [b, g, r, _] = palette[idx * 4..idx * 4 + 4].try_into().unwrap();
        let a = if !has_alpha {
            255
        } else {
            match alpha_depth {
                // 4 位 alpha 低半字节在前；1 位每字节 8 像素。
                8 => mip[idx_len + i],
                4 => {
                    let byte = mip[idx_len + i / 2];
                    let nib = if i % 2 == 0 { byte & 0x0F } else { byte >> 4 };
                    nib * 17
                }
                1 => {
                    let byte = mip[idx_len + i / 8];
                    if byte & (1 << (i % 8)) != 0 { 255 } else { 0 }
                }
                _ => 255,
            }
        };
        img.put_pixel(
            (i % width as usize) as u32,
            (i / width as usize) as u32,
            image::Rgba([r, g, b, a]),
        );
    }
    png_url(img)
}

/// DXT 压缩的 BLP：变体按 mip 数据量识别（DXT1 每块 8 字节，
/// DXT3/5 每块 16 字节；同为 16 字节时 alphaDepth 4 → DXT3、其余 → DXT5）。
fn blp_dxt(mip: &[u8], width: u32, height: u32, alpha_depth: u8) -> Result<ImageDto, String> {
    let blocks_x = width.div_ceil(4) as usize;
    let blocks_y = height.div_ceil(4) as usize;
    let blocks = blocks_x * blocks_y;
    let (variant, step) = if mip.len() >= blocks * 16 {
        (if alpha_depth == 4 { 3 } else { 5 }, 16)
    } else if mip.len() >= blocks * 8 {
        (1, 8)
    } else {
        return Err("DXT 数据不完整".to_string());
    };

    let mut img = RgbaImage::new(width, height);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let block = &mip[(bx + by * blocks_x) * step..][..step];
            // DXT1 块内颜色在前、索引在后；DXT3/5 前 8 字节是 alpha。
            let cpos = if variant == 1 { 0 } else { 8 };
            let colors = color_table(&block[cpos..cpos + 4], variant == 1);
            let mut bits = [0u8; 4];
            bits.copy_from_slice(&block[cpos + 4..cpos + 8]);
            let bits = u32::from_le_bytes(bits);
            for i in 0..16 {
                let x = bx * 4 + i % 4;
                let y = by * 4 + i / 4;
                if x >= width as usize || y >= height as usize {
                    continue;
                }
                let mut px = colors[((bits >> (i * 2)) & 3) as usize];
                if variant == 3 {
                    px[3] = dxt3_alpha(block, i);
                } else if variant == 5 {
                    px[3] = dxt5_alpha(block, i);
                }
                img.put_pixel(x as u32, y as u32, image::Rgba(px));
            }
        }
    }
    png_url(img)
}

/// 565 两色展开成 4 色查找表。punchthrough = true（DXT1）且 c0 ≤ c1 的
/// 块为 3 色 + 1 透明；DXT3/5 的颜色块恒为 4 色不透明模式。
fn color_table(bytes: &[u8], punchthrough: bool) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([bytes[0], bytes[1]]);
    let c1 = u16::from_le_bytes([bytes[2], bytes[3]]);
    let rgb = |c: u16| {
        let r = (c >> 11) as u8;
        let g = (c >> 5) as u8 & 0x3F;
        let b = c as u8 & 0x1F;
        [
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        ]
    };
    let [r0, g0, b0] = rgb(c0);
    let [r1, g1, b1] = rgb(c1);
    // u16 运算：2*255 + 255 = 765 超出 u8。
    let mix = |a: u8, b: u8, wa: u16, wb: u16| ((wa * a as u16 + wb * b as u16) / (wa + wb)) as u8;
    if punchthrough && c0 <= c1 {
        [
            [r0, g0, b0, 255],
            [r1, g1, b1, 255],
            [mix(r0, r1, 1, 1), mix(g0, g1, 1, 1), mix(b0, b1, 1, 1), 255],
            [0, 0, 0, 0],
        ]
    } else {
        [
            [r0, g0, b0, 255],
            [r1, g1, b1, 255],
            [mix(r0, r1, 2, 1), mix(g0, g1, 2, 1), mix(b0, b1, 2, 1), 255],
            [mix(r0, r1, 1, 2), mix(g0, g1, 1, 2), mix(b0, b1, 1, 2), 255],
        ]
    }
}

/// DXT3 alpha：16 个 4 位值（低半字节在前）打包在块的前 8 字节。
fn dxt3_alpha(block: &[u8], i: usize) -> u8 {
    let mut bits = 0u64;
    for (k, b) in block[0..8].iter().enumerate() {
        bits |= u64::from(*b) << (8 * k);
    }
    (((bits >> (i * 4)) & 0xF) * 17) as u8
}

/// DXT5 alpha：a0/a1 端点插值出 8 级表，16 个 3 位索引在其后 6 字节。
fn dxt5_alpha(block: &[u8], i: usize) -> u8 {
    let (a0, a1) = (block[0], block[1]);
    let mut table = [a0, a1, 0, 0, 0, 0, 0, 0];
    if a0 > a1 {
        for j in 0..6 {
            table[j + 2] = (((6 - j) * a0 as usize + (1 + j) * a1 as usize) / 7) as u8;
        }
    } else {
        for j in 0..4 {
            table[j + 2] = (((4 - j) * a0 as usize + (1 + j) * a1 as usize) / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let mut bits = 0u64;
    for (k, b) in block[2..8].iter().enumerate() {
        bits |= u64::from(*b) << (8 * k);
    }
    table[((bits >> (i * 3)) & 7) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// converted=true 的 DTO 应是可解码的 PNG，返回其像素。
    fn png_pixels(dto: &ImageDto) -> RgbaImage {
        assert!(dto.converted);
        assert_eq!(dto.mime, "image/png");
        let b64 = dto.url.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgba8()
    }

    #[test]
    fn native_formats_passthrough() {
        let dto = build("war3mapImported\\logo.png", b"\x89PNG fake")
            .unwrap()
            .unwrap();
        assert!(!dto.converted);
        assert_eq!(dto.mime, "image/png");
        assert!(dto.url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn non_image_falls_through() {
        assert!(build("war3map.j", b"function").unwrap().is_none());
        assert!(build("readme", b"text").unwrap().is_none());
    }

    #[test]
    fn tga_decodes_to_png() {
        // 手工构造 2×1 未压缩 32bpp TGA（BGR(A) 字节序）。
        let mut b = vec![0u8; 18];
        b[2] = 2; // 未压缩真彩
        b[12..14].copy_from_slice(&2u16.to_le_bytes());
        b[14..16].copy_from_slice(&1u16.to_le_bytes());
        b[16] = 32; // bpp
        b[17] = 8; // 8 位 alpha
        b.extend_from_slice(&[0, 0, 255, 255]); // 红（BGR）
        b.extend_from_slice(&[0, 255, 0, 128]); // 绿（BGR），半透明
        let dto = decode_tga(&b).unwrap();
        let img = png_pixels(&dto);
        assert_eq!(img.dimensions(), (2, 1));
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([0, 255, 0, 128]));
    }

    /// 生成一个 BLP2 文件：头 + 可选的调色板区 + mip0 载荷。
    /// 调色板变体时 mip_off 传 0x494（0x94 处预留 1024 字节调色板）。
    fn blp_file(
        blp_type: u32,
        compression: u8,
        alpha_depth: u8,
        w: u32,
        h: u32,
        mip_off: usize,
        mip: &[u8],
    ) -> Vec<u8> {
        let mut b = vec![0u8; mip_off];
        b[0..4].copy_from_slice(&0x3250_4C42u32.to_le_bytes()); // "BLP2"
        b[4..8].copy_from_slice(&blp_type.to_le_bytes());
        b[8] = compression;
        b[9] = alpha_depth;
        b[12..16].copy_from_slice(&w.to_le_bytes());
        b[16..20].copy_from_slice(&h.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&(mip_off as u32).to_le_bytes());
        b[0x54..0x58].copy_from_slice(&(mip.len() as u32).to_le_bytes());
        b.extend_from_slice(mip);
        b
    }

    #[test]
    fn blp_jpeg_passthrough() {
        let b = blp_file(0, 0, 0, 4, 4, 0x94, &[0xFF, 0xD8, 0xFF]);
        let dto = decode_blp(&b).unwrap();
        assert_eq!(dto.mime, "image/jpeg");
        assert!(!dto.converted);
        assert!(dto.url.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn blp_palette_decodes() {
        let mut b = blp_file(1, 1, 0, 2, 1, 0x494, &[0, 1]);
        b[0x94..0x98].copy_from_slice(&[0, 0, 255, 255]); // 0 号色：红
        b[0x98..0x9C].copy_from_slice(&[255, 0, 0, 128]); // 1 号色：蓝
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(img.dimensions(), (2, 1));
        // alphaDepth 0 → 不透明（调色板 alpha 不生效）。
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn blp_palette_8bit_alpha() {
        let mut b = blp_file(1, 1, 8, 2, 1, 0x494, &[0, 0, 255, 64]); // 索引 + 每像素 1 字节 alpha
        b[0x94..0x98].copy_from_slice(&[0, 0, 255, 255]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([255, 0, 0, 64]));
    }

    #[test]
    fn blp_palette_large_uses_2byte_indices() {
        // 300×300 = 90000 像素、无 alpha → 2 字节索引（低字节有效）。
        let n = 300 * 300;
        let mut mip = vec![0u8; n * 2];
        mip[2] = 1; // 第 1 像素调色板索引 1（高字节留 0）
        let mut b = blp_file(1, 1, 0, 300, 300, 0x494, &mip);
        b[0x94..0x98].copy_from_slice(&[0, 0, 255, 255]);
        b[0x98..0x9C].copy_from_slice(&[0, 255, 0, 255]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([0, 255, 0, 255]));
    }

    #[test]
    fn blp_dxt1_decodes() {
        // 单块 4×4：c0 = 白、c1 = 黑，全部索引 0 → 白。
        let b = blp_file(1, 2, 0, 4, 4, 0x94, &[0xFF, 0xFF, 0x00, 0x00, 0, 0, 0, 0]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(img.dimensions(), (4, 4));
        for p in img.pixels() {
            assert_eq!(*p, image::Rgba([255, 255, 255, 255]));
        }
    }

    #[test]
    fn blp_dxt1_punchthrough_transparent() {
        // c0 ≤ c1 时索引 3 为透明黑。
        let b = blp_file(
            1,
            2,
            1,
            4,
            4,
            0x94,
            &[0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        );
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([0, 0, 0, 0]));
    }

    #[test]
    fn blp_dxt3_decodes() {
        // 颜色 c0 = 白、索引 0；alpha 低半字节在前：像素 0 = 15、像素 1 = 0。
        let b = blp_file(
            1,
            2,
            4,
            4,
            4,
            0x94,
            &[
                0x0F, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0x00, 0x00, 0, 0, 0, 0,
            ],
        );
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 255, 255, 255])); // 15*17
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([255, 255, 255, 0]));
    }

    #[test]
    fn blp_dxt5_decodes() {
        // a0=255、a1=0；alpha 索引偶数像素 0（255）、奇数 1（0）。
        let mut bits = 0u64;
        for i in 0..16 {
            bits |= (i as u64 & 1) << (3 * i);
        }
        let mut mip = vec![255u8, 0u8];
        mip.extend_from_slice(&bits.to_le_bytes()[..6]);
        mip.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0, 0, 0, 0]); // 全白，索引 0
        let b = blp_file(1, 2, 8, 4, 4, 0x94, &mip);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 255, 255, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([255, 255, 255, 0]));
    }

    #[test]
    fn bad_blp_rejected() {
        assert!(decode_blp(b"not a blp at all........").is_err());
        assert!(decode_blp(&vec![0u8; 0x494]).is_err()); // 魔数不对
        // mip 越界
        let mut b = blp_file(1, 1, 0, 2, 1, 0x494, &[0, 1]);
        b[0x14..0x18].copy_from_slice(&0xFFFF_u32.to_le_bytes());
        assert!(decode_blp(&b).is_err());
    }
}
