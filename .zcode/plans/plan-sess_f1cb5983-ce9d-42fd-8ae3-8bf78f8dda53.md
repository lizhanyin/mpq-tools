# MPQ 导入：进度显示 + 预览分批渲染 + 多线程导入

## Context

当前导入流程（`stat_imports` 预览 → `apply_modify` 全量重编码 → `save_modified` 落盘）存在三个问题：
1. **无进度反馈**：`apply_modify` 持全局锁做「读全部文件 + 全归档重编码 + 自检回读」，大批量导入时前端只有确认按钮转圈，无任何阶段/百分比提示；`stat_imports` 扫大目录同样无反馈。
2. **预览列表全量渲染**：ModifyModal「待导入」列是裸 `v-for`（ModifyModal.vue:40），导入上万文件时 DOM 卡顿。
3. **全单线程**：文件读取（commands.rs:1071 顺序 `fs::read`）、Fresh 块压缩（modify.rs:840 布局循环内逐块调 `encode_fresh`）、单文件内 sector 压缩（modify.rs:1125）均单核执行。

已确认可行：`Payload::Fresh`（全部新增导入文件）编码与位置无关（modify.rs:158 注释），可先并行压缩再布局；flate2 zlib 确定性输出，并行化不改变产物字节。

## 改动方案

### 1. mpq crate：进度回调 + rayon 并行（mpq/src/modify.rs）

- `Cargo.toml`（workspace 根 + `mpq/` + `gui/`）增加 `rayon = "1"`。
- 新增公开进度类型：
  ```rust
  pub enum ModifyPhase { Encoding, Verifying }
  pub fn modify_with_progress(data, deletions, additions, options,
      progress: Option<&(dyn Fn(ModifyPhase, usize, usize) + Sync)>) -> Result<ModifyOutcome>
  ```
  现有 `modify_with` 改为委托它并传 `None`（对外 API 不变）。
- **Fresh 块预编码并行**：布局循环前，`blocks.par_iter_mut()` 把每个 `Payload::Fresh` 预编码为新变体 `Payload::FreshEncoded { bytes, flags }`（原子计数器报 Encoding 进度）；布局循环（834 行）遇到它直接取字节。`Raw/ReEncrypted/Aead` 因偏移/索引依赖保持串行，循环内逐块报进度（total = blocks.len()）。
- **单文件 sector 并行**：`encode_fresh` 的 sector 压缩循环（1125 行）改 rayon 并行（提取 `compress_sector` helper，拼装顺序不变）；`encode_fresh_reencrypted` 复用该 helper 并行预压缩，加密仍按序（密钥含偏移）。
- 自检回读循环（983-1016 行）报 Verifying 进度（total = adds + reencoded + aead_reencoded 条数）。

### 2. gui 后端：Channel 进度 + 并行读文件（gui/src/commands.rs）

- 新 DTO（snake_case 字段，与现有 ImportStatDto 一致）：
  ```rust
  #[derive(Serialize, Clone)] pub struct ImportProgressDto { phase: String, done: u32, total: u32 }
  ```
  phase ∈ `"reading" | "encoding" | "verifying"`。
- `apply_modify` 增加 `on_progress: Option<tauri::ipc::Channel<ImportProgressDto>>`：
  - **读文件移出全局锁**并用 rayon 并行（保序收集，保留首错文案 `读取失败 {path}: {e}`），AtomicUsize 计数按 ~1% 步长节流 emit `"reading"`；
  - 锁内改调 `mpq::modify_with_progress`，闭包把 Encoding/Verifying 映射为 emit（同样 1% 节流）。Channel 是 `Send+Sync`，rayon 线程发送安全。
- `stat_imports` 增加 `on_progress: Option<Channel<ImportProgressDto>>`，目录遍历中每 32 项 emit `"reading"`（total=0，表示"已发现 N 项"）。

### 3. 前端：进度 UI + 预览分批渲染

- `types.ts`：新增 `ImportProgress { phase: 'reading'|'encoding'|'verifying'|'saving', done: number, total: number }`。
- `store.ts`：
  - 全局状态 `S.importProgress: ImportProgress | null`；
  - `fetchStats` / `confirmModify` 用 `new Channel<ImportProgress>()`（`@tauri-apps/api/core`），设 `ch.onmessage = (p) => S.importProgress = p`，随 invoke 传 `onProgress`，finally 置 null；`save_modified` 前置 `{ phase: 'saving', done: 0, total: 0 }`；
  - 文案 helper：读取中 X/Y（或"已发现 N 项"）、导入中 P%、校验中 X/Y、保存中。
- `ModifyModal.vue`：
  - **分批渲染**：`visibleAdds = ref(100)` 常量 `IMPORT_PREVIEW_STEP = 100`，列表渲染 `additions.slice(0, visibleAdds)`，底部「加载更多（还有 N 项）／已显示 X / 共 Y」按钮；watch additions 数组引用被整体替换（如"只导入子项"重统计）时重置 100 并回滚顶部；「待删除」列同样处理；
  - busy 时在 action 区上方显示 `n-progress`（line，total=0 时 indeterminate）+ 阶段文案。
- `App.vue` + `style.css`：顶部细进度条（仿 `.drop-overlay` 模式、`pointer-events:none`），`S.importProgress && !S.modify` 时显示（弹窗内已有详细进度，不重复）。
- `mock-ipc.ts`：`stat_imports` / `apply_modify` mock 调 `args.onProgress?.onmessage?.(...)` 分步模拟进度（真实 Channel 在浏览器 mock 下不能 `send`，但直调 `onmessage` 安全）；文件夹导入 mock 展开约 300 条假数据，便于调试"加载更多"。

## 关键文件

- `mpq/src/modify.rs`、`mpq/Cargo.toml`、`Cargo.toml`（workspace）
- `gui/src/commands.rs`、`gui/Cargo.toml`
- `gui/ui/src/store.ts`、`types.ts`、`components/ModifyModal.vue`、`App.vue`、`style.css`、`mock/mock-ipc.ts`

## 验证

1. `cargo test -p mpq`：既有 modify/恢复测试保证并行化后产物字节不变（zlib 确定性 + 顺序不变）。
2. `cargo build`（workspace 编译通过，gui 命令签名变更无遗漏调用点）。
3. `gui/ui` 下 `npm run build`（vue-tsc 类型检查）。
4. `npm run dev` + `?mock`：mock 模式手测——300 条待导入的"加载更多"分批、进度条阶段切换（读取→导入→校验→保存）。
5. 有真实归档时 `cargo tauri dev`：导入大文件夹，观察进度推进、多核 CPU 占用（任务管理器确认压缩并行）、导入后归档可正常打开读取（自检保证）。

## 明确不做（控制范围）

- 不并行化自检回读（`Archive` 含 `RefCell` 非 `Sync`，改造代价大，仅报进度）。
- 不给 `save_modified` 加后端 Channel（前端显示"保存中"即可）。
- 不做进度取消（cancel）功能。