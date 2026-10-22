# del

A minimal Rust desktop app built on [GPUI](https://www.gpui.rs/) (Zed's UI
framework) with [gpui-component](https://github.com/longbridge/gpui-component).
One window, a title, a greeting, and a clickable counter button.

## Layout

```
Cargo.toml          workspace (pins gpui-kit =0.8.0 / gpui-pre =0.3.9)
crates/core         del-core: UI-agnostic domain logic (Counter + greeting), plain Rust, unit-tested
crates/app          del-app:  the GPUI window, binary name `del`
```

`del-core` has no UI dependencies. `del-app` depends on `gpui-kit`, the umbrella
crate that bundles `gpui`, `gpui_platform`, `gpui-base` and `gpui-component`
against one exact GPUI snapshot.

## GPUI pin

Zed publishes GPUI to crates.io as the `gpui-pre-*` crate family (snapshots of
the `zed-industries/zed` repo). This workspace pins:

| crate            | version  |
| ---------------- | -------- |
| gpui-kit         | =0.8.0   |
| gpui-pre (gpui)  | =0.3.9   |

Any GPUI snapshot can change the API, so both pins are exact and must be bumped
together. See `Cargo.toml` and `Cargo.lock`.

## Requirements (macOS)

- Rust stable (tested with 1.99, edition 2024).
- Full Xcode (not only Command Line Tools), with `xcode-select -p` pointing at
  it. GPUI compiles Metal shaders at build time via `xcrun metal`, which only
  ships with Xcode.app.
- First build pulls and compiles roughly 800 crates; expect several minutes.
  Incremental rebuilds of `del-app` after that are fast.

## Build and run

```sh
cargo test -p del-core     # domain logic, no window needed
cargo run -p del-app       # opens the window
```

`cargo run` alone also works because `del-app` is the only binary in the
workspace. A 480x320 window titled "del" opens with a counter button; each
click increments the count shown on the button and in the greeting.

## Notes

- `profile.dev` optimizes dependencies at `opt-level = 2` so the debug build of
  the UI stays responsive. Drop that section if you prefer faster cold builds.
- To try the alternate `gpui-fast` backend, enable the `gpui-kit/gpui-fast`
  feature; it is opt-in and not required here.
