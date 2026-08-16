//! Direct-to-WAV preview synthesis (bypasses speechd) for the CLI's
//! `voice test` / `speak` commands.

use std::path::PathBuf;

use crate::voices::{cloud_pcm_rate, VgVoice};

/// Synthesise `text` through `voice`, returning a written WAV file and
/// the player command line to use (best-effort). No speechd involved.
pub fn preview_wav(voice: &VgVoice, text: &str) -> Result<(PathBuf, Option<String>), String> {
    let engine = rust_tts_wrapper::create_engine(&voice.engine_id, &voice.credentials)
        .ok_or_else(|| format!("engine '{}' unavailable", voice.engine_id))?;

    let rate = if voice.engine_id == "sherpaonnx" {
        voice.sample_rate.unwrap_or(22_050)
    } else {
        cloud_pcm_rate(&voice.engine_id)
    };

    let mut pcm: Vec<u8> = Vec::new();
    engine
        .speak_sync(
            text,
            Some(&voice.engine_voice_id),
            1.0,
            1.0,
            1.0,
            Some(&mut |chunk: &[u8]| pcm.extend_from_slice(chunk)),
            None,
        )
        .map_err(|e| e.to_string())?;
    if pcm.is_empty() {
        return Err("synthesis produced no audio".into());
    }

    // Minimal 16-bit mono WAV.
    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);

    let path = std::env::temp_dir().join(format!("voicegarden-preview-{}.wav", std::process::id()));
    std::fs::write(&path, wav).map_err(|e| format!("{}: {e}", path.display()))?;

    let player = ["pw-play", "paplay", "aplay", "ffplay"]
        .into_iter()
        .find(|p| which(p))
        .map(|p| match p {
            "ffplay" => format!("ffplay -autoexit -nodisp -loglevel quiet {path:?}"),
            _ => format!("{p} {path:?}"),
        });
    Ok((path, player))
}

fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(prog).is_file()))
        .unwrap_or(false)
}
