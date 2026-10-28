import { getCurrentWindow, Effect } from './window-api.js'

const transparency = document.querySelector('#transparency')
const opacity = document.querySelector('#opacity')
const blur = document.querySelector('#blur')
const reset = document.querySelector('#reset')
const note = document.querySelector('#blur-note')
const status = document.querySelector('#status')
const appWindow = getCurrentWindow()
let effectsQueue = Promise.resolve()
let pending = 0
let supported = false
let enabled = false

function updateAppearance() {
  const alpha = transparency.checked ? Number(opacity.value) / 100 : 1
  document.documentElement.style.setProperty('--panel-alpha', String(alpha))
  opacity.disabled = !transparency.checked
  document.querySelector('#opacity-value').value = `${Math.round(alpha * 100)}%`
}

function serialize(action) {
  pending++
  blur.disabled = reset.disabled = true
  blur.setAttribute('aria-busy', 'true')
  status.textContent = '正在更新系统背景效果…'
  effectsQueue = effectsQueue.then(action).catch((error) => {
    blur.checked = enabled
    note.textContent = `系统背景效果不可用或操作失败：${String(error)}`
    status.textContent = note.textContent
  }).finally(() => {
    pending--
    blur.disabled = pending > 0 || !supported
    reset.disabled = pending > 0
    blur.setAttribute('aria-busy', String(pending > 0))
  })
  return effectsQueue
}

async function setBlur(value) {
  if (value) await appWindow.setEffects({ effects: [Effect.Acrylic] })
  else await appWindow.clearEffects()
  enabled = value
  blur.checked = value
  status.textContent = value ? '系统背景模糊已启用（Acrylic）' : '透明层已就绪'
}

for (const input of [transparency, opacity]) input.addEventListener('input', updateAppearance)
updateAppearance()

// A reload must really clear the previous native effect, not merely uncheck its UI.
serialize(async () => {
  await setBlur(false)
  try {
    await setBlur(true)
  } finally {
    await setBlur(false)
  }
  supported = true
  note.textContent = 'Acrylic 使用真实系统背景效果（Windows 11 22621+）；不透明度滑块只调整面板背景。'
})

blur.addEventListener('change', () => {
  const requested = blur.checked
  serialize(() => setBlur(requested))
})
reset.addEventListener('click', () => serialize(async () => {
  await setBlur(false)
  transparency.checked = true
  opacity.value = '72'
  updateAppearance()
}))

for (const [id, action] of [['minimize', () => appWindow.minimize()], ['close', () => appWindow.close()]]) {
  document.querySelector(`#${id}`).addEventListener('click', () => {
    action().catch((error) => { status.textContent = `窗口操作失败：${String(error)}` })
  })
}
