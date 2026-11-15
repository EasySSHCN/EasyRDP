//! easyrdp-rdp build script（Windows native 静态链接路径）。
//!
//! 职责：
//! 1. **Vendor cliprdr（方案 A）**：`native` feature + Windows 下用 `cc`
//!    （MSVC 自动选择 cl.exe）编译 FreeRDP 2.11.2 源码树里的 cliprdr
//!    通道实现（`channels/cliprdr/client/{cliprdr_main.c,cliprdr_format.c}`
//!    + `channels/cliprdr/cliprdr_common.c`），产出 `cliprdr_VirtualChannelEntryEx`
//!    真实符号——静态链接 `freerdp-client2.lib` 时其 tables.c 静态入口表
//!    引用该符号，缺失即 25 个 unresolved externals 之一。
//!    其余通道入口以 Rust 桩提供（见 src/channel_stubs.rs）。
//! 2. **Vendor drdynvc + rdpgfx（RDPEGFX 图形管线）**：同样思路编译
//!    `channels/drdynvc/client/drdynvc_main.c`（动态虚拟通道管理器，
//!    静态通道入口 `drdynvc_VirtualChannelEntryEx`）+
//!    `channels/rdpgfx/{rdpgfx_common.c, client/rdpgfx_main.c,
//!    client/rdpgfx_codec.c}`（RDPEGFX 插件，DVC 入口
//!    `rdpgfx_DVCPluginEntry`）。drdynvc 的 `dvcman_load_addin` 经
//!    `freerdp_load_channel_addin_entry("rdpgfx", ..., CHANNEL_DYNAMIC)`
//!    解析静态表符号，因此两个符号都必须真实存在。均不依赖 zlib
//!    之外的任何新外部库（H264 路径因未定义 WITH_GFX_H264 而编译剔除）。
//! 2. **系统库**：FreeRDP 静态库引用 `DsMakeSpnW`（ntdsapi.lib）与
//!    `__imp_UuidToStringA/RpcStringFreeA/UuidEqual`（rpcrt4.lib），
//!    在此追加 rustc-link-lib。
//!
//! vcpkg 根默认 `B:\tools\EasyRDP\vcpkg`（见 docs/WINDOWS-FREERDP-SETUP.md），
//! 可用环境变量 `EASYRDP_VCPKG_ROOT` 覆盖。

use std::env;
use std::path::PathBuf;

fn main() {
    // 只在 Windows 目标且 native feature 下做链接侧工作；
    // Linux native 由 freerdp2-sys 的 system-deps/pkg-config 处理。
    if env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    if env::var("CARGO_FEATURE_NATIVE").is_err() {
        return;
    }

    let vcpkg_root = env::var("EASYRDP_VCPKG_ROOT")
        .unwrap_or_else(|_| r"B:\tools\EasyRDP\vcpkg".to_string());
    let vcpkg = PathBuf::from(&vcpkg_root);
    // FreeRDP 2.11.2 源码树：默认取 vcpkg buildtrees 解包目录（本机已存在），
    // 可用 EASYRDP_FREERDP_SRC 覆盖（用户自行下载 2.11.2 官方源码即可）。
    let freerdp_src = PathBuf::from(
        env::var("EASYRDP_FREERDP_SRC").unwrap_or_else(|_| {
            vcpkg
                .join("buildtrees")
                .join("freerdp")
                .join("src")
                .join("2.11.2-88e305a3a0.clean")
                .to_string_lossy()
                .into_owned()
        }),
    );
    let installed = vcpkg.join("installed").join("x64-windows-static-md");
    let cliprdr = freerdp_src.join("channels").join("cliprdr");
    let drdynvc = freerdp_src.join("channels").join("drdynvc");
    let rdpgfx = freerdp_src.join("channels").join("rdpgfx");

    for p in [
        cliprdr.join("client").join("cliprdr_main.c"),
        cliprdr.join("client").join("cliprdr_format.c"),
        cliprdr.join("cliprdr_common.c"),
        drdynvc.join("client").join("drdynvc_main.c"),
        rdpgfx.join("rdpgfx_common.c"),
        rdpgfx.join("client").join("rdpgfx_main.c"),
        rdpgfx.join("client").join("rdpgfx_codec.c"),
    ] {
        if !p.is_file() {
            panic!("vendored FreeRDP channel source missing: {}", p.display());
        }
    }
    if !installed.join("lib").join("freerdp-client2.lib").is_file() {
        panic!(
            "vcpkg x64-windows-static-md FreeRDP install not found under {} \
             (see docs/WINDOWS-FREERDP-SETUP.md)",
            installed.display()
        );
    }

    // 迭代 1-3 记录：任何把命令行嵌入调试记录的旗标（cc-rs 默认 -Brepro、
    // /Z7、甚至 /Zi）都会因本仓库绝对路径过长触发 cl D8050「无法将命令行
    // 放入调试记录」。最终配置：cc 默认旗标关闭（CRATE_CC_NO_DEFAULTS），
    // 显式 /MD（对齐 vcpkg static-md 的动态 CRT）+ /O2，不带调试信息旗标。
    env::set_var("CRATE_CC_NO_DEFAULTS", "1");
    let mut build = cc::Build::new();
    build
        .file(cliprdr.join("client").join("cliprdr_main.c"))
        .file(cliprdr.join("client").join("cliprdr_format.c"))
        .file(cliprdr.join("cliprdr_common.c"))
        .file(drdynvc.join("client").join("drdynvc_main.c"))
        .file(rdpgfx.join("rdpgfx_common.c"))
        .file(rdpgfx.join("client").join("rdpgfx_main.c"))
        .file(rdpgfx.join("client").join("rdpgfx_codec.c"))
        // 通道内部头：cliprdr_main.h / cliprdr_format.h 在 cliprdr/client/，
        // cliprdr_common.h 在 cliprdr/ 根目录；drdynvc_main.h 在
        // drdynvc/client/；rdpgfx_common.h 在 rdpgfx/ 根目录，
        // rdpgfx_main.h / rdpgfx_codec.h 在 rdpgfx/client/。
        .include(cliprdr.join("client"))
        .include(&cliprdr)
        .include(drdynvc.join("client"))
        .include(&rdpgfx)
        .include(rdpgfx.join("client"))
        // FreeRDP / WinPR 公共头（静态库安装前缀）。
        .include(installed.join("include"))
        // rdpgfx_main.c 尾部的入口重命名宏（DVCPluginEntry ->
        // rdpgfx_DVCPluginEntry）在 BUILTIN_CHANNELS 下才生效，否则展开为
        // FREERDP_API dllimport 定义（MSVC C2491）。
        .define("BUILTIN_CHANNELS", "1")
        // 静态链接：api.h 的 FREERDP_API 依据 FREERDP_EXPORTS 选择
        // dllexport/dllimport（无命令行覆盖手段——api.h 无 #ifndef 守卫，
        // /DFREERDP_API= 会被 header 重定义覆盖）。定义 FREERDP_EXPORTS 使
        // vendored 通道代码对 freerdp2.lib 的调用（zgfx_* 等）成为直接符号：
        // dllimport 形式的 __imp_zgfx_* 在静态库成员未被拉入链接时报
        // LNK2019（LNK4217 自动 thunk 只对已被其它引用拉入的成员生效）。
        .define("FREERDP_EXPORTS", "1")
        .define("WINPR_EXPORTS", "1")
        .flag("/MD")
        .flag("/O2")
        .warnings(false);
    build.compile("easyrdp_cliprdr");

    for dir in [
        cliprdr.join("client"),
        cliprdr.clone(),
        drdynvc.join("client"),
        rdpgfx.clone(),
        rdpgfx.join("client"),
        installed.join("include"),
    ] {
        println!("cargo:rerun-if-changed={}", dir.display());
    }

    // 系统导入库：
    // - ntdsapi：DsMakeSpnW（CredSSP/SPN 构造）
    // - rpcrt4：UuidToStringA/RpcStringFreeA/UuidEqual（UUID 工具）
    // - ole32：CoInitializeEx/CoCreateInstance（freerdp2.lib h264_mf.c 媒体
    //   基础 H.264 解码路径；静态链接后显式补链）
    // - shell32：SHCreateDirectoryEx*/SHGetFolderPathA（winpr2.lib shell.c
    //   路径工具；静态链接后显式补链）
    for lib in ["ntdsapi", "rpcrt4", "ole32", "shell32"] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }

    // 迭代 4 记录：static-md 的 .pc 文件带 POSIX 残留 `-ldl -lpthread`（以及
    // rt），system-deps 在 Windows 上展开成 dl.lib / pthread.lib / rt.lib，
    // MSVC 无这些导入库（LNK1181）。生成空静态库垫片满足链接器（无任何符号，
    // 不引入代码）；优先级：cargo 的 link-search（含 build out 目录）先于
    // .pc 的 libdir，同名符号不冲突、垫片不遮蔽真实库。
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let blank = out.join("blank.c");
    std::fs::write(&blank, "/* empty shim: satisfies POSIX -ldl/-lpthread in freerdp .pc on Windows */\n")
        .expect("write blank.c");
    for shim in ["dl", "rt", "pthread"] {
        cc::Build::new()
            .file(&blank)
            .flag("/MD")
            .warnings(false)
            .compile(shim);
    }
    println!("cargo:rustc-link-search=native={}", out.display());
}
