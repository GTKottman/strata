//! Layers on the left (reorderable cards with on/off and opacity), meters on the right.

use crate::dsp::{pack_order, unpack_order, LayerKind};
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
                        ui.label(RichText::new("mastering layers").color(DIM));
                    });
                    ui.add_space(8.0);
                    ui.columns(2, |cols| {
                        egui::ScrollArea::vertical().id_salt("layers").show(&mut cols[0], |ui| {
                            layers(ui, &params, setter);
                        });
                        meter_panel(&mut cols[1], &params, &meters, setter);
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
                    if ui.add_enabled(slot < 3, egui::Button::new("▼")).on_hover_text("Move down").clicked() {
                        swap = Some((slot, slot + 1));
                    }
                    if ui.add_enabled(slot > 0, egui::Button::new("▲")).on_hover_text("Move up").clicked() {
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
                    }
                    LayerKind::Warmth => row(ui, "Drive", &p.warmth_drive, setter),
                    LayerKind::Width => {
                        row(ui, "Width", &p.width_amount, setter);
                        row(ui, "Mono below", &p.width_mono_below, setter);
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
        });
    });

    if let Some((a, b)) = swap {
        order.swap(a, b);
        p.layer_order.store(pack_order(order), Ordering::Relaxed);
    }
}

fn fmt(v: f32, decimals: usize) -> String {
    if v.is_finite() { format!("{v:+.decimals$}") } else { "  -inf".to_string() }
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
        egui::Grid::new("target").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            row(ui, "Target", &p.target_lufs, setter);
        });
        let target = p.target_lufs.value();
        if out_i.is_finite() {
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
    });
}
