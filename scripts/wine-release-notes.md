WineWrap's own Wine build for macOS. WineWrap downloads it automatically for Steam bottles; you don't need to install it by hand.

It's Wine 11.0 from CodeWeavers' LGPL CrossOver 26.3.0 source, with WineWrap's patches. On Apple Silicon it runs under Rosetta 2. Compared with upstream Wine on macOS:

- **Steam's window shows:** with upstream Wine it stays black. A patch runs Steam's Chromium GPU work in the browser process.
- **Steam signs in:** Wine's Windows-side DLLs are compiled with MinGW GCC. With a clang (llvm-mingw) build, Steam never connects to its servers.
- **Game controllers work:** SDL2 2.32.10 is bundled. Xbox, PlayStation and other controllers show up in Steam and games.
- **Steam finds other devices on your network:** local network game transfers can discover other Steam clients.
- **D3DMetal per program:** if you add Apple's D3DMetal with `winewrap runtime install-d3dmetal`, games use it, while programs it can't serve (the Rockstar Games Launcher) keep Wine's own Direct3D.
- **Rockstar Games Launcher works:** it starts, its sign-in window shows, and it launches games such as Red Dead Redemption.
- **No Mono prompt:** Wine Mono 10.4.1 and Wine Gecko 2.47.4 are bundled.

New since 26.3.0-3: patches 0003 (Steam LAN discovery), 0004 (D3DMetal per program; OpenGL and no NVAPI for the Rockstar Games Launcher) and 0005 (Social Club's sign-in window).

## Files

- `wine-winewrap-26.3.0-4-osx64.tar.xz`: the build, `Wine WineWrap.app/Contents/Resources/wine`.
- `crossover-sources-26.3.0.tar.gz` and the `000*.patch` files: the corresponding source, under the LGPL. Rebuild with `scripts/build-wine-macos.sh` from this tag.
- `SHA256SUMS`

WineWrap is not affiliated with CodeWeavers, and this is not CrossOver.
