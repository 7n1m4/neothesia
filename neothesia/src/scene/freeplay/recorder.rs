use std::{
    collections::HashSet,
    fmt,
    path::PathBuf,
    time::{Duration, Instant},
};

use midi_file::midly::{
    Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
};
use midi_file::midly::num::{u4, u7};
use neothesia_core::render::{NoteLabels, WaterfallRenderer};


use crate::{
    NeothesiaEvent,
    context::Context,
    icons,
    scene::{
        freeplay::{FreeplayScene, FreeplayPopup, on_async},
        playing_scene::{Keyboard, midi_player::MidiPlayer},
    },
    song::Song,
};

const TICKS_PER_BEAT: u16 = 480;
const TEMPO_MICROS_PER_BEAT: u32 = 500_000;
const TICKS_PER_SECOND: f64 = TICKS_PER_BEAT as f64 * 1_000_000.0 / TEMPO_MICROS_PER_BEAT as f64;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RecorderError {
    #[error("No note events recorded")]
    NoNotesFound,
    #[error("Failed to write MIDI file")]
    Write,
    #[error("{0}")]
    MidiFileParse(String),
}

#[derive(Default, Debug)]
pub enum RecorderStatus {
    #[default]
    Idle,
    RecordingFinished(Duration),
    Saved(PathBuf),
    Error(RecorderError),
}

impl fmt::Display for RecorderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => {}
            Self::RecordingFinished(duration) => {
                write!(f, "Recorded {:.1}s", duration.as_secs_f32())?;
            }
            Self::Error(err) => {
                write!(f, "{err}")?;
            }
            Self::Saved(path) => {
                write!(f, "Saved recording to {}", path.display())?;
            }
        }

        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct RecordedMidiEvent {
    timestamp: Duration,
    channel: u8,
    message: MidiMessage,
}

pub struct RecordingInProgressState {
    started_at: Instant,
    events: Vec<RecordedMidiEvent>,
    active_notes: HashSet<(u8, u8)>,
}

impl RecordingInProgressState {
    fn finish_active_notes(&mut self, timestamp: Duration) {
        let mut active_notes: Vec<_> = self.active_notes.drain().collect();

        // TODO: What's the point of this sort?
        active_notes.sort_unstable();

        for (channel, key) in active_notes {
            self.events.push(RecordedMidiEvent {
                timestamp,
                channel,
                message: MidiMessage::NoteOff {
                    key: key.into(),
                    vel: 0.into(),
                },
            });
        }
    }
}

pub struct RecordedTake {
    duration: Duration,
    smf: Smf<'static>,
}

#[derive(Default)]
enum RecorderState {
    #[default]
    Idle,
    Recording(RecordingInProgressState),
    Recorded(RecordedTake),
}

#[derive(Default)]
pub struct FreeplayRecorder {
    state: RecorderState,
}

pub struct Preview {
    player: MidiPlayer,
    waterfall: WaterfallRenderer,
    note_labels: Option<NoteLabels>,
}

impl Preview {
    fn new(keyboard: &Keyboard, song: Song, ctx: &Context) -> Self {
        let hidden_tracks: Vec<usize> = song
            .config
            .tracks
            .iter()
            .filter(|track| !track.visible)
            .map(|track| track.track_id)
            .collect();

        let mut waterfall = WaterfallRenderer::new(
            &ctx.gpu,
            &song.file.tracks,
            &hidden_tracks,
            &ctx.config,
            &ctx.transform,
            keyboard.layout().clone(),
        );

        let note_labels = ctx.config.note_labels().then_some(NoteLabels::new(
            *keyboard.pos(),
            waterfall.notes(),
            ctx.text_renderer_factory.new_renderer(),
        ));

        let mut player = MidiPlayer::new_with_lead_in(
            ctx.output_manager.connection().clone(),
            song,
            keyboard.layout().range.clone(),
            ctx.config.separate_channels(),
            Duration::ZERO,
        );
        player.pause();
        waterfall.update(player.time_without_lead_in() + ctx.config.animation_offset());

        Self {
            player,
            waterfall,
            note_labels,
        }
    }

    pub fn resize(&mut self, keyboard: &Keyboard, ctx: &mut Context) {
        self.waterfall
            .resize(&ctx.config, keyboard.layout().clone());

        if let Some(note_labels) = self.note_labels.as_mut() {
            note_labels.set_pos(*keyboard.pos());
        }
    }

    pub fn update(&mut self, keyboard: &mut Keyboard, ctx: &mut Context, delta: Duration) {
        let midi_events = self.player.update(delta);
        keyboard.file_midi_events(&ctx.config, &midi_events);

        if self.player.is_finished() && !self.player.is_paused() {
            self.player.pause();
        }

        let time = self.player.time_without_lead_in() + ctx.config.animation_offset();

        self.waterfall.update(time);

        if let Some(note_labels) = self.note_labels.as_mut() {
            note_labels.update(
                ctx.window_state.physical_size,
                ctx.window_state.scale_factor as f32,
                keyboard.renderer(),
                ctx.config.animation_speed(),
                time,
            );
        }
    }

    pub fn render<'pass>(&'pass mut self, rpass: &mut wgpu_jumpstart::RenderPass<'pass>) {
        self.waterfall.render(rpass);
        if let Some(note_labels) = self.note_labels.as_mut() {
            note_labels.render(rpass);
        }
    }
}

impl FreeplayRecorder {
    fn is_recording(&self) -> bool {
        matches!(self.state, RecorderState::Recording(_))
    }

    fn start(&mut self) {
        self.state = RecorderState::Recording(RecordingInProgressState {
            started_at: Instant::now(),
            events: Vec::new(),
            active_notes: HashSet::new(),
        });
    }

    fn stop(&mut self) -> Result<&Smf<'static>, RecorderError> {
        let state = std::mem::take(&mut self.state);
        let RecorderState::Recording(mut in_progress) = state else {
            return Err(RecorderError::NoNotesFound);
        };

        let stop_time = in_progress.started_at.elapsed();
        in_progress.finish_active_notes(stop_time);

        let smf = to_smf(&in_progress.events)?;

        self.state = RecorderState::Recorded(RecordedTake {
            duration: stop_time,
            smf,
        });

        self.as_smf()
    }

    fn duration(&self) -> Duration {
        match &self.state {
            RecorderState::Idle => Duration::ZERO,
            RecorderState::Recording(state) => state.started_at.elapsed(),
            RecorderState::Recorded(recorded_take) => recorded_take.duration,
        }
    }

    pub fn push_event(&mut self, channel: u8, message: MidiMessage) {
        let RecorderState::Recording(in_progress) = &mut self.state else {
            return;
        };

        let timestamp = in_progress.started_at.elapsed();
        in_progress.events.push(RecordedMidiEvent {
            timestamp,
            channel,
            message,
        });

        match message {
            MidiMessage::NoteOn { key, .. } => {
                in_progress.active_notes.insert((channel, key.as_int()));
            }
            MidiMessage::NoteOff { key, .. } => {
                in_progress.active_notes.remove(&(channel, key.as_int()));
            }
            _ => {}
        }
    }

    fn as_smf(&self) -> Result<&Smf<'static>, RecorderError> {
        let RecorderState::Recorded(recorded_take) = &self.state else {
            return Err(RecorderError::NoNotesFound);
        };

        Ok(&recorded_take.smf)
    }
}

fn duration_to_ticks(duration: Duration) -> u32 {
    (duration.as_secs_f64() * TICKS_PER_SECOND).round() as u32
}

pub fn update_preview_ui(scene: &mut FreeplayScene, ctx: &mut Context) {
    let top_bar_height = 30.0;

    let width = ctx.window_state.logical_size.width;
    let height = ctx.window_state.logical_size.height;
    let available = scene.preview.is_some();

    let is_paused = scene
        .preview
        .as_ref()
        .map(|s| s.player.is_paused())
        .unwrap_or(true);

    let status_label = if scene.recorder.is_recording() {
        format!("Recording {:.1}s", scene.recorder.duration().as_secs_f32())
    } else {
        scene.recorder_status.to_string()
    };

    enum Msg {
        TogglePlay,
        Seek,
        GoBack,
        Record,
        Save,
        OpenInstrumentSelector,
        None,
    }

    let mut msg = Msg::None;

    nuon::translate().build(&mut scene.nuon, |ui| {
        nuon::quad()
            .size(width, top_bar_height)
            .color([37, 35, 42])
            .build(ui);

        nuon::translate().build(ui, |ui| {
            if nuon::button()
                .size(30.0, 30.0)
                .border_radius([5.0; 4])
                .icon(icons::left_arrow_icon())
                .build(ui)
            {
                msg = Msg::GoBack;
            }
            nuon::translate().x(30.0).add_to_current(ui);
        });

        nuon::label()
            .size(width, 30.0)
            .text(&status_label)
            .text_justify(nuon::TextJustify::Center)
            .build(ui);

        nuon::translate().x(width).build(ui, |ui| {
            nuon::translate().x(-30.0).add_to_current(ui);

            if nuon::button()
                .size(30.0, 30.0)
                .border_radius([5.0; 4])
                .icon(if is_paused {
                    icons::play_icon()
                } else {
                    icons::pause_icon()
                })
                .font_color(if available {
                    [255, 255, 255, 255]
                } else {
                    [255, 255, 255, 100]
                })
                .build(ui)
                && available
            {
                msg = Msg::TogglePlay;
            }

            nuon::translate().x(-30.0).add_to_current(ui);

            if nuon::button()
                .size(30.0, 30.0)
                .border_radius([5.0; 4])
                .icon(icons::save_icon())
                .font_color(if available {
                    [255, 255, 255, 255]
                } else {
                    [255, 255, 255, 100]
                })
                .build(ui)
                && available
            {
                msg = Msg::Save;
            }

            nuon::translate().x(-30.0).add_to_current(ui);

            if nuon::button()
                .size(30.0, 30.0)
                .border_radius([5.0; 4])
                .icon(if scene.recorder.is_recording() {
                    icons::record_stop_icon()
                } else {
                    icons::record_icon()
                })
                .color(if scene.recorder.is_recording() {
                    [208, 18, 0, 255]
                } else {
                    [0, 0, 0, 0]
                })
                .hover_color(if scene.recorder.is_recording() {
                    [165, 47, 47]
                } else {
                    [97, 97, 97]
                })
                .preseed_color(if scene.recorder.is_recording() {
                    [145, 37, 37]
                } else {
                    [87, 87, 87]
                })
                .build(ui)
            {
                msg = Msg::Record;
            }

            nuon::translate().x(-30.0).add_to_current(ui);

            if nuon::button()
                .size(30.0, 30.0)
                .border_radius([5.0; 4])
                .icon(icons::music_icon())
                .build(ui)
            {
                msg = Msg::OpenInstrumentSelector;
            }

            if scene.popup == FreeplayPopup::InstrumentSelector {
                // Fullscreen overlay
                let win_w = width;
                let win_h = height;
                let margin = 40.0;
                let panel_w = win_w - margin * 2.0;
                let panel_h = win_h - margin * 2.0;
                let panel_x = margin;
                let panel_y = margin;

                nuon::layer().overlay(true).build(ui, |ui| {
                    // Dark backdrop
                    nuon::quad()
                        .size(win_w, win_h)
                        .color([0, 0, 0, 200])
                        .build(ui);

                    // Panel background
                    nuon::translate()
                        .x(panel_x)
                        .y(panel_y)
                        .add_to_current(ui);

                    nuon::quad()
                        .size(panel_w, panel_h)
                        .color([27, 25, 32])
                        .border_radius([12.0; 4])
                        .build(ui);

                    // Title bar
                    nuon::label()
                        .y(16.0)
                        .x(24.0)
                        .size(panel_w - 48.0, 36.0)
                        .text("Select Instrument".to_string())
                        .font_size(24.0)
                        .color([255, 255, 255])
                        .text_justify(nuon::TextJustify::Left)
                        .build(ui);

                    // Close button (X)
                    nuon::button()
                        .y(12.0)
                        .x(panel_w - 48.0)
                        .size(36.0, 36.0)
                        .border_radius([6.0; 4])
                        .color([50, 48, 55])
                        .hover_color([80, 78, 85])
                        .label("×".to_string())
                        .build(ui)
                        .then(|| {
                            scene.popup.close();
                        });
                    let content_y = 64.0;
                    let _content_h = panel_h - content_y - 16.0;
                    let col_count = 4;
                    let col_gap = 12.0;
                    let col_w = (panel_w - 48.0 - (col_gap * (col_count as f32 - 1.0))) / col_count as f32;

                    let groups: Vec<(&str, fn() -> &'static str, Vec<(u8, &str)>)> = vec![
                        ("Piano", icons::piano_icon, (0..=7).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Chromatic", icons::chromatic_icon, (8..=15).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Organ", icons::organ_icon, (16..=23).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Guitar", icons::guitar_icon, (24..=31).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Bass", icons::bass_icon, (32..=39).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Strings", icons::strings_icon, (40..=47).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Ensemble", icons::strings_icon, (48..=55).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Brass", icons::brass_icon, (56..=63).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Reed", icons::woodwind_icon, (64..=71).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Pipe", icons::woodwind_icon, (72..=79).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Synth Lead", icons::synth_lead_icon, (80..=87).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Synth Pad", icons::synth_pad_icon, (88..=95).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Synth Effects", icons::effects_icon, (96..=103).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Ethnic", icons::ethnic_icon, (104..=111).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Percussive", icons::percussion_icon, (112..=119).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                        ("Sound Effects", icons::effects_icon, (120..=127).map(|i| (i, midi_file::INSTRUMENT_NAMES[i as usize])).collect()),
                    ];

                    for (col_idx, group_chunk) in groups.chunks(3).enumerate() {
                        let col_x = 24.0 + col_idx as f32 * (col_w + col_gap);

                        let mut gy = content_y;

                        for (group_name, icon_fn, instruments) in group_chunk {
                            // Group header background
                            nuon::quad()
                                .x(col_x)
                                .y(gy)
                                .size(col_w, 22.0)
                                .color([45, 43, 50])
                                .border_radius([4.0; 4])
                                .build(ui);

                            // Group header icon + text
                            nuon::label()
                                .x(col_x + 8.0)
                                .y(gy + 2.0)
                                .size(col_w - 16.0, 18.0)
                                .text(format!("{}  {}", icon_fn(), group_name))
                                .color([180, 180, 185])
                                .font_size(12.0)
                                .text_justify(nuon::TextJustify::Left)
                                .build(ui);

                            gy += 26.0;


                            for (program, name) in instruments {
                                let is_selected = scene.current_programs[0] == *program;

                                let item_id = nuon::Id::hash_with(|h| {
                                    use std::hash::Hash;
                                    "instrument_item_".hash(h);
                                    program.hash(h);
                                });

                                if nuon::button()
                                    .id(item_id)
                                    .x(col_x)
                                    .y(gy)
                                    .size(col_w, 20.0)
                                    .label((*name).to_string())
                                    .text_justify(nuon::TextJustify::Left)
                                    .border_radius([3.0; 4])
                                    .color(if is_selected {
                                        [100, 70, 180]
                                    } else {
                                        [37, 35, 42]
                                    })
                                    .hover_color([160, 81, 255])
                                    .preseed_color([180, 90, 255])
                                    .build(ui)
                                {
                                    scene.current_programs[0] = *program;
                                    ctx.output_manager
                                        .connection()
                                        .midi_event(
                                            u4::new(0),
                                            MidiMessage::ProgramChange {
                                                program: u7::new(*program),
                                            },
                                        );
                                    scene.popup.close();
                                }

                                gy += 22.0;
                            }

                            gy += 8.0;
                        }
                    }

                });
            }
        });

        if let Some(state) = scene.preview.as_ref() {
            let length = state.player.length();
            let progress = state.player.percentage();
            let measures = &state.player.song().file.measures;

            nuon::translate().y(30.0).build(ui, |ui| {
                let event = nuon::click_area("FreeplayPreviewProgress")
                    .size(width, 45.0)
                    .build(ui);

                if event.is_pressed() {
                    msg = Msg::Seek;
                }

                nuon::quad().size(width, 45.0).color([37, 35, 42]).build(ui);
                nuon::quad()
                    .size(width * progress, 45.0)
                    .color([56, 145, 255])
                    .build(ui);

                if !length.is_zero() {
                    for measure in measures.iter() {
                        let x = (measure.as_secs_f32() / length.as_secs_f32()) * width;
                        nuon::quad()
                            .x(x)
                            .size(1.0, 45.0)
                            .color(if x < width * progress {
                                [255, 255, 255, 127]
                            } else {
                                [102, 102, 102, 255]
                            })
                            .build(ui);
                    }
                }
            });
        }

        // H-separator
        nuon::quad()
            .y(top_bar_height)
            .size(width, 1.0)
            .color([57, 55, 62])
            .build(ui);
    });

    match msg {
        Msg::TogglePlay => {
            toggle_preview_playback(scene);
        }
        Msg::Seek => {
            seek_preview_to_cursor(scene, ctx);
        }
        Msg::GoBack => {
            ctx.proxy
                .send_event(NeothesiaEvent::MainMenu(scene.song.clone()))
                .ok();
        }
        Msg::Record => {
            handle_record_click(scene, ctx);
        }
        Msg::Save => {
            handle_save_click(scene, ctx);
        }
        Msg::OpenInstrumentSelector => {
            scene.popup.toggle(FreeplayPopup::InstrumentSelector);
        }
        Msg::None => {}
    }
}

fn handle_record_click(scene: &mut FreeplayScene, ctx: &Context) {
    if scene.recorder.is_recording() {
        scene.recorder_status = match stop_recording(scene, ctx) {
            Ok(()) => RecorderStatus::RecordingFinished(scene.recorder.duration()),
            Err(err) => RecorderStatus::Error(err),
        };
        return;
    }

    scene.keyboard.set_song_config(Default::default());
    scene.keyboard.reset_notes();

    scene.preview = None;
    scene.recorder_status = RecorderStatus::default();
    scene.recorder.start();
}

fn handle_save_click(scene: &mut FreeplayScene, ctx: &Context) {
    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter("midi", &["mid", "midi"])
        .set_file_name("freeplay-recording.mid");

    // TODO: `last_opened_song` is wrong in this context
    if let Some(path) = ctx.config.last_opened_song().and_then(|path| path.parent()) {
        dialog = dialog.set_directory(path);
    }

    let smf = match scene.recorder.as_smf() {
        Ok(smf) => smf.clone(),
        Err(err) => {
            scene.recorder_status = RecorderStatus::Error(err);
            return;
        }
    };

    scene
        .futures
        .push(on_async(dialog.save_file(), move |file, state, _ctx| {
            let Some(file) = file else {
                return;
            };

            match smf.save(file.path()) {
                Ok(()) => {
                    state.recorder_status = RecorderStatus::Saved(file.path().to_owned());
                }
                Err(_) => {
                    state.recorder_status = RecorderStatus::Error(RecorderError::Write);
                }
            }
        }));
}

fn stop_recording(scene: &mut FreeplayScene, ctx: &Context) -> Result<(), RecorderError> {
    let smf = scene.recorder.stop()?;

    let midi = midi_file::MidiFile::from_smf("freeplay-recording.mid", smf)
        .map_err(RecorderError::MidiFileParse)?;
    let song = Song::new(midi);

    scene.keyboard.set_song_config(song.config.clone());
    scene.keyboard.reset_notes();

    scene.preview = Some(Preview::new(&scene.keyboard, song, ctx));

    Ok(())
}

fn seek_preview_to_cursor(scene: &mut FreeplayScene, ctx: &Context) {
    let Some(player) = scene.preview.as_mut().map(|state| &mut state.player) else {
        return;
    };

    let width = ctx.window_state.logical_size.width.max(1.0);
    let percentage = (ctx.window_state.cursor_logical_position.x / width).clamp(0.0, 1.0);

    player.set_percentage_time(percentage);
    scene.keyboard.reset_notes();
}

pub fn toggle_preview_playback(scene: &mut FreeplayScene) {
    let Some(preview) = scene.preview.as_mut() else {
        return;
    };

    preview.player.pause_resume();
}

fn to_smf(events: &[RecordedMidiEvent]) -> Result<Smf<'static>, RecorderError> {
    // Preview/export requires at least one played note, not just release/control data.
    let has_note_events = events
        .iter()
        .any(|event| matches!(event.message, MidiMessage::NoteOn { .. }));

    if !has_note_events {
        return Err(RecorderError::NoNotesFound);
    }

    let mut track = vec![
        TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(TEMPO_MICROS_PER_BEAT.into())),
        },
        TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
        },
    ];

    let mut previous_ticks = 0u32;
    for event in events {
        let current_ticks = duration_to_ticks(event.timestamp);
        let delta_ticks = current_ticks.saturating_sub(previous_ticks);
        previous_ticks = current_ticks;

        track.push(TrackEvent {
            delta: delta_ticks.into(),
            kind: TrackEventKind::Midi {
                channel: event.channel.into(),
                message: event.message,
            },
        });
    }

    track.push(TrackEvent {
        delta: 1.into(),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    Ok(Smf {
        header: Header {
            format: Format::SingleTrack,
            timing: Timing::Metrical(TICKS_PER_BEAT.into()),
        },
        tracks: vec![track],
    })
}

#[cfg(test)]
mod freeplay_recorder_tests {
    use super::*;

    #[test]
    fn restarting_recording_discards_previous_take_and_resets_event_count() {
        let mut recorder = FreeplayRecorder::default();

        recorder.start();
        recorder.push_event(
            0,
            MidiMessage::NoteOn {
                key: 60.into(),
                vel: 100.into(),
            },
        );

        assert!(recorder.stop().is_ok());

        recorder.start();

        assert!(recorder.is_recording());

        let error = recorder.stop().expect_err("Empty");
        assert_eq!(error, RecorderError::NoNotesFound);
    }

    #[test]
    fn pedal_only_recording_is_rejected_for_preview_song() {
        let mut recorder = FreeplayRecorder::default();

        recorder.start();
        recorder.push_event(
            0,
            MidiMessage::Controller {
                controller: 64.into(),
                value: 127.into(),
            },
        );
        recorder.push_event(
            0,
            MidiMessage::Controller {
                controller: 64.into(),
                value: 0.into(),
            },
        );

        let error = recorder
            .stop()
            .expect_err("pedal-only recordings should not create preview songs");
        assert_eq!(error, RecorderError::NoNotesFound);
    }

    #[test]
    fn note_off_only_recording_is_rejected_for_preview_song() {
        let mut recorder = FreeplayRecorder::default();

        recorder.start();
        recorder.push_event(
            0,
            MidiMessage::NoteOff {
                key: 60.into(),
                vel: 0.into(),
            },
        );

        let error = recorder
            .stop()
            .expect_err("note-off-only recordings should not create preview songs");
        assert_eq!(error, RecorderError::NoNotesFound);
    }
}
