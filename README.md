# Sonar Lite

A small, standalone mixer for the audio channels that **SteelSeries Sonar** installs on Windows. It talks to the Sonar audio
driver and APO directly, so you can use Sonar's channels without keeping the SteelSeries GG app running.

> **Not affiliated with SteelSeries.** Sonar Lite is an independent hobby project. "SteelSeries", "GG" and "Sonar" are trademarks of
> their owners and are used here only to describe what this program works with. See [Legal](#legal).

## What it does

- **Four channels** – Game, Chat, Media, Aux: output device, live volume, mute, level meter, per-channel priority list of devices with
  automatic fallback, "link all channels" mode.
- **Master** – moves all channel faders together; Chat/Game balance (ChatMix).
- **Equalizer** – Sonar-style page: five draggable markers with an exact-value popup (gain / frequency / Q / type), Bass / Voice / Treble
  sliders, built-in presets and up to 10 own presets (separate sets for channels and for the microphone).
- **Per-app routing** – drag an app onto a channel (or use the right-click menu). Apps that choose their own output device cannot be routed;
  they are listed separately.
- **Microphone chain** – the physical mic is captured, processed (RNNoise noise reduction, noise gate with automatic threshold,
  compressor, EQ, gain) and played into Sonar's virtual microphone. Apps using the mic are listed in the MIC strip.
- **Streamer mode** – a separate mix (own faders and EQ per source, plus the mic) sent to a virtual audio cable that OBS can record.
- **Tray icon, autostart, own title bar, Polish and English UI.**
- A small command-line tool, `sonar-lite-cli`, for scripting (`status`, `set`, `apps`, `app`, `eq`, `preset`, `refresh`, `vad`, `autostart`).

## Requirements

- Windows 11 (developed and tested on 24H2).
- SteelSeries GG with Sonar **installed once**, so that the virtual devices (`SteelSeries Sonar - Gaming/Chat/Media/Aux/Microphone`) and
  the APO exist. GG itself does not need to run – and should not run at the same time, because it rewrites the same settings.
- Administrator rights (the app is elevated: it writes the APO settings in `HKLM` and re-enables the virtual devices).
- Optional: a virtual audio cable (e.g. "Virtual Audio Cable") for streamer mode.

## Build

Needs the Rust stable toolchain with the MSVC target.

```powershell
cargo build --release
```

Produces `target\release\sonar-lite.exe` (GUI) and `target\release\sonar-lite-cli.exe`.
`cargo test --lib` runs the unit tests; `sonar-lite.exe --selftest out.txt` drives the real actions against the real registry and restores it.

## How it works

Sonar's routing is done by `Sonar.APO`, a user-mode audio processing object inside `audiodg`, not by GG. Its settings live in
`HKLM\SOFTWARE\SteelSeries ApS\Sonar.APO\<channel>\Settings\GlobalControl\Store` (`kSet_*` values). Sonar Lite writes those values
(output device, EQ bands) and uses Windows' own endpoint volume for the faders, which – unlike the APO gain – also reaches streams that are
already playing. Per-app routing uses the undocumented `AudioPolicyConfig` factory (the same one Windows Settings uses).

Things worth knowing:

- A running app keeps the APO settings it started with; after an EQ or output change Sonar Lite briefly moves the affected apps to another
  channel and back so they pick the new settings up.
- The registry layout and the undocumented interfaces can change with a GG or Windows update.
- Not implemented yet: global hotkeys, Spatial Audio, Volume Boost, importing GG's presets.

## Legal

Sonar Lite is released under the [MIT License](LICENSE) and contains no SteelSeries code or files. It only reads and writes settings on
your own machine and requires a separately installed copy of GG/Sonar, which is subject to SteelSeries' own license terms – you are
responsible for complying with them. The software is provided as is, without warranty; changing audio driver settings is done at your own risk.
