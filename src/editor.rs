//! Layers on the left (reorderable cards with on/off and opacity), meters on the right.

use crate::dsp::{pack_order, unpack_order, LayerKind};
use crate::genre::Genre;
use crate::{Meters, StrataParams};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, RichText};
use nih_plug_egui::{create_egui_editor, widgets::ParamSlider};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const ACCENT: Color32 = Color32::from_rgb(232, 160, 64);
const DIM: Color32 = Color32::from_rgb(150, 150, 160);
const BG: Color32 = Color32::from_rgb(24, 25, 29);
const CARD: Color32 = Color32::from_rgb(33, 35, 41);

pub fn create(params: Arc<StrataParams>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        (),
        |ctx, _| {
            ctx.set_visuals(egui::Visuals::dark());
        },
        move |ctx, setter, _| {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(BG).inner_margin(14.0))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("STRATA").size(20.0).strong().color(ACCENT));
                        ui.label(RichText::new("engine · mastering layers").color(DIM));
                    });
                    ui.add_space(8.0);
                    ui.columns(2, |cols| {
                        egui::ScrollArea::vertical().id_salt("layers").show(&mut cols[0], |ui| {
                            layers(ui, &params, setter);
                        });
                        egui::ScrollArea::vertical().id_salt("right").show(&mut cols[1], |ui| {
                            genre_panel(ui, &params, setter);
                            meter_panel(ui, &params, &meters, setter);
                            balance_panel(ui, &params, &meters, setter);
                        });
                    });
                });
            ctx.request_repaint();
        },
    )
}

fn toggle(ui: &mut egui::Ui, param: &BoolParam, setter: &ParamSetter) {
    let mut on = param.value();
    if ui.checkbox(&mut on, "").changed() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, on);
        setter.end_set_parameter(param);
    }
}

fn row(ui: &mut egui::Ui, label: &str, param: &FloatParam, setter: &ParamSetter) {
    ui.label(RichText::new(label).color(DIM));
    ui.add(ParamSlider::for_param(param, setter).with_width(190.0));
    ui.end_row();
}

fn row_toggle(ui: &mut egui::Ui, label: &str, param: &BoolParam, setter: &ParamSetter, hint: &str) {
    ui.label(RichText::new(label).color(DIM));
    ui.horizontal(|ui| {
        toggle(ui, param, setter);
        ui.label(RichText::new(hint).color(DIM).size(11.0));
    });
    ui.end_row();
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default().fill(CARD).corner_radius(6.0).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    ui.add_space(6.0);
}

fn layers(ui: &mut egui::Ui, p: &StrataParams, setter: &ParamSetter) {
    let mut order = unpack_order(p.layer_order.load(Ordering::Relaxed));
    let mut swap: Option<(usize, usize)> = None;

    for (slot, kind) in order.iter().enumerate() {
        let (on, opacity) = match kind {
            LayerKind::Tone => (&p.tone_on, &p.tone_opacity),
            LayerKind::Glue => (&p.glue_on, &p.glue_opacity),
            LayerKind::Warmth => (&p.warmth_on, &p.warmth_opacity),
            LayerKind::Width => (&p.width_on, &p.width_opacity),
        };
        card(ui, |ui| {
            ui.horizontal(|ui| {
                toggle(ui, on, setter);
                ui.label(RichText::new(format!("{}  {}", slot + 1, kind.name())).size(15.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(slot < 3, egui::Button::new("Down")).on_hover_text("Move this layer down").clicked() {
                        swap = Some((slot, slot + 1));
                    }
                    if ui.add_enabled(slot > 0, egui::Button::new("Up")).on_hover_text("Move this layer up").clicked() {
                        swap = Some((slot, slot - 1));
                    }
                });
            });
            egui::Grid::new(format!("grid-{}", kind.name())).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                row(ui, "Opacity", opacity, setter);
                match kind {
                    LayerKind::Tone => {
                        row(ui, "Low cut", &p.tone_low_cut, setter);
                        row(ui, "Low 110 Hz", &p.tone_low, setter);
                        row(ui, "Body 400 Hz", &p.tone_mid, setter);
                        row(ui, "Presence 3 kHz", &p.tone_presence, setter);
                        row(ui, "Air 10 kHz", &p.tone_air, setter);
                    }
                    LayerKind::Glue => {
                        row(ui, "Threshold", &p.glue_threshold, setter);
                        row(ui, "Ratio", &p.glue_ratio, setter);
                        row(ui, "Attack", &p.glue_attack, setter);
                        row(ui, "Release", &p.glue_release, setter);
                        row(ui, "Makeup", &p.glue_makeup, setter);
                        row(ui, "Sidechain HPF", &p.glue_sc_hpf, setter);
                    }
                    LayerKind::Warmth => {
                        row(ui, "Drive", &p.warmth_drive, setter);
                        row(ui, "Bass grit", &p.warmth_grit, setter);
                    }
                    LayerKind::Width => {
                        row(ui, "Width", &p.width_amount, setter);
                        row(ui, "Mono below", &p.width_mono_below, setter);
                        row_toggle(ui, "Linear phase", &p.width_linear, setter, "no phase shift, adds ~43 ms latency");
                    }
                }
            });
        });
    }

    card(ui, |ui| {
        ui.horizontal(|ui| {
            toggle(ui, &p.limiter_on, setter);
            ui.label(RichText::new("OUT  Limiter").size(15.0).strong());
            ui.label(RichText::new("(always last)").color(DIM));
        });
        egui::Grid::new("grid-limiter").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            row(ui, "Input gain", &p.limiter_gain, setter);
            row(ui, "Ceiling", &p.limiter_ceiling, setter);
            row(ui, "Release", &p.limiter_release, setter);
            row_toggle(ui, "Auto release", &p.limiter_auto_release, setter, "fast after peaks, slow when dense");
            row_toggle(ui, "Clipper", &p.clip_on, setter, "before the limiter; dirty genres only");
            row(ui, "Clip above ceiling", &p.clip_db, setter);
        });
    });

    if let Some((a, b)) = swap {
        order.swap(a, b);
        p.layer_order.store(pack_order(order), Ordering::Relaxed);
    }
}

fn set_genre(p: &StrataParams, setter: &ParamSetter, g: Genre) {
    setter.begin_set_parameter(&p.genre);
    setter.set_parameter(&p.genre, g);
    setter.end_set_parameter(&p.genre);
}

/// The genre the master leans towards, and what it is doing right now.
fn genre_panel(ui: &mut egui::Ui, p: &StrataParams, setter: &ParamSetter) {
    let g = p.genre.value();
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("GENRE").strong().color(ACCENT));
            egui::ComboBox::from_id_salt("genre").selected_text(g.name()).width(150.0).show_ui(ui, |ui| {
                for x in Genre::ALL {
                    if ui.selectable_label(g == x, x.name()).clicked() {
                        set_genre(p, setter, x);
                    }
                }
            });
            let i = g.index() as usize;
            if ui.button("Prev").clicked() {
                set_genre(p, setter, Genre::ALL[(i + Genre::ALL.len() - 1) % Genre::ALL.len()]);
            }
            if ui.button("Next").clicked() {
                set_genre(p, setter, Genre::ALL[(i + 1) % Genre::ALL.len()]);
            }
        });
        ui.label(RichText::new(g.character()).color(DIM));
        if g != Genre::Neutral {
            let s = p.settings();
            let pr = g.preset();
            ui.label(
                RichText::new(format!(
                    "ACTIVE  tone low {:+.1} body {:+.1} presence {:+.1} air {:+.1} dB, cut {:.0} Hz\n        glue {:.0} dB {:.1}:1 {:.0}/{:.0} ms   warmth {:.0} dB   bass grit {:.0} dB\n        width {:.0}% mono below {:.0} Hz   target {:.1} LUFS\n        roles: bass width {:.0}%, pads {:.0}%, drums {:.0}%",
                    s.tone.low_db, s.tone.mid_db, s.tone.presence_db, s.tone.air_db, s.tone.low_cut_hz,
                    s.glue.threshold_db, s.glue.ratio, s.glue.attack_ms, s.glue.release_ms, s.warmth.drive_db, s.warmth.grit_db,
                    s.width.width * 100.0, s.width.mono_below_hz, pr.target_lufs,
                    pr.role_width[1] * 100.0, pr.role_width[4] * 100.0, pr.role_width[0] * 100.0
                ))
                .monospace()
                .size(11.0),
            );
            ui.label(RichText::new("Tone, Glue, Warmth and Width follow the genre; the Tone dB sliders, Bass grit and Width are trims on top.").color(DIM).size(11.0));
        }
    });
}

/// Readings below -70 (the BS.1770 absolute gate) show as -inf, so silence never prints -539.6.
fn fmt(v: f32, decimals: usize) -> String {
    if v.is_finite() && v > -70.0 { format!("{v:+.decimals$}") } else { "  -inf".to_string() }
}

fn big(ui: &mut egui::Ui, label: &str, value: String, unit: &str, color: Color32) {
    ui.label(RichText::new(label).color(DIM));
    ui.label(RichText::new(value).monospace().size(22.0).color(color));
    ui.label(RichText::new(unit).color(DIM));
    ui.end_row();
}

fn meter_panel(ui: &mut egui::Ui, p: &StrataParams, m: &Meters, setter: &ParamSetter) {
    let ld = |a: &AtomicF32| a.load(Ordering::Relaxed);
    let out_i = ld(&m.out_integrated);
    let out_tp = ld(&m.out_true_peak);
    let ceiling = p.limiter_ceiling.value();
    let tp_color = if out_tp > ceiling + 0.05 { Color32::from_rgb(235, 90, 80) } else { Color32::WHITE };

    card(ui, |ui| {
        ui.label(RichText::new("OUTPUT").strong().color(ACCENT));
        egui::Grid::new("out-meters").num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
            big(ui, "Integrated", fmt(out_i, 1), "LUFS", Color32::WHITE);
            big(ui, "Short-term", fmt(ld(&m.out_short), 1), "LUFS", Color32::WHITE);
            big(ui, "Momentary", fmt(ld(&m.out_momentary), 1), "LUFS", Color32::WHITE);
            big(ui, "True peak max", fmt(out_tp, 2), "dBTP", tp_color);
            big(ui, "Correlation", fmt(ld(&m.correlation), 2), "", Color32::WHITE);
            big(ui, "Glue GR", format!("{:5.1}", ld(&m.glue_gr)), "dB", Color32::WHITE);
            big(ui, "Limiter GR", format!("{:5.1}", ld(&m.limiter_gr)), "dB", Color32::WHITE);
            big(ui, "Limiter GR max", format!("{:5.1}", ld(&m.limiter_gr_max)), "dB", Color32::WHITE);
        });
        let gr_max = ld(&m.limiter_gr_max);
        if gr_max > 6.0 {
            ui.label(
                RichText::new(format!(
                    "Limiter took {gr_max:.1} dB on the loudest peaks. Past about 6 dB limiting starts to sound bad: \
                     lower the input gain and get the loudness from Glue, or try the clipper on a dirty song."
                ))
                .color(Color32::from_rgb(235, 90, 80)),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            toggle(ui, &p.compare, setter);
            ui.label(RichText::new("Compare 1:1").strong());
            ui.label(
                RichText::new(format!("hear the input at the output's loudness ({:+.1} dB)", ld(&m.compare_db)))
                    .color(DIM),
            );
        });
    });

    card(ui, |ui| {
        ui.label(RichText::new("INPUT").strong().color(ACCENT));
        egui::Grid::new("in-meters").num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
            big(ui, "Integrated", fmt(ld(&m.in_integrated), 1), "LUFS", DIM);
            big(ui, "Short-term", fmt(ld(&m.in_short), 1), "LUFS", DIM);
            big(ui, "True peak max", fmt(ld(&m.in_true_peak), 2), "dBTP", DIM);
        });
    });

    card(ui, |ui| {
        ui.label(RichText::new("TARGET").strong().color(ACCENT));
        if p.genre.value() == Genre::Neutral {
            egui::Grid::new("target").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                row(ui, "Target", &p.target_lufs, setter);
            });
        } else {
            ui.label(RichText::new(format!("Target {:.1} LUFS ({})", p.target(), p.genre.value().name())).monospace());
        }
        let target = p.target() as f32;
        if out_i.is_finite() && out_i > -70.0 {
            let delta = target - out_i;
            ui.label(RichText::new(format!("Gain to target: {delta:+.1} dB")).monospace().size(18.0));
            let new_gain = (p.limiter_gain.value() + delta).clamp(0.0, 24.0);
            if ui.button(format!("Apply to limiter input gain ({new_gain:.1} dB)")).clicked() {
                setter.begin_set_parameter(&p.limiter_gain);
                setter.set_parameter(&p.limiter_gain, new_gain);
                setter.end_set_parameter(&p.limiter_gain);
                m.reset.store(true, Ordering::Relaxed);
            }
            ui.label(RichText::new("Play the whole song first; applying resets the meters.").color(DIM));
        } else {
            ui.label(RichText::new("Play the song to measure it.").color(DIM));
        }
        ui.add_space(4.0);
        if ui.button("Reset meters").clicked() {
            m.reset.store(true, Ordering::Relaxed);
        }
        let es: Vec<f32> = m.engine_stats.iter().map(|a| a.load(Ordering::Relaxed)).collect();
        ui.label(
            RichText::new(format!(
                "engine pass: {:.1} s  play {:.0}%  in {:.1} dB  pos {:.1}-{:.1}  inits {:.0}  resets {:.0}",
                es[0], es[1], es[2], es[3], es[4], es[5], es[6]
            ))
            .monospace()
            .size(11.0),
        );
        if ui.button("Measure latest export").on_hover_text("Newest .wav in the FL Studio Projects folder").clicked() {
            *m.file_report.lock().unwrap() = crate::measure_latest_export();
        }
        let fr = m.file_report.lock().unwrap().clone();
        if !fr.is_empty() {
            ui.label(RichText::new(fr).monospace().size(11.0));
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Event log (newest last)").color(DIM));
        let lines: Vec<String> = m.log.lock().map(|l| l.iter().rev().take(24).rev().cloned().collect()).unwrap_or_default();
        ui.label(RichText::new(lines.join("\n")).monospace().size(11.0));
    });
}

fn balance_panel(ui: &mut egui::Ui, p: &StrataParams, m: &Meters, setter: &ParamSetter) {
    use crate::cylinder::{self, LEARNING, LEARN_GEN};
    card(ui, |ui| {
        ui.horizontal(|ui| {
            toggle(ui, &p.balance_on, setter);
            ui.label(RichText::new("BALANCE").strong().color(ACCENT));
            ui.label(RichText::new("engine + cylinders").color(DIM));
        });
        let cyls = cylinder::cylinders();
        let learning = LEARNING.load(Ordering::Relaxed);
        ui.horizontal(|ui| {
            if !learning {
                if ui.button("Start learning").on_hover_text("Clears what the Cylinders learned; then play the whole song").clicked() {
                    LEARN_GEN.fetch_add(1, Ordering::Relaxed);
                    LEARNING.store(true, Ordering::Relaxed);
                }
            } else if ui.button("Stop learning + Balance").clicked() {
                LEARNING.store(false, Ordering::Relaxed);
                *m.plan_summary.lock().unwrap() = crate::balance_now();
            }
            if ui.add_enabled(!learning, egui::Button::new("Balance again")).clicked() {
                *m.plan_summary.lock().unwrap() = crate::balance_now();
            }
            if ui.button("Clear plan").clicked() {
                cyls.iter().for_each(|c| c.clear_plan());
                *m.plan_summary.lock().unwrap() = "Plan cleared.".into();
            }
        });
        if learning {
            ui.label(RichText::new("LEARNING: play the whole song from the start").color(ACCENT).strong());
        }
        egui::Grid::new("bal").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            row(ui, "Static balance", &p.balance_static, setter);
            row(ui, "Moments", &p.balance_moments, setter);
            row(ui, "Width plan", &p.balance_width, setter);
        });
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{} cylinders", cyls.len())).color(DIM));
        let mut sorted = cyls.clone();
        sorted.sort_by_key(|c| c.id);
        for c in &sorted {
            let label = c.label();
            let name = if label.is_empty() { format!("#{}", c.id) } else { format!("#{} {}", c.id, label) };
            ui.label(
                RichText::new(format!(
                    "{:<18} {:<8} learned {:>4}  gain {:+5.1} dB  width {:>3.0}%",
                    name,
                    crate::orchestra::Role::from_index(c.role.load(Ordering::Relaxed)).name(),
                    c.learned_beats.load(Ordering::Relaxed),
                    c.live_gain_db.load(Ordering::Relaxed),
                    c.width.load(Ordering::Relaxed) * 100.0
                ))
                .monospace(),
            );
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Pass statistics (since Reset meters)").color(DIM));
        let mut head = format!("{:<4}", "#");
        for n in crate::cylinder::STAT_NAMES {
            head += &format!("{n:>8}");
        }
        ui.label(RichText::new(head).monospace().size(11.0));
        for c in &sorted {
            let mut line = format!("{:<4}", c.id);
            for (i, a) in c.stats.iter().enumerate() {
                let v = a.load(Ordering::Relaxed);
                line += &if i == 8 { format!("{:>8.0}", v) } else { format!("{:>8.1}", v) };
            }
            ui.label(RichText::new(line).monospace().size(11.0));
        }
        let summary = m.plan_summary.lock().unwrap().clone();
        if !summary.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(summary).monospace().size(11.0));
        }
    });
}
