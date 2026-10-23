const transparency = document.querySelector('#transparency')
const opacity = document.querySelector('#opacity')
const blur = document.querySelector('#blur')
const status = document.querySelector('#status')

function updateAppearance() {
  const alpha = transparency.checked ? Number(opacity.value) / 100 : 1
  document.documentElement.style.setProperty('--panel-alpha', String(alpha))
  document.documentElement.style.setProperty('--panel-blur', transparency.checked && blur.checked ? 'blur(16px)' : 'none')
  opacity.disabled = !transparency.checked
  blur.disabled = !transparency.checked
  document.querySelector('#opacity-value').value = `${Math.round(alpha * 100)}%`
}

const { getCurrentWindow, Effect } = window.__TAURI__.window
const appWindow = getCurrentWindow()
const nativeBlurSupported = navigator.platform.startsWith('Win')
let effectQueue = Promise.resolve()

function updateNativeEffect() {
  const enabled = transparency.checked && blur.checked
  status.dataset.effect = 'pending'
  // Serialize native calls so a rapid on/off sequence cannot leave an older effect applied.
  effectQueue = effectQueue.then(async () => {
    if (nativeBlurSupported) {
      if (enabled) await appWindow.setEffects({ effects: [Effect.Acrylic] })
      else await appWindow.clearEffects()
    }
    status.dataset.effect = enabled ? (nativeBlurSupported ? 'on' : 'unsupported') : 'off'
    status.textContent = enabled
      ? (nativeBlurSupported ? '已请求 Windows 原生背景模糊' : '当前平台未启用原生模糊，仅应用 CSS 效果')
      : '透明层已就绪'
  }).catch((error) => {
    status.dataset.effect = 'error'
    status.textContent = `背景效果设置失败：${String(error)}`
  })
}

for (const input of [transparency, opacity, blur]) input.addEventListener('input', updateAppearance)
for (const input of [transparency, blur]) input.addEventListener('input', updateNativeEffect)
updateAppearance()
updateNativeEffect()

for (const [id, action] of [['minimize', () => appWindow.minimize()], ['close', () => appWindow.close()]]) {
  document.querySelector(`#${id}`).addEventListener('click', () => {
    action().catch((error) => { status.textContent = `窗口操作失败：${String(error)}` })
  })
}
