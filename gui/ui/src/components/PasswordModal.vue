<template>
  <n-modal
    :show="!!S.pw"
    preset="card"
    :title="TITLES[mode]"
    :style="{ width: '460px', maxWidth: '94vw', borderRadius: '12px' }"
    :mask-closable="false"
    :bordered="false"
    @update:show="(v) => { if (!v) closePw() }"
  >
    <template v-if="p">
      <div class="dialog-body">
        <div class="dialog-meta">
          <span class="meta-label">归档</span>
          <span class="meta-value">{{ S.openPath || '' }}</span>
          <n-tag v-if="mode !== 'set'" size="small" round :bordered="false" type="warning">
            🔒 已保护
          </n-tag>
        </div>

        <!-- 解锁 / 设置 / 解除：一个密码输入 -->
        <template v-if="mode !== 'change'">
          <div class="field-label">
            {{ mode === 'set' ? '密码' : mode === 'remove' ? '当前密码' : '密码' }}
          </div>
          <n-input
            v-model:value="p.password"
            type="password"
            show-password-on="click"
            placeholder="输入密码"
            :disabled="p.busy"
            @keyup.enter="mode === 'unlock' ? submitUnlock() : confirmPwOp()"
          />
          <template v-if="mode === 'set'">
            <div class="field-label">确认密码</div>
            <n-input
              v-model:value="p.confirm"
              type="password"
              show-password-on="click"
              placeholder="再输入一次"
              :disabled="p.busy"
              @keyup.enter="confirmPwOp()"
            />
          </template>
        </template>

        <!-- 修改密码：当前 + 新 + 确认 -->
        <template v-else>
          <div class="field-label">当前密码</div>
          <n-input
            v-model:value="p.password"
            type="password"
            show-password-on="click"
            placeholder="输入当前密码"
            :disabled="p.busy"
          />
          <div class="field-label">新密码</div>
          <n-input
            v-model:value="p.newPassword"
            type="password"
            show-password-on="click"
            placeholder="输入新密码"
            :disabled="p.busy"
          />
          <div class="field-label">确认新密码</div>
          <n-input
            v-model:value="p.confirm"
            type="password"
            show-password-on="click"
            placeholder="再输入一次新密码"
            :disabled="p.busy"
            @keyup.enter="confirmPwOp()"
          />
        </template>

        <ul v-if="hints.length" class="pw-hints">
          <li v-for="(h, i) in hints" :key="i">{{ h }}</li>
        </ul>
      </div>
    </template>

    <template v-if="p" #action>
      <!-- 解锁模式：单个确认按钮 -->
      <div v-if="mode === 'unlock'" class="dialog-actions">
        <n-button size="small" quaternary @click="closePw">取消</n-button>
        <div class="grow"></div>
        <n-button size="small" type="primary" :disabled="!p.password || p.busy" :loading="p.busy"
                  @click="submitUnlock">解锁</n-button>
      </div>
      <!-- set / change / remove：先选保存位置，应用后写盘并重新载入 -->
      <div v-else class="dialog-actions">
        <n-button size="small" quaternary @click="closePw">取消</n-button>
        <div class="grow"></div>
        <n-button size="small" type="primary" :disabled="!canSubmit || p.busy"
                  :loading="p.busy" @click="confirmPwOp">应用并保存…</n-button>
      </div>
    </template>
  </n-modal>
</template>

<style scoped>
.pw-hints {
  margin: 10px 0 0;
  padding-left: 18px;
  color: var(--n-text-color-3, #888);
  font-size: 12px;
  line-height: 1.7;
}
</style>

<script setup>
  import { computed } from 'vue'
  import { NModal, NButton, NInput, NTag } from 'naive-ui'
  import {
    S, submitUnlock, confirmPwOp, closePw,
  } from '../store'

  const TITLES = {
    unlock: '输入密码',
    set: '设置密码保护',
    change: '修改密码',
    remove: '解除密码保护',
  }

  const p = computed(() => S.pw)
  const mode = computed(() => S.pw?.mode || 'unlock')

  /** 各模式的操作说明 / 警示。 */
  const hints = computed(() => {
    switch (mode.value) {
      case 'unlock':
        return []
      case 'set':
        return [
          '容器将转换为 v4；修改密码时无需重新加密数据。',
          '保存后会在归档旁生成「同名 .txt」以明文记录密码，便于备忘。',
        ]
      case 'change':
        return [
          '仅重新包装主密钥，数据不会被重新加密，速度与归档大小无关。',
          '保存后会更新归档旁的「同名 .txt」密码记录。',
        ]
      case 'remove':
        return ['所有文件将被解密并恢复为普通（无密码）归档。']
      default:
        return []
    }
  })

  const canSubmit = computed(() => {
    const v = S.pw
    if (!v) return false
    if (v.mode === 'set') return !!v.password && v.password === v.confirm
    if (v.mode === 'change') return !!v.password && !!v.newPassword && v.newPassword === v.confirm
    return !!v.password
  })
</script>
