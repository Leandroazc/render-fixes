//! `amd_fidelityfx_loader_xbox.dll`: AMD's FidelityFX API, which the title
//! upscales through on Series S. Served by `victory_gpu::upscale`, so the
//! loader never loads AMD's upscaler (whose precompiled programs do not
//! translate).

use crate::{HleExport, HleModule};

pub(crate) fn module() -> HleModule {
    use victory_gpu::upscale;
    HleModule {
        name: "ffx",
        dlls: &["amd_fidelityfx_loader_xbox.dll"],
        exports: vec![
            HleExport {
                name: "ffxCreateContext",
                address: upscale::create_context as *const () as usize,
            },
            HleExport {
                name: "ffxDestroyContext",
                address: upscale::destroy_context as *const () as usize,
            },
            HleExport {
                name: "ffxConfigure",
                address: upscale::configure as *const () as usize,
            },
            HleExport {
                name: "ffxQuery",
                address: upscale::query as *const () as usize,
            },
            HleExport {
                name: "ffxDispatch",
                address: upscale::dispatch as *const () as usize,
            },
        ],
    }
}
