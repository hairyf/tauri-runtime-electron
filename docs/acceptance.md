# 官方案例与多平台 CI 验收

CI 验证提交：`26c17a9008edfcb83b0679cb50224b5eb5c7523b`。本报告记录已有证据和限制，不把 CI 绿灯解释为全部能力覆盖。

[GitHub Actions 39858533463](https://github.com/hairyf/tauri-runtime-electron/actions/runs/39858533463) 已完成，五个作业全部成功，工作区和两个源码子仓库均无未提交修改。

| 作业 | 最终结果 | 实际执行范围 |
| --- | --- | --- |
| Windows | 成功 | Rust 检查、测试、构建，真实窗口 66 项检查，六个官方案例功能断言 |
| macOS | 成功 | Rust 检查、测试、构建，真实窗口 59 项检查，六个官方案例功能断言 |
| Linux | 成功 | Rust 检查、测试、构建，xvfb + openbox 下真实窗口 52 项检查，六个官方案例功能断言 |
| Node regressions | 成功 | 原生产协议/认证 IPC、原生子视图布局与分派脚本回归 |
| coverage | 成功 | 实现与生成的能力清单一致 |

三平台重复执行的六个官方案例是 `helloworld`、`state`、`resources`、`commands`、`isolation`、`streaming`。每个平台都上传了日志、断言和原始截图；远端截图的自动结果仍明确标记视觉审阅待完成。

- [Windows 证据](https://github.com/hairyf/tauri-runtime-electron/actions/runs/39858533463/artifacts/12461960072)
- [macOS 证据](https://github.com/hairyf/tauri-runtime-electron/actions/runs/39858533463/artifacts/12461550812)
- [Linux 证据](https://github.com/hairyf/tauri-runtime-electron/actions/runs/39858533463/artifacts/12460214796)

## 本地十三例结果

下面“断言通过”只覆盖该次结果列明的操作；完整功能、原生外观、OS 安装集成和截图目视结论分别记录。原图保存在各例目录，视觉记录绑定当前结果和原图 SHA-256，旧轮次记录不复用。汇总校验见 [视觉审阅索引](visual-ledger.json)。

本地结果来自各例记录的不同开发轮次，其中部分早于本次最终提交；当时没有记录源码提交哈希，不能补推它们都属于 `26c17a9`。该提交的三平台 CI 直接证明上面六例及 smoke 检查的结果，其余本地案例不冒充本次 CI 的测试范围。

汇总索引校验通过：13 例、141 张原图均已逐张打开目视，结果及图片哈希全部匹配，没有沿用失效绑定或遗漏列出的图片。功能断言为 11 例通过、1 例失败、1 例阻塞。视觉 `partial` 保留具体证据不足或可见问题，不把正常视口裁切自动认定为运行时缺陷，也不把自动断言通过改写成全部功能已验收。

| 案例 | 最新功能断言 | 目视结论（限已列原图） | 验证内容与明确限制 | 原图数量 |
| --- | --- | --- | --- | ---: |
| [helloworld](helloworld/result.json) | 通过 | [可见呈现通过](helloworld/visual-review.json) | 原表单提交 World，显示精确 Rust 问候结果 | 2 |
| [state](state/result.json) | 通过 | [可见呈现通过](state/visual-review.json) | 加、减、重置，真正页面重载后 Rust 状态仍保留 | 4 |
| [resources](resources/result.json) | 通过 | [可见呈现通过](resources/visual-review.json) | 解析资源路径，读取与源码逐字节相同的内容，重载后仍正常 | 2 |
| [commands](commands/result.json) | 通过 | [部分：29 通过，1 裁切](commands/visual-review.json) | 原页面 29 个按钮、正常和错误返回、异步命令与原始字节补充断言；raw_request 帧底部控件不完整，响应文本完整可读 | 30 |
| [isolation](isolation/result.json) | 通过 | [可见呈现通过](isolation/visual-review.json) | 保留原隔离 iframe/hook、安全配置，实际加密调用 ping 到 Rust 并返回；隐藏安全机制由功能断言验证 | 2 |
| [streaming](streaming/result.json) | 通过 | [可见呈现通过](streaming/visual-review.json) | 真实 MP4 解码、播放时间推进、暂停与 seek；范围请求边界由独立协议回归验证，未放宽 CSP | 3 |
| [multiwindow](multiwindow/result.json) | 通过 | [部分：3 通过，3 不完整](multiwindow/visual-review.json) | 三个原生窗口、目标消息及回复、原表单创建第四窗、正常关闭；部分收信区不在视口内，不能凭这些图片直接验收消息全文 | 6 |
| [multiwebview](multiwebview/result.json) | 通过 | [布局通过，页面内容部分](multiwebview/visual-review.json) | 四个原生子视图、原远程页面、分区布局及缩小后布局可见；远程正文部分超视口，未认证站内交互和实时网络状态 | 6 |
| [splashscreen](splashscreen/result.json) | 通过 | [可见呈现通过](splashscreen/visual-review.json) | 原两秒定时器关闭启动画面，显示原先隐藏的同一个主窗；生命周期由独立原生状态观察验证 | 2 |
| [run-return](run-return/result.json) | 通过 | [退出前页面通过](run-return/visual-review.json) | 真正 WM_CLOSE，最后窗口正常退出，run_return 后标记恰好一次、进程返回 0；退出结果由进程和日志断言验证 | 1 |
| [file-associations](file-associations/result.json) | 通过，范围有限 | [路径显示通过](file-associations/visual-review.json) | 中文/空格路径和 file URL 的参数传递、显示、重载；没有证明安装器注册和系统双击文件关联 | 2 |
| [drag](drag/result.json) | 阻塞 | [部分，全部 48 帧已审](drag/visual-review.json) | 23 条真鼠标拖动/禁止拖动断言通过；跟随窗口裁图不单独证明桌面位移；真双击后最大化，后续截图被顶层窗口遮挡，恢复未验收 | 48 |
| [api](api/result.json) | 失败，部分功能通过 | [部分：13 可见项通过，20 不完整](api/visual-review.json) | 21 条检查通过，原生菜单命令回调仍失败；Auto 主题像素/状态不一致；托盘 OS 操作未认证，缺少设备使 WebRTC 阻塞 | 33 |

## 尚未通过的事项

1. **API 原生菜单回调。** 原 Sortable 拖放可插入 Normal/Check 行，菜单句柄中的条目和状态可读取，但实际命令没有进入 Rust Console 回调，等待超时。Tauri/muda 的 Windows 子类回调不能跨 Electron 与 Rust 两个进程直接安装，返回 HWND 本身不能解决这一点。没有伪造菜单回调来满足测试。
2. **拖动案例最后的双击/恢复验收。** `native-observations.json` 记录了 `maximized: true` 和实际窗口尺寸变化；后续原生截图因重叠窗口被拒绝。23 条拖动断言不能代替未完成的恢复流程。
3. **API 截图中 Auto 主题与状态、子窗元数据的稳定时刻。** 视觉审阅发现 Auto 截图的暗色内容与采集的系统亮色状态存在不一致；子窗 Welcome 截图尚显示默认元数据。它们只支持部分可见结论，不能按旧图片或最终 getter 补记通过。
4. **托盘与原生菜单的真实 OS 操作。** Create/Remove 按钮变化不证明通知区图标、提示、左右点击、菜单弹出与选择均正确。
5. **WebRTC 真设备。** 本机返回 `NotFoundError: Requested device not found`，未伪造设备或录制私人媒体。原示例的流清理逻辑也需要在有真实设备时独立验收。
6. **文件关联的安装态集成。** 开发命令的 argv 测试没有安装注册表/桌面关联，系统双击路径仍需安装包环境。
7. **其他原生效果与平台限制。** 菜单加速键、原生效果、部分光标能力、Linux/Wayland、GTK/native webview 互操作等仍按 README 和能力清单列出的限制处理。六例三平台 CI 不能代替这些能力的专门验证。

## 重复运行

在仓库根目录，已初始化源码子仓库并具备 Node.js 24+、pnpm、Rust 和系统依赖后：

```bash
pnpm example api
pnpm example <name>
pnpm check helloworld state resources commands isolation streaming
pnpm check
```

运行器自动复制和准备案例；原源码子仓库只读。所有案例共享临时工作区，按顺序执行。`pnpm check` 尝试全部十三例，失败和阻塞均返回非零，不能期待在上述限制仍存在时全绿。

原功能结果文件中的 `visualReview: pending` 保持原样；实际逐张目视记录位于同目录的 `visual-review.json`，分片审阅也保留。最终 [视觉审阅索引](visual-ledger.json) 校验每例结果哈希、原图顺序/数量与全部原图 SHA-256；索引的完整性检查不代替视觉结论或功能断言。
