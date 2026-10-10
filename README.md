# Sonar Lite

A small, light-weight replacement mixer for the audio channels of **SteelSeries Sonar**.

> **Not affiliated with SteelSeries.** Sonar Lite is an independent hobby project. "SteelSeries", "GG" and "Sonar" are trademarks of
> their owners and are used here only to describe what this program works with. See [Legal](#legal).

## What it does

- **Full Game, Chat, Media, Aux functionality**: output device, live volume, mute, level meter, per-channel priority list of devices with
  automatic fallback, "link all channels" mode.
- **Equalizer** – Sonar-style page: draggable audio markers with an exact-value popup (gain / frequency / Q / type), Bass / Voice / Treble
  sliders, built-in presets and up to 10 own presets (separate sets for channels and for the microphone).
- **Per-App Routing** – drag an app onto a channel. Apps that choose their own output device cannot be routed; they are listed separately.
- Full compatibility or replacement of Sonar's **microphone features** (AI noise reduction, noise gate with automatic threshold, compressor, EQ, gain).
- **Streamer Mode** support
- A small command-line tool, `SonarLiteCli`, for scripting (`status`, `set`, `apps`, `app`, `eq`, `preset`, `refresh`, `vad`, `autostart`).

## Requirements

- Windows 11 (developed and tested on 24H2).
- SteelSeries GG with Sonar **installed once**, so that the virtual devices (`SteelSeries Sonar - Gaming/Chat/Media/Aux/Microphone`) and
  the APO exist. GG itself does not need to run – and should not run at the same time.
- Optional: a virtual audio cable (e.g. "Virtual Audio Cable") for streamer mode.

## Download

Grab `SonarLite-<version>-portable.zip` from the Releases page, unzip it anywhere and run `SonarLite.exe` (Windows asks for administrator
rights). Nothing is installed; settings are kept in `%APPDATA%\SonarLite`. No Visual C++ Redistributable is needed.
Note: "Start with Windows" registers a scheduled task that points at the exe's current location, so if you move the exe, switch it off and on again.

## Legal

Sonar Lite is released under the [MIT License](LICENSE) and contains no SteelSeries code or files. It only reads and writes settings on
your own machine and requires a separately installed copy of GG/Sonar, which is subject to SteelSeries' own license terms – you are
responsible for complying with them. The software is provided as is, without warranty; changing audio driver settings is done at your own risk.
