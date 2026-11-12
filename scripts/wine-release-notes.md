WineWrap's own Wine build for macOS. WineWrap downloads it automatically for Steam bottles; you don't need to install it by hand.

It's Wine 11.0 from CodeWeavers' LGPL CrossOver 26.3.0 source, with WineWrap's patches. On Apple Silicon it runs under Rosetta 2. Compared with upstream Wine on macOS:

- **Steam's window shows:** with upstream Wine it stays black. A patch runs Steam's Chromium GPU work in the browser process.
- **Steam signs in:** Wine's Windows-side DLLs are compiled with MinGW GCC. With a clang (llvm-mingw) build, Steam never connects to its servers.
- **Game controllers work:** SDL2 2.32.10 is bundled. Xbox, PlayStation and other controllers show up in Steam and games.
- **Steam finds other devices on your network:** local network game transfers can discover other Steam clients.
- **D3DMetal per program:** if you add Apple's D3DMetal with `winewrap runtime install-d3dmetal`, games use it, while programs it can't serve (the Rockstar Games Launcher) keep Wine's own Direct3D.
- **Rockstar Games Launcher works:** it starts, its sign-in window shows, and it launches games such as Red Dead Redemption.
- **Games built for newer CPUs start:** Rosetta's x86 CPU lacks a few features that recent games and their copy protection check for (Phantom Blade Zero). A patch makes it look more like a real one.
- **Shaders are compiled once:** macOS keeps at most 1 GiB of compiled Metal shaders per game, so games with more recompile them on every start. The build raises the limit to 16 GiB (`WINEWRAP_SHADER_CACHE_MB` changes it).
- **No Mono prompt:** Wine Mono 10.4.1 and Wine Gecko 2.47.4 are bundled.

New since 26.3.0-5:

- **Game videos play:** GStreamer 1.28.9 is bundled, so games can decode H.264, AAC and MP3 through Media Foundation, using macOS's VideoToolbox and AudioToolbox. Patch 0007 makes the video decoder output NV12 to system memory on macOS, which Unreal Engine's video player needs. Before this, Phantom Blade Zero stopped taking input after a cutscene.
- **Controllers come back after reconnecting:** patch 0008. A game that re-checked controllers when one reconnected could miss it and ignore it until restarted (Phantom Blade Zero).

## Files

- `wine-winewrap-26.3.0-6-osx64.tar.xz`: the build, `Wine WineWrap.app/Contents/Resources/wine`.
- `crossover-sources-26.3.0.tar.gz` the `000*.patch` files and `macos-fixes.c`: the corresponding source, under the LGPL. `gstreamer-1.28.9.tar.gz`, `glib-*`, `libffi-*`, `pcre2-*` and `proxy-libintl-*`: the source of the bundled GStreamer and its libraries. Rebuild with `scripts/build-wine-macos.sh` from this tag.
- `SHA256SUMS`

WineWrap is not affiliated with CodeWeavers, and this is not CrossOver.
