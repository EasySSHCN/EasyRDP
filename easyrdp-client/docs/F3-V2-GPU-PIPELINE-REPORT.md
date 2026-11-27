# F3-V2「GPU 编码管线」交付报告（R-20261118-017）

## 一、任务定位声明（重要）

RDP 协议本身的图形压缩由 FreeRDP 与服务端协商（RFX/NSCodec/H.264 RemoteFX），客户端
GPU 编码**不能**改变与标准 RDP 服务端的图形编码协商。**本任务的准确定位是
"本地帧管线的 GPU 加速"**：把每帧执行的 CPU 热点（BGRA→RGB888/BGRA→RGBA 颜色转换、
2×2 盒式降采样、脏矩形 tile diff、box-average 缩放）用 SIMD（AVX2）或 D3D11
（VideoProcessor）加速，降低本地渲染开销。本交付**不含也不虚构**"GPU H.264 编码 RDP 流"。

## 二、改动文件

新增（全部隔离在新 crate，未触碰任何既有渲染/会话代码）：

| 文件 | 内容 |
|---|---|
| `crates/easyrdp-pixels/Cargo.toml` | 新 crate；features：`gpu-accel`（SIMD）、`d3d`（GPU，默认关） |
| `crates/easyrdp-pixels/src/lib.rs` | 公开 API + 运行时分发（`is_x86_feature_detected!("avx2")` 缓存，无 AVX2 自动走纯 Rust） |
| `crates/easyrdp-pixels/src/scalar.rs` | 纯 Rust 参考实现（正确性基准；`halve_bgra`/`box_downscale_bgra` 与 `frame_jpeg.rs`/`snapshot.rs` 现行实现同语义） |
| `crates/easyrdp-pixels/src/simd.rs` | AVX2/SSSE3 intrinsics：BGRA→RGBA（shuffle）、BGRA→RGB888（shuffle 打包）、2×2 halve（去交错 + u16 整数和，sum/4 截断与标量逐位一致）、bytes_equal（pcmpeqb+movemask） |
| `crates/easyrdp-pixels/src/diff.rs` | 脏 tile diff（tile 网格扫描，SIMD 分发点在 `bytes_equal`） |
| `crates/easyrdp-pixels/src/gpu.rs` | D3D11 VideoProcessor GPU 缩放（`windows` crate 0.62；硬件设备失败自动 WARP，再失败 `None` → 调用方回退 CPU/SIMD）；`scale_with_fallback` |
| `crates/easyrdp-pixels/tests/correctness.rs` | 9 条正确性单测（含 2 条真 GPU 测试） |
| `crates/easyrdp-pixels/examples/bench.rs` | 1080p 基准：CPU 基线 vs SIMD vs GPU |
| `crates/easyrdp-pixels/examples/debug_halve.rs`、`debug_lane.rs` | halve 内核调试工具 |
| 根 `Cargo.toml` | workspace members 增加一行 `crates/easyrdp-pixels` |

**稳定性约束落实**：未启用 feature 时行为与纯 Rust 基线逐位一致；`d3d`/`gpu-accel` 默认关；
现有 `rdp_view_final.rs`（GDI 直绘主路径）、`ffi.rs`、`snapshot.rs` 一行未改，零风险。

## 三、热点剖析（交付项 1）

每帧 CPU 热点定位（帧数据源 FRAME_STORE，BGRA32 top-down）：

1. `ffi.rs` 快照路径：`snapshot_primary_frame`（全帧抠取，memcpy 型，收益小）；
   运动期全帧走 `frame_jpeg::halve_bgra`（**每帧执行**，2×2 降采样）→ `encode_frame_jpeg`。
2. `rdp_view_final.rs::present_delta`：脏矩形行级 `copy_from_slice`（memcpy 型）；
   `present()`：FRAME_STORE 全帧 clone（~4MB/帧，内存带宽型）。
3. `snapshot.rs::downscale_bgra`：box-average 缩略图（逐像素 u64 累加，周期任务非每帧）。
4. 颜色转换（BGRA→RGB888/BGRA→RGBA）：前端编码/审计路径潜在热点。

`cargo run --release -p easyrdp-pixels --example bench` 即量化基线（见下）。

## 四、bench 数据（1080p，真机，AVX2=有，release）

| 操作 | CPU 纯 Rust | SIMD（gpu-accel） | 加速比 | GPU（d3d） |
|---|---|---|---|---|
| BGRA→RGBA | 4299.6 µs/帧 | 1564.3 µs/帧 | **2.75x** | —（RGB888 可走 D3D 后续） |
| BGRA→RGB888 | 3419.0 µs/帧 | 1444.9 µs/帧 | **2.37x** | — |
| 2×2 halve | 4330.4 µs/帧 | 2234.7 µs/帧 | **1.94x** | 3976.8 µs/帧（1080p→960x540） |
| 脏 tile diff(32px) | 3564.8 µs/帧 | 3712.1 µs/帧 | ~1.0x（memcmp 已够快，见遗留项） | — |
| box 缩放→320x180 | 4874.7 µs/帧 | —（标量） | — | — |

GPU 回退分发实测：`scale_with_fallback` 当前选择 `d3d11`（输出 2073600B = 960×540×4 ✓）。
说明：GPU 每次调用新建纹理/视图 + staging 回读（MVP），批量纹理池化后可显著下降。

## 五、测试结果（交付项 5）

- `cargo test -p easyrdp-core -p easyrdp-rdp -p easyrdp-pixels -p easyrdp-app` → **全绿**：
  core 115 ok；rdp 71 ok + 1 ignored（既有）；pixels 7 ok（默认 feature，编译组合验证）；
  app 28 ok（既有全绿，未受影响）。
- pixels `--features gpu-accel` → 7 ok；`--features gpu-accel,d3d` → **9 ok**，其中：
  - `gpu_passthrough_identity`：GPU 1:1 直通逐像素 ±1 内 ✓（真机 D3D11 设备实测）；
  - `gpu_downscale_bounded_error`：GPU 2:1 缩放 vs CPU box-average 平均偏差有界 ✓
    （GPU 采样与 box-average 数学语义不同，**GPU 允许 ±1 量化误差级别的差异**，已注明；
    缩放场景按平均偏差断言，1:1 按 ±1 逐像素断言）。
- SIMD vs 纯 Rust：转换/halve **逐字节一致**（整数运算、无舍入差异）；
  无 AVX2 环境回退：`has_avx2()` 运行时检测为 false 时分发层直接走 scalar，
  `fallback_branch_produces_reference_output` 验证公开 API == scalar 参考。

## 六、真机验证清单（人工复核项）

1. `cargo run --release -p easyrdp-pixels --example bench --features gpu-accel,d3d`
   → 本报告第四节数据可复现；输出行"回退分发当前选择"应为 `d3d11`（有 GPU）或 `cpu`（无 GPU，不报错）。
2. 无 AVX2 机器（或用 `RUSTFLAGS="-C target-feature=-avx2"` 构建后跑测试）：
   `cargo test -p easyrdp-pixels` 应全绿（SIMD 分支被运行时检测关闭）。
3. `--no-default-features` 编译：纯 Rust 基线，无 SIMD/GPU 代码进入二进制。
4. 接入观察（后续集成时）：`present_delta`→直绘、`halve_bgra`→JPEG 路径替换为
   `easyrdp_pixels::halve_bgra` 均为一行替换，语义一致。

## 七、遗留项

1. **脏 tile diff 无加速**：标量路径底层是 `memcmp`（已高度优化），AVX2 包装反慢 ~4%；
   建议保留标量或在 tile 行级做整批比较再优化。
2. GPU 路径每帧新建纹理/视图/staging（MVP）：纹理池 + 持久 VideoProcessor 视图可把
   3977µs/帧显著压低；当前默认不启用 `d3d`，无稳定性影响。
3. `box_downscale_bgra`（快照缩略图）仍是标量：非每帧热点，收益低。
4. GPU BGRA→RGB888（3 字节紧凑格式）VideoProcessor 原生不支持，需 compute shader（后续）。
5. 未接入调用方（按任务"只开发+自测"，不动 GDI 主路径）；集成点即上述一行替换。
6. `windows-sys` 0.61 已移除 D3D11/DXGI，GPU 路径使用 `windows` crate 0.62（已在依赖树，
   锁定离线可用）。
