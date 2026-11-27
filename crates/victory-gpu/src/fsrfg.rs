//! AMD FidelityFX frame generation (FSR 3) on our present, for the cards
//! DLSS Frame Generation refuses (an RTX 2060 SUPER: Streamline says "not
//! supported on current hardware"). It runs on any Direct3D 12 card.
//!
//! - `VICTORY_FSR_FG=1` turns it on. The swap chain is wrapped by
//!   FidelityFX's ([`wrap`]); every upscale the title asks for
//!   (`victory_hle::modules::fidelityfx`, through [`crate::streamline`])
//!   records FidelityFX's preparation of that frame (depth, motion vectors,
//!   camera) into the title's list where it upscales ([`upscale`]); every
//!   present tells the swap chain which frame it shows ([`present`]), and
//!   the swap chain shows a generated frame between it and the one before.
//! - `VICTORY_FFX_DIR`: the folder of `amd_fidelityfx_loader_dx12.dll` and
//!   `amd_fidelityfx_framegeneration_dx12.dll` (AMD's signed files from the
//!   FidelityFX SDK, MIT licence; `scripts\get-amd-fsr.ps1` puts them in
//!   `amd-sdk\FidelityFX\bin`, the default).
//!
//! The frame rate the layer counts stays the rendered one; the log says how
//! many frames the swap chain showed a second. The interface plane is
//! generated with the picture (no separate UI), and a change of the window's
//! size keeps the first size's context.
//!
//! Anything missing (the folder, a context) leaves our plain present, with
//! a warning in the log.
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{DXGI_FRAME_STATISTICS, IDXGISwapChain3, IDXGISwapChain4};
use windows::core::Interface;

use crate::streamline::{DEPTH, MOTION, Upscale};

/// The default folder of AMD's files, under the repository.
const DEFAULT_DIR: &str = r"amd-sdk\FidelityFX\bin";
const LOADER: &str = "amd_fidelityfx_loader_dx12.dll";

// Structure types (ffx_api.h, ffx_framegeneration.h, ffx_api_framegeneration_dx12.h).
const CREATE_BACKEND_DX12: u64 = 0x0000_0002;
const CREATE_FRAMEGENERATION: u64 = 0x0002_0001;
const CONFIGURE_FRAMEGENERATION: u64 = 0x0002_0002;
const DISPATCH_PREPARE_V2: u64 = 0x0002_000c;
const CREATE_FRAMEGENERATION_VERSION: u64 = 0x0002_000e;
const CREATE_SWAPCHAIN_WRAP_DX12: u64 = 0x0003_0001;
const CREATE_SWAPCHAIN_VERSION_DX12: u64 = 0x0003_000b;
/// The API versions this was written against (4.0.2 and 4.0.0).
const FRAMEGENERATION_VERSION: u32 = (4 << 22) | 2;
const SWAPCHAIN_VERSION: u32 = 4 << 22;

// Creation flags: the same bits as the upscaler's for motion vectors and depth.
const FLAGS_FROM_UPSCALER: u32 = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4);
/// Internal resources kept per frame identifier (`..._ASYNC_WORKLOAD_SUPPORT`):
/// a frame is prepared while the swap chain's thread generates the last one.
const FLAG_ASYNC_SUPPORT: u32 = 1 << 0;
/// Frames whose depth and motion vectors are held after their preparation.
const HELD_FRAMES: usize = 8;

const TEXTURE2D: u32 = 2;
const STATE_COMMON: u32 = 1 << 0;
const STATE_UNORDERED_ACCESS: u32 = 1 << 1;
const STATE_PIXEL_COMPUTE_READ: u32 = (1 << 2) | (1 << 3);
const STATE_COPY_SRC: u32 = 1 << 4;
const STATE_GENERIC_READ: u32 = (1 << 4) | (1 << 2);

type Context = *mut c_void;

#[repr(C)]
struct Header {
    kind: u64,
    next: *mut Header,
}

impl Header {
    const fn new(kind: u64) -> Self {
        Self { kind, next: std::ptr::null_mut() }
    }
}

type CreateFn = unsafe extern "C" fn(*mut Context, *mut Header, *const c_void) -> u32;
type ConfigureFn = unsafe extern "C" fn(*mut Context, *const Header) -> u32;
type DispatchFn = unsafe extern "C" fn(*mut Context, *const Header) -> u32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Size {
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Coords {
    x: f32,
    y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Description {
    kind: u32,
    format: u32,
    width: u32,
    height: u32,
    depth: u32,
    mips: u32,
    flags: u32,
    usage: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: *mut c_void,
    description: Description,
    state: u32,
}

impl Resource {
    const EMPTY: Resource = Resource { resource: std::ptr::null_mut(), description: Description { kind: 0, format: 0, width: 0, height: 0, depth: 0, mips: 0, flags: 0, usage: 0 }, state: 0 };

    /// A texture of ours in host state `state`, as FidelityFX describes it.
    fn texture(texture: &ID3D12Resource, state: u32) -> Resource {
        // SAFETY: a live resource.
        let desc = unsafe { texture.GetDesc() };
        Resource {
            resource: texture.as_raw(),
            description: Description {
                kind: TEXTURE2D,
                format: format(desc.Format),
                width: desc.Width as u32,
                height: desc.Height,
                depth: 1,
                mips: 1,
                flags: 0,
                usage: 0,
            },
            state: ffx_state(state),
        }
    }
}

#[repr(C)]
struct BackendDesc {
    header: Header,
    device: *mut c_void,
}

#[repr(C)]
struct VersionDesc {
    header: Header,
    version: u32,
}

#[repr(C)]
struct CreateDesc {
    header: Header,
    flags: u32,
    display: Size,
    max_render: Size,
    back_buffer_format: u32,
}

#[repr(C)]
struct WrapDesc {
    header: Header,
    swap_chain: *mut *mut c_void,
    queue: *mut c_void,
}

type FrameGenerationCallback = unsafe extern "C" fn(*mut Header, *mut c_void) -> u32;

#[repr(C)]
struct ConfigureDesc {
    header: Header,
    swap_chain: *mut c_void,
    present_callback: *const c_void,
    present_context: *mut c_void,
    frame_generation_callback: Option<FrameGenerationCallback>,
    frame_generation_context: *mut c_void,
    enabled: bool,
    allow_async: bool,
    hudless: Resource,
    flags: u32,
    only_generated: bool,
    rect: Rect,
    frame: u64,
}

#[repr(C)]
struct PrepareDesc {
    header: Header,
    frame: u64,
    flags: u32,
    list: *mut c_void,
    render: Size,
    jitter: Coords,
    motion_scale: Coords,
    frame_ms: f32,
    reset: bool,
    near: f32,
    far: f32,
    fov_y: f32,
    metres_per_unit: f32,
    depth: Resource,
    motion: Resource,
    position: [f32; 3],
    up: [f32; 3],
    right: [f32; 3],
    forward: [f32; 3],
}

/// AMD's loader, loaded.
struct Ffx {
    create: CreateFn,
    configure: ConfigureFn,
    dispatch: DispatchFn,
}

// SAFETY: plain function pointers of a module that stays loaded.
unsafe impl Send for Ffx {}
unsafe impl Sync for Ffx {}

/// Whether FidelityFX frame generation is asked for (`VICTORY_FSR_FG=1`).
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_FSR_FG"))
}

fn ffx() -> Option<&'static Ffx> {
    static FFX: OnceLock<Option<Ffx>> = OnceLock::new();
    FFX.get_or_init(|| match load() {
        Ok(ffx) => Some(ffx),
        Err(why) => {
            tracing::warn!("FSR frame generation: {why}; presenting without it");
            None
        }
    })
    .as_ref()
}

fn load() -> Result<Ffx, String> {
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LoadLibraryExW, SetDllDirectoryW,
    };
    let dir = std::env::var_os("VICTORY_FFX_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| DEFAULT_DIR.into());
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("no folder {} ({e})", dir.display()))?;
    let path = dir.join(LOADER);
    let wide = |p: &std::path::Path| {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>()
    };
    // The loader finds the frame generation library beside it: its folder
    // is searched by later loads, in place of the current folder.
    let folder = wide(&dir);
    // SAFETY: a NUL-terminated path.
    unsafe { SetDllDirectoryW(folder.as_ptr()) };
    let file = wide(&path);
    // SAFETY: a NUL-terminated full path.
    let module = unsafe { LoadLibraryExW(file.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) };
    if module.is_null() {
        return Err(format!("{} did not load ({})", path.display(), std::io::Error::last_os_error()));
    }
    let find = |name: &std::ffi::CStr| {
        // SAFETY: a loaded module and a NUL-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr().cast()) }.ok_or_else(|| format!("{LOADER} has no {}", name.to_string_lossy()))
    };
    // SAFETY: the exports have the header's signatures.
    let ffx = unsafe {
        Ffx {
            create: std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateFn>(find(c"ffxCreateContext")?),
            configure: std::mem::transmute::<unsafe extern "system" fn() -> isize, ConfigureFn>(find(c"ffxConfigure")?),
            dispatch: std::mem::transmute::<unsafe extern "system" fn() -> isize, DispatchFn>(find(c"ffxDispatch")?),
        }
    };
    tracing::info!(library = %path.display(), "FSR frame generation: AMD's loader loaded");
    Ok(ffx)
}

/// What frame generation runs with: the wrapped swap chain (not owned: the
/// presenter holds it), its size and format, and the context once made.
struct State {
    swap_chain: usize,
    display: [u32; 2],
    format: u32,
    /// The frame generation context, boxed so that its address can be the
    /// callback's.
    context: Option<&'static mut Context>,
    failed: bool,
}

// SAFETY: the context is FidelityFX's handle, used under the lock.
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State { swap_chain: 0, display: [0, 0], format: 0, context: None, failed: false });
/// The last frame prepared, and the last one a present told the swap chain.
static PREPARED: AtomicU64 = AtomicU64::new(0);
static CONFIGURED: AtomicU64 = AtomicU64::new(0);
static NEXT_FRAME: AtomicU64 = AtomicU64::new(0);
/// Calls into the context, one at a time: a preparation on a recording
/// thread and a generation on the swap chain's thread had run in it at once
/// and both faulted in FidelityFX's library (a read of freed memory).
static CALLS: Mutex<()> = Mutex::new(());
/// The textures the last frames were prepared from, held so that none of
/// them goes while FidelityFX may still read it.
static HELD: Mutex<std::collections::VecDeque<[ID3D12Resource; 2]>> = Mutex::new(std::collections::VecDeque::new());

fn one_call() -> std::sync::MutexGuard<'static, ()> {
    CALLS.lock().unwrap_or_else(|e| e.into_inner())
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The swap chain just made, wrapped by FidelityFX's (which then presents
/// the generated frames on its own queue), or as it was if that failed. It
/// must be the only reference to it: FidelityFX makes the window's swap
/// chain again.
pub(crate) fn wrap(swap_chain: IDXGISwapChain3, queue: &ID3D12CommandQueue) -> IDXGISwapChain3 {
    let Some(ffx) = ffx() else { return swap_chain };
    let Ok(four) = swap_chain.cast::<IDXGISwapChain4>() else {
        tracing::warn!("FSR frame generation: the swap chain has no IDXGISwapChain4; presenting without it");
        return swap_chain;
    };
    // SAFETY: a live swap chain.
    let Ok(desc) = (unsafe { four.GetDesc1() }) else { return swap_chain };
    drop(swap_chain);
    let mut raw = four.into_raw();
    let mut version = VersionDesc { header: Header::new(CREATE_SWAPCHAIN_VERSION_DX12), version: SWAPCHAIN_VERSION };
    let mut wrap = WrapDesc { header: Header::new(CREATE_SWAPCHAIN_WRAP_DX12), swap_chain: &mut raw, queue: queue.as_raw() };
    wrap.header.next = &mut version.header;
    let mut context: Context = std::ptr::null_mut();
    // SAFETY: filled descriptions that live through the call; `raw` holds
    // our only reference, which FidelityFX takes and replaces with its own.
    let code = unsafe { (ffx.create)(&mut context, &mut wrap.header, std::ptr::null()) };
    // SAFETY: `raw` holds one reference either way (ours back, or the new one).
    let wrapped = unsafe { IDXGISwapChain4::from_raw(raw) };
    if code != 0 {
        tracing::warn!(code, "FSR frame generation: the swap chain was not wrapped; presenting without it");
        return wrapped.cast().expect("a swap chain");
    }
    let mut s = state();
    s.swap_chain = wrapped.as_raw() as usize;
    s.display = [desc.Width, desc.Height];
    s.format = format(desc.Format);
    tracing::info!(width = desc.Width, height = desc.Height, format = desc.Format.0, "FSR frame generation: the swap chain is FidelityFX's");
    wrapped.cast().expect("a swap chain")
}

/// The context, made at the first preparation (when the camera's flags are
/// known), on the host device.
fn context(s: &mut State, ffx: &Ffx, camera_flags: u32) -> Option<*mut Context> {
    if let Some(c) = s.context.as_deref_mut() {
        return Some(c as *mut Context);
    }
    if s.failed || s.swap_chain == 0 {
        return None;
    }
    let host = crate::host::host()?;
    let mut backend = BackendDesc { header: Header::new(CREATE_BACKEND_DX12), device: host.device.as_raw() };
    let mut version = VersionDesc { header: Header::new(CREATE_FRAMEGENERATION_VERSION), version: FRAMEGENERATION_VERSION };
    backend.header.next = &mut version.header;
    let size = Size { width: s.display[0], height: s.display[1] };
    let flags = (camera_flags & FLAGS_FROM_UPSCALER) | FLAG_ASYNC_SUPPORT;
    let mut create = CreateDesc { header: Header::new(CREATE_FRAMEGENERATION), flags, display: size, max_render: size, back_buffer_format: s.format };
    create.header.next = &mut backend.header;
    let context: &'static mut Context = Box::leak(Box::new(std::ptr::null_mut()));
    // SAFETY: filled descriptions that live through the call.
    let code = unsafe { (ffx.create)(context, &mut create.header, std::ptr::null()) };
    if code != 0 {
        s.failed = true;
        tracing::warn!(code, "FSR frame generation: no context; presenting without it");
        return None;
    }
    tracing::info!(flags, display = ?s.display, "FSR frame generation: context made");
    let at: *mut Context = context;
    s.context = Some(context);
    Some(at)
}

/// The title upscales: FidelityFX's preparation of this frame (its depth
/// and motion vectors, the camera) is recorded into the title's list there.
pub(crate) fn upscale(up: &Upscale) {
    if !on() || ffx().is_none() || state().swap_chain == 0 {
        return;
    }
    if up.resources[DEPTH].0.is_null() || up.resources[MOTION].0.is_null() || up.camera.render.contains(&0) {
        return;
    }
    let up = *up;
    crate::list::external(up.list, Box::new(move |host| prepare(host, &up)));
}

fn prepare(host: &crate::host::HostList, up: &Upscale) -> bool {
    let Some(ffx) = ffx() else { return false };
    let mut inputs = [Resource::EMPTY; 2];
    let mut textures: Vec<ID3D12Resource> = Vec::with_capacity(2);
    for (n, i) in [DEPTH, MOTION].into_iter().enumerate() {
        let (resource, state) = up.resources[i];
        if crate::external::prepare_input(host, resource).is_none() {
            return false;
        }
        let wanted = crate::streamline::plan::input_state(state);
        let Some((texture, legal)) = crate::external::host_texture(host, resource, wanted) else { return false };
        inputs[n] = Resource::texture(&texture, legal);
        textures.push(texture);
    }
    let camera = up.camera.planes_in_order();
    let mut s = state();
    let Some(context) = context(&mut s, ffx, camera.flags) else { return false };
    let frame = NEXT_FRAME.fetch_add(1, Relaxed) + 1;
    let desc = PrepareDesc {
        header: Header::new(DISPATCH_PREPARE_V2),
        frame,
        flags: 0,
        list: host.list.as_raw(),
        render: Size { width: camera.render[0], height: camera.render[1] },
        jitter: Coords { x: camera.jitter[0], y: camera.jitter[1] },
        motion_scale: Coords { x: camera.motion_scale[0], y: camera.motion_scale[1] },
        frame_ms: up.frame_ms,
        reset: camera.reset,
        near: camera.near,
        far: camera.far,
        fov_y: camera.fov_y,
        metres_per_unit: 1.0,
        depth: inputs[0],
        motion: inputs[1],
        position: [0.0; 3],
        up: [0.0, 1.0, 0.0],
        right: [1.0, 0.0, 0.0],
        forward: [0.0, 0.0, 1.0],
    };
    // SAFETY: a made context, an open list, textures alive through it.
    let code = {
        let _one = one_call();
        unsafe { (ffx.dispatch)(context, &desc.header) }
    };
    drop(s);
    if let Ok(pair) = <[ID3D12Resource; 2]>::try_from(textures) {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        held.push_back(pair);
        while held.len() > HELD_FRAMES {
            held.pop_front();
        }
    }
    if code != 0 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!(code, "FSR frame generation: the preparation was refused"));
        return true;
    }
    if frame == 1 {
        tracing::info!(render = ?camera.render, motion_scale = ?camera.motion_scale, jitter = ?camera.jitter, near = camera.near, far = camera.far, fov = camera.fov_y, flags = camera.flags, "FSR frame generation: first frame prepared");
    }
    PREPARED.store(frame, Relaxed);
    true
}

/// FidelityFX's frame generation, called by its swap chain at the present.
unsafe extern "C" fn generate(params: *mut Header, user: *mut c_void) -> u32 {
    let Some(ffx) = ffx() else { return 1 };
    let _one = one_call();
    // SAFETY: the context the configuration named, and its description.
    unsafe { (ffx.dispatch)(user.cast(), params) }
}

/// Right before our present: the swap chain is told the frame it shows, and
/// to generate one before it if a new frame was prepared since the last.
pub(crate) fn present(width: u32, height: u32) {
    if !on() {
        return;
    }
    let Some(ffx) = ffx() else { return };
    let mut s = state();
    let Some(context) = s.context.as_deref_mut().map(|c| c as *mut Context) else { return };
    let frame = PREPARED.load(Relaxed);
    let last = CONFIGURED.swap(frame, Relaxed);
    let desc = ConfigureDesc {
        header: Header::new(CONFIGURE_FRAMEGENERATION),
        swap_chain: s.swap_chain as *mut c_void,
        present_callback: std::ptr::null(),
        present_context: std::ptr::null_mut(),
        frame_generation_callback: Some(generate),
        frame_generation_context: context.cast(),
        enabled: frame != 0 && frame != last,
        allow_async: false,
        hudless: Resource::EMPTY,
        flags: 0,
        only_generated: false,
        rect: Rect { left: 0, top: 0, width: width as i32, height: height as i32 },
        frame,
    };
    // SAFETY: a made context, a description alive through the call.
    let code = {
        let _one = one_call();
        unsafe { (ffx.configure)(context, &desc.header) }
    };
    let swap_chain = s.swap_chain;
    drop(s);
    if code != 0 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!(code, "FSR frame generation: the configuration was refused"));
    }
    shown(swap_chain);
}

/// Once a second: how many frames the swap chain showed (rendered and
/// generated), from its own count.
fn shown(swap_chain: usize) {
    static LAST: Mutex<Option<(std::time::Instant, u32)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if last.is_some_and(|(at, _)| now.duration_since(at).as_secs_f64() < 1.0) {
        return;
    }
    // SAFETY: the presenter holds the swap chain; borrowed for the call.
    let raw = swap_chain as *mut c_void;
    let chain = unsafe { IDXGISwapChain4::from_raw_borrowed(&raw) };
    let mut stats = DXGI_FRAME_STATISTICS::default();
    // SAFETY: a live swap chain and a structure to fill.
    if chain.is_none_or(|c| unsafe { c.GetFrameStatistics(&mut stats) }.is_err()) {
        *last = Some((now, 0));
        return;
    }
    if let Some((at, count)) = *last
        && count != 0
    {
        let seconds = now.duration_since(at).as_secs_f64();
        tracing::info!(
            shown = format_args!("{:.1}", f64::from(stats.PresentCount.wrapping_sub(count)) / seconds),
            "FSR frame generation: frames shown a second"
        );
    }
    *last = Some((now, stats.PresentCount));
}

/// A host resource state as FidelityFX names it.
fn ffx_state(state: u32) -> u32 {
    match state {
        0 => STATE_COMMON,
        0x8 => STATE_UNORDERED_ACCESS,
        0x800 => STATE_COPY_SRC,
        0xac3 => STATE_GENERIC_READ,
        _ => STATE_PIXEL_COMPUTE_READ,
    }
}

/// A DXGI format as FidelityFX's surface format (ffx_api_dx12.h's table).
fn format(f: DXGI_FORMAT) -> u32 {
    match f {
        DXGI_FORMAT_R32G32B32A32_TYPELESS => 1,
        DXGI_FORMAT_R32G32B32A32_UINT => 2,
        DXGI_FORMAT_R32G32B32A32_FLOAT => 3,
        DXGI_FORMAT_R16G16B16A16_FLOAT => 4,
        DXGI_FORMAT_R32G32_FLOAT => 6,
        DXGI_FORMAT_R8_UINT | DXGI_FORMAT_X32_TYPELESS_G8X24_UINT | DXGI_FORMAT_X24_TYPELESS_G8_UINT => 7,
        DXGI_FORMAT_R32_UINT | DXGI_FORMAT_R24G8_TYPELESS | DXGI_FORMAT_D24_UNORM_S8_UINT | DXGI_FORMAT_R24_UNORM_X8_TYPELESS => 8,
        DXGI_FORMAT_R8G8B8A8_TYPELESS => 9,
        DXGI_FORMAT_R8G8B8A8_UNORM => 10,
        DXGI_FORMAT_R8G8B8A8_SNORM => 11,
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => 12,
        DXGI_FORMAT_B8G8R8A8_TYPELESS => 13,
        DXGI_FORMAT_B8G8R8A8_UNORM => 14,
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => 15,
        DXGI_FORMAT_R11G11B10_FLOAT => 16,
        DXGI_FORMAT_R10G10B10A2_UNORM => 17,
        DXGI_FORMAT_R16G16_FLOAT => 18,
        DXGI_FORMAT_R16G16_UINT => 19,
        DXGI_FORMAT_R16G16_SINT => 20,
        DXGI_FORMAT_R16_FLOAT => 21,
        DXGI_FORMAT_R16_UINT => 22,
        DXGI_FORMAT_R16_UNORM | DXGI_FORMAT_D16_UNORM => 23,
        DXGI_FORMAT_R16_SNORM => 24,
        DXGI_FORMAT_R8_UNORM | DXGI_FORMAT_A8_UNORM => 25,
        DXGI_FORMAT_R8G8_UNORM => 26,
        DXGI_FORMAT_R8G8_UINT => 27,
        DXGI_FORMAT_R32_FLOAT | DXGI_FORMAT_D32_FLOAT | DXGI_FORMAT_R32G8X24_TYPELESS | DXGI_FORMAT_D32_FLOAT_S8X24_UINT | DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS => 28,
        DXGI_FORMAT_R9G9B9E5_SHAREDEXP => 29,
        DXGI_FORMAT_R16G16B16A16_TYPELESS => 30,
        DXGI_FORMAT_R32G32_TYPELESS => 31,
        DXGI_FORMAT_R10G10B10A2_TYPELESS => 32,
        DXGI_FORMAT_R16G16_TYPELESS => 33,
        DXGI_FORMAT_R16_TYPELESS => 34,
        DXGI_FORMAT_R8_TYPELESS => 35,
        DXGI_FORMAT_R8G8_TYPELESS => 36,
        DXGI_FORMAT_R32_TYPELESS => 37,
        DXGI_FORMAT_R32G32_UINT => 38,
        DXGI_FORMAT_R8_SNORM => 39,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_headers() {
        // 64-bit layouts of the C structures (natural alignment).
        assert_eq!(std::mem::size_of::<Description>(), 32);
        assert_eq!(std::mem::size_of::<Resource>(), 48);
        assert_eq!(std::mem::offset_of!(ConfigureDesc, hudless), 64);
        assert_eq!(std::mem::offset_of!(ConfigureDesc, frame), 136);
        assert_eq!(std::mem::offset_of!(PrepareDesc, depth), 88);
        assert_eq!(std::mem::size_of::<PrepareDesc>(), 232);
    }

    #[test]
    fn formats_follow_the_table() {
        assert_eq!(format(DXGI_FORMAT_R16G16_FLOAT), 18);
        assert_eq!(format(DXGI_FORMAT_D32_FLOAT), 28);
        assert_eq!(format(DXGI_FORMAT_R32_TYPELESS), 37);
        assert_eq!(format(DXGI_FORMAT_R8G8B8A8_UNORM), 10);
    }
}
