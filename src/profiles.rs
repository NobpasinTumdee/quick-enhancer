//! Named settings profiles: built-in (read-only, translated names) and user profiles stored
//! as one TOML file each in `<data>/profiles/`.
//!
//! A profile stores a *recipe* (`JobSettings::recipe()`: no machine values, output folder or
//! overwrite policy) for one page type. Performance values are stored only when the user
//! ticks "include performance settings". Files carry `schema_version` + the saving app
//! version; unknown / missing fields fall back to defaults (`#[serde(default)]` everywhere),
//! so files from newer or older versions load.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::{
    FrameGenMode, JobSettings, MachineSettings, Profile, ScaleMode, SharpenMode, Studio,
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_NAME_LEN: usize = 64;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ProfileFile {
    pub schema_version: u32,
    pub app_version: String,
    pub name: String,
    pub kind: Studio,
    pub settings: JobSettings,
    /// Only present when saved with "include performance settings".
    pub performance: Option<MachineSettings>,
}

impl Default for ProfileFile {
    fn default() -> Self {
        ProfileFile {
            schema_version: SCHEMA_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            name: String::new(),
            kind: Studio::All,
            settings: JobSettings::default(),
            performance: None,
        }
    }
}

impl ProfileFile {
    pub fn new(name: &str, kind: Studio, settings: &JobSettings, perf: Option<MachineSettings>) -> Self {
        ProfileFile {
            name: name.trim().to_string(),
            kind,
            settings: settings.recipe(),
            performance: perf,
            ..Default::default()
        }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_toml(text: &str) -> Result<ProfileFile, String> {
        let mut p: ProfileFile = toml::from_str(text).map_err(|e| e.to_string())?;
        p.name = p.name.trim().to_string();
        p.settings = p.settings.recipe();
        Ok(p)
    }
}

/// A built-in profile.
pub struct Builtin {
    pub id: &'static str,
    pub kind: Studio,
    pub settings: fn() -> JobSettings,
}

impl Builtin {
    pub fn name(&self) -> String {
        crate::i18n::tr(&format!("profiles.builtin_{}", self.id))
    }
}

fn video(f: impl FnOnce(&mut JobSettings)) -> JobSettings {
    let mut s = JobSettings::video_defaults();
    f(&mut s);
    s.recipe()
}

fn image(f: impl FnOnce(&mut JobSettings)) -> JobSettings {
    let mut s = JobSettings::image_defaults();
    f(&mut s);
    s.recipe()
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "anime_4k60",
        kind: Studio::Video,
        settings: || {
            video(|s| {
                s.profile = Profile::Anime;
                s.scale = ScaleMode::Target(2160);
                s.framegen = FrameGenMode::TargetFps(60);
                s.sharpen = SharpenMode::Cas;
                s.cas_strength = 0.3;
            })
        },
    },
    Builtin {
        id: "live_2x60",
        kind: Studio::Video,
        settings: || {
            video(|s| {
                s.profile = Profile::Realistic;
                s.scale = ScaleMode::X2;
                s.framegen = FrameGenMode::TargetFps(60);
                s.denoise = 0.15;
            })
        },
    },
    Builtin {
        id: "smooth60",
        kind: Studio::Video,
        settings: || {
            video(|s| {
                s.upscale_on = false;
                s.framegen = FrameGenMode::TargetFps(60);
                s.denoise_on = false;
                s.sharpen_on = false;
            })
        },
    },
    Builtin {
        id: "light_cleanup",
        kind: Studio::Video,
        settings: || {
            video(|s| {
                s.upscale_on = false;
                s.framegen_on = false;
                s.denoise = 0.35;
                s.sharpen = SharpenMode::Cas;
                s.cas_strength = 0.45;
            })
        },
    },
    Builtin {
        id: "photo_restore",
        kind: Studio::Image,
        settings: || {
            image(|s| {
                s.profile = Profile::Realistic;
                s.scale = ScaleMode::X1;
                s.sharpen = SharpenMode::Cas;
                s.cas_strength = 0.25;
                s.clarity = 0.25;
            })
        },
    },
    Builtin {
        id: "photo_4x",
        kind: Studio::Image,
        settings: || {
            image(|s| {
                s.profile = Profile::Realistic;
                s.scale = ScaleMode::X4;
            })
        },
    },
    Builtin {
        id: "anime_4x",
        kind: Studio::Image,
        settings: || {
            image(|s| {
                s.profile = Profile::Anime;
                s.scale = ScaleMode::X4;
                s.denoise = 0.0;
            })
        },
    },
    Builtin {
        id: "pixel_4x",
        kind: Studio::Image,
        settings: || {
            image(|s| {
                s.profile = Profile::PixelArt;
                s.scale = ScaleMode::X4;
                s.denoise = 0.0;
                s.sharpen = SharpenMode::Off;
                s.output.image_format = crate::config::ImageFormat::Png;
            })
        },
    },
];

pub fn builtins_for(kind: Studio) -> impl Iterator<Item = &'static Builtin> {
    BUILTINS.iter().filter(move |b| b.kind == kind)
}

/// `builtin:<id>` / `user:<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileKey {
    Builtin(String),
    User(String),
}

impl ProfileKey {
    pub fn parse(s: &str) -> Option<ProfileKey> {
        if let Some(id) = s.strip_prefix("builtin:") {
            Some(ProfileKey::Builtin(id.into()))
        } else {
            s.strip_prefix("user:").map(|n| ProfileKey::User(n.into()))
        }
    }
    pub fn encode(&self) -> String {
        match self {
            ProfileKey::Builtin(id) => format!("builtin:{id}"),
            ProfileKey::User(n) => format!("user:{n}"),
        }
    }
}

/// Why a profile name is not acceptable.
#[derive(Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    Taken,
}

impl NameError {
    pub fn message(&self) -> String {
        match self {
            NameError::Empty => crate::t!("profiles.err_empty"),
            NameError::TooLong => crate::t!("profiles.err_long", n = MAX_NAME_LEN),
            NameError::Taken => crate::t!("profiles.err_taken"),
        }
    }
}

/// File-name-safe version of a profile name (Thai and other letters kept).
pub fn safe_file_stem(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        s = "profile".into();
    }
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&base.as_str()) {
        s = format!("_{s}");
    }
    s.chars().take(80).collect()
}

fn kind_prefix(k: Studio) -> &'static str {
    match k {
        Studio::All => "all",
        Studio::Video => "video",
        Studio::Image => "image",
    }
}

/// A user profile on disk.
#[derive(Clone, Debug)]
pub struct UserProfile {
    pub file: PathBuf,
    pub data: ProfileFile,
}

pub struct ProfileStore {
    pub dir: PathBuf,
    pub user: Vec<UserProfile>,
}

impl ProfileStore {
    pub fn load(dir: &Path) -> ProfileStore {
        let mut user = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "toml") {
                    continue;
                }
                match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| ProfileFile::from_toml(&t)) {
                    Ok(data) if !data.name.is_empty() => user.push(UserProfile { file: p, data }),
                    Ok(_) => tracing::warn!("profile {} has no name; ignored", p.display()),
                    Err(e) => tracing::warn!("profile {} unreadable: {e}", p.display()),
                }
            }
        }
        user.sort_by_key(|u| u.data.name.to_lowercase());
        ProfileStore { dir: dir.to_path_buf(), user }
    }

    pub fn user_for(&self, kind: Studio) -> impl Iterator<Item = &UserProfile> {
        self.user.iter().filter(move |u| u.data.kind == kind)
    }

    pub fn find_user(&self, kind: Studio, name: &str) -> Option<&UserProfile> {
        let n = name.trim().to_lowercase();
        self.user_for(kind).find(|u| u.data.name.to_lowercase() == n)
    }

    /// Validate a new name for `kind`. `except` = the profile being renamed.
    pub fn check_name(&self, kind: Studio, name: &str, except: Option<&str>) -> Result<(), NameError> {
        let n = name.trim();
        if n.is_empty() {
            return Err(NameError::Empty);
        }
        if n.chars().count() > MAX_NAME_LEN {
            return Err(NameError::TooLong);
        }
        let taken = self.find_user(kind, n).is_some_and(|u| {
            except.is_none_or(|e| e.trim().to_lowercase() != u.data.name.to_lowercase())
        }) || builtins_for(kind).any(|b| b.name().to_lowercase() == n.to_lowercase());
        if taken {
            return Err(NameError::Taken);
        }
        Ok(())
    }

    /// A free name like "Name (2)".
    pub fn unique_name(&self, kind: Studio, base: &str) -> String {
        let base: String = base.trim().chars().take(MAX_NAME_LEN - 5).collect();
        if self.check_name(kind, &base, None).is_ok() {
            return base;
        }
        (2..1000)
            .map(|i| format!("{base} ({i})"))
            .find(|n| self.check_name(kind, n, None).is_ok())
            .unwrap_or(base)
    }

    fn free_file(&self, kind: Studio, name: &str) -> PathBuf {
        let stem = format!("{}-{}", kind_prefix(kind), safe_file_stem(name));
        let mut p = self.dir.join(format!("{stem}.toml"));
        let mut i = 2;
        while p.exists() {
            p = self.dir.join(format!("{stem}-{i}.toml"));
            i += 1;
        }
        p
    }

    fn write(file: &Path, data: &ProfileFile) -> Result<(), String> {
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = file.with_extension("toml.tmp");
        std::fs::write(&tmp, data.to_toml()?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, file).map_err(|e| e.to_string())
    }

    /// Create a new user profile.
    pub fn create(&mut self, data: ProfileFile) -> Result<(), String> {
        self.check_name(data.kind, &data.name, None).map_err(|e| e.message())?;
        let file = self.free_file(data.kind, &data.name);
        Self::write(&file, &data)?;
        self.user.push(UserProfile { file, data });
        self.user.sort_by_key(|u| u.data.name.to_lowercase());
        Ok(())
    }

    /// Overwrite an existing user profile's settings (name unchanged).
    pub fn update(&mut self, kind: Studio, name: &str, settings: &JobSettings, perf: Option<MachineSettings>) -> Result<(), String> {
        let n = name.trim().to_lowercase();
        let u = self
            .user
            .iter_mut()
            .find(|u| u.data.kind == kind && u.data.name.to_lowercase() == n)
            .ok_or_else(|| crate::t!("profiles.err_missing"))?;
        u.data.settings = settings.recipe();
        u.data.performance = perf;
        u.data.app_version = env!("CARGO_PKG_VERSION").into();
        u.data.schema_version = SCHEMA_VERSION;
        Self::write(&u.file, &u.data)
    }

    pub fn rename(&mut self, kind: Studio, old: &str, new: &str) -> Result<(), String> {
        self.check_name(kind, new, Some(old)).map_err(|e| e.message())?;
        let n = old.trim().to_lowercase();
        let idx = self
            .user
            .iter()
            .position(|u| u.data.kind == kind && u.data.name.to_lowercase() == n)
            .ok_or_else(|| crate::t!("profiles.err_missing"))?;
        let mut data = self.user[idx].data.clone();
        data.name = new.trim().to_string();
        let old_file = self.user[idx].file.clone();
        let file = if safe_file_stem(old) == safe_file_stem(new) {
            old_file.clone()
        } else {
            self.free_file(kind, new)
        };
        Self::write(&file, &data)?;
        if file != old_file {
            let _ = std::fs::remove_file(&old_file);
        }
        self.user[idx] = UserProfile { file, data };
        self.user.sort_by_key(|u| u.data.name.to_lowercase());
        Ok(())
    }

    pub fn delete(&mut self, kind: Studio, name: &str) -> Result<(), String> {
        let n = name.trim().to_lowercase();
        let idx = self
            .user
            .iter()
            .position(|u| u.data.kind == kind && u.data.name.to_lowercase() == n)
            .ok_or_else(|| crate::t!("profiles.err_missing"))?;
        let f = self.user.remove(idx).file;
        std::fs::remove_file(&f).map_err(|e| e.to_string())
    }

    /// Settings of a profile key (built-in or user) for `kind`.
    pub fn resolve(&self, kind: Studio, key: &ProfileKey) -> Option<(String, JobSettings, Option<MachineSettings>)> {
        match key {
            ProfileKey::Builtin(id) => builtins_for(kind)
                .find(|b| b.id == id)
                .map(|b| (b.name(), (b.settings)(), None)),
            ProfileKey::User(n) => self
                .find_user(kind, n)
                .map(|u| (u.data.name.clone(), u.data.settings.clone(), u.data.performance.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qe-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_file_stem("a/b:c"), "a_b_c");
        assert_eq!(safe_file_stem("  ภาพถ่าย 4x  "), "ภาพถ่าย 4x");
        assert_eq!(safe_file_stem("con"), "_con");
        assert_eq!(safe_file_stem("..."), "profile");
        assert_eq!(safe_file_stem("name. "), "name");
    }

    #[test]
    fn create_rename_duplicate_delete_survive_reload() {
        let dir = tmp("crud");
        let mut st = ProfileStore::load(&dir);
        let s = JobSettings { denoise: 0.42, ..JobSettings::video_defaults() };
        st.create(ProfileFile::new("งานของฉัน", Studio::Video, &s, None)).unwrap();
        // Same name (case-insensitive) on the same page is rejected, other page is fine.
        assert_eq!(st.check_name(Studio::Video, "  งานของฉัน ", None), Err(NameError::Taken));
        assert!(st.check_name(Studio::Image, "งานของฉัน", None).is_ok());
        assert_eq!(st.check_name(Studio::Video, " ", None), Err(NameError::Empty));
        assert_eq!(st.check_name(Studio::Video, &"x".repeat(65), None), Err(NameError::TooLong));
        let dup = st.unique_name(Studio::Video, "งานของฉัน");
        assert_eq!(dup, "งานของฉัน (2)");
        st.create(ProfileFile::new(&dup, Studio::Video, &s, Some(MachineSettings::default()))).unwrap();
        st.rename(Studio::Video, "งานของฉัน", "Mine/2").unwrap();
        let st2 = ProfileStore::load(&dir);
        assert_eq!(st2.user_for(Studio::Video).count(), 2);
        let (_, set, perf) = st2.resolve(Studio::Video, &ProfileKey::User("mine/2".into())).unwrap();
        assert_eq!(set.denoise, 0.42);
        assert!(perf.is_none());
        let (_, _, perf) = st2.resolve(Studio::Video, &ProfileKey::User(dup.clone())).unwrap();
        assert!(perf.is_some());
        let mut st2 = st2;
        st2.delete(Studio::Video, &dup).unwrap();
        assert_eq!(ProfileStore::load(&dir).user.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_and_missing_fields_use_defaults() {
        let text = "schema_version = 99\nname = \"Future\"\nkind = \"Image\"\nnew_thing = 1\n[settings]\ndenoise = 0.3\nfuture_knob = true\n";
        let p = ProfileFile::from_toml(text).unwrap();
        assert_eq!(p.name, "Future");
        assert_eq!(p.kind, Studio::Image);
        assert_eq!(p.settings.denoise, 0.3);
        assert_eq!(p.settings.scale, JobSettings::default().scale);
        // Round trip.
        let back = ProfileFile::from_toml(&p.to_toml().unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn builtins_have_names_and_match_their_page() {
        for b in BUILTINS {
            assert!(!b.name().starts_with("profiles."), "missing name for {}", b.id);
            let s = (b.settings)();
            assert_eq!(s, s.recipe());
        }
        assert_eq!(builtins_for(Studio::Video).count(), 4);
        assert_eq!(builtins_for(Studio::Image).count(), 4);
        let k = ProfileKey::parse("user:ชื่อ").unwrap();
        assert_eq!(ProfileKey::parse(&k.encode()), Some(k));
    }
}
