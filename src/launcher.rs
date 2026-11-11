use std::process::Command;

/// The Steam App ID for Monster Hunter: World.
pub const MHW_APP_ID: &str = "582010";

/// The launch option that must be set in Steam to use Astera Loader on Linux.
pub const STEAM_LAUNCH_OPTION: &str =
    r#"bash -c 'exec "${@/MonsterHunterWorld.exe/AsteraLoader.exe}"' -- %command%"#;

/// Launch MHW through Steam using its App ID.
///
/// This opens the `steam://rungameid/582010` URI which tells the running
/// Steam client to launch the game. The user must have already configured
/// the launch options in Steam for Astera Loader to work.
pub fn launch_via_steam() -> Result<(), String> {
    let uri = format!("steam://rungameid/{}", MHW_APP_ID);

    log::info!("Launching MHW via Steam URI: {uri}");

    // Try xdg-open first (most universal on Linux desktops)
    let result = Command::new("xdg-open").arg(&uri).spawn();

    match result {
        Ok(_) => {
            log::info!("Steam launch command sent successfully.");
            Ok(())
        }
        Err(e) => {
            log::warn!("xdg-open failed: {e}, trying steam directly...");
            // Fallback: call steam binary directly
            match Command::new("steam").arg(&uri).spawn() {
                Ok(_) => {
                    log::info!("Steam launch command sent via steam binary.");
                    Ok(())
                }
                Err(e2) => {
                    let msg = format!(
                        "Failed to launch Steam. Tried xdg-open ({e}) and steam ({e2}). \
                         Is Steam installed and running?"
                    );
                    log::error!("{msg}");
                    Err(msg)
                }
            }
        }
    }
}
