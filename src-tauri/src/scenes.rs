// Trovul - Mixer scenes: a saved mix of both PCs (master, every app's volume,
// mute and output, and the sound received from the other PC), applied with
// one click from the Mixer, the tray mixer or a Stream Deck. Stored in
// `settings.json`; captured and applied below.

use crate::settings;
use serde::{Deserialize, Serialize};

pub const MAX_SCENES: usize = 12;
const MAX_APPS: usize = 64;
const MAX_NAME: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppMix {
    /// Process name as the mixer shows it ("Discord", "chrome").
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    /// Output device (endpoint id on that PC); `None` = the Windows default.
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MixSide {
    pub master: Option<Level>,
    #[serde(default)]
    pub apps: Vec<AppMix>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub name: String,
    pub this_pc: MixSide,
    /// Captured while linked.
    #[serde(default)]
    pub other_pc: Option<MixSide>,
    /// Volume / mute of the sound received from the other PC.
    #[serde(default)]
    pub incoming: Option<Level>,
}

fn clean_level(level: Level) -> Option<Level> {
    level.volume.is_finite().then(|| Level {
        volume: level.volume.clamp(0.0, 1.0),
        muted: level.muted,
    })
}

fn clean_side(side: MixSide) -> MixSide {
    MixSide {
        master: side.master.and_then(clean_level),
        apps: side
            .apps
            .into_iter()
            .filter(|a| a.volume.is_finite() && !a.name.trim().is_empty())
            .take(MAX_APPS)
            .map(|a| AppMix {
                name: a.name.trim().chars().take(128).collect(),
                volume: a.volume.clamp(0.0, 1.0),
                muted: a.muted,
                output: a.output.filter(|id| !id.is_empty() && id.len() < 512),
            })
            .collect(),
    }
}

/// Keep what can be stored: trimmed names, volumes in 0–1, unique ids, a cap.
pub fn clean(scenes: Vec<Scene>) -> Vec<Scene> {
    let mut seen = std::collections::HashSet::new();
    scenes
        .into_iter()
        .filter_map(|s| {
            let name: String = s.name.trim().chars().take(MAX_NAME).collect();
            let name = name.trim_end().to_string();
            let fresh = !s.id.is_empty() && s.id.len() <= 64 && seen.insert(s.id.clone());
            (fresh && !name.is_empty()).then(|| Scene {
                id: s.id,
                name,
                this_pc: clean_side(s.this_pc),
                other_pc: s.other_pc.map(clean_side),
                incoming: s.incoming.and_then(clean_level),
            })
        })
        .take(MAX_SCENES)
        .collect()
}

pub fn list() -> Vec<Scene> {
    settings::get().mix_scenes
}

pub fn save(scenes: Vec<Scene>) -> Vec<Scene> {
    let scenes = clean(scenes);
    settings::update(|s| s.mix_scenes = scenes.clone());
    scenes
}

// ============ Capture and apply ============
//
// Here rather than in the UI so they also work with the window closed: from
// the Stream Deck (companion API) and shortcuts. Apps are matched by name,
// and only the regular mixer requests go to the other PC, so a scene works
// with a peer that knows nothing about scenes.

use crate::audio_sessions::MixerSnapshot;
use crate::peer::{Request, Response};

fn side_of(mixer: &MixerSnapshot) -> MixSide {
    MixSide {
        master: mixer.master.as_ref().map(|m| Level {
            volume: m.volume,
            muted: m.muted,
        }),
        apps: mixer
            .sessions
            .iter()
            .map(|s| AppMix {
                name: s.name.clone(),
                volume: s.volume,
                muted: s.muted,
                output: s.output.clone(),
            })
            .collect(),
    }
}

async fn local_mixer() -> Result<MixerSnapshot, String> {
    tauri::async_runtime::spawn_blocking(crate::audio_sessions::get_mixer_snapshot)
        .await
        .map_err(|e| e.to_string())?
}

async fn remote_mixer() -> Option<MixerSnapshot> {
    match crate::connection::request(Request::GetMixer).await {
        Ok(Response::Mixer { mixer }) => Some(mixer),
        _ => None,
    }
}

/// The mix both PCs have right now, as a scene named `name`.
pub async fn capture(name: String, id: Option<String>) -> Result<Scene, String> {
    let local = local_mixer().await?;
    let remote = remote_mixer().await;
    let s = settings::get();
    Ok(Scene {
        id: id.unwrap_or_else(crate::util::random_hex::<8>),
        name,
        this_pc: side_of(&local),
        other_pc: remote.as_ref().map(side_of),
        incoming: Some(Level {
            volume: s.remote_volume,
            muted: s.remote_muted,
        }),
    })
}

const NEAR: f32 = 0.005;

/// What to change on one PC so its mix matches `side`: the requests, and
/// how many of the scene's apps are running there.
fn plan(side: &MixSide, mixer: &MixerSnapshot) -> (Vec<Request>, usize) {
    let mut changes = Vec::new();
    if let (Some(want), Some(have)) = (&side.master, &mixer.master) {
        if (want.volume - have.volume).abs() > NEAR {
            changes.push(Request::SetMasterVolume {
                volume: want.volume,
            });
        }
        if want.muted != have.muted {
            changes.push(Request::SetMasterMute { muted: want.muted });
        }
    }
    let mut found = 0;
    for session in &mixer.sessions {
        let Some(app) = side
            .apps
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(&session.name))
        else {
            continue;
        };
        found += 1;
        if (app.volume - session.volume).abs() > NEAR {
            changes.push(Request::SetSessionVolume {
                pid: session.pid,
                volume: app.volume,
            });
        }
        if app.muted != session.muted {
            changes.push(Request::SetSessionMute {
                pid: session.pid,
                muted: app.muted,
            });
        }
        if mixer.per_app_output && app.output != session.output {
            changes.push(Request::SetAppOutput {
                pid: session.pid,
                device: app.output.clone(),
            });
        }
    }
    (changes, found)
}

/// Do a change on this PC (the same requests the other PC would get).
fn apply_here(change: Request) {
    let result = match change {
        Request::SetMasterVolume { volume } => crate::audio_sessions::set_master_volume(volume),
        Request::SetMasterMute { muted } => crate::audio_sessions::set_master_mute(muted),
        Request::SetSessionVolume { pid, volume } => {
            crate::audio_sessions::set_session_volume(pid, volume)
        }
        Request::SetSessionMute { pid, muted } => {
            crate::audio_sessions::set_session_mute(pid, muted)
        }
        Request::SetAppOutput { pid, device } => crate::route_app(pid, device),
        _ => Ok(()),
    };
    // An app that just closed, a device that's gone: the rest still applies.
    if let Err(e) = result {
        log::debug!("Scene: {e}");
    }
}

/// Put scene `id` back on both PCs; returns a short summary.
pub async fn apply(id: &str) -> Result<String, String> {
    let scene = list()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("That scene no longer exists")?;
    let local = local_mixer().await?;
    let (changes, mut apps) = plan(&scene.this_pc, &local);
    tauri::async_runtime::spawn_blocking(move || changes.into_iter().for_each(apply_here))
        .await
        .map_err(|e| e.to_string())?;
    let linked = crate::connection::current().is_some();
    if let (true, Some(side)) = (linked, &scene.other_pc) {
        if let Some(remote) = remote_mixer().await {
            let (changes, found) = plan(side, &remote);
            apps += found;
            for change in changes {
                if let Err(e) = crate::connection::request(change).await {
                    log::debug!("Scene on the other PC: {e}");
                }
            }
        }
    }
    if let (true, Some(incoming)) = (linked, &scene.incoming) {
        crate::set_incoming(Some(incoming.volume), Some(incoming.muted));
    }
    let summary = match apps {
        0 => format!("“{}” applied", scene.name),
        1 => format!("“{}” applied to 1 app", scene.name),
        n => format!("“{}” applied to {n} apps", scene.name),
    };
    crate::on_scene_applied(&scene.name);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(id: &str, name: &str) -> Scene {
        Scene {
            id: id.into(),
            name: name.into(),
            this_pc: MixSide {
                master: Some(Level {
                    volume: 1.7,
                    muted: false,
                }),
                apps: vec![
                    AppMix {
                        name: " Discord ".into(),
                        volume: 0.5,
                        muted: true,
                        output: Some(String::new()),
                    },
                    AppMix {
                        name: "bad".into(),
                        volume: f32::NAN,
                        muted: false,
                        output: None,
                    },
                ],
            },
            other_pc: None,
            incoming: Some(Level {
                volume: -1.0,
                muted: true,
            }),
        }
    }

    #[test]
    fn scenes_are_cleaned_before_saving() {
        let cleaned = clean(vec![
            scene("a", "  Game night  "),
            scene("a", "duplicate id"),
            scene("b", "   "),
            scene("c", &"x".repeat(80)),
        ]);
        assert_eq!(cleaned.len(), 2);
        assert_eq!(cleaned[0].name, "Game night");
        assert_eq!(cleaned[0].this_pc.master.as_ref().unwrap().volume, 1.0);
        assert_eq!(cleaned[0].this_pc.apps.len(), 1);
        assert_eq!(cleaned[0].this_pc.apps[0].name, "Discord");
        assert_eq!(cleaned[0].this_pc.apps[0].output, None);
        assert_eq!(cleaned[0].incoming.as_ref().unwrap().volume, 0.0);
        assert_eq!(cleaned[1].name.chars().count(), MAX_NAME);
    }

    #[test]
    fn at_most_a_dozen() {
        let many: Vec<Scene> = (0..20).map(|i| scene(&format!("s{i}"), "Scene")).collect();
        assert_eq!(clean(many).len(), MAX_SCENES);
    }
}
