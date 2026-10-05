//! The orchestra: turns what every Cylinder heard (per beat) into a static level offset and a
//! per-beat gain curve for each channel. Pure functions, no host, so they are tested directly.
//!
//! Rules (all in dB, before the Engine's amount scaling):
//! - Static balance: each channel's loudness while playing is moved towards a role template
//!   (drums 0, bass -2, lead -1, keys -5, pad -9, texture -15, relative to the mix), clamped to +-9.
//! - Moments, per beat. Only lead and keys parts can be "moving" (pads/texture are beds by role):
//!   one part moving = solo moment +3; two moving +1.5 each; three moving: the busiest +1;
//!   four or more (crowded): the busiest +1, the other movers -1.5.
//!   Sustained parts (drones, pads, held chords, texture) sit back -1.5 while something moves.
//!   Only one sustained melodic part and nothing moving: it is the feature, +1.5.
//! - Drums and bass keep their static level (no moments), so the groove never pumps.
//! - Curves are smoothed over 3 beats and clamped to +-6.

use nih_plug::prelude::Enum;
use serde::Serialize;

#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Role {
    Auto,
    Drums,
    Bass,
    Lead,
    Keys,
    Pad,
    Texture,
}

impl Role {
    pub const ALL: [Role; 7] = [Role::Auto, Role::Drums, Role::Bass, Role::Lead, Role::Keys, Role::Pad, Role::Texture];

    pub fn from_index(i: u32) -> Role {
        *Self::ALL.get(i as usize).unwrap_or(&Role::Auto)
    }

    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|r| *r == self).unwrap_or(0) as u32
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Auto => "Auto",
            Role::Drums => "Drums",
            Role::Bass => "Bass",
            Role::Lead => "Lead",
            Role::Keys => "Keys",
            Role::Pad => "Pad",
            Role::Texture => "Texture",
        }
    }

    fn template_db(self) -> f64 {
        match self {
            Role::Drums => 0.0,
            Role::Bass => -2.0,
            Role::Lead => -1.0,
            Role::Keys => -5.0,
            Role::Pad => -9.0,
            Role::Texture => -15.0,
            Role::Auto => -5.0,
        }
    }

    fn melodic(self) -> bool {
        matches!(self, Role::Lead | Role::Keys | Role::Pad)
    }

    /// Only these can be featured as "moving". Pads and textures are beds by role, so a tremolo or
    /// LFO on a drone (which looks like a stream of attacks) never makes it a solo.
    fn can_move(self) -> bool {
        matches!(self, Role::Lead | Role::Keys)
    }
}

/// What one Cylinder learned. All vectors have one entry per beat.
#[derive(Clone, Debug, Default)]
pub struct TrackInput {
    pub id: u32,
    pub label: String,
    pub role: Option<Role>, // None = Auto
    /// Mean square of the channel per beat.
    pub energy: Vec<f32>,
    /// Attacks detected per beat.
    pub onsets: Vec<f32>,
    /// Mean square per beat in three bands: <250 Hz, 250 Hz-2.5 kHz, >2.5 kHz.
    pub low: Vec<f32>,
    pub mid: Vec<f32>,
    pub high: Vec<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackResult {
    pub id: u32,
    pub label: String,
    pub role: Role,
    pub role_guessed: bool,
    pub active_beats: usize,
    pub loudness_db: f64,
    pub static_db: f64,
    #[serde(skip)]
    pub curve_db: Vec<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Moment {
    pub from_bar: usize,
    pub to_bar: usize,
    pub featured: Vec<String>,
    pub sitting_back: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub beats: usize,
    pub beats_per_bar: usize,
    pub tracks: Vec<TrackResult>,
    pub moments: Vec<Moment>,
}

const ACTIVE_RANGE_DB: f64 = 35.0;
const MOVING_ONSETS: f32 = 0.5;

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-12).log10()
}

fn active_mask(t: &TrackInput) -> Vec<bool> {
    let max = t.energy.iter().copied().fold(0.0f32, f32::max) as f64;
    let floor = (max * 10f64.powf(-ACTIVE_RANGE_DB / 10.0)).max(1e-9);
    t.energy.iter().map(|e| (*e as f64) > floor).collect()
}

/// Guess a role from the learned features (used when the Cylinder is set to Auto).
pub fn guess_role(t: &TrackInput, active: &[bool]) -> Role {
    let (mut e, mut lo, mut hi, mut on, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
    for b in 0..active.len() {
        if active[b] {
            e += t.energy[b] as f64;
            lo += t.low[b] as f64;
            hi += t.high[b] as f64;
            on += t.onsets[b] as f64;
            n += 1;
        }
    }
    if n == 0 || e <= 0.0 {
        return Role::Texture;
    }
    let (lowf, highf, rate) = (lo / e, hi / e, on / n as f64);
    let activity = n as f64 / active.len().max(1) as f64;
    if lowf > 0.55 {
        Role::Bass
    } else if rate >= 1.5 && highf > 0.25 {
        Role::Drums
    } else if rate < 0.4 {
        if activity > 0.8 && highf > 0.5 { Role::Texture } else { Role::Pad }
    } else if lowf < 0.15 {
        Role::Lead
    } else {
        Role::Keys
    }
}

pub fn plan(tracks: &[TrackInput], beats_per_bar: usize) -> Plan {
    let beats = tracks.iter().map(|t| t.energy.len()).max().unwrap_or(0);
    let bpb = beats_per_bar.max(1);
    let padded: Vec<TrackInput> = tracks
        .iter()
        .map(|t| {
            let pad = |v: &Vec<f32>| {
                let mut v = v.clone();
                v.resize(beats, 0.0);
                v
            };
            TrackInput { energy: pad(&t.energy), onsets: pad(&t.onsets), low: pad(&t.low), mid: pad(&t.mid), high: pad(&t.high), ..t.clone() }
        })
        .collect();

    let actives: Vec<Vec<bool>> = padded.iter().map(active_mask).collect();
    let roles: Vec<(Role, bool)> = padded
        .iter()
        .zip(&actives)
        .map(|(t, a)| match t.role {
            Some(r) if r != Role::Auto => (r, false),
            _ => (guess_role(t, a), true),
        })
        .collect();

    // Static balance.
    let loud: Vec<Option<f64>> = padded
        .iter()
        .zip(&actives)
        .map(|(t, a)| {
            let (sum, n) = t.energy.iter().zip(a).filter(|(_, on)| **on).fold((0.0f64, 0usize), |(s, n), (e, _)| (s + *e as f64, n + 1));
            (n > 0).then(|| db(sum / n as f64))
        })
        .collect();
    let mut offsets: Vec<f64> = loud.iter().zip(&roles).filter_map(|(l, (r, _))| l.map(|l| l - r.template_db())).collect();
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let reference = if offsets.is_empty() { 0.0 } else { offsets[offsets.len() / 2] };

    // Moments.
    let mut raw = vec![vec![0.0f32; beats]; padded.len()];
    for b in 0..beats {
        let moving: Vec<usize> = (0..padded.len())
            .filter(|&i| actives[i][b] && roles[i].0.can_move() && padded[i].onsets[b] >= MOVING_ONSETS)
            .collect();
        let beds: Vec<usize> = (0..padded.len())
            .filter(|&i| actives[i][b] && !moving.contains(&i) && (roles[i].0.melodic() || roles[i].0 == Role::Texture))
            .collect();
        let busiest = moving.iter().copied().max_by(|&x, &y| {
            let score = |i: usize| padded[i].onsets[b] as f64 * (1.0 + padded[i].high[b] as f64 / padded[i].energy[b].max(1e-12) as f64);
            score(x).partial_cmp(&score(y)).unwrap()
        });
        match moving.len() {
            0 => {
                let melodic_beds: Vec<usize> = beds.iter().copied().filter(|&i| roles[i].0.melodic()).collect();
                if melodic_beds.len() == 1 {
                    raw[melodic_beds[0]][b] += 1.5;
                }
            }
            1 => raw[moving[0]][b] += 3.0,
            2 => moving.iter().for_each(|&i| raw[i][b] += 1.5),
            3 => raw[busiest.unwrap()][b] += 1.0,
            _ => {
                for &i in &moving {
                    raw[i][b] += if Some(i) == busiest { 1.0 } else { -1.5 };
                }
            }
        }
        if !moving.is_empty() {
            beds.iter().for_each(|&i| raw[i][b] -= 1.5);
        }
    }

    let results: Vec<TrackResult> = padded
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let curve: Vec<f32> = (0..beats)
                .map(|b| {
                    let lo = b.saturating_sub(1);
                    let hi = (b + 1).min(beats - 1);
                    let avg = raw[i][lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
                    avg.clamp(-6.0, 6.0)
                })
                .collect();
            let static_db = loud[i].map(|l| (reference + roles[i].0.template_db() - l).clamp(-9.0, 9.0)).unwrap_or(0.0);
            TrackResult {
                id: t.id,
                label: t.label.clone(),
                role: roles[i].0,
                role_guessed: roles[i].1,
                active_beats: actives[i].iter().filter(|a| **a).count(),
                loudness_db: loud[i].unwrap_or(f64::NEG_INFINITY),
                static_db,
                curve_db: curve,
            }
        })
        .collect();

    // Bar-level summary of who is featured / sitting back.
    let bars = beats.div_ceil(bpb);
    let name = |r: &TrackResult| if r.label.is_empty() { format!("#{} {}", r.id, r.role.name()) } else { r.label.clone() };
    let mut moments: Vec<Moment> = Vec::new();
    for bar in 0..bars {
        let range = bar * bpb..((bar + 1) * bpb).min(beats);
        let mean = |r: &TrackResult| r.curve_db[range.clone()].iter().sum::<f32>() / range.len() as f32;
        let featured: Vec<String> = results.iter().filter(|r| mean(r) >= 0.75).map(name).collect();
        let back: Vec<String> = results.iter().filter(|r| mean(r) <= -0.75).map(name).collect();
        match moments.last_mut() {
            Some(m) if m.featured == featured && m.sitting_back == back => m.to_bar = bar + 1,
            _ => moments.push(Moment { from_bar: bar + 1, to_bar: bar + 1, featured, sitting_back: back }),
        }
    }
    moments.retain(|m| !m.featured.is_empty() || !m.sitting_back.is_empty());

    Plan { beats, beats_per_bar: bpb, tracks: results, moments }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u32, role: Role, energy: Vec<f32>, onsets: Vec<f32>) -> TrackInput {
        let n = energy.len();
        TrackInput {
            id,
            label: String::new(),
            role: Some(role),
            low: energy.iter().map(|e| e * 0.2).collect(),
            mid: energy.iter().map(|e| e * 0.6).collect(),
            high: energy.iter().map(|e| e * 0.2).collect(),
            energy,
            onsets: { let mut o = onsets; o.resize(n, 0.0); o },
        }
    }

    #[test]
    fn solo_mover_over_drone_is_featured_and_drone_sits_back() {
        // 12 beats: cello moves on beats 0-5 alone over a drone; from beat 6 piano and music box join.
        let cello = track(1, Role::Lead, vec![0.1; 12], vec![2.0; 12]);
        let drone = track(2, Role::Pad, vec![0.1; 12], vec![0.0; 12]);
        let mut piano_e = vec![0.0; 6];
        piano_e.extend(vec![0.1; 6]);
        let piano = track(3, Role::Keys, piano_e.clone(), vec![2.0; 12]);
        let mbox = track(4, Role::Lead, piano_e, vec![2.0; 12]);
        let p = plan(&[cello, drone, piano, mbox], 3);
        let cello_c = &p.tracks[0].curve_db;
        assert!(cello_c[2] > 2.5, "solo cello boosted: {}", cello_c[2]);
        assert!(cello_c[10] < 1.5, "cello not boosted when 3 parts move: {}", cello_c[10]);
        assert!(p.tracks[1].curve_db[2] < -1.0, "drone sits back under the solo");
        assert!(!p.moments.is_empty());
    }

    #[test]
    fn tremolo_pad_is_never_featured_over_a_lead() {
        // Organ pedal with tremolo: lots of "attacks" every beat. Cello lead holds long notes (no attacks).
        let organ = track(1, Role::Pad, vec![0.1; 9], vec![3.0; 9]);
        let cello = track(2, Role::Lead, vec![0.1; 9], vec![0.0; 9]);
        let p = plan(&[organ, cello], 3);
        assert!(p.tracks[0].curve_db.iter().all(|c| *c <= 0.0), "organ boosted: {:?}", p.tracks[0].curve_db);
    }

    #[test]
    fn crowded_section_pulls_back_all_but_the_busiest() {
        let parts: Vec<TrackInput> = (0..4).map(|i| track(i, Role::Keys, vec![0.1; 6], vec![1.0 + i as f32; 6])).collect();
        let p = plan(&parts, 3);
        assert!(p.tracks[3].curve_db[3] > 0.5, "busiest part featured");
        for t in &p.tracks[..3] {
            assert!(t.curve_db[3] < -1.0, "others pulled back: {}", t.curve_db[3]);
        }
    }

    #[test]
    fn drums_and_bass_get_no_moments_and_static_follows_template() {
        let drums = track(1, Role::Drums, vec![0.1; 6], vec![3.0; 6]);
        let bass = track(2, Role::Bass, vec![0.1; 6], vec![1.0; 6]);
        let pad = track(3, Role::Pad, vec![0.1; 6], vec![0.0; 6]);
        let p = plan(&[drums, bass, pad], 3);
        assert!(p.tracks[0].curve_db.iter().all(|c| *c == 0.0));
        assert!(p.tracks[1].curve_db.iter().all(|c| *c == 0.0));
        // Equal raw loudness: pad must end 9 dB under drums, bass 2 dB under.
        let s = |i: usize| p.tracks[i].static_db;
        assert!(((s(0) - s(2)) - 9.0).abs() < 0.01, "{} {}", s(0), s(2));
        assert!(((s(0) - s(1)) - 2.0).abs() < 0.01);
    }

    #[test]
    fn guesses_roles_from_features() {
        let mut bass = track(1, Role::Auto, vec![0.1; 8], vec![0.5; 8]);
        bass.low = vec![0.08; 8];
        bass.role = None;
        let mut rain = track(2, Role::Auto, vec![0.1; 8], vec![0.0; 8]);
        rain.high = vec![0.07; 8];
        rain.low = vec![0.0; 8];
        rain.role = None;
        let p = plan(&[bass, rain], 3);
        assert_eq!(p.tracks[0].role, Role::Bass);
        assert!(p.tracks[0].role_guessed);
        assert_eq!(p.tracks[1].role, Role::Texture);
    }
}
