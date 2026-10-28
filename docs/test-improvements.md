# 测试体系改进：分层、冗余与合并方案

> 目标：回答“`src/bin` 是否已覆盖大部分 API 测试、`tests/checks` 能否整体删除”，并给出可执行的去重方案。
> 方法：逐个读取测试入口、CI 工作流和证据协议，只依据当前工作树的实际引用关系下结论；改动前以本文档的分层地图判断“这条测试能不能删”。

## 结论

1. **`tests/checks` 不能整体删除。** 它不是 API 测试，而是三层不同职责：JS 库契约、证据/文档校验器、示例应用端到端。`src/bin` 不但没有覆盖它，反而由它驱动（见下文“反向依赖”）。
2. **真正的冗余只有两处：** `tests/runtime_lifecycle.rs` 与两个 behavior smoke 的退出生命周期重叠；CI 的 `coverage` job 与 `tests/coverage.rs` 是同一个 gate 跑两遍。
3. **建议动作：** 保留现有结构并文档化（本文件）、合并退出生命周期、删除重复的 CI job。其它“精简”一律不做，理由见“不建议做的精简”。

## 先纠正前提：`src/bin` 覆盖不了 `tests/checks`

`src/bin` 证明的是 Rust dispatch 在真实 Electron 上的逐成员行为；`tests/checks` 证明的是另外几件事，且其中三项是硬依赖：

| 依赖方 | 被依赖文件 | 删除后的后果 |
| --- | --- | --- |
| `cargo test` → [window_api.rs:9](../tests/window_api.rs) | [window-api.mjs](../tests/checks/window-api.mjs) | `cargo test` 直接失败；JS 库只剩 Windows 上的真机路径 |
| `cargo test` → [coverage.rs:8](../tests/coverage.rs) | [coverage.mjs](../tests/checks/coverage.mjs) | `docs/coverage.md` 无法生成或校验 |
| [ci.yml:101](../.github/workflows/ci.yml) | [runtime-suites.mjs:9](../tests/checks/runtime-suites.mjs) | 10 个 smoke 二进制不再被任何 CI 步骤执行 |
| [package.json:17](../package.json) | `effects/evidence/dashboard.mjs` | `pnpm test` 失败，免 GUI 回归消失 |

另外两点容易误判：

- `window-api.mjs` 是 `tests/window_api.rs` 的 **fixture**，不是独立测试；`src/bin` 不加载 `api/window.js`（`grep window-api src/bin/` 无结果）。
- `tests/runtime_smoke.rs` 只是 [src/bin/smoke.rs](../src/bin/smoke.rs) 的包装（[runtime_smoke.rs:30](../tests/runtime_smoke.rs)），负责 macOS 主线程约束和进程隔离；`smoke` 不在 [runtime-suites.mjs:10](../tests/checks/runtime-suites.mjs) 的 bin 列表里，删掉包装等于这条覆盖彻底消失。

## 测试分层地图

| 层 | 入口 | 文件 | 证明什么 | 运行环境 |
| --- | --- | --- | --- | --- |
| L1 行为证据 | `cargo test`（`smoke`）、`runtime-suites.mjs`（其余 10 个 bin） | `src/bin/*.rs` | 真实 Electron 上逐 member 的读回/原生副作用 | 桌面或 CI 虚拟显示 |
| L2 证据协议 | 由 L1 调用 | `runtime-evidence.mjs`、`runtime-suites.mjs`、`coverage.mjs` | marker 合法、精确提交、fail-dominant 聚合、coverage 文档一致 | Node，免 GUI |
| L3 库级契约 | `cargo test` | `window_api.rs` + `window-api.mjs`、`acquisition.rs`、`build_support.rs` | `api/window.js` 的 global/peer 两种加载与 invoke 载荷；Electron 暂存与 Node 子进程 | Node，免 GUI |
| L4 示例端到端 | `examples.mjs` | `examples.mjs`、`transparent.mjs`、`example-native.ps1` | `@tauri-apps/api → IPC → dispatch → Electron` 与透明 fixture | 桌面（CI 仅在 Windows 运行 transparent） |
| L5 免 GUI 回归 | `pnpm test:checks` | `effects.mjs`、`evidence.mjs`、`dashboard.mjs`、`runtime-evidence.test.mjs` | 示例页面队列逻辑、工具链失败传播、dashboard 汇总 | Node |
| L6 文档 gate | `cargo test` / CI `coverage` job | `coverage.rs`、`coverage.mjs --check` | `docs/coverage.md` 与生成器一致 | Node + 子模块 |

L2 的关键规则（改动前必须知道）：[runtime-evidence.mjs:38](../tests/checks/runtime-evidence.mjs) 只在**同一个 artifact 内部**拒绝重复断言；跨 suite 重复声明同一 member 合法，聚合时按 fail 优先于 unknown 优先于 skip 优先于 pass 取状态（[runtime-evidence.mjs:94](../tests/checks/runtime-evidence.mjs)）。

## 冗余判定

| 对象 | 判定 | 依据 |
| --- | --- | --- |
| `tests/checks` 整体 | **不冗余** | 见“反向依赖”表 |
| `effects.mjs` 与 `transparent.mjs` | 部分重叠，**保留** | 队列/reset/reload 在 `transparent.mjs:96` 有真机版本，但 CI 只在 Windows 跑 transparent；`effects.mjs` 是 Mac/Linux 唯一覆盖 |
| `tests/runtime_lifecycle.rs` | **大部分重叠** | `Runtime::run` 预置退出码、`request_exit`、veto 已分别由 [desktop_behavior_smoke.rs:130](../src/bin/desktop_behavior_smoke.rs) 与 [runtime_behavior_smoke.rs:46](../src/bin/runtime_behavior_smoke.rs) 覆盖；独有断言只有“veto 后重新 `request_exit`，`run_return` 返回新码” |
| CI `coverage` job | **重复 gate** | [ci.yml:136](../.github/workflows/ci.yml) 与 `runtime-tests` 套件里的 `tests/coverage.rs` 断言相同；后者已在三平台各跑一次 |
| `Runtime::new`、`RuntimeHandle::cursor_position`、`WindowDispatch::set_cursor_position` 被两个 bin 同时声明 | 跨 suite 合法重复，**不建议改** | 与上节聚合规则一致；`smoke.rs` 仍要为其 42 个独有 member 运行，去掉声明只减少 marker，不减少运行时间 |
| `src/bin/smoke.rs`（1332 行） | **不冗余** | 声明 45 个 member，其中 42 个没有其它 bin 覆盖；只有内容为读回/状态类的成员最“薄”，但也没有替代品 |

## 改进项

### P1 文档化分层与删除边界（本文件）

- 动作：新增本文件；在 README 的检查章节加一行指向它。
- 收益：以后判断“能不能删”不用重新读入口和 CI。
- 验收：`docs/test-improvements.md` 被 README 链接；文中每个结论都有文件/行号依据。

### P2 合并退出生命周期测试（可选，推荐）

- 现状：[runtime_lifecycle.rs:18](../tests/runtime_lifecycle.rs) 用 3 个子进程测 `return` / `restart` / `run` 三种退出模式。其中 `run` 与 `desktop_behavior_smoke` 的 `--run-child` 基本重合；`return` 和 `restart` 只是同一条独有契约（veto 后重新 `request_exit`，`run_return` 返回新码）的两个退出码变体。
- 动作：
  1. 在 [desktop_behavior_smoke.rs:34](../src/bin/desktop_behavior_smoke.rs) 的参数分派里新增一个 child 模式（如 `--veto-child`）：`request_exit(17)` → `Ready` → `ExitRequested(Some(17))` 内 veto → `request_exit(accepted)` → 断言第二次 `ExitRequested(Some(accepted))` 与事件顺序 → 断言 `run_return` 返回 `accepted`；不满足则以非零退出。
  2. 父进程像现有 `--run-child` 一样等待并断言退出码。
  3. 删除 `tests/runtime_lifecycle.rs`。
- 验收：三平台 CI 中 `desktop_behavior_smoke` 通过；`cargo test --locked` 其余测试全绿。
- 代价：本地 `cargo test` 不再覆盖该契约，需要 `cargo run --bin desktop_behavior_smoke`（CI 本来就会跑）。若不接受这一点，就保持现状——**不要只删文件不搬断言**。
- 回滚：恢复文件即可，无数据迁移。

### P3 删除重复的 CI `coverage` job

- 现状：[ci.yml:136-151](../.github/workflows/ci.yml) 的 `coverage` job 与 `runtime-tests` 套件中的 `tests/coverage.rs` 断言相同。
- 动作：删除该 job；保留 `tests/coverage.rs` 与 `pnpm test:coverage`（本地仍可用）。
- 验收：故意改 `docs/coverage.md` 一行后，任一平台 `build` job 的 `runtime-tests` 失败，并给出 `--write` 提示。
- 风险：无功能损失；只是失去一个“不用编译就能报错”的快速信号。
- 回滚：恢复 job 定义。

## 不建议做的精简

- **删除 `tests/checks`**：破坏 `cargo test`、CI 与验收文档生成，且删掉的正是“覆盖度是否可信”的证明。
- **删除 `src/bin/smoke.rs`**：丢掉 42 个无替代的 member 覆盖。
- **合并 `effects.mjs` 到 `transparent.mjs`**：后者的 CI 覆盖仅 Windows，Mac/Linux 会失去唯一的页面队列回归。
- **把 `runtime-evidence` 的跨 suite 重复声明当 bug 修**：聚合规则本就允许，改了没有收益，反而增加 marker 维护成本。
- **用 mock 替代真机断言去“提速”**：与 `kind: behavior` 的证据门槛冲突。

## 改动 → 最小验证命令

| 改动区域 | 最小验证 |
| --- | --- |
| `api/window.js` | `cargo test --locked --test window_api` |
| `docs/coverage.md` / `coverage.mjs` | `pnpm run test:coverage` |
| 工具链 / 证据协议 / `examples.mjs` | `pnpm run test:checks` |
| `tests/assets/transparent/main.js` | `pnpm run test:checks`（effects） |
| `example/probes.ts` | `node tests/checks/dashboard.mjs` |
| `build.rs` / `build_support.rs` | `cargo test --locked --test build_config`；改暂存逻辑再加 `--test acquisition` |
| Rust dispatch 行为 | `cargo run --locked --bin <suite>`（需桌面）；CI 由 `runtime-suites.mjs` 覆盖 |
| 示例真机 / 透明 fixture | `pnpm run test:examples`（按需加 `coverage` / `transparent`） |

## 统计口径

“`smoke.rs` 声明 45 个、其中 42 个独有”的统计方式：扫描 `src/bin/*.rs` 中形如 `"Trait::member"` 的字符串（仅六个 dispatch/builder trait），按文件去重后比较集合。跨平台专属重复（`WindowBuilder::icon`、`WindowDispatch::window_handle` 等）分别出现在 Linux 与 Windows 两个互斥 suite，不计为冗余。
