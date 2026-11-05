//! Per-game settings that WineWrap writes into a bottle before launching.
//!
//! Some games pick a default that can't work on a Mac and then crash before
//! they save any settings, so a player has no menu to change it from. Each
//! fix here only touches games installed in the bottle.

use std::path::{Path, PathBuf};

use crate::bottle::{Bottle, GraphicsBackend};
use crate::error::{IoContext, Result};
use crate::steam;

/// Red Dead Redemption 2's Steam app ID.
const RDR2_APP_ID: u32 = 1174180;
const RDR2_VULKAN: &str = "<API>kSettingAPI_Vulkan</API>";
const RDR2_DX12: &str = "<API>kSettingAPI_DX12</API>";
/// Enough for RDR2 to start: it fills in every other setting itself and
/// keeps the API it finds here.
const RDR2_SYSTEM_XML: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<rage__fwuiSystemSettingsCollection>
  <configSource>kSettingsConfig_Auto</configSource>
  <advancedGraphics>
    <API>kSettingAPI_DX12</API>
  </advancedGraphics>
</rage__fwuiSystemSettingsCollection>
";

/// Apply every per-game fix that fits this bottle.
pub(crate) fn apply(bottle: &Bottle) -> Result<()> {
    if bottle.config.graphics != GraphicsBackend::D3DMetal {
        return Ok(());
    }
    if steam::installed_games(bottle)
        .iter()
        .any(|g| g.app_id == RDR2_APP_ID)
    {
        rdr2_use_dx12(&bottle.path)?;
    }
    Ok(())
}

/// Make Red Dead Redemption 2 render with DirectX 12.
///
/// On a first run RDR2 defaults to Vulkan, which runs on MoltenVK. MoltenVK
/// can't compile some of the game's shaders (they use cull distances), so the
/// game crashes during init, before it writes `system.xml`, and so it starts
/// with Vulkan every time. DirectX 12 runs on D3DMetal. A file the game
/// already wrote keeps its other settings; only a Vulkan choice is switched.
fn rdr2_use_dx12(prefix: &Path) -> Result<()> {
    for docs in user_documents(prefix) {
        let file = docs.join("Rockstar Games/Red Dead Redemption 2/Settings/system.xml");
        match std::fs::read_to_string(&file) {
            Ok(text) if text.contains(RDR2_VULKAN) => {
                tracing::info!(
                    "switching RDR2 from Vulkan to DirectX 12: {}",
                    file.display()
                );
                std::fs::write(&file, text.replace(RDR2_VULKAN, RDR2_DX12)).at(&file)?;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("writing RDR2 DirectX 12 settings: {}", file.display());
                let dir = file.parent().expect("settings file has a parent");
                std::fs::create_dir_all(dir).at(dir)?;
                std::fs::write(&file, RDR2_SYSTEM_XML).at(&file)?;
            }
            Err(e) => return Err::<(), _>(e).at(&file),
        }
    }
    Ok(())
}

/// The Documents folder of each Windows user in the prefix (not `Public`).
fn user_documents(prefix: &Path) -> Vec<PathBuf> {
    let users = prefix.join("drive_c").join("users");
    let Ok(entries) = std::fs::read_dir(&users) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name() != "Public")
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.is_symlink())
        .map(|p| p.join("Documents"))
        .filter(|d| d.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bottle::BottleConfig;

    fn bottle_with_rdr2(dir: &Path, graphics: GraphicsBackend) -> (Bottle, PathBuf) {
        let mut config = BottleConfig::new("Steam");
        config.graphics = graphics;
        let bottle = Bottle {
            path: dir.to_path_buf(),
            config,
        };
        let apps = steam::steam_dir(&bottle).join("steamapps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("appmanifest_1174180.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"1174180\"\n\t\"name\"\t\t\"Red Dead Redemption 2\"\n\
             \t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"Red Dead Redemption 2\"\n}\n",
        )
        .unwrap();
        let docs = dir.join("drive_c/users/crossover/Documents");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(dir.join("drive_c/users/Public/Documents")).unwrap();
        let file = docs.join("Rockstar Games/Red Dead Redemption 2/Settings/system.xml");
        (bottle, file)
    }

    #[test]
    fn rdr2_starts_on_dx12() {
        let dir = tempfile::tempdir().unwrap();
        let (bottle, file) = bottle_with_rdr2(dir.path(), GraphicsBackend::D3DMetal);

        apply(&bottle).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), RDR2_SYSTEM_XML);
        assert!(!dir
            .path()
            .join("drive_c/users/Public/Documents/Rockstar Games")
            .exists());

        // The game's own settings survive; only Vulkan is switched.
        let saved = "<version value=\"37\" />\n<API>kSettingAPI_Vulkan</API>\n<x>1</x>\n";
        std::fs::write(&file, saved).unwrap();
        apply(&bottle).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "<version value=\"37\" />\n<API>kSettingAPI_DX12</API>\n<x>1</x>\n"
        );
        let dx11 = "<API>kSettingAPI_DX11</API>\n";
        std::fs::write(&file, dx11).unwrap();
        apply(&bottle).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), dx11);
    }

    #[test]
    fn rdr2_left_alone_without_d3dmetal_or_game() {
        let dir = tempfile::tempdir().unwrap();
        let (bottle, file) = bottle_with_rdr2(dir.path(), GraphicsBackend::Dxvk);
        apply(&bottle).unwrap();
        assert!(!file.exists());

        let dir = tempfile::tempdir().unwrap();
        let (bottle, file) = bottle_with_rdr2(dir.path(), GraphicsBackend::D3DMetal);
        std::fs::remove_file(steam::steam_dir(&bottle).join("steamapps/appmanifest_1174180.acf"))
            .unwrap();
        apply(&bottle).unwrap();
        assert!(!file.exists());
    }
}
