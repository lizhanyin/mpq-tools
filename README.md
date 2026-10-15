# mpq — Mo'PaQ (MPQ) v3/v4 协议库与工具

Rust 实现的 MPQ 归档读取/修改库（`mpq`）与命令行工具（`mpqtool`），按照
标准 **MPQ v3 协议**实现并**完整支持 v4**（头部全字段解析、六处 MD5
校验、v4 容器写回），兼容 v1/v2 头部。格式细节以 StormLib 源码为准
逐一验证（见 `docs/mpq-format-notes.md`）。

## 特性

- **头部**：v1 (0x20) / v2 (0x2C) / v3 (0x44) / v4 (0xD0) 全字段解析；
  `MPQ\x1B` 用户数据头（War3/SC2 地图前置）；嵌入归档 0x200 对齐扫描
- **经典表**：hash / block / hi-block 表（storm 加密，可压缩存储）
- **v3 扩展表**：HET + BET（Jenkins lookup3 名字哈希、位打包记录、
  表体加密，支持压缩存储）
- **v4 校验**：hash / block / hi-block / HET / BET 表 MD5（按**存储态**
  字节）+ 头部自身 MD5（前 0xC0 字节）+ 每文件 raw chunk MD5
  （`raw_chunk_size` 非零时，数组紧跟存储数据、不计入 csize）；
  校验失败一律**仅告警**，不阻断打开（StormLib 对头部 MD5 是硬失败）
- **文件读取**：扇区布局与单单元、zlib / PKWARE implode / bzip2 /
  LZMA 解压、storm 加密（含 key-v2 `(key+offset)^size`）、扇区 CRC
  （校验失败仅告警）
- **`(listfile)` / `(attributes)`**：完全可选，缺失不影响打开
- **容错打开**（非必要数据允许不存在）：
  - 缺 `(listfile)` → 仍可列出条目（名字未知），未加密文件可读
  - 缺 `(attributes)`、`(signature)`、HET/BET、hi-block 表 → 正常打开
  - 归档大小字段错误、表读取越界（补零）→ 仅告警
  - 只要 hash 表或 HET 之一可用 + block 表或 BET 之一可用即可打开
- **修改归档**（导入/替换与删除，见下节）：保留文件块原样拷贝，
  重建容器（hash/block 表、`(listfile)`、头部），自带结果校验
- **密码保护**（自定义 v4 扩展，见「密码保护」节）：每扇区
  ChaCha20-Poly1305 认证加密，Argon2id 包装的每归档主密钥存放于
  `(password)` 内部文件；支持换密码（不重写数据）与解除保护

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

命令行工具 `mpqtool`；图形界面是独立的 `mpqtool-gui`（见下节）：

```
mpqtool info   <archive>              # 头部与表概要
mpqtool list   [-v] <archive>         # 列出文件（-v 显示标志/大小/来源）
mpqtool cat    <archive> <name>       # 输出单个文件内容
mpqtool extract [options] <archive> [files...]
        -o, --output <DIR>            # 输出目录（默认为归档名去扩展名）
            --all-entries             # 连未命名条目一起解出
mpqtool add    <archive> <files...>   # 导入文件（同名替换）
        -o, --output <FILE>           # 写到别处（默认就地修改）
            --no-reencrypt-key2       # key-v2 文件存明文（默认重新加密）
mpqtool delete <archive> <names...>   # 按名删除文件
        -o, --output <FILE>           # 写到别处（默认就地修改）
mpqtool encrypt <archive> -p <密码>   # 启用密码保护（自定义 v4 扩展）
        -o, --output <FILE>           # 写到别处（默认就地修改）
mpqtool decrypt <archive> -p <密码>   # 解除密码保护
mpqtool passwd  <archive> <旧密码> <新密码>
                                      # 更换密码（不重写数据）
mpqtool repair [options] <archive>    # 诊断 / 修复头部元数据

受密码保护的归档：info / list / extract / cat / add / delete 均可加
`-p/--password`；未提供时 info / list 仅提示（文件名可见），读取与
修改类命令给出明确错误。
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
- v4 头部字段修复后**按最终头部重算 `MD5_MpqHeader`**（头部 MD5 覆盖
  前 0xC0 字节，任何字段改动都会使其失效，且 StormLib 对失配是硬拒绝）

修复只改写头部字段，绝不触碰文件数据或表内容；无法自动修复的问题会
明确列出。库接口：`mpq::diagnose(&[u8])`（只读诊断）与
`mpq::repair(&mut [u8], deep)`（就地修复）。

```console
$ mpqtool repair --dry-run corrupt.w3x     # 仅诊断
[可修复] hash_table_pos: 哈希表位置 0x7f000000 越界 → 扫描定位到 0x4c
[可修复] archive_size: 归档大小 0x1000 与实际 0xac 不符 → 改写为 0xac

$ mpqtool repair corrupt.w3x               # 写出 corrupt.fixed.w3x
```

## 导入 / 删除

`mpq::modify`（CLI 的 `add` / `delete`，或 GUI 的「导入文件到归档…」
「删除选中」）向归档导入文件（同名替换）并删除条目，返回完整的新
文件字节与变更报告，写回前还会重新打开校验。策略：

- **保留文件原样拷贝**：压缩方式、扇区 CRC、加密全部不变，只重建
  容器（块表、哈希表、头部与 `(listfile)`）
- 归档前的字节（`MPQ\x1B` 用户数据头、嵌入垃圾前缀）与归档之后的
  追加数据原样保留，地图前后缀不丢
- 无 listfile 的保护图：**原哈希表槽位布局保留**，无名单的条目
  修改后仍可按名寻址；有名单时重建（可扩容的）哈希表
- key-v2 加密（密钥依赖块偏移）按新偏移**重新加密**（内容不变，加密
  保持）；`--no-reencrypt-key2` / GUI 勾选项可改为存明文；
  `(attributes)` / `(signature)` 因任何修改都会失效而自动移除
  （与 StormLib 行为一致）
- 纯 HET/BET 归档转为经典表：有名条目完整支持；无名单来源时拒绝
  转换并说明原因（避免无名条目失联）
- v4 归档写回仍为 v4：按新表重算 hash/block 表 MD5 与头部 MD5
  （`raw_chunk_size` 清零，文件级分块 MD5 数组不随原样拷贝保留）；
  也可经 `convert` 在 v1–v4 间显式互转

```console
$ mpqtool add map.w3x war3map.j newfile.txt
导入 war3map.j（48213 字节）
导入 newfile.txt（12 字节）
替换 1：war3map.j
新增 1：newfile.txt
自动移除（修改后失效）：(attributes)
已写入 map.w3x（74321 字节）

$ mpqtool delete map.w3x oldfile.txt
删除 1：oldfile.txt
已写入 map.w3x（73888 字节）
```

## 密码保护（自定义 v4 扩展）

MPQ 格式本身没有密码机制，本库在其上做私有扩展（设计文档见
`docs/password-encryption-design.md` 方案②）：文件内容加密、打开归档
需输入密码。**加密后的档只有本工具（配合密码）能打开**，StormLib、
MPQ Editor 等第三方工具一律不识别——这是既定代价，也正是加密的目的。

方案要点：

- **先压缩后加密，按扇区 AEAD**（ChaCha20-Poly1305）：每扇区独立
  nonce 与 AAD（由文件索引 + 扇区索引派生），随机访问天然保留；
  16 字节认证标签提供逐扇区防篡改——密文改动一位、密钥不对或扇区
  挪位都会当场报错
- **密钥体系（KEK 包装）**：每归档随机 32 字节主密钥；实际加密文件
  的密钥由主密钥经 HKDF-SHA256 按**文件索引**派生；主密钥用
  Argon2id(密码, salt) 异或包装后存入 `(password)` 内部文件
  （128 字节定长、明文单块存储）。**换密码只需原位重写这 128 字节**，
  不必重加密任何数据
- **密码校验器**：`(password)` 中还有一条用主密钥 AEAD 密封的已知
  常量（verifier）——打开时先验它，密码错立刻拒绝，而不是读到某个
  文件才发现
- **标记**：块标志 `FILE_ENCRYPTED | 0x0800_0000`（自定义位）；
  归档是否受保护以 `(password)` 文件存在为准（BET 表只编码部分
  标志位，不可靠）。文件名仍可见（基础版不隐藏名单）
- **互转限制**：受保护归档拒绝转 v1–v3（扩展基于 v4）；`protect`
  会自动把容器升级为 v4 并重算全部 MD5
- **修改受保护归档**：需先解锁（主密钥由 GUI state / CLI `--password`
  提供）；保持表槽位不变的文件原样拷贝（密钥按索引派生，槽位不变
  即有效），槽位变化的按新索引重新密封，导入的新文件同样加密

库接口：`mpq::protect` / `mpq::unprotect` / `mpq::change_password`，
`Archive::unlock(password)` 解锁后正常 `read_file` / `entries`。

```console
$ mpqtool encrypt map.w3x -p hunter2
已启用密码保护：3 个文件按扇区密封（ChaCha20-Poly1305），容器为 MPQ v4。
注意：第三方 MPQ 工具将无法打开该文件。
已写入 map.w3x（74488 字节）

$ mpqtool cat map.w3x war3map.j            # 无密码 → 明确报错
$ mpqtool cat map.w3x war3map.j -p hunter2 # 正常输出

$ mpqtool passwd map.w3x hunter2 n3w-pw    # 换密码，数据不动
$ mpqtool decrypt map.w3x -p n3w-pw        # 解除保护
```

## GUI（mpqtool-gui）

图形界面基于 **Tauri 2**（Rust 后端 + Vue 3 / Naive UI 前端，中文由
系统字体直接渲染，无需字体补丁）：

```
npm --prefix gui/ui install     # 首次：安装前端依赖
npm --prefix gui/ui run build   # 构建前端到 gui/ui/dist（改动后重跑；
                                 #      开发可改用 run watch 自动重建）
cargo run -p mpqtool-gui        # 开发运行
cargo build --release -p mpqtool-gui
```

功能与原 egui 版一致：

- 打开 / 浏览归档：按钮选择或**直接把文件拖进窗口**（未打开归档时拖入
  即打开；已打开归档时拖入即加入导入队列），拖入时高亮提示并注明将执行
  的操作，状态栏显示版本、表、警告数
- **文件管理器式浏览**：左侧目录树 + 右侧文件列表。MPQ 本身没有目录，
  树由 `(listfile)` 中的 `\` 路径合成（大小写不敏感合并文件夹）；
  `(listfile)` 等内部文件归入「(系统文件)」，无名条目归入「(未命名)」
- 面包屑导航 / 上一级 / 双击进入文件夹；**🔄 / F5 /「文件 → 重新载入
  归档」从磁盘刷新**（保留当前目录、展开状态与过滤条件）；文件列表含
  大小、压缩后、属性（加密/压缩等）、**修改时间（取自 `(attributes)`
  的 FILETIME）**、地区、来源等列
- 过滤框按全路径搜索，命中分支在树中自动展开，右侧切换为搜索结果视图
- 勾选文件或**整棵文件夹**后「提取选中」/「提取全部」（目录选择对话框，
  路径防逃逸）；双击文件预览文本内容
- 「文件 → 导入文件到归档…」多选导入（同名自动替换），或**直接把文件
  拖进已打开归档的窗口**；勾选后「删除选中」（未命名条目也可删）；
  待办列表集中在「修改归档」窗口，「应用修改」→「保存到文件…」
  （自动重载）或「在界面中打开结果」
- 「工具 → 修复头信息」：问题列表 → 应用修复 → 保存修复副本
  或直接在界面中重新打开修复结果
- **密码保护**（自定义 v4 扩展）：打开受保护归档时自动弹密码框
  （可取消，文件名仍可浏览；「工具 → 输入密码解锁」可再解锁）；
  「工具 → 设置密码 / 修改密码 / 解除密码保护」与修改归档同款
  「应用 → 保存 / 在界面中打开」流程；解锁状态跨刷新保持
- 「设置 → 界面设置」调整全局字号（自动保存）

前端（`gui/ui`）为 Vite 构建的 **Vue 3 + Naive UI** 单页应用，通过
Tauri IPC 调用 `gui/src` 中的命令（打开/预览/提取/修复/修改）；
`npm --prefix gui/ui run dev` 后访问
`http://localhost:5173/?mock` 可加载假后端做纯前端调试。打包安装包需
`cargo tauri build`（tauri-cli v2）。

## 正确性验证

- 哈希/加密：命中 StormLib 内建常数（`(hash table)` → `0xC3AF3770` 等）
- Jenkins 哈希：与编译后的原版 lookup3.c 输出比对（7 组参考向量）
- PKWARE explode：与编译后的原版 explode.c 逐字节一致；测试向量由
  Blizzard 实际使用的 implode 压缩器生成（`tests/fixtures/pk_*.bin`）
- 集成测试：测试内构建器从零合成 v1/v2/v3/v4 归档（加密、压缩、单单元、
  key-v2、扇区 CRC、listfile、attributes、userdata 包装、嵌入扫描、
  纯 HET/BET 表），再经公开 API 读回校验
- v4 测试：MD5 覆盖范围与 StormLib `LoadMpqTable`/`SaveMpqTable` 语义
  一致（存储态字节）；头部/各表/分块 MD5 损坏时打开与读取不受影响、
  仅产生告警；修改/新建/转换保持 v4 且重算后的校验和全部通过验证；
  修复 v4 头部后 `MD5_MpqHeader` 重算
- 修改测试：v1/v2/v3/v4 与 userdata 包装下导入/替换/删除后读回一致；
  无 listfile 时哈希槽位保留（按名寻址不丢）；key-v2 按新偏移重新加密
  （含多扇区混合压缩/不可压缩数据）及明文选项；未命名条目按表键删除；
  空文件、不可压缩数据、保留前后缀
- 修复测试：11 类损坏场景（表位置越界重定位、表项数截断、版本推断、
  头长度规整、v3 字段清零等）修复后归档可正常读取
- 密码测试：加密写→解锁读取 roundtrip（v1 源自动升 v4、单单元/空文件/
  不可压缩数据）、错密码立即拒绝、未解锁读取报错、字节篡改触发 AEAD
  标签失败（其他文件不受影响）、换密码仅重写 128 字节且旧密码失效、
  解除保护 roundtrip、受保护归档上增删文件、直接删除 `(password)`
  被拒、v1–v3 转换被拒、空归档保护/解除

```console
$ cargo test        # 112 项测试
```

## 目录结构

```
mpq/            读取/修改库（edition 2024, no unsafe）
  src/crypt.rs        storm 加密、HashString、文件密钥
  src/jenkins.rs      lookup3 hashlittle2（HET/BET 名字哈希）
  src/digest.rs       MD5（v4 表/头部/分块校验与写回）
  src/header.rs       v1–v4 头部与 MPQ\x1B 用户数据头
  src/tables.rs       经典 hash/block/hi-block 表
  src/hetbet.rs       HET/BET 扩展表
  src/compression.rs  zlib/pkware/bzip2/lzma 解压调度
  src/pkware.rs       PKWARE DCL explode 移植
  src/passcrypt.rs    密码保护扩展（Argon2id/HKDF/ChaCha20-Poly1305）
  src/repair.rs       头部诊断/修复 + 表位置深度扫描（v4 修复后重算头部 MD5）
  src/modify.rs       导入/删除：块原样拷贝 + 容器重建（v4 保持版本并重算 MD5）
  src/create.rs       新建空归档（v1–v4）与版本转换辅助
  src/archive.rs      Archive：打开/查找/读取 + 容错与校验策略
mpqtool/        CLI + 共享任务库（lib）
  src/main.rs         命令行入口
  src/lib.rs          lib 入口（暴露 tasks）
  src/tasks.rs        CLI/GUI 共享的提取、目录树等逻辑
gui/            Tauri 2 图形界面（mpqtool-gui）
  src/main.rs         窗口与插件装配
  src/state.rs        打开的归档状态与前端 DTO
  src/commands.rs     IPC 命令（打开/预览/提取/修复/修改）
  ui/                 前端（静态 HTML/CSS/JS，无构建步骤）
  tauri.conf.json     Tauri 配置（窗口、图标、capabilities）
docs/           格式依据备忘（源自 StormLib 源码验证）
```

## 构建说明（本机）

仓库内的 `.cargo/config.toml` 固定了 MSVC 链接器路径与 `LIB`，
用于绕开 Git Bash 下 `/usr/bin/link` 遮蔽 `link.exe` 的问题；
在其他机器上如无此问题可删除该文件。

## 依赖

- 库：`flate2`（zlib，miniz_oxide 后端）、`bzip2-rs`、`lzma-rs`、
  `md-5`（RustCrypto，v4 校验）（全部纯 Rust），`no unsafe`
- 工具：`clap`（CLI）、`eframe/egui`（GUI）、`rfd`（文件对话框）
