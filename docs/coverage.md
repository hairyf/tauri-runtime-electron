# 桌面运行时逐成员验收

由 `node tests/checks/coverage.mjs --write` 生成。用途说明官方契约；**源码连线、编译、负测和 setter 返回 Ok 均不算功能通过。**

## 口径与当前结果

- 分母：上游六个公开 trait 的桌面可调用成员，共 **192** 项；关联类型、空标记 trait、纯移动端 7 项不计。Windows/macOS 同名重载按一个成员、各自平台验证。
- 平台 N/A 只依据上游 cfg 或明确官方平台限制；本库不支持、跳过、失败、无证据都不得移出分母。devtools 特性计入。
- 逐平台通过只接收同一提交、同次 CI attempt、真实 Electron suite 成功后的显式行为断言；所有适用平台均通过才算完整成员。扩展另计，不冒充官方 API。
- 未导入可追溯的逐成员运行证据，因此各平台保持未验证；旧 CI 总检查数不能反推逐成员通过。

| 范围 | 真实行为通过 / 分母 |
| --- | --- |
| windows | 0/169（0.0%） |
| macos | 0/181（0.0%） |
| linux | 0/158（0.0%） |
| 全部适用平台 | 0/192（0.0%） |

80% 完整成员门槛至少 154 项，90% 至少 173 项；未知或跳过仍留在分母。完整 marker 协议与校验器见 [tests/checks/runtime-evidence.mjs](<../tests/checks/runtime-evidence.mjs>)。

## 已知边界（不由单个成员通过自动消除）

- **原生弹窗**：官方 PendingWebview::new_window_handler 需要同进程原生 opener；Electron 回调 Allow/Create/Deny 是独立扩展，必须分别验收。
- **创建属性**：proxy_url、additional_browser_args、browser_extensions_enabled / extensions_path 尚需等价行为验证。
- **原生互操作**：with_webview、GTK、AppKit、Windows 菜单代理 HWND 的限制保留；错误返回的负测不证明原生功能可用。
- **测试范围**：菜单/托盘真实 OS 操作、安装态文件关联、截图视觉结论仍需专项证据，见 [docs/acceptance.md](<../docs/acceptance.md>)；[scripts/example.mjs](<../scripts/example.mjs>) 不得删除官方 setup 回调绕过失败。

## WindowDispatch（79）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`run_on_main_thread`](<../src/window.rs#L145>) | 将任务交给运行时主线程执行 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L683>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1847>) |
| [`on_window_event`](<../src/window.rs#L150>) | 注册窗口移动、缩放、关闭等事件回调 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L686>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1851>) |
| [`scale_factor`](<../src/window.rs#L156>) | 获取逻辑像素与物理像素的缩放比例 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L691>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1862>) |
| [`inner_position`](<../src/window.rs#L160>) | 获取内容区左上角的桌面坐标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L694>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1866>) |
| [`outer_position`](<../src/window.rs#L164>) | 获取含边框窗口左上角的桌面坐标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L697>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1870>) |
| [`inner_size`](<../src/window.rs#L168>) | 获取内容区尺寸，不含标题栏和边框 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L702>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1874>) |
| [`outer_size`](<../src/window.rs#L172>) | 获取整个窗口尺寸，包含标题栏和边框 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L707>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1878>) |
| [`is_fullscreen`](<../src/window.rs#L176>) | 查询窗口是否处于全屏状态 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L710>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1882>) |
| [`is_minimized`](<../src/window.rs#L180>) | 查询窗口是否已最小化 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L713>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1886>) |
| [`is_maximized`](<../src/window.rs#L184>) | 查询窗口是否已最大化 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L716>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1890>) |
| [`is_focused`](<../src/window.rs#L191>) | 查询窗口是否拥有输入焦点 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L719>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1894>) |
| [`is_decorated`](<../src/window.rs#L195>) | 查询是否启用系统标题栏和边框 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L722>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1899>) |
| [`is_resizable`](<../src/window.rs#L203>) | 查询是否允许用户调整窗口大小 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L725>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1904>) |
| [`is_maximizable`](<../src/window.rs#L207>) | 查询原生最大化按钮是否可用 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持最大化按钮查询 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L732>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1909>) |
| [`is_minimizable`](<../src/window.rs#L211>) | 查询原生最小化按钮是否可用 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持最小化按钮查询 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L739>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1914>) |
| [`is_closable`](<../src/window.rs#L215>) | 查询原生关闭按钮是否可用 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L746>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1919>) |
| [`is_visible`](<../src/window.rs#L223>) | 查询窗口当前是否可见 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L749>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1923>) |
| [`is_enabled`](<../src/window.rs#L703>) | 查询窗口是否允许用户交互 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L752>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1958>) |
| [`is_always_on_top`](<../src/window.rs#L711>) | 查询窗口是否置顶 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L759>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1962>) |
| [`title`](<../src/window.rs#L227>) | 获取窗口标题 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L762>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1927>) |
| [`current_monitor`](<../src/window.rs#L236>) | 获取窗口当前所在显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L767>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1931>) |
| [`primary_monitor`](<../src/window.rs#L242>) | 获取系统主显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L772>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1935>) |
| [`monitor_from_point`](<../src/window.rs#L247>) | 查找包含指定桌面坐标的显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L775>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1939>) |
| [`available_monitors`](<../src/window.rs#L253>) | 列出系统所有可用显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L778>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1950>) |
| [`gtk_window`](<../src/window.rs#L278>) | 获取 Linux/BSD 的本地 GTK 窗口对象 | N/A：上游 cfg 不在此平台提供 | N/A：上游 cfg 不在此平台提供 | 未验证 | 无本地 GTK 窗口；返回错误不算功能通过 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L788>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1973>) |
| [`default_vbox`](<../src/window.rs#L289>) | 获取 GTK 窗口默认的垂直布局容器 | N/A：上游 cfg 不在此平台提供 | N/A：上游 cfg 不在此平台提供 | 未验证 | 无本地 GTK 控件树；返回错误不算功能通过 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L798>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1984>) |
| [`window_handle`](<../src/window.rs#L303>) | 获取用于原生互操作的窗口句柄 | 未验证 | 未验证 | 未验证 | Windows 为菜单代理 HWND；macOS 不可用；Linux 为 X11 ID [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L809>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2000>) |
| [`theme`](<../src/window.rs#L263>) | 获取当前窗口的明暗主题 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L814>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1954>) |
| [`center`](<../src/window.rs#L337>) | 将窗口居中 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L819>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2010>) |
| [`request_user_attention`](<../src/window.rs#L341>) | 请求或取消窗口闪烁等用户提醒 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L824>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2016>) |
| [`create_window`](<../src/window.rs#L346>) | 创建原生窗口，可附带初始 webview | 未验证 | 未验证 | 未验证 | 官方 new_window_handler 仍需进程内原生 opener；扩展回调另表 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L827>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2025>) |
| [`create_webview`](<../src/window.rs#L354>) | 在指定窗口中创建 webview | 未验证 | 未验证 | 未验证 | 官方 new_window_handler 不能以 Electron 扩展通过替代 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L834>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2035>) |
| [`set_resizable`](<../src/window.rs#L361>) | 设置是否允许用户调整窗口大小 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L840>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2042>) |
| [`set_enabled`](<../src/window.rs#L699>) | 启用或禁用窗口交互 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L847>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2049>) |
| [`set_maximizable`](<../src/window.rs#L365>) | 启用或禁用原生最大化按钮 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持设置最大化按钮 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L855>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2056>) |
| [`set_minimizable`](<../src/window.rs#L369>) | 启用或禁用原生最小化按钮 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持设置最小化按钮 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L862>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2063>) |
| [`set_closable`](<../src/window.rs#L373>) | 启用或禁用原生关闭按钮 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L871>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2070>) |
| [`set_title`](<../src/window.rs#L380>) | 修改窗口标题 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L874>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2077>) |
| [`maximize`](<../src/window.rs#L384>) | 将窗口最大化 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L877>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2084>) |
| [`unmaximize`](<../src/window.rs#L391>) | 将窗口从最大化恢复 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L880>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2090>) |
| [`minimize`](<../src/window.rs#L398>) | 将窗口最小化 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L883>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2096>) |
| [`unminimize`](<../src/window.rs#L402>) | 将窗口从最小化恢复 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L886>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2102>) |
| [`show`](<../src/window.rs#L406>) | 显示窗口 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L889>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2108>) |
| [`hide`](<../src/window.rs#L410>) | 隐藏窗口 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L892>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2114>) |
| [`close`](<../src/window.rs#L414>) | 请求正常关闭，可由关闭事件否决 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L895>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2120>) |
| [`destroy`](<../src/window.rs#L422>) | 直接销毁窗口，不走关闭请求确认 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L898>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2129>) |
| [`set_decorations`](<../src/window.rs#L430>) | 显示或隐藏系统标题栏和边框 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L901>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2138>) |
| [`set_shadow`](<../src/window.rs#L458>) | 启用或禁用窗口阴影 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L904>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2145>) |
| [`set_always_on_bottom`](<../src/window.rs#L464>) | 设置窗口是否始终位于其他窗口下方 | 未验证 | 未验证 | 未验证 | 非 Windows 无操作；Windows 仅 true 单次置底 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L907>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2152>) |
| [`set_always_on_top`](<../src/window.rs#L474>) | 设置窗口是否始终位于其他窗口上方 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L910>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2159>) |
| [`set_visible_on_all_workspaces`](<../src/window.rs#L486>) | 设置窗口是否显示在所有虚拟桌面 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L913>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2166>) |
| [`set_background_color`](<../src/window.rs#L718>) | 设置窗口或 webview 的背景色 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L916>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2363>) |
| [`set_content_protected`](<../src/window.rs#L493>) | 请求阻止其他应用截取窗口内容 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L919>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2173>) |
| [`set_size`](<../src/window.rs#L497>) | 调整窗口内容区或 webview 的尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L922>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2180>) |
| [`set_min_size`](<../src/window.rs#L507>) | 设置或清除窗口内容区最小尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L925>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2187>) |
| [`set_max_size`](<../src/window.rs#L511>) | 设置或清除窗口内容区最大尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L928>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2194>) |
| [`set_size_constraints`](<../src/window.rs#L684>) | 设置各轴的最小和最大尺寸约束 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L931>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2201>) |
| [`set_position`](<../src/window.rs#L515>) | 移动窗口或窗口内的 webview | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L934>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2208>) |
| [`set_fullscreen`](<../src/window.rs#L528>) | 进入或退出全屏 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L937>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2222>) |
| [`set_fullscreen_on_monitor`](<../src/window.rs#L532>) | 在指定桌面坐标所在的显示器全屏 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L942>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2215>) |
| [`set_simple_fullscreen`](<../src/window.rs#L540>) | 切换 macOS 不创建独立空间的简单全屏 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L945>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2230>) |
| [`set_focus`](<../src/window.rs#L544>) | 将窗口置前并获取输入焦点 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L948>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2237>) |
| [`set_focusable`](<../src/window.rs#L548>) | 设置窗口是否可以获取输入焦点 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L951>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2243>) |
| [`set_icon`](<../src/window.rs#L552>) | 修改窗口图标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L954>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2250>) |
| [`set_skip_taskbar`](<../src/window.rs#L556>) | 设置是否隐藏任务栏中的窗口图标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L957>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2257>) |
| [`set_cursor_grab`](<../src/window.rs#L560>) | 将鼠标指针限制在窗口范围内 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L963>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2264>) |
| [`set_cursor_visible`](<../src/window.rs#L571>) | 显示或隐藏窗口中的鼠标指针 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L968>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2271>) |
| [`set_cursor_icon`](<../src/window.rs#L575>) | 修改窗口中的鼠标指针样式 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L971>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2278>) |
| [`set_cursor_position`](<../src/window.rs#L579>) | 以窗口坐标设置鼠标指针位置 | 未验证 | 未验证 | 未验证 | Linux 仅警告，不得改为平台 N/A [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L974>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2285>) |
| [`set_ignore_cursor_events`](<../src/window.rs#L599>) | 设置鼠标事件是否穿透窗口 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L977>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2292>) |
| [`start_dragging`](<../src/window.rs#L604>) | 启动由系统管理的窗口拖动 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L980>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2299>) |
| [`start_resize_dragging`](<../src/window.rs#L619>) | 从指定边缘或角启动窗口缩放拖动 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L983>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2305>) |
| [`set_badge_count`](<../src/window.rs#L653>) | 设置或清除应用图标上的数字角标 | N/A：官方契约：Windows 不支持数字角标 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L993>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2312>) |
| [`set_badge_label`](<../src/window.rs#L659>) | 设置或清除 macOS Dock 图标角标文字 | N/A：官方契约：仅 macOS | 未验证 | N/A：官方契约：仅 macOS |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L996>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2319>) |
| [`set_overlay_icon`](<../src/window.rs#L663>) | 设置或清除 Windows 任务栏叠加图标 | 未验证 | N/A：官方契约：仅 Windows | N/A：官方契约：仅 Windows |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L1001>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2326>) |
| [`set_progress_bar`](<../src/window.rs#L635>) | 设置任务栏或 Dock 的进度与状态 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L1009>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2335>) |
| [`set_title_bar_style`](<../src/window.rs#L670>) | 修改 macOS 标题栏样式 | N/A：官方契约：仅 macOS | 未验证 | N/A：官方契约：仅 macOS |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L1016>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2342>) |
| [`set_traffic_light_position`](<../src/window.rs#L680>) | 移动 macOS 标题栏红黄绿按钮 | N/A：官方契约：仅 macOS | 未验证 | N/A：官方契约：仅 macOS |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L1025>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2349>) |
| [`set_theme`](<../src/window.rs#L690>) | 设置明暗主题，或跟随系统 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L1033>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2356>) |

## WebviewDispatch（31）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`run_on_main_thread`](<../src/webview.rs#L67>) | 将任务交给运行时主线程执行 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L550>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1538>) |
| [`on_webview_event`](<../src/webview.rs#L74>) | 注册 webview 事件回调 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L553>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1542>) |
| [`with_webview`](<../src/webview.rs#L85>) | 在回调中访问平台原生 webview 对象 | 未验证 | 未验证 | 未验证 | 报错且不调用回调，不算功能通过 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L556>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1552>) |
| [`open_devtools`](<../src/webview.rs#L99>) | 打开 webview 开发者工具 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L560>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1561>) |
| [`close_devtools`](<../src/webview.rs#L105>) | 关闭 webview 开发者工具 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L564>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1570>) |
| [`is_devtools_open`](<../src/webview.rs#L111>) | 查询开发者工具是否已打开 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L568>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1580>) |
| [`url`](<../src/webview.rs#L169>) | 获取 webview 当前页面 URL | 未验证 | 未验证 | 未验证 | 查询 Electron 当前已提交 URL，页面自行导航后也需一致 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L573>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1586>) |
| [`bounds`](<../src/webview.rs#L173>) | 获取 webview 在窗口内的位置和尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L576>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1590>) |
| [`position`](<../src/webview.rs#L177>) | 获取 webview 相对窗口内容区的位置 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L579>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1594>) |
| [`size`](<../src/webview.rs#L181>) | 获取 webview 内容区的物理像素尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L582>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1598>) |
| [`navigate`](<../src/webview.rs#L185>) | 让 webview 导航到指定 URL | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L587>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1604>) |
| [`reload`](<../src/webview.rs#L191>) | 重新加载 webview 当前页面 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L590>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1612>) |
| [`print`](<../src/webview.rs#L195>) | 打开当前网页的打印对话框 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L593>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1620>) |
| [`close`](<../src/webview.rs#L199>) | 关闭 webview 原生视图，保留所属宿主窗口 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L596>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1628>) |
| [`set_bounds`](<../src/webview.rs#L204>) | 同时设置 webview 的位置和尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L599>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1636>) |
| [`set_size`](<../src/webview.rs#L211>) | 调整窗口内容区或 webview 的尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L602>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1644>) |
| [`set_position`](<../src/webview.rs#L215>) | 移动窗口或窗口内的 webview | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L605>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1652>) |
| [`set_focus`](<../src/webview.rs#L219>) | 将所属窗口置前并聚焦 webview | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L608>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1660>) |
| [`hide`](<../src/webview.rs#L280>) | 隐藏 webview 内容 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L611>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1801>) |
| [`show`](<../src/webview.rs#L286>) | 显示 webview 内容 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L614>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1809>) |
| [`eval_script`](<../src/webview.rs#L121>) | 执行 JavaScript，不接收返回值 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L617>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1725>) |
| [`eval_script_with_callback`](<../src/webview.rs#L142>) | 执行 JavaScript，并回调 JSON 序列化结果 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L623>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1749>) |
| [`reparent`](<../src/webview.rs#L223>) | 将 webview 移到另一个窗口 | 未验证 | 未验证 | 未验证 | 迁移原生视图及 Rust 所属关系；需保留页面状态、IPC 和父窗销毁语义 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L630>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1668>) |
| [`cookies_for_url`](<../src/webview.rs#L232>) | 读取适用于指定 URL 的 Cookie | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L637>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1683>) |
| [`cookies`](<../src/webview.rs#L227>) | 读取当前会话的全部 Cookie | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L645>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1694>) |
| [`set_cookie`](<../src/webview.rs#L237>) | 向当前会话写入 Cookie | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L652>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1698>) |
| [`delete_cookie`](<../src/webview.rs#L256>) | 从当前会话删除指定 Cookie | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L659>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1707>) |
| [`set_auto_resize`](<../src/webview.rs#L268>) | 设置 webview 是否随父窗口自动调整布局 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L662>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1716>) |
| [`set_zoom`](<../src/webview.rs#L115>) | 设置网页缩放倍数 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L665>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1785>) |
| [`set_background_color`](<../src/webview.rs#L292>) | 设置窗口或 webview 的背景色 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L668>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1817>) |
| [`clear_all_browsing_data`](<../src/webview.rs#L274>) | 清除 webview 会话的浏览数据 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L671>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1793>) |

## Runtime（20）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`new`](<../src/runtime.rs#L944>) | 在主线程创建并初始化运行时 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L429>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2800>) |
| [`new_any_thread`](<../src/runtime.rs#L956>) | 在允许的平台上从任意线程创建运行时 | 未验证 | N/A：上游 cfg 不在此平台提供 | 未验证 | 必须在非主线程实际创建，new 的成功不能代替 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L451>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2810>) |
| [`create_proxy`](<../src/runtime.rs#L960>) | 创建向事件循环发送用户事件的代理 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L454>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2825>) |
| [`handle`](<../src/runtime.rs#L966>) | 获取可跨线程调用的运行时句柄 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L457>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2829>) |
| [`create_window`](<../src/runtime.rs#L972>) | 创建原生窗口，可附带初始 webview | 未验证 | 未验证 | 未验证 | 官方 new_window_handler 仍需进程内原生 opener；扩展回调另表 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L460>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2835>) |
| [`create_webview`](<../src/runtime.rs#L980>) | 在指定窗口中创建 webview | 未验证 | 未验证 | 未验证 | 官方 new_window_handler 不能以 Electron 扩展通过替代 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L467>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2898>) |
| [`primary_monitor`](<../src/runtime.rs#L988>) | 获取系统主显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L476>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2961>) |
| [`monitor_from_point`](<../src/runtime.rs#L997>) | 查找包含指定桌面坐标的显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L479>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2970>) |
| [`available_monitors`](<../src/runtime.rs#L1006>) | 列出系统所有可用显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L482>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2979>) |
| [`cursor_position`](<../src/runtime.rs#L1124>) | 获取鼠标指针相对桌面的物理坐标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L485>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2989>) |
| [`set_theme`](<../src/runtime.rs#L1020>) | 设置明暗主题，或跟随系统 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L488>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2998>) |
| [`set_activation_policy`](<../src/runtime.rs#L1039>) | 设置 macOS 应用激活策略 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L493>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3003>) |
| [`set_activate_ignoring_other_apps`](<../src/runtime.rs#L1069>) | 设置 macOS 启动时是否抢占应用焦点 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L503>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3010>) |
| [`set_dock_visibility`](<../src/runtime.rs#L1057>) | 显示或隐藏 macOS Dock 应用图标 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L508>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3015>) |
| [`show`](<../src/runtime.rs#L1082>) | 显示 macOS 应用，不自动聚焦 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L513>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3020>) |
| [`hide`](<../src/runtime.rs#L1090>) | 隐藏整个 macOS 应用 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L518>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3025>) |
| [`set_device_event_filter`](<../src/runtime.rs#L1096>) | 设置未聚焦窗口的底层设备事件过滤策略 | 未验证 | N/A：官方契约：macOS 不支持设备事件过滤 | N/A：官方契约：Linux 不支持设备事件过滤 | 本库仅警告；Windows 仍留在分母 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L531>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3029>) |
| [`run_iteration`](<../src/runtime.rs#L1112>) | 处理一轮事件，然后将控制权交回调用方 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L535>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3036>) |
| [`run_return`](<../src/runtime.rs#L1120>) | 运行事件循环，退出时返回退出码 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L538>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3097>) |
| [`run`](<../src/runtime.rs#L1116>) | 运行事件循环，结束时退出进程 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L541>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L3091>) |

## RuntimeHandle（18）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`create_proxy`](<../src/window_handle.rs#L27>) | 创建向事件循环发送用户事件的代理 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L293>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2579>) |
| [`set_activation_policy`](<../src/window_handle.rs#L35>) | 设置 macOS 应用激活策略 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L298>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2584>) |
| [`set_dock_visibility`](<../src/window_handle.rs#L59>) | 显示或隐藏 macOS Dock 应用图标 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L303>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2591>) |
| [`request_exit`](<../src/window_handle.rs#L72>) | 携带退出码请求结束事件循环 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L306>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2597>) |
| [`create_window`](<../src/window_handle.rs#L77>) | 创建原生窗口，可附带初始 webview | 未验证 | 未验证 | 未验证 | 官方 new_window_handler 仍需进程内原生 opener；扩展回调另表 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L309>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2608>) |
| [`create_webview`](<../src/window_handle.rs#L85>) | 在指定窗口中创建 webview | 未验证 | 未验证 | 未验证 | 官方 new_window_handler 不能以 Electron 扩展通过替代 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L316>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2618>) |
| [`run_on_main_thread`](<../src/window_handle.rs#L94>) | 将任务交给运行时主线程执行 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L323>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2626>) |
| [`display_handle`](<../src/window_handle.rs#L98>) | 获取窗口系统的原生显示连接句柄 | 未验证 | 未验证 | 未验证 | Linux 为 Xlib(None)，不等于可用原生显示连接 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L326>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2630>) |
| [`primary_monitor`](<../src/window_handle.rs#L123>) | 获取系统主显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L333>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2636>) |
| [`monitor_from_point`](<../src/window_handle.rs#L132>) | 查找包含指定桌面坐标的显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L336>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2640>) |
| [`available_monitors`](<../src/window_handle.rs#L145>) | 列出系统所有可用显示器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L339>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2650>) |
| [`cursor_position`](<../src/window_handle.rs#L246>) | 获取鼠标指针相对桌面的物理坐标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L342>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2654>) |
| [`set_theme`](<../src/window_handle.rs#L159>) | 设置明暗主题，或跟随系统 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L345>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2659>) |
| [`show`](<../src/window_handle.rs#L177>) | 显示 macOS 应用，不自动聚焦 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L350>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2668>) |
| [`hide`](<../src/window_handle.rs#L189>) | 隐藏整个 macOS 应用 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L355>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2675>) |
| [`set_device_event_filter`](<../src/window_handle.rs#L198>) | 设置未聚焦窗口的底层设备事件过滤策略 | 未验证 | N/A：官方契约：macOS 不支持设备事件过滤 | N/A：官方契约：Linux 不支持设备事件过滤 | 本库仅警告；Windows 仍留在分母 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L364>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2681>) |
| [`fetch_data_store_identifiers`](<../src/window_handle.rs#L226>) | 枚举 Apple webview 数据存储的标识 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 | 本库报错且不回调 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L385>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2710>) |
| [`remove_data_store`](<../src/window_handle.rs#L236>) | 按标识删除 Apple webview 数据存储 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 | 本库报错且不回调 [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L392>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2720>) |

## EventLoopProxy（1）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`send_event`](<../src/runtime.rs#L686>) | 将自定义用户事件送入运行时事件循环 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/lib.rs#L400>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L2482>) |

## WindowBuilder（43）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`new`](<../src/window_builder.rs#L49>) | 初始化窗口属性构建器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L263>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L772>) |
| [`with_config`](<../src/window_builder.rs#L53>) | 按窗口配置初始化构建器 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L266>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L796>) |
| [`center`](<../src/window_builder.rs#L61>) | 设置新窗口居中 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L270>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L919>) |
| [`position`](<../src/window_builder.rs#L68>) | 设置新窗口的逻辑像素位置 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L274>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L924>) |
| [`inner_size`](<../src/window_builder.rs#L75>) | 设置新窗口的逻辑像素内容区尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L278>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L929>) |
| [`min_inner_size`](<../src/window_builder.rs#L81>) | 设置窗口创建时的最小内容区尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L282>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L934>) |
| [`max_inner_size`](<../src/window_builder.rs#L91>) | 设置窗口创建时的最大内容区尺寸 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L286>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L941>) |
| [`inner_size_constraints`](<../src/window_builder.rs#L101>) | 设置窗口创建时各轴的尺寸约束 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L290>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L948>) |
| [`prevent_overflow`](<../src/window_builder.rs#L119>) | 创建时将窗口限制在显示器工作区内 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L298>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L963>) |
| [`prevent_overflow_with_margin`](<../src/window_builder.rs#L124>) | 创建时保留指定边距并防止窗口越出工作区 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L307>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L976>) |
| [`resizable`](<../src/window_builder.rs#L133>) | 设置窗口创建时是否允许调整大小 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L312>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L981>) |
| [`maximizable`](<../src/window_builder.rs#L138>) | 设置窗口创建时原生最大化按钮是否可用 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持设置最大化按钮 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L322>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L986>) |
| [`minimizable`](<../src/window_builder.rs#L143>) | 设置窗口创建时原生最小化按钮是否可用 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持设置最小化按钮 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L330>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L991>) |
| [`closable`](<../src/window_builder.rs#L148>) | 设置窗口创建时原生关闭按钮是否可用 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L340>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L996>) |
| [`title`](<../src/window_builder.rs#L153>) | 设置新窗口标题 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L344>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1001>) |
| [`fullscreen`](<../src/window_builder.rs#L158>) | 设置窗口创建时是否全屏 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L348>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1006>) |
| [`focused`](<../src/window_builder.rs#L163>) | 设置窗口创建时是否获取焦点 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L352>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1017>) |
| [`focusable`](<../src/window_builder.rs#L168>) | 设置窗口创建时是否允许获取焦点 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L356>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1022>) |
| [`maximized`](<../src/window_builder.rs#L173>) | 设置窗口创建时是否最大化 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L360>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1027>) |
| [`visible`](<../src/window_builder.rs#L178>) | 设置窗口创建时是否立即可见 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L364>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1032>) |
| [`transparent`](<../src/window_builder.rs#L183>) | 设置窗口创建时是否支持透明内容 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L372>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1037>) |
| [`decorations`](<../src/window_builder.rs#L188>) | 设置窗口创建时是否显示系统标题栏和边框 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L376>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1042>) |
| [`always_on_bottom`](<../src/window_builder.rs#L193>) | 设置窗口创建时是否始终置底 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L380>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1047>) |
| [`always_on_top`](<../src/window_builder.rs#L198>) | 设置窗口创建时是否始终置顶 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L384>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1052>) |
| [`visible_on_all_workspaces`](<../src/window_builder.rs#L203>) | 设置窗口创建时是否出现在所有虚拟桌面 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L388>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1057>) |
| [`content_protected`](<../src/window_builder.rs#L208>) | 设置窗口创建时是否阻止外部截图 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L392>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1064>) |
| [`icon`](<../src/window_builder.rs#L213>) | 设置窗口创建时的图标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L395>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1163>) |
| [`skip_taskbar`](<../src/window_builder.rs#L229>) | 设置窗口创建时是否隐藏任务栏图标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L399>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1183>) |
| [`background_color`](<../src/window_builder.rs#L329>) | 设置窗口创建时的背景色 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L403>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1170>) |
| [`shadow`](<../src/window_builder.rs#L244>) | 设置窗口创建时是否启用阴影 | 未验证 | 未验证 | N/A：官方契约：Linux 不支持窗口阴影 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L415>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1069>) |
| [`owner`](<../src/window_builder.rs#L250>) | 设置 Windows 新窗口的所有者窗口 | 未验证 | N/A：上游 cfg 不在此平台提供 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L427>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1082>) |
| [`parent`](<../src/window_builder.rs#L256>) | 设置新窗口关联的原生父窗口 | 未验证 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L436>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1088>) |
| [`transient_for`](<../src/window_builder.rs#L274>) | 设置 Linux 新窗口的临时父 GTK 窗口 | N/A：上游 cfg 不在此平台提供 | N/A：上游 cfg 不在此平台提供 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L455>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1106>) |
| [`drag_and_drop`](<../src/window_builder.rs#L283>) | 启用或禁用 Windows 文件拖放支持 | 未验证 | N/A：上游 cfg 不在此平台提供 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L460>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1112>) |
| [`title_bar_style`](<../src/window_builder.rs#L289>) | 设置 macOS 新窗口的标题栏样式 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L465>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1118>) |
| [`traffic_light_position`](<../src/window_builder.rs#L295>) | 设置 macOS 新窗口红黄绿按钮的位置 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L472>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1145>) |
| [`hidden_title`](<../src/window_builder.rs#L305>) | 设置 macOS 新窗口是否隐藏标题文字 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L477>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1151>) |
| [`tabbing_identifier`](<../src/window_builder.rs#L311>) | 设置 macOS 新窗口的标签分组标识 | N/A：上游 cfg 不在此平台提供 | 未验证 | N/A：上游 cfg 不在此平台提供 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L487>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1157>) |
| [`theme`](<../src/window_builder.rs#L316>) | 设置新窗口主题或跟随系统 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L490>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1193>) |
| [`has_icon`](<../src/window_builder.rs#L321>) | 查询构建器是否已设置窗口图标 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L493>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1206>) |
| [`get_theme`](<../src/window_builder.rs#L325>) | 查询构建器保存的窗口主题 | 未验证 | 未验证 | 未验证 |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L495>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1210>) |
| [`window_classname`](<../src/window_builder.rs#L234>) | 设置 Windows 新窗口的原生类名 | 未验证 | N/A：官方契约：仅 Windows | N/A：官方契约：仅 Windows |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L499>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1218>) |
| [`no_redirection_bitmap`](<../src/window_builder.rs#L239>) | 设置 Windows 新窗口的无重定向位图样式 | 未验证 | N/A：官方契约：仅 Windows | N/A：官方契约：仅 Windows |  [契约](<../source/tauri/crates/tauri-runtime/src/window.rs#L506>) / [wry](<../source/tauri/crates/tauri-runtime-wry/src/lib.rs#L1227>) |

## Electron 扩展（独立分母，不补官方 API 的缺口）

| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |
| --- | --- | --- | --- | --- | --- |
| [`ElectronWebviewDispatcher::on_new_window`](<../src/new_window.rs#L84>) | 注册 Chromium 新窗口的 Allow/Create/Deny 回调 | 未验证 | 未验证 | 未验证 | Electron 专有契约 |
| [`tauri_runtime_electron::with_opener`](<../src/new_window.rs#L57>) | 在原生弹窗请求上下文中构建目标窗口 | 未验证 | 未验证 | 未验证 | Electron 专有契约 |
| [`tauri_runtime_electron::on_new_window`](<../src/new_window.rs#L84>) | 在普通 Tauri webview 上注册 Electron 新窗口回调 | 未验证 | 未验证 | 未验证 | Electron 专有契约 |
| [`NewWindowFeatures::opener`](<../src/new_window.rs#L36>) | 取得当前 Chromium 弹窗请求的一次性 opener | 未验证 | 未验证 | 未验证 | Electron 专有契约 |
| [`NewWindowResponse::create`](<../src/new_window.rs#L111>) | 将普通 Tauri 新窗口选作原生弹窗目标 | 未验证 | 未验证 | 未验证 | Electron 专有契约 |

扩展完整三平台通过：0/5（0.0%）。
