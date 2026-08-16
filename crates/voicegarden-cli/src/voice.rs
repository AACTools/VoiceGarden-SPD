//! `voicegarden-spd voice …` — search/list/info/test over the merged
//! local + cached-cloud voice list.

use clap::Subcommand;
use serde::Serialize;
use voicegarden_spd::config::ModuleConfig;
use voicegarden_spd::voices::{filter_voices, merged_voices, Source, VgVoice, VoiceFilter};

use crate::Style;

/// CLI mirror of [`Source`].
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum SourceArg {
    Local,
    Cloud,
}

#[derive(Subcommand)]
pub enum VoiceCmd {
    /// List every voice (same as `voice search` with no filters)
    List,
    /// Search voices with filters; positional terms match name/engine/language
    #[command(alias = "find")]
    Search {
        /// Free-text terms (all must match somewhere)
        terms: Vec<String>,
        /// Voice source: local (offline models) or cloud (online engines)
        #[arg(long, value_enum)]
        source: Option<SourceArg>,
        /// Shorthand for --source local
        #[arg(long, conflicts_with_all = ["cloud", "source"])]
        local: bool,
        /// Shorthand for --source cloud
        #[arg(long, conflicts_with_all = ["local", "source"])]
        cloud: bool,
        /// Filter by engine id (repeatable): --engine edge --engine azure
        #[arg(long = "engine")]
        engines: Vec<String>,
        /// Language: base ("en") matches en-US; full ("en-GB") matches exactly
        #[arg(long)]
        lang: Option<String>,
        /// Gender: male | female | unknown
        #[arg(long)]
        gender: Option<String>,
        /// Quality tier (local models): high | medium | low | x_low | …
        #[arg(long)]
        quality: Option<String>,
        /// Only multilingual voices
        #[arg(long)]
        multilingual: bool,
        /// Machine-readable output (JSON; never includes credentials)
        #[arg(long)]
        json: bool,
    },
    /// Full detail for one voice
    Info { name: String },
    /// Speak a line through one voice (direct preview, no speechd)
    Test {
        name: String,
        #[arg(default_value = "Hello from VoiceGarden.")]
        text: String,
    },
}

#[derive(Debug, Default, Clone)]
pub struct ListArgs;

pub fn run(cmd: VoiceCmd, cfg_path: Option<&str>) -> Result<(), String> {
    match cmd {
        VoiceCmd::List => list(cfg_path, &ListArgs),
        VoiceCmd::Search {
            terms,
            source,
            local,
            cloud,
            engines,
            lang,
            gender,
            quality,
            multilingual,
            json,
        } => {
            let source = source
                .map(|s| match s {
                    SourceArg::Local => Source::Local,
                    SourceArg::Cloud => Source::Cloud,
                })
                .or(if local {
                    Some(Source::Local)
                } else if cloud {
                    Some(Source::Cloud)
                } else {
                    None
                });
            let filter = VoiceFilter {
                terms,
                source,
                engines,
                lang,
                gender,
                quality,
                multilingual,
            };
            render(cfg_path, &filter, json)
        }
        VoiceCmd::Info { name } => info(cfg_path, &name),
        VoiceCmd::Test { name, text } => preview(&name, &text, cfg_path),
    }
}

pub fn list(cfg_path: Option<&str>, _args: &ListArgs) -> Result<(), String> {
    render(cfg_path, &VoiceFilter::default(), false)
}

// ---------------------------------------------------------------------------
// search rendering
// ---------------------------------------------------------------------------

/// JSON-safe projection — deliberately excludes `credentials`.
#[derive(Serialize)]
struct VoiceJson<'a> {
    name: &'a str,
    engine: &'a str,
    voice_id: &'a str,
    language: &'a str,
    languages: &'a [String],
    gender: &'a str,
    quality: &'a str,
    multilingual: bool,
    source: &'a str,
    sample_rate: Option<u32>,
    license: &'a str,
}

impl<'a> From<&'a VgVoice> for VoiceJson<'a> {
    fn from(v: &'a VgVoice) -> Self {
        Self {
            name: &v.spd_name,
            engine: &v.engine_id,
            voice_id: &v.engine_voice_id,
            language: &v.language,
            languages: &v.languages,
            gender: &v.gender,
            quality: &v.quality,
            multilingual: v.multilingual,
            source: match v.source() {
                Source::Local => "local",
                Source::Cloud => "cloud",
            },
            sample_rate: v.sample_rate,
            license: &v.license,
        }
    }
}

fn render(cfg_path: Option<&str>, filter: &VoiceFilter, json: bool) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let all = merged_voices(&cfg);
    let matches = filter_voices(&all, filter);

    if json {
        let out: Vec<VoiceJson<'_>> = matches.iter().map(VoiceJson::from).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    let st = Style::new();
    let rows: Vec<Vec<String>> = matches
        .iter()
        .map(|v| {
            let source = match v.source() {
                Source::Local => st.cyan("local"),
                Source::Cloud => st.magenta("cloud"),
            };
            let note = if v.multilingual {
                "◈ multilingual".to_string()
            } else if !v.license.is_empty() {
                st.dim(&v.license)
            } else {
                String::new()
            };
            vec![
                v.spd_name.clone(),
                source,
                v.display_name.clone(),
                v.language.clone(),
                v.gender.clone(),
                v.quality.clone(),
                note,
            ]
        })
        .collect();
    println!(
        "{}",
        crate::render_table(
            &["VOICE", "SOURCE", "NAME", "LANG", "GENDER", "QUALITY", ""],
            &rows
        )
    );
    println!(
        "{} {} voices ({} local, {} cloud){}",
        st.dim("→"),
        matches.len(),
        matches
            .iter()
            .filter(|v| v.source() == Source::Local)
            .count(),
        matches
            .iter()
            .filter(|v| v.source() == Source::Cloud)
            .count(),
        st.dim(" — `voice info <name>` for detail, `voice test <name>` to hear it")
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// info / test
// ---------------------------------------------------------------------------

fn info(cfg_path: Option<&str>, name: &str) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let v = merged_voices(&cfg)
        .into_iter()
        .find(|v| v.spd_name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("voice '{name}' not found — see `voice search`"))?;

    println!("{}", v.spd_name);
    println!(
        "  name:        {} ({})",
        v.display_name,
        match v.source() {
            Source::Local => "offline / local model",
            Source::Cloud => "online / cloud engine",
        }
    );
    println!(
        "  engine:      {} (voice id {})",
        v.engine_id, v.engine_voice_id
    );
    println!(
        "  language:    {} ({} spoken)",
        v.language,
        v.languages.join(", ")
    );
    println!("  gender:      {}", v.gender);
    if !v.quality.is_empty() {
        println!("  quality:     {}", v.quality);
    }
    if v.multilingual {
        println!("  multilingual: yes");
    }
    if let Some(rate) = v.sample_rate {
        println!("  sample rate: {rate} Hz");
    }
    if v.num_speakers > 1 {
        println!(
            "  speakers:    {} (this is speaker {})",
            v.num_speakers, v.engine_voice_id
        );
    }
    if !v.model_type.is_empty() {
        println!("  model type:  {}", v.model_type);
    }
    if !v.license.is_empty() {
        println!("  license:     {}", v.license);
    }
    println!();
    println!(
        "  speak via speechd: spd-say -o voicegarden-spd -y {:?} \"…\"",
        v.spd_name
    );
    Ok(())
}

pub fn preview(name: &str, text: &str, cfg_path: Option<&str>) -> Result<(), String> {
    let cfg = ModuleConfig::load(cfg_path);
    let v = merged_voices(&cfg)
        .into_iter()
        .find(|v| v.spd_name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("voice '{name}' not found — see `voice search`"))?;
    let (path, player) = voicegarden_spd::preview::preview_wav(&v, text)?;
    match player {
        Some(cmd) => {
            let sh = format!("{cmd} >/dev/null 2>&1");
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(&sh)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                println!(
                    "audio written to {} (player exited non-zero)",
                    path.display()
                );
            }
        }
        None => {
            return Err(format!(
                "no audio player found (pw-play/paplay/aplay/ffplay); the WAV is at {}",
                path.display()
            ));
        }
    }
    Ok(())
}
