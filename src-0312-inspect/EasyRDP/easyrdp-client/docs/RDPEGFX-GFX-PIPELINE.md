# RDPEGFX 图形管线（α）

2026-11-13 启用。对齐 mstsc 的现代图形路径（动态虚拟通道管理器 drdynvc + RDPEGFX 插件）。

## 架构

```
tables.c 静态表 ──引用──> drdynvc_VirtualChannelEntryEx        (vendor: channels/drdynvc/client/drdynvc_main.c)
drdynvc（DVC 管理器）──加载──> rdpgfx_DVCPluginEntry           (vendor: channels/rdpgfx/{rdpgfx_common.c, client/rdpgfx_main.c, client/rdpgfx_codec.c})
rdpgfx ──listener──> "1.Microsoft::Windows::RDS::Graphics."    （服务端经 drdynvc 发起 DVC 创建）
ffi.rs on_channel_connected("rdpgfx") ──> gdi_graphics_pipeline_init（libfreerdp gdi/gfx.c，静态库内已有）
```

GFX 表面更新由 `gdi_OutputUpdate`（libfreerdp/gdi/gfx.c）写 `primary_buffer` 后走
`update_begin_paint/update_end_paint` —— 即 ffi.rs 已有的 BeginPaint/EndPaint 链式回调，
**帧管线零改动**。服务端不选 GFX 时仍走传统 BitmapUpdate 路径。

## vendor 编译要点（build.rs）

- cc（MSVC cl.exe）编译上述 4 个 C 文件；include：`channels/{cliprdr,cliprdr/client,drdynvc/client,rdpgfx,rdpgfx/client}` + vcpkg `installed\x64-windows-static-md\include`。
- `.define("BUILTIN_CHANNELS", "1")`：rdpgfx_main.c 尾部入口重命名宏（`DVCPluginEntry → rdpgfx_DVCPluginEntry`）在此宏下才生效，否则展开成 `FREERDP_API`（dllimport）定义 → MSVC C2491。
- `.define("FREERDP_EXPORTS", "1")` + `.define("WINPR_EXPORTS", "1")`：api.h 无条件按 FREERDP_EXPORTS 重定义 FREERDP_API（无 `#ifndef` 守卫，命令行 `/DFREERDP_API=` 会被 header 覆盖，触发 C4005 后照旧 dllimport）。以此使 vendored 代码对 freerdp2.lib 的调用（`zgfx_*` 等）成为直接符号；dllimport 形式的 `__imp_zgfx_*` 在静态库成员未被其它引用拉入时 LNK2019。
- 无 zlib 等新外部依赖；`WITH_GFX_H264` 未定义 ⇒ H264 解码路径编译剔除。
- 源码路径：`EASYRDP_FREERDP_SRC`（默认 `EASYRDP_VCPKG_ROOT\buildtrees\freerdp\src\2.11.2-88e305a3a0.clean`）。

## 会话设置（ffi.rs on_pre_connect）

- `SupportGraphicsPipeline=TRUE` ⇒ `freerdp_client_load_addins` 自动把 "rdpgfx" 加入
  DynamicChannelArray、置 `SupportDynamicChannels=TRUE` 并加载静态 drdynvc；
  主连接能力集声明 `RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL`（settings.h 142 行宏，
  capinfos.c 由该 setting 驱动）。
- **不启用 H264/AVC444**（`GfxH264/GfxAVC444/GfxAVC444v2=FALSE`，静态库无解码后端）：
  rdpgfx 能力通告按源码路径自动带 `RDPGFX_CAPS_FLAG_AVC_DISABLED`（CAPVERSION_10+）。
  **α 收益来源**：服务端合成优化（surface 合成、脏区合并）与 CAPS 协商，非 H264。
- `GfxProgressive=TRUE`（PROGRESSIVE 能力声明；编解码器在 libfreerdp 核心内，无外部依赖）。
- `ColorDepth=32`（UINT32 setter）：RDPEGFX 要求 32bpp 会话。**实测 16bpp + GFX 声明 =
  服务端全静默**（既不发位图更新也不开 RDPEGFX DVC；mstsc /gfx 同样强制 32bpp）。
- `EASYRDP_DISABLE_GFX=1` 环境变量可整体关闭（A/B 对比与回退开关）。

## 验证

- 通道证据（WLog DEBUG / `EASYRDP_FRAME_DEBUG=1`）：
  - `loading channelEx drdynvc` / `[gfx] ChannelConnected: drdynvc`
  - `Loading Dynamic Virtual Channel rdpgfx` + `create_listener: 1.Microsoft::Windows::RDS::Graphics.`
  - DVC 接通后 `[gfx] ChannelConnected: rdpgfx` + `gdi_graphics_pipeline_init ok`
- A/B 对比工具：`cargo run --release -p easyrdp-rdp --features native --example gfx_compare`
  （on/off 各 20s，统计全帧/增量包数与字节）。

## 遗留限制

- 未启用 H264/AVC444v2（静态库无解码后端；后续接入 openh264 或 MF 后端再放开）。
- disp/rdpei/video/geometry/echo/ainput 等 DVC 仍为桩；drdynvc 加载失败会忽略该插件（返回
  CHANNEL_RC_NOT_OPEN 语义），不影响连接。
- 16bpp 会话不可与 GFX 声明共存（启用 GFX 即强制 32bpp）。
- 服务端在长时间反复连接测试期间会出现 nego 阶段 RST（cooldown/会话占用），与客户端无关。
