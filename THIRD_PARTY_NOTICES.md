# Third-party notices

Sidechain's own code is MIT-licensed (see `LICENSE`). `Sidechain.exe` and the hook addon also
contain the following third-party work, under their own licenses.

## Built into Sidechain.exe

| Component | License | Source |
|---|---|---|
| HT-Demucs v4 (the neural stem model) | MIT, Copyright (c) Meta Platforms, Inc. and affiliates | [facebookresearch/demucs](https://github.com/facebookresearch/demucs) |
| Forward-only ONNX export of HT-Demucs (converted to fp16 by `tools/convert_fp16.py`) | MIT | [webnn/stem-separator](https://huggingface.co/webnn/stem-separator) |
| ONNX Runtime (statically linked) | MIT, Copyright (c) Microsoft Corporation | [microsoft/onnxruntime](https://github.com/microsoft/onnxruntime) |
| DirectML (`DirectML.dll`, unpacked on first run) | Microsoft DirectML redistributable license terms | [Microsoft.AI.DirectML](https://www.nuget.org/packages/Microsoft.AI.DirectML) |
| Phosphor icons (font) | MIT | [phosphor-icons](https://github.com/phosphor-icons/core) |
| egui default fonts: Ubuntu-Light, Hack, Noto Emoji, emoji-icon-font | Ubuntu Font Licence 1.0, MIT / Bitstream Vera, OFL-1.1, MIT / OFL-1.1 | [emilk/egui](https://github.com/emilk/egui/tree/master/crates/epaint_default_fonts) |

The Segoe UI font is loaded from Windows at run time and is not distributed.

## The hook addon (sidechain-hook.vpk)

`addon/panorama/layout/citadel_hud_top_bar.xml` is Deadlock's own HUD layout, decompiled from the
game with one `<scripts>` include added. It is Valve's work, is not covered by Sidechain's license,
and is included only so the addon can load its script, as is usual for Deadlock HUD mods.
Deadlock is a trademark of Valve Corporation. Sidechain is not affiliated with or endorsed by Valve
or Spotify.

## Rust crates

Linked into `Sidechain.exe` (from `cargo tree -e normal --target x86_64-pc-windows-msvc`). Each
crate's full license text ships in its source package on [crates.io](https://crates.io).

| Crate | Version | License |
|---|---|---|
| ab_glyph | 0.2.32 | Apache-2.0 |
| ab_glyph_rasterizer | 0.1.10 | Apache-2.0 |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| ahash | 0.8.12 | MIT OR Apache-2.0 |
| aho-corasick | 1.1.5 | Unlicense OR MIT |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| arboard | 3.6.1 | MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT |
| byteorder-lite | 0.1.0 | Unlicense OR MIT |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| clipboard-win | 5.4.1 | BSL-1.0 |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| cursor-icon | 1.2.0 | MIT OR Apache-2.0 OR Zlib |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 |
| document-features | 0.2.12 | MIT OR Apache-2.0 |
| dpi | 0.1.2 | Apache-2.0 AND MIT |
| ecolor | 0.32.3 | MIT OR Apache-2.0 |
| eframe | 0.32.3 | MIT OR Apache-2.0 |
| egui | 0.32.3 | MIT OR Apache-2.0 |
| egui-phosphor | 0.10.0 | MIT OR Apache-2.0 |
| egui-winit | 0.32.3 | MIT OR Apache-2.0 |
| egui_glow | 0.32.3 | MIT OR Apache-2.0 |
| emath | 0.32.3 | MIT OR Apache-2.0 |
| epaint | 0.32.3 | MIT OR Apache-2.0 |
| epaint_default_fonts | 0.32.3 | (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 |
| error-code | 3.4.0 | BSL-1.0 |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 |
| glow | 0.16.0 | MIT OR Apache-2.0 OR Zlib |
| glutin | 0.32.3 | Apache-2.0 |
| glutin-winit | 0.5.0 | MIT |
| glutin_egl_sys | 0.7.1 | Apache-2.0 |
| glutin_wgl_sys | 0.6.1 | Apache-2.0 |
| icu_collections | 2.3.0 | Unicode-3.0 |
| icu_locale_core | 2.3.0 | Unicode-3.0 |
| icu_normalizer | 2.3.0 | Unicode-3.0 |
| icu_normalizer_data | 2.3.0 | Unicode-3.0 |
| icu_properties | 2.3.0 | Unicode-3.0 |
| icu_properties_data | 2.3.0 | Unicode-3.0 |
| icu_provider | 2.3.1 | Unicode-3.0 |
| idna | 1.1.0 | MIT OR Apache-2.0 |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT |
| image | 0.25.10 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| keyboard-types | 0.8.3 | MIT OR Apache-2.0 |
| libloading | 0.8.9 | ISC |
| litemap | 0.8.3 | Unicode-3.0 |
| litrs | 1.0.1 | MIT OR Apache-2.0 |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.34 | MIT OR Apache-2.0 |
| matrixmultiply | 0.3.11 | MIT/Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| memoffset | 0.9.1 | MIT |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| moxcms | 0.8.1 | BSD-3-Clause OR Apache-2.0 |
| muda | 0.21.2 | Apache-2.0 OR MIT |
| ndarray | 0.17.2 | MIT OR Apache-2.0 |
| nohash-hasher | 0.2.0 | Apache-2.0 OR MIT |
| num-complex | 0.4.6 | MIT OR Apache-2.0 |
| num-integer | 0.1.47 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| ort | 2.0.0-rc.14 | MIT OR Apache-2.0 |
| ort-sys | 2.0.0-rc.14 | MIT OR Apache-2.0 |
| owned_ttf_parser | 0.25.1 | Apache-2.0 |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| png | 0.18.1 | MIT OR Apache-2.0 |
| potential_utf | 0.1.6 | Unicode-3.0 |
| primal-check | 0.3.4 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| profiling | 1.0.18 | MIT OR Apache-2.0 |
| pxfm | 0.1.30 | BSD-3-Clause OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib |
| rawpointer | 0.2.1 | MIT/Apache-2.0 |
| realfft | 3.5.0 | MIT |
| regex | 1.13.1 | MIT OR Apache-2.0 |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 |
| rustfft | 6.4.1 | MIT OR Apache-2.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 |
| serde_spanned | 1.1.2 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| smallvec | 1.16.3 | MIT OR Apache-2.0 |
| smol_str | 0.2.2 | MIT OR Apache-2.0 |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 |
| strength_reduce | 0.2.4 | MIT OR Apache-2.0 |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.9 | MIT OR Apache-2.0 |
| synstructure | 0.14.0 | MIT |
| thiserror | 2.0.21 | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 |
| tinystr | 0.8.4 | Unicode-3.0 |
| toml | 1.1.8+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_parser | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_writer | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| tracing | 0.1.44 | MIT |
| tracing-core | 0.1.36 | MIT |
| transpose | 0.2.3 | MIT OR Apache-2.0 |
| tray-icon | 0.26.1 | MIT OR Apache-2.0 |
| ttf-parser | 0.25.1 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 |
| url | 2.5.8 | MIT OR Apache-2.0 |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT |
| wasapi | 0.26.0 | MIT |
| web-time | 1.1.0 | MIT OR Apache-2.0 |
| webbrowser | 1.2.4 | MIT OR Apache-2.0 |
| winapi | 0.3.9 | MIT/Apache-2.0 |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 |
| windows-sys | 0.60.2 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows-targets | 0.53.5 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.53.1 | MIT OR Apache-2.0 |
| winit | 0.30.13 | Apache-2.0 |
| winnow | 1.0.4 | MIT |
| writeable | 0.6.4 | Unicode-3.0 |
| yoke | 0.8.3 | Unicode-3.0 |
| yoke-derive | 0.8.4 | Unicode-3.0 |
| zerocopy | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerofrom | 0.1.8 | Unicode-3.0 |
| zerofrom-derive | 0.1.8 | Unicode-3.0 |
| zerotrie | 0.2.5 | Unicode-3.0 |
| zerovec | 0.11.8 | Unicode-3.0 |
| zerovec-derive | 0.11.6 | Unicode-3.0 |
| zmij | 1.0.23 | MIT |
