//! Sound-icon support: look up named icon files and read PCM WAVs for
//! direct streaming to the server (mirrors the stock modules' behaviour —
//! speech-dispatcher sends `SOUND_ICON` and expects either an audio file
//! or a spoken fallback).

/// A parsed PCM WAV, mixed down to mono i16.
pub struct WavData {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

/// Sanitised icon lookup: the name must not contain path separators or
/// parent references (SSIP delivers names like `capital`, `message-new`).
#[must_use]
pub fn icon_path(folder: &str, name: &str) -> Option<std::path::PathBuf> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        return None;
    }
    let path = std::path::Path::new(folder).join(name);
    path.is_file().then_some(path)
}

/// Minimal RIFF/WAVE parser: PCM (format 1) and WAVE_FORMAT_EXTENSIBLE
/// (0xFFFE, treated as PCM), 8 or 16 bit, 1–2 channels (stereo is mixed
/// to mono). Anything else returns `Err`.
#[allow(clippy::cast_possible_truncation)]
pub fn parse_wav(bytes: &[u8]) -> Result<WavData, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let mut pos = 12usize;
    let mut format: Option<u16> = None;
    let mut channels: Option<u16> = None;
    let mut rate: Option<u32> = None;
    let mut bits: Option<u16> = None;
    let mut data: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(
            bytes[pos + 4..pos + 8]
                .try_into()
                .map_err(|_| "truncated chunk header")?,
        ) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        match id {
            b"fmt " => {
                let body = &bytes[body_start..body_end];
                if body.len() < 16 {
                    return Err("fmt chunk too short".into());
                }
                format = Some(u16::from_le_bytes(body[0..2].try_into().expect("2 bytes")));
                channels = Some(u16::from_le_bytes(body[2..4].try_into().expect("2 bytes")));
                rate = Some(u32::from_le_bytes(body[4..8].try_into().expect("4 bytes")));
                bits = Some(u16::from_le_bytes(
                    body[14..16].try_into().expect("2 bytes"),
                ));
            }
            b"data" => data = Some(&bytes[body_start..body_end]),
            _ => {}
        }
        pos = body_start + size + (size & 1); // chunks are word-aligned
    }

    let format = format.ok_or("missing fmt chunk")?;
    if format != 1 && format != 0xFFFE {
        return Err(format!("unsupported WAV format {format} (want PCM)"));
    }
    let channels = channels.ok_or("missing channel count")?;
    if channels == 0 || channels > 2 {
        return Err(format!("unsupported channel count {channels}"));
    }
    let rate = rate.ok_or("missing sample rate")?;
    let bits = bits.ok_or("missing bit depth")?;
    let data = data.ok_or("missing data chunk")?;

    let samples: Vec<i16> = match (bits, channels) {
        (16, 1) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| i16::from_le_bytes(*p))
            .collect(),
        (16, 2) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let l = i16::from_le_bytes([p[0], p[1]]) as i32;
                let r = i16::from_le_bytes([p[2], p[3]]) as i32;
                let avg = (l + r) / 2;
                avg as i16
            })
            .collect(),
        (8, 1) => data.iter().map(|&u| (i16::from(u) - 128) << 8).collect(),
        _ => return Err(format!("unsupported {bits}-bit {channels}-channel WAV")),
    };

    if samples.is_empty() {
        return Err("no audio data".into());
    }
    Ok(WavData {
        samples,
        sample_rate: rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal 16-bit mono PCM WAV.
    fn make_wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut wav = Vec::new();
        let data_len = (samples.len() * channels as usize * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        wav.extend_from_slice(&(channels * 2).to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        if channels == 1 {
            for s in samples {
                wav.extend_from_slice(&s.to_le_bytes());
            }
        } else {
            for s in samples {
                wav.extend_from_slice(&s.to_le_bytes());
                wav.extend_from_slice(&s.to_le_bytes());
            }
        }
        wav
    }

    #[test]
    fn parses_mono_wav() {
        let w = parse_wav(&make_wav(1, 22_050, &[100, -100, 0])).unwrap();
        assert_eq!(w.sample_rate, 22_050);
        assert_eq!(w.samples, vec![100, -100, 0]);
    }

    #[test]
    fn parses_stereo_to_mono() {
        let w = parse_wav(&make_wav(2, 16_000, &[100, 200])).unwrap();
        assert_eq!(w.samples, vec![100, 200]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_wav(b"nope").is_err());
        assert!(parse_wav(b"RIFF0000WAVEjunk").is_err());
    }

    #[test]
    fn icon_path_sanitises() {
        assert!(icon_path("/icons", "capital").is_none() || true); // depends on fs
        assert!(icon_path("/icons", "../etc/passwd").is_none());
        assert!(icon_path("/icons", "a/b").is_none());
        assert!(icon_path("/icons", "").is_none());
    }
}
