# 新增功能：对比生成 patch.mpq 与 合并 MPQ

## Context

为 mpqtool 新增两个功能（GUI + CLI 都做，用户已确认）：

1. **生成补丁**：当前打开的归档（新文件）与磁盘上的旧文件（基线）对比，把差异（新增 + 修改文件的新版本）打包成独立的 `patch.mpq`（War3 补丁式覆盖加载用途）。
2. **合并归档**：把另一个 MPQ（来源）合并进当前归档。来源独有文件直接导入；同名且内容不同的文件视为「修改」，弹列表由用户勾选是否覆盖（**默认全部覆盖**，用户已确认），提供一键「全部覆盖 / 全部保留」。

关键现状：项目尚无任何 diff/merge 代码；`modify` 的 additions 同名会自动替换（无需 deletions）；`modify` 会自动扩容哈希表（`mpq/src/modify.rs:944`）；`SPECIAL_FILES`（5 个内部文件名）已导出可直接排除。

## 1. 核心库 mpq — 新增 `mpq/src/diff.rs`

```rust
pub enum DiffKind { Added, Changed, Same, Removed }   // 相对「新」侧
pub struct DiffEntry { pub name: String, pub kind: DiffKind,
                       pub old_size: Option<u64>, pub new_size: Option<u64> }
pub fn diff_archives<R1, R2>(old: &Archive<R1>, new: &Archive<R2>,
    progress: Option<&(dyn Fn(usize, usize) + Sync)>) -> Result<Vec<DiffEntry>>
```

- 只比较**具名**条目，排除 `SPECIAL_FILES`；名字匹配用 `crypt::normalize_name`（大小写不敏感、`/`↔`\`）；显示名优先取新侧拼写
- 快速路径：size 不同直接判 Changed；size 相同才 rayon 并行读两侧解压字节逐字节比较
- 结果按名字排序；进度回调 `(done, total)` 供上层接 Channel
- `mpq/src/lib.rs` 注册 `mod diff;` 并导出

## 2. mpqtool — tasks 层与 CLI 子命令

**`mpqtool/src/tasks.rs`** 新增 `build_patch<R>(new: &Archive<R>, names: &[String], version: FormatVersion, progress) -> Result<Vec<u8>>`：从 new 提取指定名字的字节 → `mpq::create`（空归档）→ `mpq::modify_with_progress` 导入（CLI 与 GUI 共用）。

**`mpqtool/src/main.rs`** 新增两个子命令（复用 `open_archive_with_listfiles` 名字恢复、`PasswordArg` 受保护处理、`run_modify` 写回模式）：
- `mpqtool patch <new.mpq> --baseline <old.mpq> [-o out] [--version 1-4]`：diff → 打印摘要（新增/修改/相同/仅旧）→ Added+Changed 打包写出（默认 `<名>-patch.mpq`）
- `mpqtool merge <target.mpq> <source.mpq> [-o out] [--no-overwrite]`：diff(old=target, new=source) → Added + Changed（`--no-overwrite` 时跳过 Changed）从 source 提取 → `modify` 到 target → 写盘 + 摘要

## 3. GUI 后端 — `gui/src/commands.rs` 新增 3 条 `#[tauri::command(async)]`（`main.rs` 注册）

1. `diff_with_disk(path, baseline: bool, on_progress: Channel, state) -> DiffResultDto`
   - `baseline=true`（补丁场景）：old=磁盘文件、new=当前归档；`false`（合并场景）：old=当前归档、new=磁盘来源
   - 锁内快照 `raw + master_key`，锁外重建 `Archive`（受保护时 `unlock_with_master_key`；磁盘侧受保护且无法解锁则报错说明）→ `diff_archives`
   - 进度用新 phase `"diffing"`；DTO 含 entries + 两侧未命名条目计数
2. `build_patch(names, out_path, version, on_progress, state) -> NoteDto`：从当前归档并行提取 names（phase reading）→ `tasks::build_patch`（phase encoding/verifying）→ 写盘 → 返回摘要
3. `stage_merge_files(source_path, names, state) -> Vec<StagedDto>`：打开磁盘来源逐名读内容 → 写 `%TEMP%\mpqtool-merge` 临时文件 → `replaces` 按当前归档判断（完全对照现有 `stage_move_files`，commands.rs:628）

## 4. GUI 前端（Vue 3 + Naive UI）

**`types.ts`**：`DiffEntryDto { name, kind: 'added'|'changed'|'same'|'removed', old_size, new_size }`、`DiffResultDto`（snake_case 契约）。

**`store.ts`**：
- `PatchState { oldPath, diff, loading, unchecked: Set<string>, version, busy }`、`MergeState { sourcePath, diff, loading, unchecked, staging }`（两组可勾选项默认全选，记录取消项）
- 动作对：`startPatch/closePatch/pickPatchOld/confirmBuildPatch`、`startMerge/closeMerge/pickMergeSource/confirmMerge`
- `confirmMerge`：`stage_merge_files` → 入队 `S.modify.additions`（抽公共 helper 复用 `moveKeysTo` 的同名去重入队逻辑）→ 关闭弹窗，`ModifyModal` 因 `S.modify` 非空自动弹出（与「移动」一致的两阶段流程），toast 提示在「修改归档」窗口应用后生效
- `confirmBuildPatch`：dialogSave（默认 `patch.mpq`）→ `build_patch`（复用 `progressChannel`/`S.importProgress`）→ 成功 toast+log
- `importProgressLabel` 增加 `diffing → 对比中`；`anyModalOpen` 补两个切片

**新组件**（挂到 `App.vue`，样式复用 `.dialog-body`/`.dialog-actions`）：
- `DiffList.vue`：按 kind 分组的共享列表——可勾选组（added/changed）带 `n-checkbox` + 组头「全选/清空」；只读组（same/removed）可折叠；行显示名字 + 新旧大小
- `PatchModal.vue`：新=当前归档、旧=选择基线文件 → 对比 → DiffList → 输出版本（默认跟随 `S.info.version`，v1–v4 单选）→「生成 patch.mpq」
- `MergeModal.vue`：目标=当前归档、选择来源归档 → 对比 → DiffList（changed 组标题「覆盖（同名不同内容）」+ 一键全部覆盖/全部保留）→「合并到当前归档」

**`MenuBar.vue`**：工具菜单在「版本转换」后加分隔线 +「生成补丁 (patch.mpq)…」「合并归档…」（disabled: `!hasArchive`）。

## 5. 测试与验证

- 新增 `mpq/tests/diff.rs`：用 `tests/common/mod.rs` 的 `ArchiveSpec` 构造 old/new（相同 / 修改 / 各自独有 / 大小写与斜杠变体同名 / 特殊文件 / 未命名条目）断言分类；再验证 `build_patch` 组合产物能被 `Archive` 打开且内容与 new 一致、条目数正确
- `cargo test --workspace`、`cargo clippy`；`gui/ui` 下 `npm run build`（含 vue-tsc 类型检查）
- CLI 冒烟：`cargo run -p mpqtool -- patch/merge` 用测试生成的归档走一遍
- GUI 手动流程：打开归档 → 工具菜单两个新入口 → 对比/生成/合并全程有进度反馈

## 涉及文件

新增：`mpq/src/diff.rs`、`mpq/tests/diff.rs`、`gui/ui/src/components/{DiffList,PatchModal,MergeModal}.vue`
修改：`mpq/src/lib.rs`、`mpqtool/src/{tasks,main}.rs`、`gui/src/{commands,main}.rs`、`gui/ui/src/{types,store}.ts`、`gui/ui/src/components/{App.vue→根,MenuBar.vue}`、`gui/ui/src/style.css`（少量）