//! D3D11 GPU 路径（`d3d` feature，Windows）。
//!
//! 用 ID3D11Device + ID3D11VideoProcessor（VideoProcessorBlt）做 BGRA 帧的
//! GPU 缩放/格式转换；设备创建或任何 COM 调用失败 → [`GpuScaler::new`] /
//! [`GpuScaler::scale`] 返回 `None`/`Err`，调用方自动回退 CPU/SIMD。
//!
//! 量化误差说明：VideoProcessor 内部采样（bilinear/box，由驱动实现）与
//! CPU box-average 数学语义不同——**非整数倍缩放时允许与 CPU 参考存在偏差**；
//! 单测口径：1:1 直通要求逐像素 ±1（颜色不重采样），缩放场景断言平均绝对
//! 误差有界（GPU 允许 ±1 量化误差级别的差异，见 tests/correctness.rs）。

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11VideoContext, ID3D11VideoDevice,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_VPIV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Resource,
    ID3D11Texture2D, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Foundation::RECT;

pub struct GpuScaler {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_ctx: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
}

// D3D11 COM 对象 free-threaded，可跨线程。
unsafe impl Send for GpuScaler {}

impl GpuScaler {
    /// 创建 GPU 缩放器（固定 src→dst 几何）。任何一步失败返回 None（调用方回退）。
    pub fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<GpuScaler> {
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
            return None;
        }
        let mut device: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        let mut level = D3D_FEATURE_LEVEL_11_0;
        // 硬件优先；失败走 WARP（软渲染——仍可验证管线正确性，不抛错）
        let hr = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(std::slice::from_ref(&D3D_FEATURE_LEVEL_11_0)),
                D3D11_SDK_VERSION,
                Some(&mut device as *mut Option<ID3D11Device>),
                Some(&mut level as *mut _),
                Some(&mut ctx as *mut Option<ID3D11DeviceContext>),
            )
        }
        .or_else(|_| {
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    windows::Win32::Foundation::HMODULE::default(),
                    D3D11_CREATE_DEVICE_FLAG(0),
                    Some(std::slice::from_ref(&D3D_FEATURE_LEVEL_11_0)),
                    D3D11_SDK_VERSION,
                    Some(&mut device as *mut Option<ID3D11Device>),
                    Some(&mut level as *mut _),
                    Some(&mut ctx as *mut Option<ID3D11DeviceContext>),
                )
            }
        });
        let (device, ctx) = match (hr, device, ctx) {
            (Ok(()), Some(d), Some(c)) => (d, c),
            _ => return None,
        };

        let video_device: ID3D11VideoDevice = match device.cast() {
            Ok(v) => v,
            Err(_) => return None,
        };
        let video_ctx: ID3D11VideoContext = match ctx.cast() {
            Ok(v) => v,
            Err(_) => return None,
        };

        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_FRAME_FORMAT(0), // PROGRESSIVE
            InputFrameRate: windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL {
                Numerator: 1,
                Denominator: 1,
            },
            InputWidth: src_w,
            InputHeight: src_h,
            OutputFrameRate: windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL {
                Numerator: 1,
                Denominator: 1,
            },
            OutputWidth: dst_w,
            OutputHeight: dst_h,
            Usage: windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_USAGE(0),
        };
        let enumerator = match unsafe { video_device.CreateVideoProcessorEnumerator(&desc) } {
            Ok(e) => e,
            Err(_) => return None,
        };
        let vp = match unsafe { video_device.CreateVideoProcessor(&enumerator, 0) } {
            Ok(p) => p,
            Err(_) => return None,
        };

        Some(GpuScaler {
            device,
            ctx,
            video_device,
            video_ctx,
            enumerator,
            vp,
            src_w,
            src_h,
            dst_w,
            dst_h,
        })
    }

    /// GPU 缩放一帧 BGRA（top-down，src 几何），返回 BGRA(top-down, dst 几何)。
    pub fn scale(&mut self, bgra: &[u8]) -> Result<Vec<u8>, String> {
        let need = self.src_w as usize * self.src_h as usize * 4;
        if bgra.len() < need {
            return Err(format!("输入像素不足: 需 {need} 实 {}", bgra.len()));
        }
        unsafe { self.scale_impl(bgra) }
    }

    unsafe fn scale_impl(&mut self, bgra: &[u8]) -> Result<Vec<u8>, String> {
        let fmt = DXGI_FORMAT_B8G8R8A8_UNORM;
        let mk_desc = |w: u32, h: u32| D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        // 输入纹理 + 上传
        let in_desc = mk_desc(self.src_w, self.src_h);
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: bgra.as_ptr() as *const core::ffi::c_void,
            SysMemPitch: self.src_w * 4,
            SysMemSlicePitch: 0,
        };
        let mut in_tex_opt = None;
        self.device
            .CreateTexture2D(&in_desc, Some(&init), Some(&mut in_tex_opt))
            .map_err(|e| format!("CreateTexture2D(输入) 失败: {e}"))?;
        let in_tex: ID3D11Texture2D = in_tex_opt.unwrap();
        let out_desc = mk_desc(self.dst_w, self.dst_h);
        let mut out_tex_opt = None;
        self.device
            .CreateTexture2D(&out_desc, None, Some(&mut out_tex_opt))
            .map_err(|e| format!("CreateTexture2D(输出) 失败: {e}"))?;
        let out_tex: ID3D11Texture2D = out_tex_opt.unwrap();

        // 输入/输出视图
        let in_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut in_view_opt = None;
        self.video_device
            .CreateVideoProcessorInputView(&in_tex, &self.enumerator, &in_view_desc, Some(&mut in_view_opt))
            .map_err(|e| format!("CreateVideoProcessorInputView 失败: {e}"))?;
        let in_view: ID3D11VideoProcessorInputView = in_view_opt.unwrap();
        let out_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: windows::Win32::Graphics::Direct3D11::D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut out_view_opt = None;
        self.video_device
            .CreateVideoProcessorOutputView(&out_tex, &self.enumerator, &out_view_desc, Some(&mut out_view_opt))
            .map_err(|e| format!("CreateVideoProcessorOutputView 失败: {e}"))?;
        let out_view: ID3D11VideoProcessorOutputView = out_view_opt.unwrap();

        // 流/输出矩形：全幅映射（SourceRect/DestRect 均为 RECT 语义）
        self.video_ctx.VideoProcessorSetStreamSourceRect(
            &self.vp,
            0,
            true,
            Some(&RECT {
                left: 0,
                top: 0,
                right: self.src_w as i32,
                bottom: self.src_h as i32,
            } as *const RECT),
        );
        self.video_ctx.VideoProcessorSetStreamDestRect(
            &self.vp,
            0,
            true,
            Some(&RECT {
                left: 0,
                top: 0,
                right: self.dst_w as i32,
                bottom: self.dst_h as i32,
            } as *const RECT),
        );
        let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: windows::core::BOOL(1),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: core::ptr::null_mut(),
            pInputSurface: core::mem::ManuallyDrop::new(Some(in_view.clone())),
            ppFutureSurfaces: core::ptr::null_mut(),
            pInputSurfaceRight: core::mem::ManuallyDrop::new(None),
            ppPastSurfacesRight: core::ptr::null_mut(),
            ppFutureSurfacesRight: core::ptr::null_mut(),
        }];
        self.video_ctx
            .VideoProcessorBlt(&self.vp, &out_view, 0, &streams)
            .map_err(|e| format!("VideoProcessorBlt 失败: {e}"))?;

        // 回读：staging 纹理 + Map
        let mut staging_desc = mk_desc(self.dst_w, self.dst_h);
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.BindFlags = 0;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging_opt = None;
        self.device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging_opt))
            .map_err(|e| format!("CreateTexture2D(staging) 失败: {e}"))?;
        let staging: ID3D11Texture2D = staging_opt.unwrap();
        self.ctx
            .CopyResource(&staging.cast::<ID3D11Resource>().unwrap(), &out_tex.cast::<ID3D11Resource>().unwrap());
        let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
        self.ctx
            .Map(&staging.cast::<ID3D11Resource>().unwrap(), 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("Map(staging) 失败: {e}"))?;
        let pitch = mapped.RowPitch as usize;
        let mut out = vec![0u8; self.dst_w as usize * self.dst_h as usize * 4];
        let src = mapped.pData as *const u8;
        for y in 0..self.dst_h as usize {
            core::ptr::copy_nonoverlapping(
                src.add(y * pitch),
                out.as_mut_ptr().add(y * self.dst_w as usize * 4),
                self.dst_w as usize * 4,
            );
        }
        self.ctx
            .Unmap(&staging.cast::<ID3D11Resource>().unwrap(), 0);
        Ok(out)
    }

    pub fn dst_geometry(&self) -> (u32, u32) {
        (self.dst_w, self.dst_h)
    }
}

/// GPU 缩放 + 失败自动回退（CPU box-average）。
/// 返回 (像素, 实际使用的路径："d3d11" | "cpu")。
/// 注意：GPU 路径（bilinear 类采样）与 CPU box-average 在非整数倍缩放下
/// 允许 ±1 级量化差异——需要逐像素一致时直接用 CPU 路径。
pub fn scale_with_fallback(
    bgra: &[u8],
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
) -> (Vec<u8>, &'static str) {
    if let Some(mut g) = GpuScaler::new(sw, sh, dw, dh) {
        if let Ok(out) = g.scale(bgra) {
            return (out, "d3d11");
        }
    }
    (crate::box_downscale_bgra(bgra, sw, sh, dw, dh), "cpu")
}
