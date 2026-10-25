# Del local GPUI Apple renderer extension

This directory vendors the published `gpui-pre-apple` **0.3.9** source package
from crates.io: <https://crates.io/crates/gpui-pre-apple/0.3.9>.
The original registry package checksum recorded in Del's lockfile was
`b52f58893fe1a0836464428dfad2b5eaf42c9c303a9638f4c2aced5a69072a15`.

The package metadata identifies the upstream source as
<https://github.com/zed-industries/zed> at revision
`93d745668805029476e6d7fbb19652116fed51de` (`gpui_apple`).
The original package manifest, its bundled GPUI build-source subset, and
[Apache 2.0 license](LICENSE-APACHE) are retained.

Del's workspace uses a local `[patch.crates-io]` override. Local modifications
are confined to `src/metal_renderer.rs` and `src/shaders.metal`:

- Accept BGRA CoreVideo surfaces with straight alpha, alongside the unchanged
  YUV surface path.
- Expose a modal backdrop marker callback between scene render encoders, so
  the accumulated scene can be blurred before modal foreground is drawn.
- Expose an optional GPU producer callback before scene rendering. The callback
  encodes into GPUI's own uncommitted Metal command buffer; the producer must
  not commit it, wait for it, or mutate the GPUI scene.

The application uses this extension to composite its GPU cellular background
and composer backdrop blur without native overlays or CPU pixel readback.
This directory contains source only; shader libraries and build outputs are
produced by Cargo outside this directory.
