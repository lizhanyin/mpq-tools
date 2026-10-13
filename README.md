# mpq — Mo'PaQ (MPQ) v3 协议库与工具

Rust 实现的 MPQ 归档读取库（`mpq`）与命令行工具（`mpqtool`），按照
标准 **MPQ v3 协议**实现，同时兼容 v1/v2 头部（v4 头部可解析并经由
经典表打开）。格式细节以 StormLib 源码为准逐一验证（见
`docs/mpq-format-notes.md`）。

## 特性

- **头部**：v1 (0x20) / v2 (0x2C) / v3 (0x44) / v4 (0xD0) 解析；
  `MPQ\x1B` 用户数据头（War3/SC2 地图前置）；嵌入归档 0x200 对齐扫描
- **经典表**：hash / block / hi-block 表（storm 加密，可压缩存储）
- **v3 扩展表**：HET + BET（Jenkins lookup3 名字哈希、位打包记录、
  表体加密，支持压缩存储）
- **文件读取**：扇区布局与单单元、zlib / PKWARE implode / bzip2 /
  LZMA 解压、storm 加密（含 key-v2 `(key+offset)^size`）、扇区 CRC
  （校验失败仅告警）
- **`(listfile)` / `(attributes)`**：完全可选，缺失不影响打开
- **容错打开**（非必要数据允许不存在）：
  - 缺 `(listfile)` → 仍可列出条目（名字未知），未加密文件可读
  - 缺 `(attributes)`、`(signature)`、HET/BET、hi-block 表 → 正常打开
  - 归档大小字段错误、表读取越界（补零）→ 仅告警
  - 只要 hash 表或 HET 之一可用 + block 表或 BET 之一可用即可打开

## 用法（库）

```rust
use mpq::Archive;

let archive = Archive::open("map.w3x")?;
if archive.has_file("war3map.j") {
    let data = archive.read_file("war3map.j")?;
}
for entry in archive.entries() {
    println!("{} {} bytes", entry.display_name(), entry.size);
}
for w in archive.warnings() {
    eprintln!("warning: {w}");
}
```

## 用法（工具）

直接运行 `mpqtool`（不带参数）即启动 **GUI**；子命令用于命令行模式：

```
mpqtool                              # 默认启动 GUI（等价于 mpqtool gui）
mpqtool info   <archive>              # 头部与表概要
mpqtool list   [-v] <archive>         # 列出文件（-v 显示标志/大小/来源）
mpqtool cat    <archive> <name>       # 输出单个文件内容
mpqtool extract [options] <archive> [files...]
        -o, --output <DIR>            # 输出目录（默认为归档名去扩展名）
            --all-entries             # 连未命名条目一起解出
```

示例：

```console
$ mpqtool info patch.mpq
Archive:        patch.mpq
Format:         MPQ v3 (Cataclysm, HET/BET capable)
Tables:         hash=1024 block=517 + HET + BET
...

$ mpqtool extract map.w3x war3map.j -o out/
out/war3map.j (48213)
1 extracted, 0 failed
```

## 头信息修复

针对下载截断、地图保护工具破坏或劣质写出器造成的**容器元数据损坏**，
`mpqtool repair`（或 GUI 的「工具 → 修复头信息」）可诊断并重写头部字段：

- 非法 `format_version` / `header_size` / 扇区指数
- 错误的 `archive_size` / `archive_size64`
- 哈希表 / 块表位置越界或指错（通过**解密内容扫描**重新定位，等价于
  StormLib 的畸形归档恢复路径）
- 保护工具虚增的表项数（哈希表截断后按 2 的幂对齐）
- v2/v3 的 hi-block / HET / BET 位置越界清零

修复只改写头部字段，绝不触碰文件数据或表内容；无法自动修复的问题会
明确列出。库接口：`mpq::diagnose(&[u8])`（只读诊断）与
`mpq::repair(&mut [u8], deep)`（就地修复）。

```console
$ mpqtool repair --dry-run corrupt.w3x     # 仅诊断
[可修复] hash_table_pos: 哈希表位置 0x7f000000 越界 → 扫描定位到 0x4c
[可修复] archive_size: 归档大小 0x1000 与实际 0xac 不符 → 改写为 0xac

$ mpqtool repair corrupt.w3x               # 写出 corrupt.fixed.w3x
```

## GUI

`mpqtool`（无参数，或显式 `mpqtool gui`）启动图形界面（egui，自动
加载系统中文字体）：

- 打开 / 浏览归档，状态栏显示版本、表、警告数
- 文件列表：勾选、过滤、双击预览文本内容
- 提取选中 / 提取全部（目录选择对话框，路径防逃逸）
- 「工具 → 修复头信息」：问题列表 → 应用修复 → 保存修复副本
  或直接在界面中重新打开修复结果

## 正确性验证

- 哈希/加密：命中 StormLib 内建常数（`(hash table)` → `0xC3AF3770` 等）
- Jenkins 哈希：与编译后的原版 lookup3.c 输出比对（7 组参考向量）
- PKWARE explode：与编译后的原版 explode.c 逐字节一致；测试向量由
  Blizzard 实际使用的 implode 压缩器生成（`tests/fixtures/pk_*.bin`）
- 集成测试：测试内构建器从零合成 v1/v2/v3 归档（加密、压缩、单单元、
  key-v2、扇区 CRC、listfile、attributes、userdata 包装、嵌入扫描、
  纯 HET/BET 表），再经公开 API 读回校验
- 修复测试：11 类损坏场景（表位置越界重定位、表项数截断、版本推断、
  头长度规整、v3 字段清零等）修复后归档可正常读取

```console
$ cargo test        # 50 项测试
```

## 目录结构

```
mpq/            读取库（edition 2024, no unsafe）
  src/crypt.rs        storm 加密、HashString、文件密钥
  src/jenkins.rs      lookup3 hashlittle2（HET/BET 名字哈希）
  src/header.rs       v1–v4 头部与 MPQ\x1B 用户数据头
  src/tables.rs       经典 hash/block/hi-block 表
  src/hetbet.rs       HET/BET 扩展表
  src/compression.rs  zlib/pkware/bzip2/lzma 解压调度
  src/pkware.rs       PKWARE DCL explode 移植
  src/repair.rs       头部诊断/修复 + 表位置深度扫描
  src/archive.rs      Archive：打开/查找/读取 + 容错策略
mpqtool/        CLI + GUI
  src/main.rs         命令行入口（无参数默认启动 GUI）
  src/gui.rs          egui 图形界面
  src/tasks.rs        CLI/GUI 共享的提取等逻辑
docs/           格式依据备忘（源自 StormLib 源码验证）
```

## 构建说明（本机）

仓库内的 `.cargo/config.toml` 固定了 MSVC 链接器路径与 `LIB`，
用于绕开 Git Bash 下 `/usr/bin/link` 遮蔽 `link.exe` 的问题；
在其他机器上如无此问题可删除该文件。

## 依赖

- 库：`flate2`（zlib，miniz_oxide 后端）、`bzip2-rs`、`lzma-rs`
  （全部纯 Rust），`no unsafe`
- 工具：`clap`（CLI）、`eframe/egui`（GUI）、`rfd`（文件对话框）
