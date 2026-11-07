//! Pet packages and the spritesheet layout they share.
//!
//! A pet is a folder with `pet.json` (`displayName`, `spritesheetPath`) and a
//! spritesheet: a grid of 192x208 frames, 8 columns, one animation per row.
//! This is the OpenPets pet format, so existing pets can be dropped in as-is.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::motion::Anim;

pub const FRAME_WIDTH: f64 = 192.0;
pub const FRAME_HEIGHT: f64 = 208.0;
pub const COLUMNS: f64 = 8.0;

pub const BUILT_IN_ID: &str = "glitchcat";
pub const BUILT_IN_NAME: &str = "Glitchcat";
pub static BUILT_IN_SPRITESHEET: &[u8] = include_bytes!("../assets/glitchcat.webp");

struct SpriteRow {
    row: u32,
    frames: u32,
    duration_secs: f64,
}

fn sprite_row(anim: Anim) -> SpriteRow {
    let (row, frames, duration_ms) = match anim {
        Anim::Idle => (0, 6, 5500),
        Anim::RunRight => (1, 8, 1060),
        Anim::RunLeft => (2, 8, 1060),
        Anim::Waving => (3, 4, 700),
        Anim::Jumping => (4, 5, 840),
        Anim::Failed => (5, 8, 1220),
        Anim::Waiting => (6, 6, 1010),
        Anim::Running => (7, 6, 820),
        Anim::Review => (8, 6, 1030),
    };
    SpriteRow {
        row,
        frames,
        duration_secs: f64::from(duration_ms) / 1000.0,
    }
}

/// The (row, column) of the frame to show `elapsed` seconds into `anim`.
pub fn frame_at(anim: Anim, elapsed: f64) -> (u32, u32) {
    let sprite = sprite_row(anim);
    let frame_secs = sprite.duration_secs / f64::from(sprite.frames);
    let column = (elapsed / frame_secs) as u64 % u64::from(sprite.frames);
    (sprite.row, column as u32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PetEntry {
    pub id: String,
    pub name: String,
    /// `None` for the bundled pet.
    pub spritesheet: Option<PathBuf>,
}

impl PetEntry {
    pub fn built_in() -> PetEntry {
        PetEntry {
            id: BUILT_IN_ID.into(),
            name: BUILT_IN_NAME.into(),
            spritesheet: None,
        }
    }

    pub fn spritesheet_bytes(&self) -> std::io::Result<Vec<u8>> {
        match &self.spritesheet {
            Some(path) => std::fs::read(path),
            None => Ok(BUILT_IN_SPRITESHEET.to_vec()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PetManifest {
    display_name: String,
    spritesheet_path: String,
}

/// The bundled pet followed by every valid pet folder in `dir`, sorted by
/// name. A folder named like the bundled pet is skipped, so it is never
/// listed twice.
pub fn list_pets(dir: &Path) -> Vec<PetEntry> {
    let mut installed: Vec<PetEntry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name() != BUILT_IN_ID)
        .filter_map(|entry| match read_pet(&entry.path()) {
            Ok(pet) => Some(pet),
            Err(reason) => {
                eprintln!(
                    "[pets] skipping {}: {reason}",
                    entry.file_name().to_string_lossy()
                );
                None
            }
        })
        .collect();
    installed.sort_by(|a, b| a.name.cmp(&b.name));

    let mut pets = vec![PetEntry::built_in()];
    pets.extend(installed);
    pets
}

fn read_pet(folder: &Path) -> Result<PetEntry, String> {
    let manifest_text =
        std::fs::read_to_string(folder.join("pet.json")).map_err(|_| "no readable pet.json")?;
    let manifest: PetManifest =
        serde_json::from_str(&manifest_text).map_err(|_| "invalid pet.json")?;

    let relative = Path::new(&manifest.spritesheet_path);
    let stays_inside = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !stays_inside {
        return Err("spritesheetPath must be a relative path inside the pet folder".into());
    }
    let is_supported = matches!(
        relative
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("webp" | "png")
    );
    if !is_supported {
        return Err("spritesheet must be .webp or .png".into());
    }

    let spritesheet = folder.join(relative);
    if !spritesheet.is_file() {
        return Err("spritesheet file is missing".into());
    }

    let id = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("pet folder has no name")?;
    Ok(PetEntry {
        id,
        name: manifest.display_name,
        spritesheet: Some(spritesheet),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pet(root: &Path, folder: &str, manifest: &str) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pet.json"), manifest).unwrap();
        std::fs::write(dir.join("sheet.webp"), b"not decoded here").unwrap();
    }

    #[test]
    fn lists_the_bundled_pet_and_valid_installed_pets_only() {
        let root = std::env::temp_dir().join(format!("openpets-rs-pets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_pet(
            &root,
            "fox",
            r#"{ "displayName": "Fox", "spritesheetPath": "sheet.webp" }"#,
        );
        write_pet(
            &root,
            "escape",
            r#"{ "displayName": "Escape", "spritesheetPath": "../fox/sheet.webp" }"#,
        );
        write_pet(&root, "broken", "{");
        write_pet(
            &root,
            BUILT_IN_ID,
            r#"{ "displayName": "Duplicate", "spritesheetPath": "sheet.webp" }"#,
        );

        let names: Vec<String> = list_pets(&root).into_iter().map(|pet| pet.name).collect();

        assert_eq!(names, [BUILT_IN_NAME, "Fox"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
