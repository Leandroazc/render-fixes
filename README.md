# Azc Low End

The Xbox Series build of GTA VI running natively on **Windows**, tuned for a
**low-end PC**: an RTX 2060 SUPER with 8 GB, a six-core Ryzen 5 5600 and
**16 GB of RAM**. Every other effort is tested on 12-24 GB cards and 32-64 GB
of memory; this one is about the machines most players actually have.

> **No game files, code or assets are in this repository.** You need your own
> dump of the title's Xbox Series build v1.0.2.1. Nothing here is affiliated
> with or endorsed by the title's developer or publisher, NVIDIA or AMD. See
> [docs/asset-policy.md](docs/asset-policy.md).

It is a compatibility layer (loader, runtime services, a Direct3D 12 back end
and a shader recompiler, from [VIctory](https://github.com/Marvisak/VIctory))
on the [foxhound](https://github.com/readmet3xt-lab/foxhound) line as carried by
[VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA), with the
low-end work below on top.

## Where it stands (2026-11-27)

One PC: Windows 11, Ryzen 5 5600 (6 cores, 12 threads), 16 GB RAM, GeForce
RTX 2060 SUPER 8 GB, 2560x1080 monitor. Series S profile, opening of the game
(the prison cell and its window sequence), 300-720 s sessions.

| | |
|---|---|
| Frames shown on screen | **28-32 a second** (FSR 3 frame generation; RivaTuner's D3D12 counter: 30) |
| Frames the game renders | 16-18 a second (56-62 ms a frame) |
| Upscaling | DLSS Super Resolution, Performance (540p to 1080p) |
| Device losses, crashes | none in the last eight sessions |
| Flicker (bursts of screenshots compared) | none |
| Picture | orange uniform, dark hair with strands, frosted glass, walls and posters in their colours |

How it got there, on this PC, measured each step:

| Base and change | fps rendered | on screen |
|---|---|---|
| ztrx / 7900 XTX fork with NVIDIA fixes, Series S | 2-5 | same |
| Vice-Tay base, card copies, list pool, 6-thread profile | 8 | same |
| VIctory-NVIDIA (foxhound line), DLSS Quality | 16-17 | same |
| DLSS Performance on 8 GB cards | 17-18 | same |
| **FSR 3 frame generation (this repository)** | 17-18 | **30** |

The card is the limit: it is about 95 % busy, and its time is spread over a
thousand kinds of the title's own passes (a timing run's heaviest is 5.4 ms a
frame), so render scale and barriers barely move it. Input follows the
rendered rate.

## What this repository adds

- **FSR 3 frame generation on any card** (`crates/victory-gpu/src/fsrfg.rs`).
  DLSS Frame Generation needs an RTX 40: on an RTX 2060 SUPER NVIDIA's
  Streamline answers "not supported on current hardware". AMD's FidelityFX
  frame generation runs on any Direct3D 12 card. The swap chain is wrapped by
  FidelityFX's; each upscale the title asks for records the frame's
  preparation (depth, motion vectors, camera, as the title hands them to its
  upscaler) in the title's own command list; each present names the frame
  shown. Calls into the context go one at a time (a preparation and a
  generation at once crashed AMD's library) and the inputs of the last eight
  frames are held. AMD's two signed DLLs are fetched at run time by
  `scripts\get-amd-fsr.ps1`, never committed.
- **Purple and lilac patches gone** (`ambient.rs`). Without the title's ray
  tracing its probes leave stale memory in the indirect light: with the light
  estimate off, paper on the cell wall is strong purple. Magenta and lilac
  there (green below both red and blue) is dropped; warm light and sky light
  are never taken for it.
- **The reflections' estimate off by default**: it turned walls and the bunk
  purple in patches when a magenta poster was in view, at no gain.
- **Low-end defaults in the launcher**: DLSS Performance on cards of 8 GB or
  less; FSR frame generation on by itself on cards before the RTX 40.
- **A profiler** (`scripts\profile.ps1`): runs a session and writes a report of
  what takes the CPU (by thread), the card (by kind of list and, in a timing
  run, by pipeline) and memory (private, resident, paging, card), with the
  layer's timeline for [Perfetto](https://ui.perfetto.dev).

## Run it

You need Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 card with shader
model 6.6 and mesh shaders, an SSD, [Rust](https://rustup.rs) (pinned in
`rust-toolchain.toml`), the Visual Studio Build Tools ("Desktop development
with C++") and your own dump.

```powershell
git clone https://github.com/Leandroazc/Azc-Low-End
cd Azc-Low-End
copy victory.example.toml victory.toml     # set game_dir to your dump's folder
.\Get-NVIDIA-DLSS.cmd                      # once: NVIDIA's DLSS files (about 70 MB, from NVIDIA)
.\scripts\get-amd-fsr.ps1                  # once: AMD's frame generation files (about 30 MB, from AMD)
.\scripts\play-nvidia.ps1 -Exe 'D:\MyDump\game_scarlett_master.exe' -SeriesS
```

The first start builds the layer. F1 in the game opens the settings menu.
`-FsrFrameGen:$false` turns frame generation off; `-Dlss quality` trades a
frame or so for a sharper picture.

On 16 GB of RAM the game commits close to 20 GB and lives off the page file:
close browsers, chat apps and virtual machines (WSL's `vmmem` alone held 4 GB
here) before playing.

## Known limits

- Tested only in the opening (the cell and its window sequence).
- Generated frames include the interface; subtitles may shimmer in fast
  motion.
- No ray tracing: an estimate stands in for indirect light.
- The first run of a new build stutters while the driver compiles pipelines.

## Credits

A tree of other people's work with a little on top; the history is kept.
[VIctory](https://github.com/Marvisak/VIctory) (Marvisak and contributors),
[foxhound](https://github.com/readmet3xt-lab/foxhound),
[VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) (its README is
kept as [docs/README-VIctory-NVIDIA.md](docs/README-VIctory-NVIDIA.md)),
[VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX),
[fxceless/victory-fork](https://github.com/fxceless/victory-fork),
[Belobug](https://github.com/Belobug/VIctory-Belobug-AMD) (the magenta guard this
extends), [Vice-Tay](https://github.com/TaynanGT/Vice-Tay),
[cool_fox](https://github.com/codec-d/cool_fox), DexNom and the others they
name. NVIDIA DLSS/Streamline and AMD FidelityFX are their makers' and are
loaded at run time from files you fetch. Much of this repository's own code
was written with [Claude Code](https://claude.com/claude-code).

## License

GPL-3.0-or-later, as the projects it comes from ([LICENSE](LICENSE)).
