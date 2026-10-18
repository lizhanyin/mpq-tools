//! 归档内图片条目的预览：浏览器 `<img>` 原生支持的格式以 base64
//! data URL 原样传给前端；TGA 与 Warcraft III 的 BLP（BLP1 / BLP2）
//! 在此解码后转成 PNG。图片不走 256 KiB 文本截断，也不可编辑。

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

// -- 网格视图缩略图 --------------------------------------------------------------

/// 缩略图边长上限：网格单元图标区约 80px，按 2x 高分屏余量取 192。
const THUMB_MAX: u32 = 192;
/// 原生光栅格式解码的尺寸/内存上限（防止损坏的头声明超大尺寸）。
const MAX_DIM: u32 = 8192;
const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// 网格视图缩略图：解码（原生光栅 / TGA / BLP，含 BLP 内嵌 JPEG）→
/// 等比缩放到 THUMB_MAX 内 → PNG data URL。svg 矢量原样透传由浏览器
/// 缩放；非图片返回 None；解码失败不报错（前端回退显示图标）。
pub fn thumbnail(name: &str, data: &[u8]) -> Result<Option<String>, String> {
    let ext = ext_of(name);
    let pixels = match ext.as_str() {
        "svg" => {
            // 矢量交给浏览器缩放；过大（或超 16 MiB）就不出缩略图。
            return Ok(if data.len() <= NATIVE_LIMIT {
                Some(data_url_str("image/svg+xml", data))
            } else {
                None
            });
        }
        "tga" => tga_pixels(data),
        "blp" => blp_pixels(data),
        "png" | "jpg" | "jpeg" | "jfif" | "gif" | "bmp" | "dib" | "webp" | "ico" => {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
                .with_guessed_format()
                .map_err(|e| format!("识别图片格式失败：{e}"))?;
            reader.limits(native_limits());
            reader
                .decode()
                .map(|img| img.to_rgba8())
                .map_err(|e| format!("图片解码失败：{e}"))
        }
        _ => return Ok(None),
    };
    // 单个条目解码失败只是不出缩略图，不影响网格其它内容。
    let img = match pixels {
        Ok(img) => img,
        Err(_) => return Ok(None),
    };
    // 已小于上限的不再放大（thumbnail 会把小图拉大到目标尺寸）。
    let dyn_img = DynamicImage::ImageRgba8(img);
    let thumb = if dyn_img.width() <= THUMB_MAX && dyn_img.height() <= THUMB_MAX {
        dyn_img
    } else {
        dyn_img.thumbnail(THUMB_MAX, THUMB_MAX)
    };
    let mut png = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    Ok(Some(data_url_str("image/png", &png)))
}

/// 按固定格式解码（BLP 内嵌 JPEG 用），带尺寸/内存上限。
fn decode_with_limits(data: &[u8], format: ImageFormat) -> Result<RgbaImage, String> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(data), format);
    reader.limits(native_limits());
    reader
        .decode()
        .map(|img| img.to_rgba8())
        .map_err(|e| format!("图片解码失败：{e}"))
}

fn native_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIM);
    limits.max_image_height = Some(MAX_DIM);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
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
        url: data_url_str(mime, data),
        mime: mime.to_string(),
        converted: false,
    }
}

fn data_url_str(mime: &str, data: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    )
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

/// 解码 TGA 为 RGBA 像素（尺寸头先做上限检查再交给 image）。
fn tga_pixels(data: &[u8]) -> Result<RgbaImage, String> {
    if data.len() < 18 {
        return Err("TGA 文件头不完整".to_string());
    }
    let w = u16::from_le_bytes([data[12], data[13]]) as u64;
    let h = u16::from_le_bytes([data[14], data[15]]) as u64;
    if w * h > MAX_PIXELS {
        return Err(format!("TGA 尺寸异常（{w}×{h}）"));
    }
    image::load_from_memory_with_format(data, ImageFormat::Tga)
        .map_err(|e| format!("TGA 解码失败：{e}"))
        .map(|img| img.to_rgba8())
}

fn decode_tga(data: &[u8]) -> Result<ImageDto, String> {
    tga_pixels(data).and_then(png_url)
}

/// 解析后的 BLP 内容（BLP1 / BLP2 共用；预览与缩略图共用）。
enum BlpImage<'a> {
    /// BLP2 type 0：mip 0 是标准 JPEG，直接透传给浏览器。
    Jpeg(&'a [u8]),
    /// BLP1 JPEG：共享头 ++ mip 0 拼成的完整 JPEG 流（4 分量 B,G,R,A）；
    /// alpha8 = 头部 alphaBits 是否为 8（决定第 4 平面是否按 alpha 采用）。
    Blp1Jpeg {
        jpeg: Vec<u8>,
        alpha8: bool,
    },
    /// 调色板（BLP2 压缩 1 / BLP1 content 1）：palette 为 256×BGRA，
    /// mip 为每像素索引 + 可选附加 alpha 平面。
    Palette {
        palette: &'a [u8],
        mip: &'a [u8],
        width: u32,
        height: u32,
        alpha_depth: u8,
    },
    /// DXT（BLP2 压缩 2）。
    Dxt {
        mip: &'a [u8],
        width: u32,
        height: u32,
        alpha_depth: u8,
    },
}

fn u32at(data: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
}

/// 按头里的 mip 偏移/长度切出 mip 0 载荷（防越界）。
fn mip_slice(data: &[u8], off: usize, len: usize) -> Result<&[u8], String> {
    off.checked_add(len)
        .filter(|&end| end <= data.len())
        .map(|end| &data[off..end])
        .ok_or_else(|| "BLP mipmap 数据越界".to_string())
}

/// 识别 BLP 版本并解析（预览与缩略图的共用入口）。
fn blp_analyze(data: &[u8]) -> Result<BlpImage<'_>, String> {
    if data.len() < 0x94 {
        return Err("BLP 头不完整".to_string());
    }
    match u32at(data, 0) {
        0x3250_4C42 => blp2_analyze(data), // "BLP2"
        0x3150_4C42 => blp1_analyze(data), // "BLP1"
        _ => Err("不是有效的 BLP 文件".to_string()),
    }
}

/// 校验 BLP2 头并取出 mip 0 载荷。
/// 头 0x14 字节 + 两张 mip 表各 0x40 = 0x94；调色板变体在 0x94 处
/// 还有 1024 字节调色板（mip 数据因此从 0x494 起）。
fn blp2_analyze(data: &[u8]) -> Result<BlpImage<'_>, String> {
    let blp_type = u32at(data, 4);
    let compression = data[8];
    let alpha_depth = data[9];
    let width = u32at(data, 12);
    let height = u32at(data, 16);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("BLP 尺寸异常（{width}×{height}）"));
    }
    let mip = mip_slice(data, u32at(data, 0x14) as usize, u32at(data, 0x54) as usize)?;
    match (blp_type, compression) {
        (0, _) => Ok(BlpImage::Jpeg(mip)),
        (1, 1) => {
            if data.len() < 0x94 + 1024 {
                return Err("BLP 头不完整（缺调色板）".to_string());
            }
            Ok(BlpImage::Palette {
                palette: &data[0x94..0x94 + 1024],
                mip,
                width,
                height,
                alpha_depth,
            })
        }
        (1, 2) => Ok(BlpImage::Dxt {
            mip,
            width,
            height,
            alpha_depth,
        }),
        _ => Err(format!("未知 BLP 类型 {}（压缩 {}）", blp_type, compression)),
    }
}

/// 校验 BLP1 头（Warcraft III 早期 / WoW alpha 时代的旧格式）并解析内容。
/// 头 0x9C 字节：content（0=JPEG、1=调色板）、alphaBits、宽、高、extra、
/// hasMipmaps 各 u32，随后 16 个 mip 偏移 + 16 个 mip 大小（mip 0 即全图）。
fn blp1_analyze(data: &[u8]) -> Result<BlpImage<'_>, String> {
    if data.len() < 0x9C {
        return Err("BLP 头不完整".to_string());
    }
    let content = u32at(data, 4);
    let alpha_bits = u32at(data, 8);
    let width = u32at(data, 0x0C);
    let height = u32at(data, 0x10);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("BLP 尺寸异常（{width}×{height}）"));
    }
    let mip_off = u32at(data, 0x1C) as usize;
    let mip = mip_slice(data, mip_off, u32at(data, 0x5C) as usize)?;
    match content {
        0 => {
            // 0x9C 处是 u32 共享 JPEG 头长度；完整 JPEG = 共享头 ++ mip 0。
            let hdr_len = u32at(data, 0x9C) as usize;
            let hdr = 0xA0usize
                .checked_add(hdr_len)
                .filter(|&end| end <= data.len())
                .map(|end| &data[0xA0..end])
                .ok_or_else(|| "BLP1 JPEG 共享头越界".to_string())?;
            let mut jpeg = Vec::with_capacity(hdr.len() + mip.len());
            jpeg.extend_from_slice(hdr);
            jpeg.extend_from_slice(mip);
            Ok(BlpImage::Blp1Jpeg {
                jpeg,
                alpha8: alpha_bits == 8,
            })
        }
        1 => {
            // 0x9C 起 1024 字节调色板。损坏文件可能把 mip 数据压进调色板
            // 区（短调色板），越界索引按黑色处理；alphaBits 非法值按引擎
            // 行为视作 0。
            let palette = &data[0x9C..(0x9C + 1024).min(mip_off.max(0x9C))];
            Ok(BlpImage::Palette {
                palette,
                mip,
                width,
                height,
                alpha_depth: match alpha_bits {
                    1 | 4 | 8 => alpha_bits as u8,
                    _ => 0,
                },
            })
        }
        c => Err(format!("未知 BLP1 内容类型 {c}")),
    }
}

/// 解码 BLP（Warcraft III 的贴图格式）。只取 mip 0（最高清一级）：
/// BLP1 的 JPEG / 调色板与 BLP2 的调色板 / DXT 在此解码转 PNG；
/// BLP2 type 0 的标准 JPEG 直接透传给浏览器。
fn decode_blp(data: &[u8]) -> Result<ImageDto, String> {
    match blp_analyze(data)? {
        BlpImage::Jpeg(mip) => Ok(data_url("image/jpeg", mip)),
        BlpImage::Blp1Jpeg { jpeg, alpha8 } => blp1_jpeg_pixels(&jpeg, alpha8).and_then(png_url),
        BlpImage::Palette {
            palette,
            mip,
            width,
            height,
            alpha_depth,
        } => blp_palette(palette, mip, width, height, alpha_depth).and_then(png_url),
        BlpImage::Dxt {
            mip,
            width,
            height,
            alpha_depth,
        } => blp_dxt(mip, width, height, alpha_depth).and_then(png_url),
    }
}

/// BLP 解码为 RGBA 像素（缩略图用）：透传的 JPEG 也在此解码以便缩放。
fn blp_pixels(data: &[u8]) -> Result<RgbaImage, String> {
    match blp_analyze(data)? {
        BlpImage::Jpeg(mip) => decode_with_limits(mip, ImageFormat::Jpeg),
        BlpImage::Blp1Jpeg { jpeg, alpha8 } => blp1_jpeg_pixels(&jpeg, alpha8),
        BlpImage::Palette {
            palette,
            mip,
            width,
            height,
            alpha_depth,
        } => blp_palette(palette, mip, width, height, alpha_depth),
        BlpImage::Dxt {
            mip,
            width,
            height,
            alpha_depth,
        } => blp_dxt(mip, width, height, alpha_depth),
    }
}

/// BLP1 内嵌 JPEG 是 4 分量（B、G、R、A 平面直存、无色彩变换）的非标
/// 流（Intel Imaging Framework 产物）——常规解码器会按 Adobe 惯例误当
/// CMYK 做墨色/反相处理而严重错色。这里把输出色彩空间钉在 CMYK（与
/// 输入一致即不做任何转换），拿到交错的原始样本后重排成 RGBA；极少数
/// 3 分量/灰度流按标准 JPEG 解码。
fn blp1_jpeg_pixels(jpeg: &[u8], alpha8: bool) -> Result<RgbaImage, String> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::CMYK);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(jpeg), opts);
    dec.decode_headers()
        .map_err(|e| format!("BLP1 JPEG 解码失败：{e}"))?;
    let info = dec.info().ok_or_else(|| "BLP1 JPEG 头缺失".to_string())?;
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
        return Err(format!(
            "BLP1 JPEG 尺寸异常（{}×{}）",
            info.width, info.height
        ));
    }
    if dec.input_colorspace() != Some(ColorSpace::CMYK) {
        return decode_with_limits(jpeg, ImageFormat::Jpeg);
    }
    let raw = dec
        .decode()
        .map_err(|e| format!("BLP1 JPEG 解码失败：{e}"))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let n = w as usize * h as usize;
    if raw.len() < n * 4 {
        return Err("BLP1 JPEG 像素数据不完整".to_string());
    }
    // 交错 4 字节/像素，存储顺序 B、G、R、A。
    let px = |i: usize| [raw[i * 4 + 2], raw[i * 4 + 1], raw[i * 4], raw[i * 4 + 3]];
    // 第 4 平面是否携带信息：全 0 或全 255 都视为不透明（作者工具两种
    // 填法都出现过），且头部 alphaBits 为 8 时才采用。
    let a0 = raw[3];
    let use_alpha = alpha8
        && (raw[3..n * 4]
            .iter()
            .skip(4)
            .step_by(4)
            .any(|&a| a != a0)
            || a0 != 0 && a0 != 255);
    let mut img = RgbaImage::new(w, h);
    for i in 0..n {
        let mut p = px(i);
        if !use_alpha {
            p[3] = 255;
        }
        img.put_pixel((i % w as usize) as u32, (i / w as usize) as u32, image::Rgba(p));
    }
    Ok(img)
}

/// 调色板 BLP（BLP2 压缩 1 / BLP1 content 1）：mip 0 是每像素 1 字节的
/// 调色板索引（无 alpha 且数据量翻倍时为 2 字节、低字节有效——按数据量
/// 自校正识别）；alphaDepth > 0 时索引数据之后还有 1/4/8 位的附加 alpha
/// 平面。调色板条目为 B,G,R,pad（pad 非 alpha，两种版本一致）。
fn blp_palette(
    palette: &[u8],
    mip: &[u8],
    width: u32,
    height: u32,
    alpha_depth: u8,
) -> Result<RgbaImage, String> {
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
        // 短调色板（BLP1 损坏文件）中越界索引按黑色不透明处理。
        let [b, g, r] = palette
            .get(idx * 4..idx * 4 + 4)
            .map(|e| [e[0], e[1], e[2]])
            .unwrap_or([0, 0, 0]);
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
    Ok(img)
}

/// DXT 压缩的 BLP：变体按 mip 数据量识别（DXT1 每块 8 字节，
/// DXT3/5 每块 16 字节；同为 16 字节时 alphaDepth 4 → DXT3、其余 → DXT5）。
fn blp_dxt(mip: &[u8], width: u32, height: u32, alpha_depth: u8) -> Result<RgbaImage, String> {
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
    Ok(img)
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

    // -- BLP1 -------------------------------------------------------------------

    /// 生成一个 BLP1 文件：0x9C 处放 extra_header（调色板，或 JPEG
    /// 共享头长度 + 共享头），mip_off 起放 mip 0 载荷。
    fn blp1_file(
        content: u32,
        alpha_bits: u32,
        w: u32,
        h: u32,
        mip_off: usize,
        extra_header: &[u8],
        mip: &[u8],
    ) -> Vec<u8> {
        let mut b = vec![0u8; mip_off];
        b[0..4].copy_from_slice(&0x3150_4C42u32.to_le_bytes()); // "BLP1"
        b[4..8].copy_from_slice(&content.to_le_bytes());
        b[8..12].copy_from_slice(&alpha_bits.to_le_bytes());
        b[12..16].copy_from_slice(&w.to_le_bytes());
        b[16..20].copy_from_slice(&h.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&5u32.to_le_bytes()); // extra：WorldEdit 写 5
        b[0x1C..0x20].copy_from_slice(&(mip_off as u32).to_le_bytes());
        b[0x5C..0x60].copy_from_slice(&(mip.len() as u32).to_le_bytes());
        b[0x9C..0x9C + extra_header.len()].copy_from_slice(extra_header);
        b.extend_from_slice(mip);
        b
    }

    /// 1024 字节调色板，0 号色红、1 号色蓝（B,G,R,pad 字节序）。
    fn blp1_palette_header() -> Vec<u8> {
        let mut p = vec![0u8; 1024];
        p[0..4].copy_from_slice(&[0, 0, 255, 255]);
        p[4..8].copy_from_slice(&[255, 0, 0, 255]);
        p
    }

    #[test]
    fn blp1_palette_decodes() {
        let b = blp1_file(1, 0, 2, 1, 0x9C + 1024, &blp1_palette_header(), &[0, 1]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(img.dimensions(), (2, 1));
        // alphaBits 0 → 不透明（调色板 pad 字节不生效）。
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn blp1_palette_8bit_alpha() {
        // 索引 [0,0] + 附加 alpha 平面 [255, 64]。
        let b = blp1_file(1, 8, 2, 1, 0x9C + 1024, &blp1_palette_header(), &[0, 0, 255, 64]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([255, 0, 0, 64]));
    }

    #[test]
    fn blp1_palette_packed_alpha() {
        // 4 位 alpha 低半字节在前：0x5A → 像素 0 = 10、像素 1 = 5（×17）。
        let b = blp1_file(1, 4, 2, 1, 0x9C + 1024, &blp1_palette_header(), &[0, 0, 0x5A]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 170]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([255, 0, 0, 85]));
        // 1 位 alpha 每字节 8 像素：0b10 → 像素 0 透明、像素 1 不透明。
        let b = blp1_file(1, 1, 2, 1, 0x9C + 1024, &blp1_palette_header(), &[0, 0, 0b10]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 0]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn blp1_palette_short_palette_tolerated() {
        // 损坏文件：调色板被 mip 数据截短只剩 1 个条目；越界索引按黑色不透明。
        let b = blp1_file(1, 0, 2, 1, 0xA0, &blp1_palette_header()[..4], &[0, 1]);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*img.get_pixel(1, 0), image::Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn blp1_jpeg_real_sample() {
        // Pillow 测试套件里的真实 BLP1 JPEG 文件（WoW alpha 遗物）。
        // 基准取自 Pillow 的参考渲染 blp1_jpeg.png：中心像素偏蓝
        // (22,47,93)——通道顺序接反会变成 (93,47,22)，是强判别点。
        let data = include_bytes!("../tests/fixtures/blp1_jpeg.blp");
        let img = png_pixels(&decode_blp(data).unwrap());
        assert_eq!(img.dimensions(), (256, 256));
        let near = |got: image::Rgba<u8>, want: [u8; 3], tol: i16| {
            for i in 0..3 {
                assert!(
                    (i16::from(got[i]) - i16::from(want[i])).abs() <= tol,
                    "got {got:?} want {want:?}"
                );
            }
        };
        near(*img.get_pixel(128, 128), [22, 47, 93], 16);
        near(*img.get_pixel(200, 50), [26, 53, 54], 16);
        // 全图均值（参考 (22.3, 58.9, 23.5)）：接错成 CMYK 反相时严重偏离。
        let (mut r, mut g, mut bl) = (0u64, 0u64, 0u64);
        for p in img.pixels() {
            r += u64::from(p[0]);
            g += u64::from(p[1]);
            bl += u64::from(p[2]);
        }
        let n = 256 * 256;
        for (mean, want) in [(r / n, 22), (g / n, 59), (bl / n, 23)] {
            assert!((mean as i64 - i64::from(want)).abs() <= 6, "mean {mean} want {want}");
        }
    }

    #[test]
    fn blp1_jpeg_standard_fallback() {
        // 极少数工具写入的标准 3 分量 JPEG：按共享头 ++ mip 重组后常规解码。
        let jpeg = encoded(8, 8, ImageFormat::Jpeg);
        let (hdr, rest) = jpeg.split_at(jpeg.len() / 2);
        let mut extra = (hdr.len() as u32).to_le_bytes().to_vec();
        extra.extend_from_slice(hdr);
        let b = blp1_file(0, 0, 8, 8, 0x9C + extra.len(), &extra, rest);
        let img = png_pixels(&decode_blp(&b).unwrap());
        assert_eq!(img.dimensions(), (8, 8));
        // JPEG 有损，纯色块取中心像素放宽比较。
        let got = img.get_pixel(4, 4);
        for (g, w) in [(got[0], 200i16), (got[1], 60), (got[2], 60)] {
            assert!((i16::from(g) - w).abs() <= 16, "got {got:?}");
        }
    }

    #[test]
    fn blp1_bad_rejected() {
        // 头截断
        let mut b = blp1_file(1, 0, 2, 1, 0x9C + 1024, &blp1_palette_header(), &[0, 1]);
        b.truncate(0x50);
        assert!(decode_blp(&b).is_err());
        // 未知内容类型
        let b = blp1_file(2, 0, 2, 1, 0x9C, &[], &[0, 1]);
        assert!(decode_blp(&b).is_err());
        // mip 越界
        let mut b = blp1_file(1, 0, 2, 1, 0x9C + 1024, &blp1_palette_header(), &[0, 1]);
        b[0x1C..0x20].copy_from_slice(&0xFFFF_u32.to_le_bytes());
        assert!(decode_blp(&b).is_err());
        // JPEG 共享头越界
        let mut extra = 0x00FF_FFFFu32.to_le_bytes().to_vec();
        extra.extend_from_slice(&[0xFF, 0xD8, 0xFF]);
        let b = blp1_file(0, 0, 2, 1, 0x9C + extra.len(), &extra, &[0]);
        assert!(decode_blp(&b).is_err());
    }

    #[test]
    fn thumbnail_blp1_jpeg_is_resized() {
        let data = include_bytes!("../tests/fixtures/blp1_jpeg.blp");
        let url = thumbnail("old.blp", data).unwrap().unwrap();
        assert_eq!(url_png_pixels(&url).dimensions(), (192, 192));
    }

    // -- 缩略图 -----------------------------------------------------------------

    fn url_png_pixels(url: &str) -> RgbaImage {
        let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgba8()
    }

    /// 编码一张纯色图（无 alpha 通道的 RGB，JPEG 不支持透明）。
    fn encoded(w: u32, h: u32, format: ImageFormat) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 60, 60]));
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), format)
            .unwrap();
        out
    }

    #[test]
    fn thumbnail_non_image_none() {
        assert!(thumbnail("war3map.j", b"function").unwrap().is_none());
        assert!(thumbnail("readme", b"text").unwrap().is_none());
    }

    #[test]
    fn thumbnail_png_is_resized() {
        // 400×200 → 等比缩到 192×96。
        let url = thumbnail("logo.png", &encoded(400, 200, ImageFormat::Png))
            .unwrap()
            .unwrap();
        assert_eq!(url_png_pixels(&url).dimensions(), (192, 96));
    }

    #[test]
    fn thumbnail_tga_decodes() {
        // 复用 tga_decodes_to_png 的 2×1 构造（小于上限，不缩放）。
        let mut b = vec![0u8; 18];
        b[2] = 2;
        b[12..14].copy_from_slice(&2u16.to_le_bytes());
        b[14..16].copy_from_slice(&1u16.to_le_bytes());
        b[16] = 32;
        b[17] = 8;
        b.extend_from_slice(&[0, 0, 255, 255]);
        b.extend_from_slice(&[0, 255, 0, 128]);
        let url = thumbnail("x.tga", &b).unwrap().unwrap();
        assert_eq!(url_png_pixels(&url).dimensions(), (2, 1));
    }

    #[test]
    fn thumbnail_blp_jpeg_is_decoded_and_resized() {
        // 预览走 JPEG 透传；缩略图必须真正解码才能缩放：300×300 → 192×192。
        let jpeg = encoded(300, 300, ImageFormat::Jpeg);
        let b = blp_file(0, 0, 0, 300, 300, 0x94, &jpeg);
        let url = thumbnail("tex.blp", &b).unwrap().unwrap();
        assert_eq!(url_png_pixels(&url).dimensions(), (192, 192));
    }

    #[test]
    fn thumbnail_svg_passthrough() {
        let url = thumbnail("icon.svg", b"<svg/>").unwrap().unwrap();
        assert!(url.starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn thumbnail_corrupt_data_falls_back_to_none() {
        // 损坏数据不报错，仅不出缩略图（前端回退图标）。
        assert!(thumbnail("broken.png", b"\x89PNG garbage").unwrap().is_none());
        assert!(thumbnail("broken.tga", b"xx").unwrap().is_none());
    }
}
