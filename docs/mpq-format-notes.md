# MPQ 格式关键事实（从 StormLib master 源码验证，2026-10）

本文件是实现的格式依据备忘。来源：github.com/ladislav-zezula/StormLib（src/StormLib.h、SBaseCommon.cpp、
SBaseFileTable.cpp、SFileOpenArchive.cpp、SFileReadFile.cpp、SCompression.cpp、SFileAttributes.cpp、
pklib/explode.c、jenkins/lookup3.c）。

## 头部

- magic `MPQ\x1A` = u32 LE 0x1A51504D；user data magic `MPQ\x1B` = 0x1B51504D
- 版本字段 format_version：0=v1, 1=v2, 2=v3, 3=v4
- v1 头 0x20：magic, header_size u32, archive_size u32, version u16, block_size u16(扇区=512<<bs),
  hash_table_pos u32, block_table_pos u32, hash_table_size u32, block_table_size u32
- v2 头 0x2C：+ hi_block_table_pos u64, hash_table_pos_hi u16, block_table_pos_hi u16（表位置相对归档起点，48位）
- v3 头 0x44：+ archive_size64 u64, bet_table_pos64 u64, het_table_pos64 u64（均为相对归档起点）
- v4 头 0xD0：+ hash/block/hiblock/het/bet_table_size64（5×u64 @ 0x44–0x6C，**存储态**表大小）,
  raw_chunk_size u32 @ 0x6C, MD5_BlockTable @ 0x70 / MD5_HashTable @ 0x80 / MD5_HiBlockTable @ 0x90 /
  MD5_BetTable @ 0xA0 / MD5_HetTable @ 0xB0 / MD5_MpqHeader @ 0xC0（6×16B；注意 BET 在 HET 之前）
- v3 无表大小字段：按顺序 HET→BET→hash→block→hi-block 递减排列，从 archive_size64 逐级倒推
  （size = 上一边界 - pos，MaxOffset 依次更新）；v2 hash 表大小 = block_pos - hash_pos
- header_size 允许小于标准（v3 允许 = 0x2C 之类），缺失字段按 0 处理
- 扫描：0x200 对齐扫描至 128MB，magic + header_size >= 0x20 判定候选；userdata 头给出 mpq_pos

## user data 头（'MPQ\x1B'， Warcraft III 地图 / SC2 地图前置）

user_data_size u32（数据最大尺寸）、header_offs u32（MPQ 头相对偏移）、user_data_header_size u32（通常 0x10）

## 加密

- crypt table 0x500 个 u32：seed=0x00100001；idx=i1+i*0x100（i1∈0..0x100, i∈0..5）；
  seed=(seed*125+3)%0x2AAAAB → t1=(seed&0xFFFF)<<16；再算一次 → t2=seed&0xFFFF；tbl=t1|t2
- HashString：seed1=0x7FED7FED, seed2=0xEEEEEEEE；ch=大写表[c]（'/'→'\\'，a-z→A-Z，≥0x80 不变）；
  seed1 = tbl[htype+ch] ^ (seed1+seed2)；seed2 = ch + seed1 + seed2 + (seed2<<5) + 3（全 wrapping，seed1 为新值）
  htype: 0x000 表索引, 0x100 nameA, 0x200 nameB, 0x300 文件密钥, 0x400 key2mix
- Decrypt：key2=0xEEEEEEEE；对每个 u32(LE) v：key2 += tbl[0x400+(key&0xFF)]；ch=v^(key+key2)；
  key = ((!key)<<21).wrapping_add(0x11111111) | (key>>11)；key2 = ch+key2+(key2<<5)+3
- Encrypt 同构，但 key2 更新用明文 v
- 密钥：HashString("(hash table)",0x300)=0xC3AF3770；HashString("(block table)",0x300)=0xEC83B3A3（用于自检）

## 经典表

- hash 表（整体加密，16B/项）：name1 u32, name2 u32, locale u16, platform u8, flags u8, block_index u32
  FREE=0xFFFFFFFF（终止探测），DELETED=0xFFFFFFFE（继续）
- block 表（整体加密，16B/项）：pos u32(相对归档), csize u32, fsize u32, flags u32
- hi-block 表：每项 u16（block 偏移高 16 位），明文，位于 hi_block_table_pos
- 文件 flags：EXISTS 0x80000000, SINGLE_UNIT 0x01000000, DELETE_MARKER 0x02000000, SECTOR_CRC 0x04000000,
  SIGNATURE 0x10000000, PATCH 0x00100000, ENCRYPTED 0x00010000, KEY_V2 0x00020000,
  COMPRESS 0x00000200, IMPLODE 0x00000100, COMPRESS_MASK 0xFF00
- 表存储尺寸 < 未压缩尺寸（count*16）时：先解密再按单单元解压（首字节掩码）—— v2/v3 压缩表、v4 显式尺寸
- 读越界 EOF：读到多少算多少，余下补零（LoadMpqTable 行为）

## 文件密钥与扇区

- key = HashString(去掉路径的纯文件名, 0x300)；若 KEY_V2：key = (key + (block_pos as u32)) ^ fsize
- 扇区偏移表（压缩且非单单元时存在，位于文件块开头）：(nsec+1) u32 [+ nsec u32 CRC]，加密时用 key-1 解密
  偏移相对块首；offsets[0]=表长；StormLib 校验单调性且每扇区长 ≤ sector_size
- 未压缩扇区文件无偏移表：扇区位于块首 + i*sector_size
- 扇区 i 数据加密用 key+i；单单元整块用 key；解压仅当 raw < expected（否则原样拷贝）
- 末扇区 expected = fsize - (nsec-1)*ss；nsec = ceil(fsize/ss)
- 扇区 CRC：StormLib 用 adler32（对解密后压缩数据），规格文档称 CRC32(解压数据)。仅作警告。

## 压缩方法（数据首字节掩码）

0x01 huffman, 0x02 zlib(zlib 流含 78 xx 头), 0x08 pkware explode, 0x10 bzip2, 0x12 lzma,
0x20 sparse(组合 0x22 sparse+zlib, 0x30 sparse+bzip2), 0x41 huff+adpcm_mono, 0x81 huff+adpcm_stereo
多方法按 dcmp_table 顺序（bzip2→pkware→zlib→huff→adpcm_s→adpcm_m→sparse）逐层解。
IMPLODE flag（非 COMPRESS）数据无掩码字节，直接 pkware 流。
LZMA 帧为 [filter 字节=0][props 5B][未压缩大小 8B][数据]，可用 .lzma 格式拼接交给解码器。

## HET/BET（v3 扩展表，ExtHdr: sig u32 + version u32(=1) + data_size u32，表体可加密(对应经典表密钥)+可压缩）

- HET sig 0x1A544548；BET sig 0x1A544542；HET 用 hash-table 密钥，BET 用 block-table 密钥
- HET 头(u32×)：table_size, entry_count, total_count, name_hash_bit_size, index_size_total,
  index_size_extra, index_size, index_table_size(字节)
- HET 体：total_count 字节（槽位 NameHash1，0=空）+ index_table_size 字节位打包索引
  （每槽 index_size_total 位，有效 index_size 位）
- AndMask=(1<<hashbits)-1（hashbits=0x40 时为全 1），OrMask=1<<(hashbits-1)
- 查找：h=(jenkins(name)&And)|Or；b1=h>>(hashbits-8)；slot=h%total_count 线性探测；
  槽字节==b1 → 位图取索引；全哈希校验 = (b1<<bet.bitcount_namehash2)|bet.namehash2[idx]
- BET 头(u32×)：table_size, entry_count, unknown08, table_entry_size(位/记录),
  bitidx_filepos, bitidx_filesize, bitidx_cmpsize, bitidx_flagidx, bitidx_unknown,
  bitcount_filepos, bitcount_filesize, bitcount_cmpsize, bitcount_flagidx, bitcount_unknown,
  bittotal_namehash2, bitextra_namehash2, bitcount_namehash2, namehash_array_size(字节), flag_count
- BET 体：flag_count 个 u32 flags + entry_count×table_entry_size 位文件记录 + 名字哈希数组
  （每项 bittotal_namehash2 位，有效 bitcount_namehash2 位，LSB-first 位打包）
- Jenkins 哈希：名字转小写表（'/'→'\\'）后 hashlittle2，init pc=2, pb=1；
  结果 = (b_out << 32) | c_out（lookup3 字节路径）

## (attributes)

u32 version(=100), u32 flags(1=CRC32,2=FILETIME,4=MD5,8=PATCH_BIT)；随后按序每文件数组：
CRC32 u32 × n，FILETIME u64 × n，MD5 16B × n，patch bit × n

## (listfile)

文件名以 0x0D 0x0A / ';' 分隔；(listfile) 本身通常压缩存储。

## v4 校验（MD5，源自 StormLib SBaseFileTable.cpp / SFileOpenArchive.cpp 逐一验证）

- **表 MD5 覆盖存储态字节**：hash/block 表 MD5 对**加密（若压缩则压缩后）**
  的磁盘字节、按 `*_table_size64`（或推导的存储大小）精确长度计算——
  即 StormLib `SaveMpqTable` 先 `EncryptMpqBlock` 再 `CalculateDataBlockHash`，
  加载侧 `LoadMpqTable` 在解密**前**对刚读入的原始字节校验（不匹配 →
  ERROR_FILE_CORRUPT，表作废）。全零 MD5 表示"未填写"，跳过（IsValidMD5）
- **HET/BET MD5**：覆盖**整个存储态表**（含 12 字节明文 ExtHdr），
  `SaveExtTable` 在（可选）压缩+加密后对存储总长计算；master 分支加载侧
  已不再校验（旧版曾校验失败即弃表）
- **MD5_MpqHeader**：对头部 `0x00..0xC0`（signature 到 MD5_HetTable）计算；
  打开时非零且失配 → ERROR_FAKE_MPQ_HEADER（StormLib 硬拒绝，SC2 尤其
  依赖）；保存时最后写入。修复头部任何字段都会使其失效，需重算
- **hi-block 表**：StormLib 保存时**不**回填其 MD5（保持 0）；
  `SFileVerifyRawData(SFILE_VERIFY_HIBLOCK_TABLE)` 可按需校验存储态字节
- **raw_chunk_size**：非零时每个文件的**存储态数据**（csize 字节）按
  chunk 分块，`ceil(csize/chunk)` 个 MD5 组成的数组**紧跟在存储数据之后**
  （不占 csize、不加密），见 `WriteMemDataMD5`/`AllocateMd5Buffer`；
  写入侧空间分配把数组长度计入 FreeSpacePos
- 本库取舍：以上校验失败一律**降为告警**（容错打开哲学），写回 v4 时
  重算 hash/block 表 MD5 与头部 MD5（HET/BET/hi-block 不写出、MD5 置零，
  raw_chunk_size 清零不携带文件级分块 MD5）
