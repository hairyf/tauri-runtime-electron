// Exposes the `window.ipc.postMessage` interface Tauri's generated IPC script
// expects (the same bridge wry installs). Runs in every page this runtime hosts.
'use strict'

const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('ipc', {
  postMessage(data) {
    ipcRenderer.send('tauri-ipc', String(data))
  },
})
