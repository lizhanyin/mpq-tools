/* 提示窗（toast）：naive-ui 离散消息 API，让 store 等组件树之外的代码
 * 也能弹出提示。错误停留更久，方便看清原因。 */

import { createDiscreteApi } from 'naive-ui'

const ACCENT = '#2f6fe0'

const { message } = createDiscreteApi(['message'], {
  configProviderProps: {
    themeOverrides: {
      common: {
        primaryColor: ACCENT,
        primaryColorSuppl: ACCENT,
      },
    },
  },
})

export const toast = {
  info: (content) => message.info(content),
  success: (content) => message.success(content),
  error: (content) => message.error(content, { duration: 6000 }),
}
