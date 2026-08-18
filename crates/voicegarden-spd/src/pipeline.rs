//! Speak orchestration: run an engine on a worker thread and stream PCM to
//! the speech-dispatcher server as chunks arrive, interleaving index marks
//! at the right audio positions.
//!
//! rust-tts-wrapper delivers **PCM16 mono** through `on_audio` for every
//! engine (cloud MP3 is decoded inside the crate), so the module needs no
//! decoder of its own — chunks pass straight through to
//! `module_tts_output_server` at `chunk_ms` granularity while synthesis is
//! still running.
//!
//! Threading contract: `speak()` must be called from the thread that runs
//! `module_process` (the main thread), because `module_tts_output_server`
//! polls the server pipe for STOP between pieces. Engine synthesis happens
//! on a worker thread so the main thread can keep polling while long
//! synthesis runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use rust_tts_wrapper::engine::TtsEngine;

use crate::glue::{self, AudioTrack, SPD_AUDIO_LE};
use crate::ssml::SsmlMark;
use crate::voices::VgVoice;

/// Settings snapshot → engine multipliers.
#[derive(Debug, Clone, Copy)]
pub struct Prosody {
    pub rate_mult: f32,
    pub pitch_mult: f32,
    pub volume_mult: f32,
}

/// Map speech-dispatcher's -100..100 parameters to engine multipliers
/// (1.0 = normal). Linear, clamped to sane engine ranges.
impl Prosody {
    #[must_use]
    pub fn from_spd(rate: i32, pitch: i32, volume: i32) -> Self {
        Self {
            rate_mult: spd_to_mult(rate, 0.1, 3.0),
            pitch_mult: spd_to_mult(pitch, 0.5, 2.0),
            volume_mult: spd_to_mult(volume, 0.0, 2.0),
        }
    }
}

fn spd_to_mult(v: i32, min: f32, max: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let m = (v + 100) as f32 / 100.0;
    m.clamp(min, max)
}

/// Outcome of a speak run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All audio (and pending marks) were handed to the server.
    Completed,
    /// STOP/PAUSE arrived mid-run; remaining audio was dropped.
    Aborted,
    /// Synthesis produced nothing speakable; the server was told
    /// `301 ERROR CANT SPEAK` so the failure is visible to the daemon
    /// (and `spd-say`/clients) instead of masquerading as success.
    Failed,
}

/// One word-boundary event relayed by the engine.
#[derive(Debug, Clone)]
pub struct Boundary {
    pub word: String,
    pub start: f32,
    pub end: f32,
    /// Byte offset into the spoken text (-1 when unknown).
    pub byte_offset: i32,
}

/// A word with its byte span in the cleaned text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordSpan {
    pub text: String,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Split text into whitespace-delimited words, remembering byte offsets.
#[must_use]
pub fn word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    let mut idx = 0usize;
    for word in text.split_whitespace() {
        let Some(rel) = text[idx..].find(word) else {
            continue;
        };
        let start = idx + rel;
        let end = start + word.len();
        spans.push(WordSpan {
            text: word.to_string(),
            byte_start: start,
            byte_end: end,
        });
        idx = end;
    }
    spans
}

/// Index of the first word not entirely before `byte_offset` (a mark inside
/// word W yields W's index; a mark between words yields the next word).
/// `None` = the offset is past the last word.
fn word_index_at(words: &[WordSpan], byte_offset: usize) -> Option<usize> {
    words.iter().position(|w| w.byte_end > byte_offset)
}

/// Time at which a mark should fire, given currently-known boundaries.
///
/// * word index within `boundaries` → that word's start time (confident).
/// * `finalized=true` (synthesis done, boundary set closed): a mark past
///   the last word fires at the last boundary's end.
/// * word index at/after the boundary frontier → `None` (not yet knowable).
#[must_use]
pub fn mark_time(
    mark: &SsmlMark,
    words: &[WordSpan],
    boundaries: &[Boundary],
    boundary_time_scale: f32,
    finalized: bool,
) -> Option<f32> {
    match word_index_at(words, mark.byte_offset) {
        Some(idx) => boundaries.get(idx).map_or_else(
            || {
                // Boundary set closed but this word never reported (engine
                // estimated coarsely / clipped): anchor at the last known end.
                if finalized {
                    boundaries.last().map(|b| b.end * boundary_time_scale)
                } else {
                    None
                }
            },
            |b| Some(b.start * boundary_time_scale),
        ),
        None if finalized => boundaries.last().map(|b| b.end * boundary_time_scale),
        None => None,
    }
}

/// Map every mark to a time using the full (closed) boundary set. Marks
/// positioned before their word's boundary data fire at the last known
/// boundary end; with no boundaries at all they fire at 0.
#[must_use]
pub fn map_marks_to_times(
    marks: &[SsmlMark],
    boundaries: &[Boundary],
    text: &str,
    boundary_time_scale: Option<f32>,
) -> Vec<(f32, String)> {
    if marks.is_empty() {
        return Vec::new();
    }
    let scale = boundary_time_scale.unwrap_or(1.0);
    let words = word_spans(text);
    let mut out = Vec::with_capacity(marks.len());
    for mark in marks {
        let time = match word_index_at(&words, mark.byte_offset) {
            Some(idx) => match boundaries.get(idx) {
                Some(b) => b.start * scale,
                None => boundaries.last().map_or(0.0, |b| b.end * scale),
            },
            None => boundaries.last().map_or(0.0, |b| b.end * scale),
        };
        out.push((time, mark.name.clone()));
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Messages from the synthesis worker.
enum Msg {
    /// PCM16 mono bytes from `on_audio`.
    Pcm(Vec<u8>),
    Boundary(Boundary),
    Done(Result<(), String>),
}

/// Run one utterance end to end, owning the whole SPEAK reply sequence.
///
/// The reply to the server's terminating dot is deferred until synthesis
/// has actually produced audio (issue #1): the server blocks on that
/// reply with no timeout, which is the module protocol's only channel
/// for reporting a failed utterance. So:
///
/// * first PCM chunk ready → `200 OK SPEAKING`, `701 BEGIN`, then audio
///   streams as it arrives and `702 END` closes the utterance;
/// * synthesis fails or yields **zero audio** → `301 ERROR CANT SPEAK`
///   and no events — the daemon logs the failure and drops the message
///   instead of reporting a silent success;
/// * STOP/PAUSE before any audio → the handshake is still completed
///   (`200 OK SPEAKING` + BEGIN + END) because the server is waiting
///   for the reply; no audio is sent.
///
/// * `synth_text` — what the engine receives (may be SSML for
///   passthrough-capable engines; timing marks are stripped from it).
/// * `timing_text` — plain text the mark byte-offsets refer to; also the
///   basis for word/timing alignment.
///
/// `poll` is invoked regularly while waiting for the worker — in
/// production it pumps `module_process(0)` so STOP/PAUSE are honoured.
#[allow(clippy::too_many_arguments)]
pub fn speak(
    engine: Arc<dyn TtsEngine>,
    voice: &VgVoice,
    synth_text: &str,
    timing_text: &str,
    prosody: Prosody,
    marks: &[SsmlMark],
    chunk_ms: u32,
    stop_flag: &Arc<AtomicBool>,
    poll: &dyn Fn(),
) -> Outcome {
    speak_inner(
        &engine,
        voice,
        synth_text,
        timing_text,
        prosody,
        marks,
        chunk_ms,
        stop_flag,
        poll,
    )
}

/// Stream pre-decoded PCM16 mono (sound icons) with the full
/// begin/end event sequence.
///
/// No `706 ICON` is reported: the server treats 706 as "play this file
/// yourself" (speak_queue_send_file_to_audio) — since we already stream
/// the PCM inline, reporting it would double-play every icon.
pub fn stream_raw_pcm(
    samples: &[i16],
    sample_rate: u32,
    chunk_ms: u32,
    stop_flag: &Arc<AtomicBool>,
    poll: &dyn Fn(),
) -> Outcome {
    unsafe { glue::module_speak_ok() };
    unsafe { glue::module_report_event_begin() };
    let per_chunk =
        ((u64::from(sample_rate.max(1)) * u64::from(chunk_ms) / 1000).max(1) as usize).max(1);
    let mut sent = 0usize;
    while sent < samples.len() && !stop_flag.load(Ordering::SeqCst) {
        let end = (sent + per_chunk).min(samples.len());
        send_pcm(&samples[sent..end], sample_rate);
        sent = end;
        poll();
    }
    unsafe { glue::module_report_event_end() };
    if sent >= samples.len() {
        Outcome::Completed
    } else {
        Outcome::Aborted
    }
}

#[allow(clippy::too_many_arguments)]
fn speak_inner(
    engine: &Arc<dyn TtsEngine>,
    voice: &VgVoice,
    synth_text: &str,
    timing_text: &str,
    prosody: Prosody,
    marks: &[SsmlMark],
    chunk_ms: u32,
    stop_flag: &Arc<AtomicBool>,
    poll: &dyn Fn(),
) -> Outcome {
    let (tx, rx) = mpsc::channel::<Msg>();
    let voice_id = voice.engine_voice_id.clone();
    let text_owned = synth_text.to_string();

    let worker = std::thread::Builder::new().name("vg-synth".into()).spawn({
        let engine = Arc::clone(engine);
        let stop_flag = Arc::clone(stop_flag);
        move || {
            // catch_unwind: an engine panic must surface as Err, not as a
            // mystery "worker vanished" success-shaped silence.
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.speak_sync(
                    &text_owned,
                    Some(&voice_id),
                    prosody.rate_mult,
                    prosody.pitch_mult,
                    prosody.volume_mult,
                    Some(&mut |chunk: &[u8]| {
                        if stop_flag.load(Ordering::SeqCst) {
                            return;
                        }
                        let _ = tx.send(Msg::Pcm(chunk.to_vec()));
                    }),
                    Some(&mut |word, start, end, byte_offset, _len| {
                        let _ = tx.send(Msg::Boundary(Boundary {
                            word: word.to_string(),
                            start,
                            end,
                            byte_offset,
                        }));
                    }),
                )
            }));
            let done = match res {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("synthesis worker panicked".into()),
            };
            let _ = tx.send(Msg::Done(done));
        }
    });
    let worker = match worker {
        Ok(w) => w,
        Err(e) => {
            eprintln!("voicegarden-spd: failed to spawn synth thread: {e}");
            unsafe { glue::module_speak_error() };
            return Outcome::Failed;
        }
    };

    // sherpa boundaries come from the crate's 150-wpm estimator and are
    // unaffected by the engine `speed` factor, so rescale by 1/rate; cloud
    // timings are computed against the prosody actually applied.
    let is_local = voice.engine_id == "sherpaonnx";
    let rate = if is_local {
        voice.sample_rate.unwrap_or(22_050)
    } else {
        voice.pcm_rate.max(8_000)
    };
    let time_scale = if is_local && prosody.rate_mult > 0.0 {
        1.0 / prosody.rate_mult
    } else {
        1.0
    };

    let words = word_spans(timing_text);
    let samples_per_chunk = ((u64::from(rate) * u64::from(chunk_ms) / 1000).max(1) as usize).max(1);
    let mut pending: Vec<i16> = Vec::new(); // leftover PCM between chunks
    let mut boundaries: Vec<Boundary> = Vec::new();
    let mut next_mark = 0usize; // marks fire in document order
    let mut sent_secs = 0.0f32;
    let mut done: Option<Result<(), String>> = None;
    // Deferred SPEAK reply: nothing is sent to the server until audio
    // actually exists (see `speak` docs) — unless the run ends first.
    let mut started = false;

    loop {
        // Drain everything currently queued before deciding to wait.
        loop {
            match rx.try_recv() {
                Ok(Msg::Pcm(bytes)) => append_le_bytes(&mut pending, &bytes),
                Ok(Msg::Boundary(b)) => boundaries.push(b),
                Ok(Msg::Done(res)) => {
                    done = Some(res);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    done = Some(Err("synthesis worker vanished".into()));
                    break;
                }
            }
        }

        // Stream whatever a full chunk allows while data keeps flowing.
        while pending.len() >= samples_per_chunk && !stop_flag.load(Ordering::SeqCst) {
            if !started {
                // First audio: complete the SPEAK handshake and open the
                // event sequence.
                unsafe { glue::module_speak_ok() };
                unsafe { glue::module_report_event_begin() };
                started = true;
            }
            let chunk: Vec<i16> = pending.drain(..samples_per_chunk).collect();
            send_pcm(&chunk, rate);
            #[allow(clippy::cast_precision_loss)]
            {
                sent_secs += chunk.len() as f32 / rate as f32;
            }
            fire_due_marks(
                marks,
                &mut next_mark,
                &words,
                &boundaries,
                time_scale,
                sent_secs,
                false,
            );
        }

        if let Some(res) = &done {
            if let Err(e) = res {
                eprintln!("voicegarden-spd: synthesis failed: {e}");
            }
            break;
        }
        if stop_flag.load(Ordering::SeqCst) {
            let _ = engine.stop();
            break;
        }

        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(msg) => {
                // Push back into the queue path by handling directly.
                match msg {
                    Msg::Pcm(bytes) => append_le_bytes(&mut pending, &bytes),
                    Msg::Boundary(b) => boundaries.push(b),
                    Msg::Done(res) => done = Some(res),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => poll(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                done = Some(Err("synthesis worker vanished".into()))
            }
        }
    }
    let _ = worker.join();

    if started {
        // Final flush: remaining samples, then all unfired marks (boundary
        // set is now closed, so every mark is mappable). Skipped on abort:
        // the remaining audio is dropped.
        let aborted = stop_flag.load(Ordering::SeqCst);
        if !aborted {
            while !pending.is_empty() {
                let take = pending.len().min(samples_per_chunk);
                let chunk: Vec<i16> = pending.drain(..take).collect();
                send_pcm(&chunk, rate);
            }
            let _ = sent_secs; // audio position no longer needed once flushing
            fire_due_marks(
                marks,
                &mut next_mark,
                &words,
                &boundaries,
                time_scale,
                f32::INFINITY,
                true,
            );
        }
        unsafe { glue::module_report_event_end() };
        if aborted {
            Outcome::Aborted
        } else {
            Outcome::Completed
        }
    } else if !pending.is_empty() {
        // Sub-chunk utterance (audio shorter than one ChunkMs block —
        // e.g. a single spoken character): the streaming loop never saw a
        // full chunk, so open the event sequence now and flush it.
        unsafe { glue::module_speak_ok() };
        unsafe { glue::module_report_event_begin() };
        while !pending.is_empty() {
            let take = pending.len().min(samples_per_chunk);
            let chunk: Vec<i16> = pending.drain(..take).collect();
            send_pcm(&chunk, rate);
        }
        fire_due_marks(
            marks,
            &mut next_mark,
            &words,
            &boundaries,
            time_scale,
            f32::INFINITY,
            true,
        );
        unsafe { glue::module_report_event_end() };
        Outcome::Completed
    } else if stop_flag.load(Ordering::SeqCst) {
        // Cancelled before any audio. The server is still waiting for the
        // SPEAK reply, so complete the handshake with no audio between
        // the events (the server's speak queue turns END-after-stop into
        // the right client-side stop event).
        unsafe { glue::module_speak_ok() };
        unsafe { glue::module_report_event_begin() };
        unsafe { glue::module_report_event_end() };
        Outcome::Aborted
    } else {
        // Nothing speakable came out of the engine. Report the failure —
        // never a silent success (issue #1).
        if matches!(&done, Some(Ok(()))) {
            eprintln!("voicegarden-spd: synthesis produced no audio");
        }
        unsafe { glue::module_speak_error() };
        Outcome::Failed
    }
}

/// Emit marks whose (now-known) time has been reached. Only marks whose
/// word boundary is known are eligible unless `finalized`.
#[allow(clippy::too_many_arguments)]
fn fire_due_marks(
    marks: &[SsmlMark],
    next_mark: &mut usize,
    words: &[WordSpan],
    boundaries: &[Boundary],
    time_scale: f32,
    sent_secs: f32,
    finalized: bool,
) {
    while *next_mark < marks.len() {
        let Some(time) = mark_time(&marks[*next_mark], words, boundaries, time_scale, finalized)
        else {
            return; // not yet knowable — try again when more boundaries land
        };
        if time > sent_secs {
            return; // later in the audio stream
        }
        report_mark(&marks[*next_mark].name);
        *next_mark += 1;
    }
}

fn report_mark(name: &str) {
    if let Ok(c) = std::ffi::CString::new(name) {
        unsafe { glue::module_report_index_mark(c.as_ptr()) };
    }
}

/// Send one PCM16 mono chunk to the speech-dispatcher server.
fn send_pcm(chunk: &[i16], rate: u32) {
    let track = AudioTrack {
        bits: 16,
        num_channels: 1,
        sample_rate: rate as i32,
        num_samples: chunk.len() as i32,
        samples: chunk.as_ptr() as *mut i16,
    };
    unsafe { glue::module_tts_output_server(&track, SPD_AUDIO_LE) };
}

/// Append little-endian PCM16 bytes to a `Vec<i16>`.
fn append_le_bytes(pcm: &mut Vec<i16>, bytes: &[u8]) {
    pcm.reserve(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        pcm.push(i16::from_le_bytes([pair[0], pair[1]]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(word: &str, start: f32, end: f32) -> Boundary {
        Boundary {
            word: word.into(),
            start,
            end,
            byte_offset: -1,
        }
    }

    fn mark(name: &str, byte_offset: usize) -> SsmlMark {
        SsmlMark {
            name: name.into(),
            byte_offset,
        }
    }

    #[test]
    fn prosody_mapping() {
        let p = Prosody::from_spd(0, 0, 0);
        assert!((p.rate_mult - 1.0).abs() < 1e-6);
        let fast = Prosody::from_spd(100, 100, 100);
        assert!((fast.rate_mult - 2.0).abs() < 1e-6);
        let slow = Prosody::from_spd(-100, -100, -100);
        assert!((slow.rate_mult - 0.1).abs() < 1e-6);
        assert!(slow.volume_mult.abs() < 1e-6);
    }

    #[test]
    fn word_spans_byte_offsets() {
        let spans = word_spans("héllo  wörld x");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].byte_start, 0);
        assert_eq!(spans[0].byte_end, 6); // h é l l o → 1+2+1+1+1
        assert_eq!(spans[1].byte_start, 8);
        assert_eq!(spans[2].byte_start, 15);
    }

    #[test]
    fn marks_map_to_word_start_times() {
        let text = "one two three";
        let marks = vec![
            mark("a", text.find("two").unwrap()),
            mark("b", text.find("three").unwrap()),
        ];
        let boundaries = vec![
            boundary("one", 0.0, 0.5),
            boundary("two", 0.5, 1.0),
            boundary("three", 1.0, 1.5),
        ];
        let times = map_marks_to_times(&marks, &boundaries, text, None);
        assert_eq!(times.len(), 2);
        assert!((times[0].0 - 0.5).abs() < 1e-6);
        assert_eq!(times[0].1, "a");
        assert!((times[1].0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mark_past_end_uses_last_boundary_end() {
        let text = "hello";
        let marks = vec![mark("tail", text.len())];
        let boundaries = vec![boundary("hello", 0.0, 0.9)];
        let times = map_marks_to_times(&marks, &boundaries, text, None);
        assert!((times[0].0 - 0.9).abs() < 1e-6);
    }

    #[test]
    fn boundary_scale_applies() {
        let text = "one two";
        let marks = vec![mark("m", text.find("two").unwrap())];
        let boundaries = vec![boundary("one", 0.0, 0.5), boundary("two", 0.5, 1.0)];
        let times = map_marks_to_times(&marks, &boundaries, text, Some(0.5));
        assert!((times[0].0 - 0.25).abs() < 1e-6);
    }

    #[test]
    fn mark_time_requires_boundary_until_finalized() {
        let text = "one two";
        let words = word_spans(text);
        let m = mark("m", text.find("two").unwrap());
        // Only the first boundary known: mark on word 1 is not yet knowable.
        let partial = vec![boundary("one", 0.0, 0.5)];
        assert!(mark_time(&m, &words, &partial, 1.0, false).is_none());
        let full = vec![boundary("one", 0.0, 0.5), boundary("two", 0.5, 1.0)];
        assert_eq!(mark_time(&m, &words, &full, 1.0, false), Some(0.5));
    }

    #[test]
    fn mark_past_end_waits_for_finalization() {
        let words = word_spans("hello");
        let m = mark("tail", 5);
        let bs = vec![boundary("hello", 0.0, 0.9)];
        assert!(mark_time(&m, &words, &bs, 1.0, false).is_none());
        assert_eq!(mark_time(&m, &words, &bs, 1.0, true), Some(0.9));
    }

    #[test]
    fn fire_due_marks_respects_order_and_confidence() {
        let text = "one two three";
        let words = word_spans(text);
        let marks = vec![
            mark("a", text.find("two").unwrap()),
            mark("b", text.find("three").unwrap()),
        ];
        let mut next = 0usize;
        let partial = vec![boundary("one", 0.0, 0.5), boundary("two", 0.5, 1.0)];
        // "a" is knowable and due at 0.6s; "b" not yet knowable.
        fire_due_marks(&marks, &mut next, &words, &partial, 1.0, 0.6, false);
        assert_eq!(next, 1);
        // Advance without new boundaries: "b" still held back.
        fire_due_marks(&marks, &mut next, &words, &partial, 1.0, 5.0, false);
        assert_eq!(next, 1);
        // Finalized: everything fires.
        fire_due_marks(
            &marks,
            &mut next,
            &words,
            &partial,
            1.0,
            f32::INFINITY,
            true,
        );
        assert_eq!(next, 2);
    }

    #[test]
    fn append_le_bytes_pairs() {
        let mut pcm = Vec::new();
        append_le_bytes(&mut pcm, &[0x01, 0x00, 0xFF, 0xFF, 0x00]);
        assert_eq!(pcm, vec![1, -1]);
    }
}
