//! AMD's FidelityFX API (`amd_fidelityfx_loader_xbox.dll`), served here.
//!
//! On Series S Grand Theft Auto VI renders below the display's size and has
//! AMD's upscaler (FSR 4, a neural network) bring the picture up to it. Its
//! programs come as AMD's own precompiled GFX10 code (`ACCC` parts, "FSPX"),
//! not in the console compiler's layout, so they do not translate: the
//! output stayed black, and the device was removed soon after. With the
//! upscaler's DLL refused the title divides by zero at the end of the
//! prologue's first cutscene.
//!
//! So the title's four calls are answered here (the effect is the public
//! FidelityFX API, `ffx_api.h` and `ffx_upscale.h`): contexts are plain
//! records, the queries answer as the SDK's upscaler does but with no
//! jitter (nothing accumulates frames here, so a jittered camera would
//! shake), and an upscale dispatch records a pass that samples the render
//! area of the colour input, Catmull-Rom (bilinear blurred a 720p image
//! stretched to 1080p), clamped to the four nearest texels, into the output.
//! shortcut: no temporal accumulation, no sharpening; the picture is the
//! render resolution scaled up.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::host::{HostList, host};
use crate::object::This;

const OK: i32 = 0;
const ERROR: i32 = 1;
const UNKNOWN_DESCTYPE: i32 = 2;
const PARAMETER: i32 = 6;

const CREATE_UPSCALE: u64 = 0x0001_0000;
const DISPATCH_UPSCALE: u64 = 0x0001_0001;
const QUERY_RATIO: u64 = 0x0001_0002;
const QUERY_RENDER_SIZE: u64 = 0x0001_0003;
const QUERY_PHASE_COUNT: u64 = 0x0001_0004;
const QUERY_JITTER: u64 = 0x0001_0005;
const DISPATCH_REACTIVE: u64 = 0x0001_0006;
const QUERY_MEMORY: u64 = 0x0001_0008;
const QUERY_MEMORY_V2: u64 = 0x0001_0009;
/// `FFX_API_QUERY_DESC_TYPE_GET_VERSIONS`.
const QUERY_VERSIONS: u64 = 4;

/// A description's header: its type, then the next in its chain.
#[repr(C)]
pub struct Header {
    kind: u64,
    next: *const Header,
}

/// `FfxApiResource`: the resource, its description, its state.
#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: This,
    /// type, format, width, height, depth, mips, flags, usage.
    description: [u32; 8],
    state: u32,
}

/// `ffxDispatchDescUpscale`.
#[repr(C)]
struct DispatchUpscale {
    header: Header,
    list: This,
    color: Resource,
    depth: Resource,
    motion: Resource,
    exposure: Resource,
    reactive: Resource,
    transparency: Resource,
    output: Resource,
    jitter: [f32; 2],
    motion_scale: [f32; 2],
    render: [u32; 2],
    upscale: [u32; 2],
    sharpen: bool,
    sharpness: f32,
    frame_time: f32,
    pre_exposure: f32,
    reset: bool,
    near: f32,
    far: f32,
    fov: f32,
    meters: f32,
    flags: u32,
}

/// A context: what the title created it with.
struct Context {
    max_render: [u32; 2],
    max_upscale: [u32; 2],
}

/// The upscale ratio of an FSR quality mode (native AA, quality,
/// balanced, performance, ultra performance).
fn ratio(mode: u32) -> Option<f32> {
    [1.0, 1.5, 1.7, 2.0, 3.0].get(mode as usize).copied()
}

/// `ffxCreateContext(context, description, allocators)`.
pub extern "system" fn create_context(out: *mut *mut c_void, desc: *const Header, _alloc: *const c_void) -> i32 {
    // SAFETY: the title passes its output and a description chain.
    let (Some(out), Some(header)) = (unsafe { out.as_mut() }, unsafe { desc.as_ref() }) else {
        return PARAMETER;
    };
    if header.kind != CREATE_UPSCALE {
        tracing::warn!(kind = format_args!("{:#x}", header.kind), "FidelityFX: a context of another effect refused");
        return UNKNOWN_DESCTYPE;
    }
    // ffxCreateContextDescUpscale: header, u32 flags, render size, upscale size.
    // SAFETY: the title's description, of that type.
    let words = unsafe { std::slice::from_raw_parts(desc.cast::<u32>(), 9) };
    let context = Context {
        max_render: [words[5], words[6]],
        max_upscale: [words[7], words[8]],
    };
    tracing::info!(
        flags = format_args!("{:#x}", words[4]),
        render = ?context.max_render,
        upscale = ?context.max_upscale,
        "FidelityFX: upscaler context made (here: a Catmull-Rom pass)"
    );
    *out = Box::into_raw(Box::new(context)).cast();
    OK
}

/// `ffxDestroyContext(context, allocators)`.
pub extern "system" fn destroy_context(context: *mut *mut c_void, _alloc: *const c_void) -> i32 {
    // SAFETY: the title passes a context it made with `create_context`.
    if let Some(context) = unsafe { context.as_mut() }
        && !context.is_null()
    {
        // SAFETY: made by `Box::into_raw` above.
        drop(unsafe { Box::from_raw(context.cast::<Context>()) });
        *context = std::ptr::null_mut();
    }
    OK
}

/// `ffxConfigure(context, description)`: key-value settings, taken.
pub extern "system" fn configure(_context: *mut *mut c_void, desc: *const Header) -> i32 {
    // SAFETY: the title's description, if any.
    if let Some(header) = unsafe { desc.as_ref() } {
        once_per_kind(header.kind, "configure");
    }
    OK
}

/// `ffxQuery(context, description)`.
pub extern "system" fn query(_context: *mut *mut c_void, desc: *mut Header) -> i32 {
    // SAFETY: the title's description.
    let Some(header) = (unsafe { desc.as_ref() }) else {
        return PARAMETER;
    };
    once_per_kind(header.kind, "query");
    let words = desc.cast::<u32>();
    // SAFETY: each type's fields as `ffx_upscale.h` lays them out after the
    // 16-byte header; output pointers the title passes.
    unsafe {
        match header.kind {
            QUERY_RATIO => {
                let Some(r) = ratio(*words.add(4)) else { return PARAMETER };
                write(*desc.cast::<u8>().add(24).cast::<*mut f32>(), r);
            }
            QUERY_RENDER_SIZE => {
                let (w, h, mode) = (*words.add(4), *words.add(5), *words.add(6));
                let Some(r) = ratio(mode) else { return PARAMETER };
                let base = desc.cast::<u8>().add(32);
                write(*base.cast::<*mut u32>(), (w as f32 / r) as u32);
                write(*base.add(8).cast::<*mut u32>(), (h as f32 / r) as u32);
            }
            QUERY_PHASE_COUNT => write(*desc.cast::<u8>().add(24).cast::<*mut i32>(), 1),
            QUERY_JITTER => {
                let base = desc.cast::<u8>().add(24);
                write(*base.cast::<*mut f32>(), 0.0);
                write(*base.add(8).cast::<*mut f32>(), 0.0);
            }
            QUERY_MEMORY | QUERY_MEMORY_V2 => {}
            QUERY_VERSIONS => {}
            _ => return UNKNOWN_DESCTYPE,
        }
    }
    OK
}

fn write<T>(to: *mut T, value: T) {
    if !to.is_null() {
        // SAFETY: an output the title passes.
        unsafe { to.write_unaligned(value) };
    }
}

/// Logs the first description of each type, to see what the title asks.
fn once_per_kind(kind: u64, what: &str) {
    static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.contains(&kind) {
        seen.push(kind);
        tracing::info!(kind = format_args!("{kind:#x}"), what, "FidelityFX: first description of a type");
    }
}

/// `ffxDispatch(context, description)`.
pub extern "system" fn dispatch(_context: *mut *mut c_void, desc: *const Header) -> i32 {
    // SAFETY: the title's description.
    let Some(header) = (unsafe { desc.as_ref() }) else {
        return PARAMETER;
    };
    once_per_kind(header.kind, "dispatch");
    match header.kind {
        DISPATCH_UPSCALE => {}
        DISPATCH_REACTIVE => return OK,
        _ => return UNKNOWN_DESCTYPE,
    }
    // SAFETY: a description of that type.
    let d = unsafe { &*desc.cast::<DispatchUpscale>() };
    static DISPATCHES: AtomicU64 = AtomicU64::new(0);
    if DISPATCHES.fetch_add(1, Ordering::Relaxed) == 0 {
        tracing::info!(
            render = ?d.render,
            upscale = ?d.upscale,
            color = ?d.color.resource,
            color_size = ?&d.color.description[2..4],
            output = ?d.output.resource,
            output_size = ?&d.output.description[2..4],
            pre_exposure = d.pre_exposure,
            "FidelityFX: first upscale"
        );
    }
    if d.color.resource.is_null() || d.output.resource.is_null() {
        return PARAMETER;
    }
    let done = crate::list::with_host_state(d.list, |list, state| {
        let ran = scale(list, d.color.resource, d.output.resource, d.render, d.upscale);
        if ran {
            state.forget_host_bindings();
        }
        ran
    });
    if done == Some(true) { OK } else { ERROR }
}

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0)), DescriptorTable(UAV(u0)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer Sizes : register(b0) { float2 render; float2 upscale; };
Texture2D<float4> color : register(t0);
RWTexture2D<float4> output : register(u0);
SamplerState linear_clamp : register(s0);
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 at : SV_DispatchThreadID) {
    if (at.x >= (uint)upscale.x || at.y >= (uint)upscale.y) return;
    float w, h;
    color.GetDimensions(w, h);
    float2 size = float2(w, h);
    // The render area is the top left of the colour input; in its texels.
    float2 p = (float2(at.xy) + 0.5) / upscale * render;
    // Catmull-Rom, nine taps through the bilinear filter (the middle two
    // texels of each row and column taken as one tap).
    float2 t1 = floor(p - 0.5) + 0.5;
    float2 f = p - t1;
    float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    float2 w3 = f * f * (-0.5 + 0.5 * f);
    float2 w12 = w1 + w2;
    float2 lo = 0.5, hi = render - 0.5;
    float2 t0 = clamp(t1 - 1.0, lo, hi) / size;
    float2 t3 = clamp(t1 + 2.0, lo, hi) / size;
    float2 t12 = clamp(t1 + w2 / w12, lo, hi) / size;
    float4 c = 0;
    c += color.SampleLevel(linear_clamp, float2(t0.x, t0.y), 0) * (w0.x * w0.y);
    c += color.SampleLevel(linear_clamp, float2(t12.x, t0.y), 0) * (w12.x * w0.y);
    c += color.SampleLevel(linear_clamp, float2(t3.x, t0.y), 0) * (w3.x * w0.y);
    c += color.SampleLevel(linear_clamp, float2(t0.x, t12.y), 0) * (w0.x * w12.y);
    c += color.SampleLevel(linear_clamp, float2(t12.x, t12.y), 0) * (w12.x * w12.y);
    c += color.SampleLevel(linear_clamp, float2(t3.x, t12.y), 0) * (w3.x * w12.y);
    c += color.SampleLevel(linear_clamp, float2(t0.x, t3.y), 0) * (w0.x * w3.y);
    c += color.SampleLevel(linear_clamp, float2(t12.x, t3.y), 0) * (w12.x * w3.y);
    c += color.SampleLevel(linear_clamp, float2(t3.x, t3.y), 0) * (w3.x * w3.y);
    // No ringing: within the four nearest texels (HDR highlights would
    // otherwise leave dark halos).
    int2 q = int2(clamp(t1, lo, hi - 1.0));
    float4 a = color.Load(int3(q, 0)), b = color.Load(int3(q + int2(1, 0), 0));
    float4 d = color.Load(int3(q + int2(0, 1), 0)), e = color.Load(int3(q + int2(1, 1), 0));
    float4 mn = min(min(a, b), min(d, e)), mx = max(max(a, b), max(d, e));
    output[at.xy] = clamp(c, mn, mx);
}
"#;

/// Views, used in pairs, in turn.
const SLOTS: u32 = 512;

struct Pass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

fn make(device: &ID3D12Device) -> windows::core::Result<Pass> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; out pointers ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("upscale"),
            None,
            None,
            s!("main"),
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    let bytes = |b: &ID3DBlob| {
        // SAFETY: the blob's own bytes.
        unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }.to_vec()
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            tracing::warn!("upscale pass: {}", String::from_utf8_lossy(&bytes(&errors)));
        }
        return Err(e);
    }
    let code = bytes(&code.ok_or_else(windows::core::Error::empty)?);
    // SAFETY: the root signature is inside the shader; plain creations.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code)?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: code.as_ptr().cast(),
                BytecodeLength: code.len(),
            },
            ..Default::default()
        })?;
        let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: SLOTS * 2,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?;
        Ok(Pass {
            root,
            pso,
            views,
            step: device.GetDescriptorHandleIncrementSize(kind),
            next: AtomicU32::new(0),
        })
    }
}

/// The format a view of a texture of format `f` reads or writes as.
fn typed(f: DXGI_FORMAT) -> DXGI_FORMAT {
    match f {
        DXGI_FORMAT_R16G16B16A16_TYPELESS => DXGI_FORMAT_R16G16B16A16_FLOAT,
        DXGI_FORMAT_R32G32B32A32_TYPELESS => DXGI_FORMAT_R32G32B32A32_FLOAT,
        DXGI_FORMAT_R10G10B10A2_TYPELESS => DXGI_FORMAT_R10G10B10A2_UNORM,
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => DXGI_FORMAT_B8G8R8A8_UNORM,
        other => other,
    }
}

const SHADER_RESOURCE: u32 = 0x40;
const UNORDERED_ACCESS: u32 = 0x8;

/// Records the scaling of `color`'s render area into `output` in `list`.
/// Whether it recorded anything (the list's bindings are then ours).
fn scale(list: &HostList, color: This, output: This, render: [u32; 2], upscale: [u32; 2]) -> bool {
    let Some(host) = host() else { return false };
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    let Some(pass) = PASS
        .get_or_init(|| {
            make(&host.device)
                .inspect_err(|e| tracing::warn!(error = %e, "no upscale pass"))
                .ok()
        })
        .as_ref()
    else {
        return false;
    };
    let (Some(from), Some(to)) = (crate::resource::host_resource(color), crate::resource::host_resource(output)) else {
        return false;
    };
    // SAFETY: live resources.
    let (f, t) = unsafe { (from.GetDesc(), to.GetDesc()) };
    let upscale = [
        if upscale[0] == 0 { t.Width as u32 } else { upscale[0].min(t.Width as u32) },
        if upscale[1] == 0 { t.Height } else { upscale[1].min(t.Height) },
    ];
    let render = [
        if render[0] == 0 { f.Width as u32 } else { render[0].min(f.Width as u32) },
        if render[1] == 0 { f.Height } else { render[1].min(f.Height) },
    ];
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| tracing::info!(color = f.Format.0, output = t.Format.0, ?render, ?upscale, "upscale pass: host formats"));
    if t.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!(format = t.Format.0, "upscale: the output takes no unordered access"));
        return false;
    }
    list.keep(from);
    list.keep(to);
    crate::list::ensure_state(list, color, SHADER_RESOURCE);
    crate::list::ensure_state(list, output, UNORDERED_ACCESS);
    let slot = pass.next.fetch_add(1, Ordering::Relaxed) % SLOTS;
    let l = &list.list;
    // SAFETY: an open list; live textures the list keeps, in the states
    // set above; views in slots no list in flight reads.
    unsafe {
        let cpu = pass.views.GetCPUDescriptorHandleForHeapStart();
        let gpu = pass.views.GetGPUDescriptorHandleForHeapStart();
        let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu.ptr + ((slot * 2 + i) * pass.step) as usize,
        };
        let read = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: typed(f.Format),
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MipLevels: 1,
                    ..Default::default()
                },
            },
        };
        host.device.CreateShaderResourceView(from, Some(&read), at(0));
        let write = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: typed(t.Format),
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        host.device
            .CreateUnorderedAccessView(to, None::<&ID3D12Resource>, Some(&write), at(1));
        l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
        l.SetComputeRootSignature(&pass.root);
        l.SetPipelineState(&pass.pso);
        let sizes = [render[0] as f32, render[1] as f32, upscale[0] as f32, upscale[1] as f32];
        l.SetComputeRoot32BitConstants(0, 4, sizes.as_ptr().cast(), 0);
        let table = |i: u32| D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: gpu.ptr + u64::from((slot * 2 + i) * pass.step),
        };
        l.SetComputeRootDescriptorTable(1, table(0));
        l.SetComputeRootDescriptorTable(2, table(1));
        l.Dispatch(upscale[0].div_ceil(8), upscale[1].div_ceil(8), 1);
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            ..Default::default()
        };
        l.ResourceBarrier(&[barrier]);
    }
    crate::texture::host_written(output);
    true
}

#[cfg(test)]
mod tests {
    /// The pass compiles with the system's compiler and the host takes it.
    #[test]
    fn upscale_pass_builds() {
        let Some(host) = crate::host::host() else {
            return;
        };
        super::make(&host.device).expect("the upscale pass");
    }

    #[test]
    fn the_dispatch_description_has_the_sdk_layout() {
        assert_eq!(std::mem::offset_of!(super::DispatchUpscale, color), 24);
        assert_eq!(std::mem::offset_of!(super::DispatchUpscale, output), 312);
        assert_eq!(std::mem::offset_of!(super::DispatchUpscale, render), 376);
        assert_eq!(std::mem::offset_of!(super::DispatchUpscale, pre_exposure), 404);
        assert_eq!(std::mem::offset_of!(super::DispatchUpscale, flags), 428);
    }
}
