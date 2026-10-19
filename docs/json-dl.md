# json-dl：Java 启动器下载子进程

`json-dl` 是 fork 中新增的 HTTP/HTTPS 程序，复用 `libdl`，不启用 torrent 依赖。上游 `dl` CLI、参数和发布工作流保留。这里不解析或安装 muapack，也不验证 muapack 签名；这些仍由 Java 调用方处理。

## 启动

普通命令行：

```sh
json-dl 'https://example.org/pack.muapack' -o pack.muapack --sha256 <64位十六进制哈希> --expected-size 123456
```

自动化推荐使用 `json-dl --request-stdin`，第一行传入 UTF-8 JSON，避免签名 URL 或鉴权头出现在进程命令行里：

```json
{"url":"https://example.org/pack.muapack?signature=...","output":"downloads/pack.muapack","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","expected_size":123456,"connections":4,"headers":{"Authorization":"Bearer ..."}}
```

参数：

| stdin 字段 | CLI 参数 | 默认值 / 说明 |
|---|---|---|
| `url` | 位置参数 | 必填；HTTP(S)，不允许 userinfo、fragment |
| `output` | `-o`, `--output` | 必填；最终文件路径，支持 Unicode / 空格 |
| `sha256` | `--sha256` | 可选；期望 SHA-256，不区分大小写 |
| `expected_size` | `--expected-size` | 可选；期望字节数 |
| `headers` | 重复 `-H`, `--header` | 可选；CLI 格式 `Name: Value` |
| `connections` | `-j`, `--connections` | 默认自适应；显式值 1..32 |
| `chunk_size` | `--chunk-size` | 默认 2097152；单位为字节，65536..67108864 |
| `resume` | `--no-resume` 关闭 | 默认 true；**只有提供期望 SHA-256 时才启用续传** |
| `overwrite` | `--overwrite` | 默认 false；替换已有文件需显式开启 |

`--request-stdin` 不可与以上下载参数混用。初始请求须在 30 秒内到达。输入每行最多 64 KiB，拒绝未知字段。`Range`、`If-Range`、`Host`、`Content-Length`、`Accept-Encoding` 和 `Transfer-Encoding` 不允许自行覆盖。

## stdout 协议 v1

stdout **每行一个 JSON 对象**，包含 `protocol: 1`。实时事件每行立即 flush；进度约每 200 ms 最多一条。程序不打印终端进度条，不输出 URL、请求头或原始服务端错误。stderr 留给运行时诊断；Java 应同时读取两个管道。

事件顺序：

```json
{"protocol":1,"event":"started","output":"downloads/pack.muapack"}
{"protocol":1,"event":"progress","phase":"Downloading","downloaded":65536,"total":123456,"active_workers":4}
{"protocol":1,"event":"verifying"}
{"protocol":1,"event":"complete","output":"downloads/pack.muapack","bytes":123456,"sha256":"...","resumed":false}
```

`phase` 可能为 `Probing`、`Downloading`、`PersistingState`、`Finalizing`；未知总大小时 `total` 为 null。进度允许因重试回退。只有终结事件 `complete` 表示验证和最终发布都已完成，不能仅凭下载字节数判断成功。

失败示例：

```json
{"protocol":1,"event":"error","code":"http_403","message":"Download failed; inspect the error code and HTTP status","http_status":403,"retryable":false,"refresh_url":true}
```

`http_status` 可为 null。401/403 的 `refresh_url: true` 提示调用方检查鉴权或重新获取 URL，并不保证错误一定来自链接过期。`retryable` 只是重试提示；程序内部分段重试结束后，是否重新启动由 Java 决定。

`--help`、`--version` 输出 `event: info` 的 JSON（说明文字位于 `text`）；参数错误同样输出 JSON，不回显不合法参数中的秘密。

| 退出码 | 终结事件 | 含义 |
|---|---|---|
| 0 | `complete`，或帮助/版本的 `info` | 成功 |
| 1 | `error` | 网络、HTTP、协议或文件 I/O 失败 |
| 2 | `error` | 参数、stdin 请求/命令、输出冲突或锁冲突 |
| 3 | `cancelled` | 取消，已完成分块的续传状态已保存 |
| 4 | `error` / `integrity_mismatch` | 大小或 SHA-256 不匹配，未发布到最终路径 |

调用方须同时检查退出码和终结事件。管道损坏、系统强制终止等情况下无法保证终结事件送达。

## 取消、续传和凭据

启动后向同一个 stdin 写入一行并 flush：

```json
{"command":"cancel"}
```

收到取消后程序停止传输、保存检查点，以退出码 3 和 `cancelled` 结束。Ctrl+C 也支持传输阶段取消。stdin EOF 本身不取消任务；调用方可以在发送初始请求后关闭 stdin。非法控制指令会取消任务并返回退出码 2。

通过外部强杀取消时，仅保证已有检查点可复用；Java 应先发送取消，等待合理时间后再使用 `destroyForcibly()` 兜底。

链接过期时，由 Java 获取新 URL 后重新启动本程序，传入**相同输出路径和相同期望 SHA-256**。同源同路径下签名查询参数变化可复用检查点；变更源主机或路径时保守地重新下载。没有期望 SHA-256 时关闭续传，避免把同 URL 的不同内容拼接起来。

程序只自动跟随同源重定向，跨主机、端口、协议重定向返回错误；请传入后端解析后的最终下载地址，避免将自定义凭据转发给其他源。

## 文件和并发

假设输出为 `pack.muapack`：

- `pack.muapack.json-dl-part.dl`：传输中的分块及检查点；续传所需。
- `pack.muapack.json-dl-part`：下载完成但尚未发布的暂存文件；验证失败时可检查或删除。
- `pack.muapack.json-dl.lock`：空的持久锁文件。通过 OS 文件锁防止两个 json-dl 同时写一个目标；进程退出自动释放锁，不需要删除文件。

请求头不会写入续传状态；json-dl 的续传记录会移除源 URL 查询参数。签名 URL 和 token 也不会被本程序主动输出。

最终文件校验通过后才发布；已有文件在失败时保留。不覆盖模式使用同文件系统硬链接完成原子的无覆盖发布，目标文件系统需支持硬链接（例如 NTFS、ext4、APFS）。`overwrite` 模式使用同目录 rename。暂存和最终路径应由调用方独占管理，不要与其他程序共用暂存后缀。

每次都会计算实际 SHA-256；只有传入可信的期望哈希才能验证内容身份。muapack 签名、解包路径检查和安装事务不属于本程序。

## Java 接入要点

```java
Process process = new ProcessBuilder(binaryPath, "--request-stdin").start();
// 独立线程持续读取 stdout JSON Lines 和 stderr，避免管道填满。
// 用 UTF-8 writer 写初始请求和换行，flush；需要取消时写 cancel JSON。
// stdout 事件切换到 Minecraft/UI 主线程后更新界面。
// 等待进程退出，确认退出码为 0 且收到 complete，再验证 muapack 签名。
```

参数分开传给 `ProcessBuilder`，不要拼接成 shell 命令；不需要 JNI。根据 OS/CPU 选择原生程序，解压时保留/设置 Unix 执行权限。

## 构建和验证

```sh
cargo build --locked --release -p json-dl
cargo test --locked -p json-dl -p libdl --no-default-features
python scripts/test-json-dl.py target/release/json-dl
python scripts/package-json-dl.py --target x86_64-unknown-linux-gnu --binary target/release/json-dl
```

构建工作流 `.github/workflows/json-dl.yml` 配置了以下目标；**配置不代表它们已经构建成功，成功状态以对应 CI 运行及产物为准**：

| 系统 | 目标 |
|---|---|
| Windows | x86_64 / ARM64，MSVC，静态 CRT |
| Linux GNU | x86_64 / ARM64，在 Ubuntu 22.04 构建，按 glibc 2.35+ 部署 |
| Linux musl | x86_64 / ARM64，优先作为无 glibc 依赖的分发版本 |
| macOS | Intel / Apple Silicon，deployment target 11.0 |
| macOS Universal | 合并上述两个架构，ad-hoc 签名；不包含 Developer ID 签名或公证 |

各目标运行库测试、Python 黑盒测试，再打包二进制、许可证、本文档、SHA-256 文件和 JSON 元数据。原 `dl` CLI 另做兼容编译检查。工作流只上传 Actions artifacts，不创建公开 Release、不更新上游 Homebrew Tap。

Windows 本机没有 MSVC 时，可以用 Linux + MinGW 生成 `x86_64-pc-windows-gnu` 补充构建；它不替代 CI 中的 MSVC / ARM64 验证。容器中构建的 GNU/Linux 包也不继承 CI Ubuntu 22.04 的 glibc 基线，需要另行检查。
