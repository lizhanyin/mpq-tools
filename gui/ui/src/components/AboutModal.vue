<template>
  <n-modal
    :show="S.aboutOpen"
    preset="card"
    title="关于 mpqtool"
    :style="{ width: '460px', maxWidth: '94vw', borderRadius: '12px' }"
    :bordered="false"
    @update:show="onShowUpdate"
  >
    <div class="dialog-body about-body">
      <div class="about-head">
        <div class="about-logo">📦</div>
        <div>
          <div class="about-name">mpqtool</div>
          <div class="about-sub">MPQ 归档浏览器 · {{ version }}</div>
        </div>
      </div>

      <p class="about-desc">
        mpq —— Rust 实现的 Mo'PaQ (MPQ) v3/v4 协议库与工具集：完整支持
        v1–v4 头部解析、HET/BET 扩展表、v4 MD5 校验、缺 (listfile)
        归档的文件名恢复与密码保护；含命令行工具 mpqtool 与本图形界面。
      </p>

      <div class="about-rows">
        <div class="about-row"><span>作者</span><b>lizy</b></div>
        <div class="about-row"><span>开源许可</span><b>BSD 3-Clause License</b></div>
        <div class="about-row"><span>技术栈</span><b>Rust · Tauri 2 · Vue 3 · naive-ui</b></div>
      </div>
    </div>

    <template #action>
      <div class="dialog-actions">
        <div class="grow"></div>
        <n-button size="small" @click="S.aboutOpen = false">关闭</n-button>
      </div>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
  import { ref } from 'vue'
  import { NModal, NButton } from 'naive-ui'
  import { S } from '../store'
  import { isMock } from '../ipc'

  function onShowUpdate(v: boolean) {
    S.aboutOpen = v
  }

  // 版本号与 tauri.conf.json / workspace Cargo.toml 保持同步；
  // 真实环境优先取打包进 Tauri 的版本，mock / 失败时用常量。
  const FALLBACK_VERSION = '0.1.0'
  const version = ref(FALLBACK_VERSION)
  if (!isMock) {
    import('@tauri-apps/api/app').then(({ getVersion }) => getVersion())
      .then((v) => { version.value = v })
      .catch(() => { /* 保留 fallback */ })
  }
</script>

<style scoped>
  .about-head {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .about-logo {
    font-size: 40px;
    line-height: 1;
    flex: none;
  }

  .about-name {
    font-size: 1.35em;
    font-weight: 600;
  }

  .about-sub {
    color: var(--text-weak);
    margin-top: 2px;
  }

  .about-desc {
    margin: 4px 0 0;
    line-height: 1.65;
  }

  .about-rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 2px;
  }

  .about-row {
    display: flex;
    gap: 12px;
    font-size: 0.95em;
  }

  .about-row span {
    color: var(--text-weak);
    flex: none;
    width: 4.5em;
  }
</style>
