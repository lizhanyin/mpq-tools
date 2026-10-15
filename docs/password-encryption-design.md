# MPQ v4 密码加密方案（设计稿，2026-10-15）

基于 v4 做自定义扩展：文件内容加密，打开归档需输入密码。本文记录可行性结论、方案对比与推荐方案的设计要点。

## 前提与结论

MPQ 格式**本身没有密码机制**。库内已实现的 Storm 加密（`mpq/src/crypt.rs`）密钥从**文件名**推导
（`file_key()`，读取路径见 `mpq/src/archive.rs:826` 一带）——任何了解格式的人用 StormLib 即可解开，
它是防读取的混淆，不是密码保护。

因此"输入密码才能打开"必然是**自定义扩展**，代价：加密后的档，第三方 MPQ 工具
（StormLib、MPQ Editor 等）一律打不开，只有本工具（配合密码）能读。这是无法避免的，也正是加密的目的。

代码基础适合做此事：扇区级加密/解密管线已完整（读路径 `mpq/src/archive.rs:826-912`，
写路径 `mpq/src/modify.rs` 已有 `FILE_ENCRYPTED | FILE_KEY_V2` 写入测试），
v4 头部 / HET/BET / MD5 重算齐备，缺的只是"密钥从哪来"与"密码校验"这一层。

## 方案对比

| 方案 | 做法 | 强度 | 改动量 | 代价 |
|---|---|---|---|---|
| ① 复用 Storm 加密，密钥改由密码派生 | block 置 `FILE_ENCRYPTED`，key = KDF(密码) 截 32 位 | 弱：密钥空间仅 2³²，攻击者无需密码即可暴力解开单文件 | 最小 | 付了不兼容的代价却没换来真强度 |
| ② 每文件现代 AEAD 加密（**推荐**） | 结构不动，扇区加密层替换为 ChaCha20-Poly1305 / AES-256-GCM，key = Argon2id(密码) | 强，自带完整性校验 | 中等 | 私有扩展；文件名仍可见（可进阶隐藏） |
| ③ 整档容器加密 | 在 MPQ 之外把整个文件加密（可用 `age` 库） | 最强：连结构、文件名都不可见 | 小 | 无随机访问；大档打开/每次修改都要整档重加密；文件不再被识别为 MPQ |

结论：选**方案 ②**。方案 ① 只适合防误读/轻度遮挡，且会造成"安全"的错觉；
方案 ③ 适合存档封存，不适合 GUI 频繁浏览、修改、v1–v4 互转的场景。

## AEAD 认证加密说明

AEAD（Authenticated Encryption with Associated Data，带关联数据的认证加密）不是具体算法，
而是一类方式：**加密 + 完整性认证二合一**。常见实现即 AES-256-GCM 与 ChaCha20-Poly1305。

调用形式：

```
encrypt(key, nonce, 明文, aad)              →  密文 + tag（16 字节认证标签）
decrypt(key, nonce, 密文, aad, tag)         →  明文；任一项不对直接报错
```

tag 由密钥、明文、nonce、aad 共同决定，解密时先验 tag——密文被改过一位、或密钥不对，
验证当场失败。这是它与纯流加密（AES-CTR、MPQ 的 Storm 加密）的本质区别：
纯流加密用错密钥也"解密成功"，只是得到乱码，可能要到解压失败才发现，甚至永远发现不了
（静默数据损坏）。

对本方案的三个用途：

1. **天然密码校验器**：`"(password)"` 文件里 AEAD 密封的 verifier（正确 master key 加密的
   已知常量），打开时试着解它，tag 验证不过即密码错，立刻拒绝——避免"读到某个文件才发现密码错"。
2. **逐扇区防篡改**：每扇区带自己的 tag，归档字节被改动（哪怕位翻转）读到该扇区即报错，
   不会把坏数据当解压结果交出；比 v4 头部的表 MD5 更精细（MD5 不覆盖文件内容）。
3. **AAD 绑定位置**：把 file index + sector index 作为 aad 传入（aad 只参与生成 tag、不加密），
   效果是密文扇区无法挪到别的文件或调换顺序——换位置后 aad 对不上、tag 验证失败。
   与"nonce 由 file index + sector index 派生"（防 nonce 重用）配合即完整。

## 方案 ② 设计要点

### 加密层

- 先压缩后加密（与现有管线一致），**按扇区**加密：MPQ 本就按扇区读取，
  每扇区独立 nonce（由 file index + sector index 派生），随机访问天然保留。
- AEAD 选 ChaCha20-Poly1305 或 AES-256-GCM，tag 提供逐扇区完整性校验。
- 加密后扇区长度变化（+tag），注意 sector offset table 与 block size 字段的偏移计算。

### 密钥体系（KEK 包装）

- 每个归档随机生成 master key（32 字节），实际加密文件的密钥由 master key 经 HKDF 按 file index 派生。
- master key 用 Argon2id(密码, salt) 加密后存放。**改密码只需重新包装 master key**，
  不必重加密全部数据；也自然支持"无密码档 → 加密码档"与反向转换（解包装后明文写回）。
- KDF 必须 Argon2id（不能单次 SHA-256），防 GPU 暴力破解密码。

### `"(password)"` 内部文件

充当密码标记 + 校验器，与 `(listfile)`、`(attributes)` 同类。布局：

- magic（自定义）、方案版本号
- Argon2 参数（m_cost/t_cost/p_cost）+ salt
- Argon2id(密码) 加密包装的 master key
- AEAD 密封的 verifier（已知常量）：打开归档时先验它，密码错立刻报错，
  而不是读到某个文件才失败

### 加密方案标记

- block flags：`FILE_ENCRYPTED`（0x0001_0000）+ 保留高位（如 0x0800_0000）表示"密码 AEAD 方案"。
- 注意 BET 表只编码部分 flag 位（StormLib 的 BET flags mask 不含自定义位）：
  要么扩展本库 BET 的 flags mask，要么以 `"(password)"` 文件存在与否为准——
  **建议后者**，更稳且不受表实现约束。

### GUI / CLI 流程

- GUI：`open_archive`（`gui/src/commands.rs:33`）检测到 `"(password)"` 文件 → 弹密码框 →
  验证 verifier → master key 存入 state（`gui/src/state.rs`）供后续读取。
- CLI（`mpqtool`）：加 `--password` 参数；无密码访问加密档时给出明确错误。

### 依赖

新增 `argon2` + `chacha20poly1305`（RustCrypto，纯 Rust，体积小）。

## 兼容与转换策略

- 未加密档照常打开，不受影响。
- 版本互转（v1–v4）：遇到含密码加密文件的档，转 v1–v3 时**拒绝或要求先解密**
  （老格式装不下该扩展）。
- 第三方工具打不开加密档是既定代价，文档与 UI 提示中应明示。

## 可选加强：隐藏文件名

把 `(listfile)` 也用 master key 加密，无密码时连文件名列表都看不到
（hash 表本身是散列，不直接泄露名字，除非暴力枚举文件名）。可在基础版之后追加。

## 实现触点清单

- 读路径：`mpq/src/archive.rs` 扇区解密处按方案分派（Storm / 密码 AEAD）
- 写路径：`mpq/src/modify.rs`、`mpq/src/create.rs` 加密写入 + `"(password)"` 文件生成
- 头部/表：v4 各表 MD5 照常重算；BET flags mask 决策见上
- GUI：密码输入弹窗、state 存 master key、错误提示
- CLI：`--password` 参数
- 测试：加密写→读 roundtrip、错密码拒绝、改密码（re-pack）、无密码↔有密码转换、
  v1–v3 转换拒绝策略、(listfile) 加密加强版（若做）
