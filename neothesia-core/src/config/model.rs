use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration for Pocket Teto synthesis
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PocketTetoConfig {
    /// Base MIDI note for pitch calculation (A4 = 69)
    pub base_note: u8,
    /// Maximum pitch shift ratio (prevents extreme values)
    pub max_pitch_ratio: f32,
    /// Minimum pitch shift ratio
    pub min_pitch_ratio: f32,
    /// Path to directory containing syllable WAV files
    pub samples_dir: PathBuf,
}

impl Default for PocketTetoConfig {
    fn default() -> Self {
        Self {
            base_note: 69, // A4
            max_pitch_ratio: 4.0,  // 2 octaves up
            min_pitch_ratio: 0.25, // 2 octaves down
            samples_dir: PathBuf::from("assets/pocket_teto"),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Model {
    #[serde(default)]
    pub waterfall: WaterfallConfig,
    #[serde(default)]
    pub playback: PlaybackConfig,
    #[serde(default)]
    pub history: History,
    #[serde(default)]
    pub synth: SynthConfig,
    #[serde(default)]
    pub keyboard_layout: LayoutConfig,
    #[serde(default)]
    pub devices: DevicesConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub pc_keyboard: PcKeyboardConfig,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WaterfallConfigV1 {
    #[serde(default = "default_animation_speed")]
    pub animation_speed: f32,
    #[serde(default = "default_animation_offset")]
    pub animation_offset: f32,
    #[serde(default = "default_note_labels")]
    pub note_labels: bool,
}

#[derive(Serialize, Deserialize)]
pub enum WaterfallConfig {
    V1(WaterfallConfigV1),
}

impl Default for WaterfallConfig {
    fn default() -> Self {
        Self::V1(WaterfallConfigV1 {
            animation_speed: default_animation_speed(),
            animation_offset: default_animation_offset(),
            note_labels: default_note_labels(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PlaybackConfigV1 {
    #[serde(default = "default_speed_multiplier")]
    pub speed_multiplier: f32,
}

#[derive(Serialize, Deserialize)]
pub enum PlaybackConfig {
    V1(PlaybackConfigV1),
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self::V1(PlaybackConfigV1 {
            speed_multiplier: default_speed_multiplier(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryV1 {
    pub last_opened_song: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub enum History {
    V1(HistoryV1),
}

impl Default for History {
    fn default() -> Self {
        Self::V1(HistoryV1 {
            last_opened_song: None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SynthConfigV1 {
    pub soundfont_path: Option<PathBuf>,
    #[serde(default = "default_audio_gain")]
    pub audio_gain: f32,
    #[serde(default = "default_polyphony")]
    pub polyphony: u16,
    #[serde(default = "default_velocity_curve")]
    pub velocity_curve: VelocityCurve,
    #[serde(default)]
    pub pocket_teto: PocketTetoConfig,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum VelocityCurve {
    #[default]
    Linear,
    Concave,
    Convex,
    Fixed,
}

#[derive(Serialize, Deserialize)]
pub enum SynthConfig {
    V1(SynthConfigV1),
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self::V1(SynthConfigV1 {
            soundfont_path: None,
            audio_gain: default_audio_gain(),
            polyphony: default_polyphony(),
            velocity_curve: default_velocity_curve(),
            pocket_teto: PocketTetoConfig::default(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LayoutConfigV1 {
    #[serde(default = "default_piano_range")]
    pub range: (u8, u8),
}

#[derive(Serialize, Deserialize)]
pub enum LayoutConfig {
    V1(LayoutConfigV1),
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self::V1(LayoutConfigV1 {
            range: default_piano_range(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DevicesConfigV1 {
    pub output: Option<String>,
    pub input: Option<String>,
    #[serde(default = "default_separate_channels")]
    pub separate_channels: bool,
}

#[derive(Serialize, Deserialize)]
pub enum DevicesConfig {
    V1(DevicesConfigV1),
}

impl Default for DevicesConfig {
    fn default() -> Self {
        Self::V1(DevicesConfigV1 {
            output: default_output(),
            input: None,
            separate_channels: default_separate_channels(),
        })
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ColorSchemaV1 {
    pub name: String,
    pub base: (u8, u8, u8),
    pub dark: (u8, u8, u8),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AppearanceConfigV1 {
    #[serde(default = "default_vertical_guidelines")]
    pub vertical_guidelines: bool,
    #[serde(default = "default_horizontal_guidelines")]
    pub horizontal_guidelines: bool,
    #[serde(default = "default_glow")]
    pub glow: bool,
    #[serde(default = "default_chord_identifier")]
    pub chord_identifier: bool,
    #[serde(default = "default_background_color")]
    pub background_color: (u8, u8, u8),
    #[serde(default = "default_color_schema")]
    pub color_schema: Vec<ColorSchemaV1>,
}

#[derive(Serialize, Deserialize)]
pub enum AppearanceConfig {
    V1(AppearanceConfigV1),
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self::V1(AppearanceConfigV1 {
            vertical_guidelines: default_vertical_guidelines(),
            horizontal_guidelines: default_horizontal_guidelines(),
            glow: default_glow(),
            chord_identifier: default_chord_identifier(),
            background_color: default_background_color(),
            color_schema: default_color_schema(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PcKeyboardConfigV1 {
    #[serde(default = "default_octave_shift")]
    pub octave_shift: u8,
}

#[derive(Serialize, Deserialize)]
pub enum PcKeyboardConfig {
    V1(PcKeyboardConfigV1),
}

impl Default for PcKeyboardConfig {
    fn default() -> Self {
        Self::V1(PcKeyboardConfigV1 {
            octave_shift: default_octave_shift(),
        })
    }
}

fn default_piano_range() -> (u8, u8) {
    (21, 108)
}

fn default_speed_multiplier() -> f32 {
    1.0
}

fn default_animation_speed() -> f32 {
    400.0
}

fn default_animation_offset() -> f32 {
    0.0
}

fn default_note_labels() -> bool {
    false
}

fn default_audio_gain() -> f32 {
    0.2
}

fn default_velocity_curve() -> VelocityCurve {
    VelocityCurve::Linear
}

fn default_polyphony() -> u16 {
    256
}

fn default_vertical_guidelines() -> bool {
    true
}

fn default_horizontal_guidelines() -> bool {
    true
}

fn default_glow() -> bool {
    true
}

fn default_chord_identifier() -> bool {
    true
}

fn default_separate_channels() -> bool {
    false
}

fn default_background_color() -> (u8, u8, u8) {
    (24, 24, 24)
}

fn default_color_schema() -> Vec<ColorSchemaV1> {
    vec![
        ColorSchemaV1 {
            name: "Default".into(),
            base: (0x3c, 0x6e, 0xf0),
            dark: (0x28, 0x4a, 0xa0),
        },
        ColorSchemaV1 {
            name: "Red".into(),
            base: (0xf0, 0x3c, 0x3c),
            dark: (0xa0, 0x28, 0x28),
        },
        ColorSchemaV1 {
            name: "Green".into(),
            base: (0x3c, 0xf0, 0x6e),
            dark: (0x28, 0xa0, 0x4a),
        },
        ColorSchemaV1 {
            name: "Yellow".into(),
            base: (0xf0, 0xf0, 0x3c),
            dark: (0xa0, 0xa0, 0x28),
        },
        ColorSchemaV1 {
            name: "Purple".into(),
            base: (0xb0, 0x3c, 0xf0),
            dark: (0x70, 0x28, 0xa0),
        },
        ColorSchemaV1 {
            name: "Cyan".into(),
            base: (0x3c, 0xf0, 0xb0),
            dark: (0x28, 0xa0, 0x70),
        },
    ]
}

fn default_output() -> Option<String> {
    Some("Buildin Synth".into())
}

fn default_octave_shift() -> u8 {
    5
}