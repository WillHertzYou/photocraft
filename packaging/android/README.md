# PhotoCraft on Android

This folder turns the PhotoCraft web build into an app you can install on an Android phone
(tested layout: Samsung Galaxy S25, landscape). It is the same Rust/WebAssembly app as
`apps/photocraft-web`; this adds only what Chrome needs to install it to the home screen:

| File | What it is |
|---|---|
| `manifest.webmanifest` | App name, icons, full-screen landscape display, theme colours |
| `icons/`, `apple-touch-icon.png` | Home-screen icons made from `assets/app-icon` (same licence) |
| `build.sh` | Downloads a web release, verifies it against `SHA256SUMS.txt`, adds the files above |

## Build

```sh
packaging/android/build.sh 0.7.0 dist/android    # from the upstream GitHub release
# or, after `trunk build --release` in apps/photocraft-web:
packaging/android/build.sh local dist/android
```

`dist/android` is a static site (it includes a `netlify.toml`). Host it on any HTTPS static
host. HTTPS is required for installing it and for WebGPU.

## Install on the phone

1. Open the site in **Chrome** on the phone.
2. Tap **⋮ → Add to home screen → Install** (Chrome may also offer an install banner).
3. Launch **PhotoCraft** from the home screen or app drawer. It opens full screen in landscape.

Everything runs on the phone: Open uses the Android file picker, and Save/Export download to
`Downloads`. There is no autosave on the web build, so save before closing the app.
