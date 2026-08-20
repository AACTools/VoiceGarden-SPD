//! `voicegarden-spd model …` — search the full sherpa-onnx registry
//! (all 1300+ models, including ones not installed).

use clap::Subcommand;
use sherpa_onnx_models::ModelInfo;

use crate::Style;

#[derive(Subcommand)]
pub enum ModelCmd {
    /// Find registry models by free text + filters; installed ones marked
    #[command(alias = "search")]
    Find {
        /// Terms matched against id / name / model type / language
        terms: Vec<String>,
        /// Quality tier: high | medium | low | x_low | unknown | …
        #[arg(long)]
        quality: Option<String>,
        /// Language code ("en", "nl", …)
        #[arg(long)]
        lang: Option<String>,
        /// Only multilingual models
        #[arg(long)]
        multilingual: bool,
        /// Show at most N results
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
}

pub fn run(cmd: ModelCmd) -> Result<(), String> {
    match cmd {
        ModelCmd::Find {
            terms,
            quality,
            lang,
            multilingual,
            limit,
        } => find(
            &terms,
            quality.as_deref(),
            lang.as_deref(),
            multilingual,
            limit,
        ),
    }
}

fn find(
    terms: &[String],
    quality: Option<&str>,
    lang: Option<&str>,
    multilingual: bool,
    limit: usize,
) -> Result<(), String> {
    let models: Vec<&ModelInfo> = sherpa_onnx_models::models()
        .values()
        .filter(|m: &&ModelInfo| {
            if let Some(q) = quality {
                if !m.quality.eq_ignore_ascii_case(q) {
                    return false;
                }
            }
            if let Some(want) = lang {
                let w = want.to_lowercase();
                if !m
                    .language
                    .iter()
                    .any(|l| l.lang_code.to_lowercase().starts_with(&w))
                {
                    return false;
                }
            }
            if multilingual && m.language.len() < 2 {
                return false;
            }
            for term in terms {
                let t = term.to_lowercase();
                let hay = format!(
                    "{} {} {} {}",
                    m.id,
                    m.name,
                    m.model_type,
                    m.language
                        .iter()
                        .map(|l| format!("{} {}", l.lang_code, l.language_name))
                        .collect::<String>()
                )
                .to_lowercase();
                if !hay.contains(&t) {
                    return false;
                }
            }
            true
        })
        .collect();

    // Installed models first, then by id for stable output.
    let home = std::env::var("HOME").unwrap_or_default();
    let primary = std::path::Path::new(&home).join(".local/share/voicegarden/sherpa-onnx-models");
    let legacy = std::path::Path::new(&home).join(".rust-tts-wrapper/sherpaonnx");
    let installed = |m: &ModelInfo| primary.join(&m.id).exists() || legacy.join(&m.id).exists();

    let mut sorted: Vec<&ModelInfo> = models;
    sorted.sort_by(|a, b| {
        installed(b)
            .cmp(&installed(a))
            .then_with(|| a.id.cmp(&b.id))
    });

    let st = Style::new();
    let total = sorted.len();
    let shown: Vec<Vec<String>> = sorted
        .iter()
        .take(limit)
        .map(|m| {
            let langs = if m.language.len() > 3 {
                format!("{}…", m.language.len())
            } else {
                m.language
                    .iter()
                    .map(|l| l.lang_code.clone())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            // fp16 archives abort in the CPU-only ONNX runtime this build
            // links (uncatchable foreign exception) — flag them loudly.
            let quality = if m.url.contains("fp16") {
                st.yellow(&format!("{} ⚠fp16", m.quality))
            } else {
                m.quality.clone()
            };
            vec![
                if installed(m) {
                    st.green("●").to_string()
                } else {
                    " ".into()
                },
                m.id.clone(),
                m.model_type.clone(),
                quality,
                langs,
                format!("{:.0} MB", m.filesize_mb),
                st.dim(&m.license),
            ]
        })
        .collect();
    println!(
        "{}",
        crate::render_table(
            &["", "MODEL", "TYPE", "QUALITY", "LANGS", "SIZE", "LICENSE"],
            &shown
        )
    );
    println!(
        "{} {total} model(s) match (showing up to {limit}; ● = installed)",
        st.dim("→")
    );
    if total > 0 && !sorted.iter().take(limit).all(|m| installed(m)) {
        println!(
            "{}",
            st.dim(&format!(
                "install: download the archive, extract into {}/<model-id>/ — voices appear\n  after a speech-dispatcher restart",
                primary.display()
            ))
        );
        // print the first non-installed match's URL as a concrete example
        if let Some(m) = sorted.iter().find(|m| !installed(m)) {
            println!("{}", st.dim(&format!("example: {}", m.url)));
        }
    }
    if sorted.iter().take(limit).any(|m| m.url.contains("fp16")) {
        println!(
            "{}",
            st.yellow(
                "⚠ fp16 archives do not load in the CPU ONNX runtime this build links — pick a\n  non-fp16 variant of the model instead"
            )
        );
    }
    Ok(())
}
