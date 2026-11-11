# Sekai Hunter

Sekai Hunter is a native Linux launcher and mod manager for Monster Hunter: World. It detects Astera Loader and Stracker's Loader and adapts its status, plugin listing, and Steam launch guidance to the detected setup.

## Loader compatibility

- **Astera Loader:** Sekai Hunter detects `AsteraLoader.exe` and the `AsteraLoader/` directory, and shows Astera's documented Linux/Proton Steam launch option.
- **Stracker's Loader:** Sekai Hunter detects the legacy `dinput8.dll` and `loader.dll` markers and launches the game through Steam using the user's existing Steam settings. No Stracker-specific Linux/Proton launch option could be verified from accessible official documentation, so Sekai Hunter does not invent or apply one.
- **Plugins:** Stracker-compatible plugins in `nativePC/plugins/` are supported by Stracker and by Astera Loader's compatibility layer. Astera-specific plugins in `AsteraLoader/plugins/` require Astera Loader.
- If files for both loaders are detected, launching is blocked until the user resolves the conflict manually. Sekai Hunter does not remove loader or plugin files.

See the [Astera Loader documentation](https://github.com/wowo-dot/AsteraLoader#linux-proton) for its Proton launch procedure and compatibility details. Stracker users should consult the [official Stracker's Loader page](https://www.nexusmods.com/monsterhunterworld/mods/1982) for current installation instructions.

## Acknowledgment

The mod-management concept is inspired by [MHW Mod Manager](https://github.com/UncleClapton/MHWModManager), created by BoltMan. The upstream repository is maintained by UncleClapton. Sekai Hunter is an independent Linux implementation.

## Disclaimer

Sekai Hunter is not affiliated with or endorsed by Capcom, the Astera Loader authors, or the MHW Mod Manager contributors.
