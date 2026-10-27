# RUST 侧改进

1. 配置项构建：从“宏大参数对象（Metadata）”转向“流式构建器（Builder Pattern）”

现状：代码中使用了 Metadata 结构体，并通过 .metadata(Metadata { ... }) 传入。

改进：Rust 中更惯用的做法是将元数据直接拍平为 ElectronBuilder 的链式调用方法（Fluent API），减少用户的样板代码，并且更容易利用类型系统做编译期检查。

```rust
let prepared = ElectronBuilder::new(DEFAULT_VERSION)
    .app_name("My App")
    .identifier("com.example.myapp")
    .icon(Path::new("icon.ico"))
    .build()?;
```

2. 同步 API vs 异步生态（Async/Await）

现状：目前提供的是同步接口（如 electron.call(...)，阻塞直到超时）。

改进：现代 Rust 桌面/GUI 应用（如 Tauri、Iced、Dioxus）几乎全面拥抱了 async/await（Tokio 异步运行时）。如果 electron.call 是同步的，在主线程调用会阻塞 UI，在子线程调用又需要频繁切换上下文。

```rust
use std::path::Path;
use std::time::Duration;
use electron_rs::{find_binary, Electron};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Path::new("./runtime");
    let binary = find_binary(runtime)?;
    
    // 异步启动并建立连接
    let electron = Electron::new(...).await?;
    
    // 异步非阻塞发起 RPC 调用，不卡顿主线程
    let output = electron
        .call("app.theme", json!({ "theme": "system" }), Duration::from_secs(5))
        .await?;
        
    println!("Theme response: {:?}", output);
    Ok(())
}
```

3. 细粒度错误处理（基于 thiserror 的精确匹配）

现有设计：返回一个通用的 electron_rs::Error，上层难以针对特定错误（如超时、校验失败、管道断开）做自动化容灾恢复。

改进后的消费侧代码：上层可以通过 match 匹配不同的错误变体，实现精细化处理（例如：超时自动重试、校验失败触发重新下载）。


```rust
use electron_rs::{ElectronError, find_binary, Electron};
use serde_json::json;
use std::time::Duration;

async fn handle_electron_call(electron: &Electron) {
    let result = electron
        .call("app.get_config", json!({}), Duration::from_secs(3))
        .await;

    match result {
        Ok(res) => {
            println!("成功获取配置: {:?}", res);
        }
        // 精确捕获：RPC 超时错误
        Err(ElectronError::IpcTimeout { timeout_duration }) => {
            eprintln!("RPC 请求超时（耗时超过 {:?}），正在尝试自动重试...", timeout_duration);
            // 执行重试逻辑...
        }
        // 精确捕获：二进制文件校验不匹配
        Err(ElectronError::ChecksumMismatch { expected, actual }) => {
            eprintln!("安全警报：Electron 二进制被篡改或损坏！期望 SHA256: {}, 实际: {}", expected, actual);
            // 触发重新下载/修复流程
        }
        // 捕获底层 IPC 通道断开
        Err(ElectronError::ChannelDisconnected) => {
            eprintln!("Electron 主进程已意外退出，正在拉起新实例...");
        }
        Err(e) => {
            eprintln!("发生其他未知错误: {}", e);
        }
    }
}
```

4. 类型安全的 RPC 方法（消除硬编码字符串与 serde_json::Value）

现有设计：方法名全靠字符串手写（如 "app.theme"），参数靠 json!({ ... }) 动态拼装，极易因为拼写错误或字段改动导致运行时崩溃。   

改进后的消费侧代码：通过结构体和 Trait 约束，实现强类型、编译期检查的 RPC 调用。

```rust
use electron_rs::{Electron, RpcRequest};
use serde::{Serialize, Deserialize};

// 1. 定义强类型的请求与响应结构体
#[derive(Serialize)]
struct SetThemeRequest {
    theme: String,
}

#[derive(Deserialize, Debug)]
struct SetThemeResponse {
    success: bool,
    applied_theme: String,
}

// 2. 为请求实现 Trait，绑定对应的远程方法名和返回类型
impl RpcRequest for SetThemeRequest {
    type Response = SetThemeResponse;
    const METHOD: &'static str = "app.theme";
}

// 3. 消费侧调用（完全类型安全，杜绝拼写错误）
async fn update_theme(electron: &Electron) -> Result<(), Box<dyn std::error::Error>> {
    let req = SetThemeRequest {
        theme: "system".to_string(),
    };

    // 编译器会直接推导返回类型为 SetThemeResponse，无需手动解析 JSON
    let res: SetThemeResponse = electron.call_typed(req).await?;
    
    println!("主题设置成功: {}", res.applied_theme);
    Ok(())
}
```

5. 使用 rmp-serde (MessagePack) 进行高效二进制 RPC 调用


```rust
use serde::{Serialize, Deserialize};
use std::time::Duration;
use electron_rs::Electron;

// 1. 定义请求和响应结构体（依然支持标准的 Serde 宏）
#[derive(Serialize)]
struct SetThemeParams {
    theme: String,
}

#[derive(Deserialize, Debug)]
struct SetThemeResult {
    success: bool,
    applied: String,
}

async fn update_theme_via_msgpack(electron: &Electron) -> Result<(), Box<dyn std::error::Error>> {
    let params = SetThemeParams {
        theme: "system".to_string(),
    };

    // 2. 核心改动：使用 rmp_serde 将结构体直接序列化为二进制字节流 (Vec<u8>)
    // 相比 JSON，它没有繁琐的引号和大括号，体积更小、序列化更快
    let payload = rmp_serde::to_vec(&params)?;
    
    println!("MessagePack 序列化后大小: {} bytes", payload.len());

    // 3. 发送二进制 RPC 请求，并获取二进制响应结果
    let response_bytes = electron
        .call("app.theme", payload, Duration::from_secs(5))
        .await?;

    // 4. 使用 rmp_serde 直接从二进制切片反序列化还原响应对象
    let result: SetThemeResult = rmp_serde::from_slice(&response_bytes)?;

    println!("主题设置成功: {:?}", result);
    Ok(())
}
```

修改 ipc.ts 底层接收和发送的位置：

```ts
import { pack, unpack } from 'msgpackr'

// ... 前略 ...

function thread(): void {
  // ... 初始化 socket ...
  let buffer = Buffer.alloc(0)

  // 发送：带长度前缀的二进制写入
  channel.on('message', (message: IPCChannelMessage) => {
    if (message.close) {
      socket.destroy()
    } else if (!socket.destroyed) {
      const encoded = pack(message.send) // 用 msgpackr 序列化为二进制
      const lengthHeader = Buffer.alloc(4)
      lengthHeader.writeInt32BE(encoded.length, 0)
      
      socket.write(Buffer.concat([lengthHeader, encoded]))
    }
  })

  // 接收：处理二进制粘包与长度前缀分帧
  function receive(chunk: Buffer): void {
    buffer = Buffer.concat([buffer, chunk])

    while (buffer.length >= 4) {
      const packetLength = buffer.readInt32BE(0)
      if (buffer.length < 4 + packetLength) {
        break // 数据还没收完整，等待下一个 chunk
      }

      const packet = buffer.subarray(4, 4 + packetLength)
      buffer = buffer.subarray(4 + packetLength)

      try {
        const decoded = unpack(packet) // 用 msgpackr 解包
        notify({ request: decoded as IPCRequestPayload })
      } catch {
        socket.destroy(new Error('invalid host IPC binary message'))
      }
    }
  }

  socket.on('data', (chunk: Buffer) => receive(chunk))
  // ... 后略 ...
}
```