/**
 * 命令表的类型定义。
 *
 * 放在 socket 层而不是 commands.ts，是为了让 transport 只依赖这两个类型：
 * socket 不认识 Window / Webview，可以用假 IPC 单测。
 *
 * 入参形态由线协议决定，只能宽松处理。
 */
export type Params = Record<string, any>

export type Command = (params: Params) => unknown

export type CommandTable = Record<string, Command>
