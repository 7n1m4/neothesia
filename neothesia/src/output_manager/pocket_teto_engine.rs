use encoding_rs;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc,
};

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source};
use std::num::{NonZeroU16, NonZeroU32};

use neothesia_core::config::PocketTetoConfig;

/// A single syllable sample loaded into memory
struct SyllableSample {
    data: Vec<f32>,
    sample_rate: u32,
    channels: u16,
}

/// Pocket Teto synthesis engine
/// Manages a round-robin queue of Japanese syllable samples,
/// plays them with pitch shifting based on MIDI note number
pub struct PocketTetoEngine {
    config: PocketTetoConfig,
    syllables: Vec<SyllableSample>,
    current_index: usize,
    sink: MixerDeviceSink,
}

impl PocketTetoEngine {
    /// Create a new PocketTetoEngine, loading syllable samples from the configured directory
    pub fn new(config: PocketTetoConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let sink = DeviceSinkBuilder::open_default_sink()?;
        
        let syllables = Self::load_syllables(&config.samples_dir).unwrap_or_else(|e| {
            log::warn!("Failed to load syllable samples: {}", e);
            Vec::new()
        });
        
        if syllables.is_empty() {
            log::warn!("No syllable samples found in {:?}, Pocket Teto will not produce sound", config.samples_dir);
        } else {
            log::info!("PocketTetoEngine loaded {} syllables", syllables.len());
        }

        Ok(Self {
            config,
            syllables,
            current_index: 0,
            sink,
        })
    }

    /// Load all WAV files from the samples directory, sorted by filename
    /// Parses oto.ini for preutterance offsets to trim silence before syllables
    fn load_syllables(dir: &Path) -> Result<Vec<SyllableSample>, Box<dyn std::error::Error + Send + Sync>> {
        // Parse oto.ini for preutterance offsets (column 3 in oto format)
        let oto_path = dir.join("oto.ini");
        let preutterance_offsets: HashMap<String, f32> = if oto_path.exists() {
            let raw_bytes = std::fs::read(&oto_path)?;
            // oto.ini is SHIFT-JIS encoded; try UTF-8 first, then SHIFT-JIS
            let content = match String::from_utf8(raw_bytes.clone()) {
                Ok(s) => s,
                Err(_) => {
                    // Try SHIFT-JIS decoding
                    let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(&raw_bytes);
                    cow.to_string()
                }
            };
            content.lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                        return None;
                    }
                    // Format: filename.wav=alias,offset,preutterance,overlap,window,velocity
                    let parts: Vec<&str> = line.splitn(2, '=').collect();
                    if parts.len() != 2 { return None; }
                    let filename = parts[0].trim().to_string();
                    let values: Vec<&str> = parts[1].split(',').collect();
                    if values.len() < 3 { return None; }
                    // preutterance is the 3rd value (index 2)
                    let preutterance: f32 = values[2].trim().parse().ok()?;
                    Some((filename, preutterance))
                })
                .collect()
        } else {
            HashMap::new()
        };

        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|ext| ext == "wav" || ext == "WAV").unwrap_or(false))
            .collect();

        // Sort by filename for consistent ordering
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        let mut syllables = Vec::new();
        for path in entries {
            let filename = path.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            
            match Self::load_wav(&path) {
                Ok(sample) => {
                    // Trim to preutterance offset if available
                    let trimmed = if let Some(&preutterance_ms) = preutterance_offsets.get(&filename) {
                        let sample_offset_samples = (preutterance_ms / 1000.0 * sample.sample_rate as f32) as usize;
                        let start = sample_offset_samples.min(sample.data.len());
                        SyllableSample {
                            data: sample.data[start..].to_vec(),
                            sample_rate: sample.sample_rate,
                            channels: sample.channels,
                        }
                    } else {
                        sample
                    };
                    
                    log::debug!("Loaded syllable: {:?} ({} samples, {} Hz)", 
                        path.file_name().unwrap(), trimmed.data.len(), trimmed.sample_rate);
                    syllables.push(trimmed);
                }
                Err(e) => {
                    log::warn!("Failed to load {:?}: {}", path, e);
                }
            }
        }

        Ok(syllables)
    }

    /// Load a single WAV file into memory as f32 samples
    fn load_wav(path: &Path) -> Result<SyllableSample, Box<dyn std::error::Error + Send + Sync>> {
        let file = std::fs::File::open(path)?;
        let mut decoder = Decoder::new(file)?;
        
        let sample_rate = decoder.sample_rate().get();
        let channels = decoder.channels().get();
        
        // Collect all samples into a Vec<f32>
        let mut data = Vec::new();
        for sample in decoder.by_ref() {
            data.push(sample);
        }

        // Convert to mono if stereo (average channels)
        let mono_data = if channels == 2 {
            data.chunks(2)
                .map(|chunk| (chunk[0] + chunk[1]) * 0.5)
                .collect()
        } else {
            data
        };

        Ok(SyllableSample {
            data: mono_data,
            sample_rate,
            channels,
        })
    }

    /// Handle a MIDI NoteOn event
    /// - Selects the next syllable in round-robin order
    /// - Calculates pitch shift ratio from MIDI note
    /// - Plays the pitched sample with velocity-based volume
    pub fn on_note_on(&mut self, midi_note: u8, velocity: u8) {
        if self.syllables.is_empty() {
            log::warn!("No syllables loaded, cannot play note");
            return;
        }

        // Get current syllable and advance index (round-robin)
        let syllable = &self.syllables[self.current_index];
        self.current_index = (self.current_index + 1) % self.syllables.len();

        // Calculate pitch shift ratio: 2^((midi_note - base_note) / 12)
        let note_diff = midi_note as f32 - self.config.base_note as f32;
        let pitch_ratio = 2.0_f32.powf(note_diff / 12.0);

        // Clamp to safe bounds
        let pitch_ratio = pitch_ratio
            .clamp(self.config.min_pitch_ratio, self.config.max_pitch_ratio);

        // Volume from velocity (0-127 -> 0.0-1.0)
        let volume = (velocity as f32 / 127.0).clamp(0.0, 1.0);

        log::debug!(
            "PocketTeto: note={}, velocity={}, syllable_index={}, pitch_ratio={:.3}, volume={:.3}",
            midi_note, velocity, self.current_index.saturating_sub(1), pitch_ratio, volume
        );

        // Play the pitched sample
        self.play_syllable(syllable, pitch_ratio, volume);
    }

    /// Play a syllable sample with pitch shifting and volume
    fn play_syllable(&self, syllable: &SyllableSample, pitch_ratio: f32, volume: f32) {
        let data = syllable.data.clone();
        let sample_rate = syllable.sample_rate;

        // Create a source that plays the samples at the adjusted rate
        let source = PitchedSampleSource {
            data,
            sample_rate,
            pitch_ratio,
            volume,
            position: 0,
        };

        // Play on the mixer for polyphony
        self.sink.mixer().add(source);
    }

    /// Stop all currently playing syllables
    pub fn stop_all(&self) {
        // The mixer doesn't provide a way to stop individual sources.
        // Sources will naturally finish when their samples end.
    }

    /// Get the number of loaded syllables
    pub fn syllable_count(&self) -> usize {
        self.syllables.len()
    }

    /// Get the current syllable index (next to be played)
    pub fn current_index(&self) -> usize {
        self.current_index
    }
}

/// A rodio Source that plays samples with pitch shifting
struct PitchedSampleSource {
    data: Vec<f32>,
    sample_rate: u32,
    pitch_ratio: f32,
    volume: f32,
    position: usize,
}

impl Iterator for PitchedSampleSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }

        // Calculate the source position with pitch shifting
        // We use linear interpolation for better quality
        let src_pos = self.position as f32 / self.pitch_ratio;
        let index = src_pos as usize;
        
        if index >= self.data.len().saturating_sub(1) {
            return None;
        }

        // Linear interpolation
        let frac = src_pos - index as f32;
        let sample = self.data[index] * (1.0 - frac) + self.data[index + 1] * frac;
        
        self.position += 1;
        
        Some(sample * self.volume)
    }
}

impl Source for PitchedSampleSource {
    fn current_span_len(&self) -> Option<usize> {
        Some((self.data.len() as f32 / self.pitch_ratio) as usize)
    }

    fn channels(&self) -> NonZeroU16 {
        NonZeroU16::new(1).unwrap() // Mono output
    }

    fn sample_rate(&self) -> NonZeroU32 {
        // Report the effective sample rate after pitch shifting
        NonZeroU32::new((self.sample_rate as f32 * self.pitch_ratio) as u32).unwrap()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        let frames = self.current_span_len()?;
        Some(std::time::Duration::from_secs_f32(
            frames as f32 / self.sample_rate().get() as f32
        ))
    }
}

/// Backend for Pocket Teto output - manages the engine instance
pub struct PocketTetoBackend {
    // Engine runs in a separate thread, we just keep the sender
    _engine_handle: Option<std::thread::JoinHandle<()>>,
}

impl PocketTetoBackend {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Don't initialize engine yet - wait for config
        Ok(Self { _engine_handle: None })
    }

    pub fn get_outputs(&self) -> Vec<crate::output_manager::OutputDescriptor> {
        vec![crate::output_manager::OutputDescriptor::PocketTeto(PocketTetoConfig::default())]
    }
    pub fn new_output_connection(&mut self, config: PocketTetoConfig) -> PocketTetoOutputConnection {
        let engine = PocketTetoEngine::new(config).expect("Failed to create PocketTetoEngine");
        
        // Create channel for MIDI events
        let (sender, receiver) = mpsc::channel::<(u8, u8)>();
        
        // Spawn thread to handle MIDI events
        let engine_handle = std::thread::spawn(move || {
            let mut engine = engine;
            while let Ok((note, velocity)) = receiver.recv() {
                engine.on_note_on(note, velocity);
            }
        });
        
        self._engine_handle = Some(engine_handle);
        
        PocketTetoOutputConnection::new(sender)
    }
}

impl Default for PocketTetoBackend {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            log::error!("Failed to initialize PocketTetoBackend: {}", e);
            Self { _engine_handle: None }
        })
    }
}

/// Connection for Pocket Teto output - sends MIDI events to the engine
#[derive(Clone)]
pub struct PocketTetoOutputConnection {
    sender: mpsc::Sender<(u8, u8)>, // (midi_note, velocity)
}

impl PocketTetoOutputConnection {
    pub fn new(sender: mpsc::Sender<(u8, u8)>) -> Self {
        Self { sender }
    }

    pub fn midi_event(&self, _channel: u8, msg: midi_file::midly::MidiMessage) {
        use midi_file::midly::MidiMessage;
        
        if let MidiMessage::NoteOn { key, vel } = msg {
            let note = key.as_int();
            let velocity = vel.as_int();
            let _ = self.sender.send((note, velocity));
        }
    }

    pub fn set_gain(&self, gain: f32) {
        log::trace!("PocketTeto set_gain: {}", gain);
    }

    pub fn stop_all(&self) {
        log::trace!("PocketTeto stop_all");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pitch_ratio_calculation() {
        let config = PocketTetoConfig::default();
        
        // A4 (69) should be 1.0
        let ratio = 2.0_f32.powf((69.0 - config.base_note as f32) / 12.0);
        assert!((ratio - 1.0).abs() < 0.001);
        
        // A5 (81) should be 2.0 (one octave up)
        let ratio = 2.0_f32.powf((81.0 - config.base_note as f32) / 12.0);
        assert!((ratio - 2.0).abs() < 0.001);
        
        // A3 (57) should be 0.5 (one octave down)
        let ratio = 2.0_f32.powf((57.0 - config.base_note as f32) / 12.0);
        assert!((ratio - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_pitch_ratio_clamping() {
        let config = PocketTetoConfig::default();
        
        // Very high note should be clamped
        let ratio = 2.0_f32.powf((127.0 - config.base_note as f32) / 12.0);
        let clamped = ratio.clamp(config.min_pitch_ratio, config.max_pitch_ratio);
        assert_eq!(clamped, config.max_pitch_ratio);
        
        // Very low note should be clamped
        let ratio = 2.0_f32.powf((0.0 - config.base_note as f32) / 12.0);
        let clamped = ratio.clamp(config.min_pitch_ratio, config.max_pitch_ratio);
        assert_eq!(clamped, config.min_pitch_ratio);
    }
}