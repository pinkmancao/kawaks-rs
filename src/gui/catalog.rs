use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

const PLATFORMS: [(&str, &str); 3] = [("cps1", "CPS1"), ("cps2", "CPS2"), ("neogeo", "NEO GEO")];

pub(super) struct Entry {
    pub id: String,
    pub label: String,
    pub platform: &'static str,
    pub driver: Option<&'static str>,
    pub rom: PathBuf,
    pub present: bool,
}

impl Entry {
    pub fn matches(&self, query: &str) -> bool {
        self.id.contains(query)
            || self.label.to_ascii_lowercase().contains(query)
            || self.platform.to_ascii_lowercase().contains(query)
            || (matches!(self.driver, Some("wof" | "wofa")) && "三国志ii 三国志2".contains(query))
    }
}

pub(super) fn discover() -> Vec<Entry> {
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let explicit = std::env::var_os("KAWAKS_ROM_DIR").map(PathBuf::from);
    let inferred = cwd
        .as_deref()
        .into_iter()
        .chain(exe.as_deref().and_then(Path::parent))
        .flat_map(|base| base.ancestors().take(6))
        .map(|base| base.join("roms"));
    collect(explicit.into_iter().chain(inferred))
}

fn collect(roots: impl IntoIterator<Item = PathBuf>) -> Vec<Entry> {
    let mut archives = Vec::<Entry>::new();
    let mut visited = HashSet::new();
    let mut keys = HashSet::new();
    for root in roots {
        if !root.is_dir() || !visited.insert(root.clone()) {
            continue;
        }
        for (directory, platform) in PLATFORMS.into_iter().chain([("", "ROM")]) {
            let Ok(files) = fs::read_dir(root.join(directory)) else {
                continue;
            };
            for file in files.flatten() {
                let rom = file.path();
                if !rom
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                    || !rom.is_file()
                {
                    continue;
                }
                let Some(id) = rom.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let id = id.to_ascii_lowercase();
                if id == "neogeo" || !keys.insert((platform, id.clone())) {
                    continue;
                }
                archives.push(Entry {
                    label: id.to_ascii_uppercase(),
                    id,
                    platform,
                    driver: None,
                    rom,
                    present: true,
                });
            }
        }
    }

    let mut games = Vec::<Entry>::new();
    for (id, name, (directory, platform), archive) in [
        ("dino", "DINO", PLATFORMS[0], "dino"),
        ("wof", "WOF WORLD", PLATFORMS[0], "wof"),
        ("wofa", "WOF ASIA", PLATFORMS[0], "wof"),
        ("punisher", "PUNISHER", PLATFORMS[0], "punisher"),
        ("ssf2", "SSF2", PLATFORMS[1], "ssf2"),
        ("kof97", "KOF97", PLATFORMS[2], "kof97"),
    ] {
        let index = archives
            .iter()
            .position(|e| e.id == id && e.platform == platform)
            .or_else(|| {
                archives
                    .iter()
                    .position(|e| e.id == id && e.platform == "ROM")
            });
        let found = if let Some(index) = index {
            Some(archives.remove(index).rom)
        } else if id != archive {
            games
                .iter()
                .find(|entry| entry.id == archive && entry.present)
                .map(|entry| entry.rom.clone())
        } else {
            None
        };
        let present = found.is_some();
        let rom = found.unwrap_or_else(|| {
            Path::new("roms")
                .join(directory)
                .join(format!("{archive}.zip"))
        });
        games.push(Entry {
            id: id.to_owned(),
            label: name.to_owned(),
            platform,
            driver: Some(id),
            rom,
            present,
        });
    }
    archives.sort_by(|a, b| (a.platform, &a.id).cmp(&(b.platform, &b.id)));
    games.extend(archives);
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_roms_and_preserves_merged_wof_variant() {
        let root = std::env::temp_dir().join(format!("kawaks-catalog-{}", std::process::id()));
        fs::create_dir_all(root.join("cps1")).unwrap();
        fs::create_dir_all(root.join("neogeo")).unwrap();
        for name in ["cps1/wof.zip", "cps1/1941.ZIP", "neogeo/neogeo.zip"] {
            fs::write(root.join(name), []).unwrap();
        }
        let entries = collect([root.clone(), root.clone()]);
        let wof = entries.iter().find(|game| game.id == "wofa").unwrap();
        assert_eq!(wof.rom, root.join("cps1/wof.zip"));
        assert!(wof.present && wof.matches("三国志"));
        assert_eq!(entries.iter().filter(|entry| entry.id == "1941").count(), 1);
        assert!(entries
            .iter()
            .any(|entry| entry.id == "1941" && entry.driver.is_none()));
        assert!(!entries.iter().any(|entry| entry.id == "neogeo"));
        for name in ["cps1/wof.zip", "cps1/1941.ZIP", "neogeo/neogeo.zip"] {
            fs::remove_file(root.join(name)).unwrap();
        }
        for name in ["cps1", "neogeo"] {
            fs::remove_dir(root.join(name)).unwrap();
        }
        fs::remove_dir(root).unwrap();
    }
}
