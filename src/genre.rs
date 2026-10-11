//! Genre presets: what a mastered song in each genre usually leans towards. A preset sets the master
//! chain (tone tilt, glue, warmth, bass grit, width, loudness target) and the Engine's balance plan
//! (static level and stereo width per role). The user's own sliders are trims on top of it.
//!
//! These are starting points, not measurements of any one record. Trip hop follows the researched
//! genre profile in Mortiflix Music ("dark, warm, low-heavy and lo-fi... tape saturation... wide
//! cinematic strings over a dry, dull, heavy drum loop. Bass is huge; the top end is soft and rolled
//! off"; avoid "bright, glossy... loud, sharp hi-hats... sidechain pumping").

use crate::orchestra::Role;
use nih_plug::prelude::Enum;
use serde::Serialize;

#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Genre {
    Neutral,
    #[name = "Trip hop"]
    TripHop,
    #[name = "Hip hop"]
    HipHop,
    #[name = "Lo-fi"]
    LoFi,
    House,
    Techno,
    Synthwave,
    #[name = "French house"]
    FrenchHouse,
    Pop,
    Rock,
    Cinematic,
    #[name = "Drum & bass"]
    DrumAndBass,
    #[name = "Bass first"]
    BassFirst,
}

/// Per role, in the order of `Role::ALL` minus Auto: drums, bass, lead, keys, pad, texture.
pub type PerRole = [f64; 6];

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Preset {
    pub low_cut_hz: f64,
    pub low_db: f64,
    pub body_db: f64,
    pub presence_db: f64,
    pub air_db: f64,
    pub glue_threshold_db: f64,
    pub glue_ratio: f64,
    pub glue_attack_ms: f64,
    pub glue_release_ms: f64,
    /// Full-band tape-like saturation, dB of drive (0 = off).
    pub warmth_db: f64,
    /// Saturation of the low band only (below ~180 Hz): makes bass low and harsh.
    pub grit_db: f64,
    pub width: f64,
    pub mono_below_hz: f64,
    pub target_lufs: f64,
    /// Static level of each role relative to the mix, dB.
    pub role_db: PerRole,
    /// Stereo width each role is set to (side gain: 0 = mono, 1 = as is, 2 = double).
    pub role_width: PerRole,
}

impl Genre {
    pub const ALL: [Genre; 13] = [
        Genre::Neutral,
        Genre::TripHop,
        Genre::HipHop,
        Genre::LoFi,
        Genre::House,
        Genre::Techno,
        Genre::Synthwave,
        Genre::FrenchHouse,
        Genre::Pop,
        Genre::Rock,
        Genre::Cinematic,
        Genre::DrumAndBass,
        Genre::BassFirst,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Genre::Neutral => "Neutral",
            Genre::TripHop => "Trip hop",
            Genre::HipHop => "Hip hop",
            Genre::LoFi => "Lo-fi",
            Genre::House => "House",
            Genre::Techno => "Techno",
            Genre::Synthwave => "Synthwave",
            Genre::FrenchHouse => "French house",
            Genre::Pop => "Pop",
            Genre::Rock => "Rock",
            Genre::Cinematic => "Cinematic",
            Genre::DrumAndBass => "Drum & bass",
            Genre::BassFirst => "Bass first",
        }
    }

    /// One line on what the preset is going for (shown in the Engine).
    pub fn character(self) -> &'static str {
        match self {
            Genre::Neutral => "Your own settings, no genre leaning",
            Genre::TripHop => "Dark, warm, huge gritty bass, soft rolled-off top, slow glue, wide strings over dry drums",
            Genre::HipHop => "Heavy low end, punchy drums, centred vocal, firm glue",
            Genre::LoFi => "Dusty and soft: top rolled off, gentle glue, warm saturation",
            Genre::House => "Bright clean top, solid bass, pumping-free glue, wide pads",
            Genre::Techno => "Big low end, dark mids, fast glue, wide atmospheres",
            Genre::Synthwave => "Bright and wide: lifted presence and air, wide pads and leads",
            Genre::FrenchHouse => "Compressed and saturated, firm fast glue, filtered warmth",
            Genre::Pop => "Clear, bright and forward vocal, polished top",
            Genre::Rock => "Mid-forward, presence for guitars and vocal, punchy glue",
            Genre::Cinematic => "Dynamic and wide: light glue, no grit, quieter target",
            Genre::DrumAndBass => "Heavy sub, gritty bass, fast glue, loud",
            Genre::BassFirst => "The bass leads: bass over the drums, leads and keys further back, presence and air tamed so nothing harsh sits over the low end",
        }
    }

    pub fn preset(self) -> Preset {
        #[rustfmt::skip]
        let p = |cut, lo, body, pres, air, thr, ratio, att, rel, warm, grit, width, mono, lufs, role_db: PerRole, role_width: PerRole| Preset {
            low_cut_hz: cut, low_db: lo, body_db: body, presence_db: pres, air_db: air,
            glue_threshold_db: thr, glue_ratio: ratio, glue_attack_ms: att, glue_release_ms: rel,
            warmth_db: warm, grit_db: grit, width, mono_below_hz: mono, target_lufs: lufs, role_db, role_width,
        };
        //          cut   low  body  pres   air   thr  ratio att  rel   warm grit width mono  lufs
        //          role dB: drums bass lead keys pad texture         role width: drums bass lead keys pad texture
        #[rustfmt::skip]
        let preset = match self {
            Genre::Neutral =>     p(25.0, 0.0, 0.0, 0.0, 0.0, -18.0, 2.0, 30.0, 200.0, 0.0, 0.0, 1.0, 120.0, -14.0, [0.0, -2.0, -1.0, -5.0, -9.0, -15.0], [1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
            Genre::TripHop =>     p(28.0, 2.0, -0.5, -0.5, -2.5, -20.0, 2.0, 30.0, 300.0, 6.0, 9.0, 1.1, 150.0, -13.0, [0.0, -1.0, -1.0, -6.0, -8.0, -14.0], [0.8, 0.0, 0.7, 1.0, 1.5, 1.4]),
            Genre::HipHop =>      p(25.0, 1.5, -1.0, 1.0, 0.5, -18.0, 3.0, 10.0, 120.0, 3.0, 4.0, 1.0, 120.0, -11.0, [0.0, -1.0, 0.0, -6.0, -10.0, -15.0], [0.9, 0.0, 0.6, 1.0, 1.3, 1.3]),
            Genre::LoFi =>        p(30.0, 1.0, 0.5, -1.5, -4.0, -22.0, 2.0, 40.0, 250.0, 8.0, 3.0, 0.9, 120.0, -14.0, [0.0, -2.0, -2.0, -3.0, -8.0, -10.0], [0.8, 0.0, 0.9, 1.1, 1.3, 1.2]),
            Genre::House =>       p(30.0, 1.5, -1.5, 1.0, 1.5, -16.0, 2.5, 10.0, 100.0, 3.0, 2.0, 1.15, 150.0, -10.0, [0.0, -1.5, -2.0, -4.0, -8.0, -12.0], [0.9, 0.0, 1.0, 1.2, 1.5, 1.4]),
            Genre::Techno =>      p(30.0, 2.0, -2.0, 0.0, 1.0, -15.0, 3.0, 5.0, 80.0, 4.0, 5.0, 1.1, 160.0, -9.0, [0.0, -1.0, -3.0, -5.0, -8.0, -10.0], [0.9, 0.0, 1.1, 1.2, 1.6, 1.6]),
            Genre::Synthwave =>   p(28.0, 1.0, -1.0, 1.5, 2.0, -18.0, 2.0, 20.0, 150.0, 3.0, 1.0, 1.25, 130.0, -10.0, [0.0, -2.0, -1.0, -4.0, -7.0, -12.0], [1.0, 0.0, 1.1, 1.3, 1.6, 1.5]),
            Genre::FrenchHouse => p(30.0, 1.5, -1.0, 1.0, 1.0, -14.0, 4.0, 3.0, 60.0, 5.0, 3.0, 1.1, 140.0, -9.0, [0.0, -1.5, -2.0, -3.0, -8.0, -12.0], [0.9, 0.0, 1.0, 1.2, 1.4, 1.3]),
            Genre::Pop =>         p(25.0, 1.0, -1.0, 1.5, 2.0, -18.0, 2.0, 15.0, 150.0, 2.0, 0.0, 1.15, 120.0, -10.0, [0.0, -2.0, 1.0, -5.0, -9.0, -14.0], [0.9, 0.0, 0.5, 1.1, 1.4, 1.3]),
            Genre::Rock =>        p(30.0, 0.5, -1.5, 2.0, 1.0, -16.0, 3.0, 20.0, 120.0, 4.0, 2.0, 1.1, 120.0, -10.0, [0.0, -2.0, 0.0, -4.0, -8.0, -14.0], [0.9, 0.0, 0.7, 1.2, 1.3, 1.2]),
            Genre::Cinematic =>   p(22.0, 1.0, -0.5, 0.0, 1.0, -24.0, 1.5, 50.0, 400.0, 1.0, 0.0, 1.3, 100.0, -16.0, [-2.0, -3.0, 0.0, -3.0, -5.0, -8.0], [0.9, 0.2, 1.0, 1.3, 1.7, 1.6]),
            Genre::DrumAndBass => p(30.0, 2.0, -2.0, 1.0, 1.5, -14.0, 3.0, 3.0, 60.0, 4.0, 6.0, 1.1, 150.0, -8.0, [0.0, -1.0, -3.0, -5.0, -9.0, -12.0], [0.9, 0.0, 1.0, 1.2, 1.6, 1.5]),
            Genre::BassFirst =>   p(25.0, 2.5, -1.0, -1.5, -2.0, -18.0, 2.5, 15.0, 150.0, 3.0, 2.0, 1.1, 140.0, -14.0, [0.0, 1.0, -2.0, -6.0, -9.0, -15.0], [0.9, 0.0, 0.9, 1.1, 1.4, 1.3]),
        };
        preset
    }

    pub fn from_index(i: u32) -> Genre {
        *Self::ALL.get(i as usize).unwrap_or(&Genre::Neutral)
    }

    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|g| *g == self).unwrap_or(0) as u32
    }
}

impl Preset {
    fn slot(role: Role) -> Option<usize> {
        match role {
            Role::Drums => Some(0),
            Role::Bass => Some(1),
            Role::Lead => Some(2),
            Role::Keys => Some(3),
            Role::Pad => Some(4),
            Role::Texture => Some(5),
            Role::Auto => None,
        }
    }

    /// Static level for a role relative to the mix (Auto sits with keys).
    pub fn role_db(&self, role: Role) -> f64 {
        Self::slot(role).map(|i| self.role_db[i]).unwrap_or(self.role_db[3])
    }

    pub fn role_width(&self, role: Role) -> f64 {
        Self::slot(role).map(|i| self.role_width[i]).unwrap_or(1.0)
    }

    /// Below this the role's side signal is removed: bass and drums keep their low end centred.
    pub fn role_mono_below_hz(&self, role: Role) -> f64 {
        match role {
            Role::Bass => 250.0,
            Role::Drums => self.mono_below_hz,
            _ => 100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_matches_the_original_balance_templates() {
        let p = Genre::Neutral.preset();
        assert_eq!(p.role_db, [0.0, -2.0, -1.0, -5.0, -9.0, -15.0]);
        assert!(p.role_width.iter().all(|w| *w == 1.0));
        assert_eq!(p.grit_db, 0.0);
    }

    #[test]
    fn trip_hop_is_dark_low_gritty_and_wide_over_mono_bass() {
        let p = Genre::TripHop.preset();
        assert!(p.low_db > 0.0 && p.air_db < 0.0, "low up, top rolled off");
        assert!(p.grit_db >= 6.0, "gritty bass");
        assert!(p.glue_release_ms >= 250.0, "slow glue, no pumping");
        assert_eq!(p.role_width(Role::Bass), 0.0);
        assert!(p.role_width(Role::Pad) > 1.3 && p.role_width(Role::Drums) < 1.0);
    }

    #[test]
    fn bass_first_puts_the_bass_over_everything_and_tames_the_top() {
        let p = Genre::BassFirst.preset();
        for r in [Role::Drums, Role::Lead, Role::Keys, Role::Pad, Role::Texture] {
            assert!(p.role_db(Role::Bass) > p.role_db(r), "bass over {:?}", r);
        }
        assert!(p.role_db(Role::Lead) <= -2.0 && p.role_db(Role::Keys) <= -6.0, "leads and keys sit back");
        assert!(p.low_db >= 2.0 && p.presence_db < 0.0 && p.air_db < 0.0, "low up, harsh top down");
        assert_eq!(p.role_width(Role::Bass), 0.0);
    }

    #[test]
    fn every_genre_has_sane_values() {
        for g in Genre::ALL {
            let p = g.preset();
            assert!((10.0..=150.0).contains(&p.low_cut_hz), "{}", g.name());
            for v in [p.low_db, p.body_db, p.presence_db, p.air_db] {
                assert!((-6.0..=6.0).contains(&v), "{}", g.name());
            }
            assert!((1.0..=8.0).contains(&p.glue_ratio) && (-24.0..=-6.0).contains(&p.target_lufs));
            assert!(p.role_width.iter().all(|w| (0.0..=2.0).contains(w)));
            assert_eq!(Genre::from_index(g.index()), g);
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::dsp::Settings;
    use crate::with_genre;

    #[test]
    fn neutral_keeps_the_manual_chain_and_a_genre_is_active_with_trims() {
        let mut manual = Settings::default();
        manual.tone.air_db = 1.0;
        manual.warmth.grit_db = 2.0;
        manual.warmth.on = false;
        assert_eq!(with_genre(Genre::Neutral, manual), manual);
        let s = with_genre(Genre::TripHop, manual);
        let p = Genre::TripHop.preset();
        assert!(s.tone.on && s.glue.on && s.warmth.on && s.width.on, "every layer active");
        assert_eq!(s.tone.air_db, p.air_db + 1.0, "air trim adds");
        assert_eq!(s.warmth.grit_db, p.grit_db + 2.0, "grit trim adds");
        assert_eq!(s.glue.release_ms, p.glue_release_ms);
        assert_eq!(s.limiter, manual.limiter, "limiter stays the user's");
    }
}
