# Windows FreeRDP 2.x 开发库安装与配置（easyrdp-rdp native feature）

> 状态：**已在本机验证通过**（2026-11-12，Windows 11，Rust 1.99.1 MSVC，VS2022）。
> `cargo check / build / 运行示例` 全链路成功，真实链接 freerdp2 / freerdp-client2 / winpr2。

## 选定路径（B 组候选路径 2：vcpkg 版本钉扎）

| 项 | 值 |
|---|---|
| FreeRDP 版本 | **2.11.2**（vcpkg 端口 `freerdp@2.11.2#2`，git-tree `00aea4e71fcfa8fa2ea4c294996726290abe3243`） |
| 三元组 | `x64-windows`（动态 DLL + MSVC `.lib` 导入库） |
| vcpkg 位置 | `B:\tools\EasyRDP\vcpkg`（master zip，bootstrap vcpkg-tool 2026-10-29） |
| 安装前缀 | `B:\tools\EasyRDP\vcpkg\installed\x64-windows` |
| pkg-config | pkgconf 3.0.10（GitHub 官方 MSI 解出）：<br>`B:\tools\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10\pkg-config.exe` |
| Rust 绑定 | `freerdp2-sys 0.2.0`（system-deps 6，pkg-config 探测 `freerdp2 ≥ 2.4` / `freerdp-client2 ≥ 2.4`） |

## 为什么不用路径 1 / 3

- **conda-forge**：`conda-forge/freerdp` 只有 3.30.0–3.33.1，且 **无 win-64 构建**（conda_platforms 仅 linux-*）→ 不可行。
- **MSYS2**：mingw 只给 `.dll.a`（GNU 导入库），MSVC 链接器不可用 → 不可行。
- **vcpkg 注意**：master 端口树已升级到 freerdp 3.32.2；2.x 只存在于历史版本（最高 **2.11.2**，无 2.11.7）。2.11.2 满足 freerdp2-sys 锚定的 2.x ABI（≥2.4）。

## 复现安装步骤（本机已完成，供重装参考）

```powershell
# 1) vcpkg（GitHub zip 经镜像下载：https://ghfast.top/<原URL>）
B:\tools\EasyRDP\vcpkg\bootstrap-vcpkg.bat -disableMetrics

# 2) 经典模式 + overlay 钉扎 2.11.2 端口（manifest 模式需要 git 历史，zip 版不可用）
$env:X_VCPKG_ASSET_SOURCES = 'x-script,curl.exe -L -sS --fail --output {dst} --create-dirs https://ghfast.top/{url}'
B:\tools\EasyRDP\vcpkg\vcpkg.exe install freerdp:x64-windows `
  --overlay-ports=B:\tools\EasyRDP\overlay-ports   # 内含 freerdp@2.11.2 端口目录

# 3) pkgconf：GitHub release MSI 解出（msiexec /a 管理映像方式，无全局安装）
msiexec /a pkgconf-x64-3.0.10.msi /qn TARGETDIR=B:\tools\EasyRDP\pkgconf-msi
```

产物（`installed\x64-windows` 下）：

- `bin\freerdp2.dll` / `freerdp-client2.dll` / `winpr2.dll`（+ openssl 运行时 dll）
- `lib\freerdp2.lib` / `freerdp-client2.lib` / `winpr2.lib`
- `include\freerdp\` / `include\winpr\`（头文件）
- `lib\pkgconfig\freerdp2.pc` / `freerdp-client2.pc` / `winpr2.pc`

## 构建 / 运行所需环境变量（每次会话设置）

```powershell
$env:PKG_CONFIG_PATH = 'B:\tools\EasyRDP\vcpkg\installed\x64-windows\lib\pkgconfig'
$env:PATH = 'B:\tools\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10;' + $env:PATH          # 构建：pkg-config 探测
$env:PATH = 'B:\tools\EasyRDP\vcpkg\installed\x64-windows\bin;' + $env:PATH               # 运行：加载 DLL
```

验证：

```powershell
cd EasyRDP\easyrdp-client
cargo check -p easyrdp-rdp --features native     # 编译 + system-deps 探测
cargo build -p easyrdp-rdp --features native --example freerdp_probe
target\debug\examples\freerdp_probe.exe          # 打印 freerdp_new() 指针，验证 DLL 加载
```

本机实测输出：`freerdp_new() -> 0x1c48370ad00`、`freerdp_free() ok`。

## 遗留风险与备注

1. **AV 文件锁**：安装期间 Defender/杀软可能瞬时锁住新产出的 exe/dll（本次 openssl nasm 与打包阶段各失败一次），重跑 `vcpkg install` 即可（增量缓存）。
2. **vcpkg master 漂移**：`B:\tools\EasyRDP\vcpkg` 是 master 快照，overlay 端口钉在 2.11.2；若重装请保留 `overlay-ports\freerdp` 目录。
3. **GitHub 直连受限**：本网络 `github.com` archive/codeload 会被重置，release 资产与 raw 可用；下载脚本已用 `ghfast.top` 镜像。
4. **连接级行为未验证**：仅验证 FFI 链接与实例创建（α 范围）；真实 RDP 会话需在 Linux 侧按 ffi.rs 记录继续验收。
5. **未污染系统**：所有组件位于 `B:\tools\EasyRDP`，仅会话级 env，未写入系统 PATH/注册表（pkgconf 为 msiexec /a 解包，非安装）。
