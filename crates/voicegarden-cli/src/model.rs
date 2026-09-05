//! `voicegarden-spd model …` — search the full sherpa-onnx registry
//! (all 1300+ models, including ones not installed), and install models.

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
    /// Download + install a model from the registry
    #[command(alias = "download")]
    Install {
        /// Model id from `model find` (e.g. "coqui-en-ljspeech")
        model_id: String,
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
        ModelCmd::Install { model_id } => install(&model_id),
    }
}

fn install(model_id: &str) -> Result<(), String> {
    let st = Style::new();

    let model = sherpa_onnx_models::models()
        .values()
        .find(|m: &&ModelInfo| m.id == model_id)
        .ok_or_else(|| {
            format!(
                "model '{model_id}' not found in registry. Use `model find {model_id}` to search."
            )
        })?;

    if model.url.contains("fp16") {
        eprintln!(
            "{}",
            st.yellow(
                "⚠ fp16 archives do not load in the CPU ONNX runtime this build links — pick a\n  non-fp16 variant of the model instead"
            )
        );
        return Err("aborting installation of fp16 model".into());
    }

    // Check model type compatibility with the floravox engine.
    // floravox drives piper-family models (vits, mms, matcha, kokoro).
    // Audio-LM families (kitten, pocket, supertonic, zipvoice) were
    // removed in v0.4.1.
    let supported = ["vits", "mms", "matcha", "kokoro"];
    if !supported.contains(&model.model_type.as_str()) {
        eprintln!(
            "{}",
            st.yellow(&format!(
                "⚠ model type '{}' may not be supported by the floravox engine.\n  Supported types: {}. Install anyway?",
                model.model_type,
                supported.join(", ")
            ))
        );
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let models_dir =
        std::path::Path::new(&home).join(".local/share/voicegarden/sherpa-onnx-models");
    let target = models_dir.join(model_id);

    if target.exists() {
        eprintln!(
            "model '{model_id}' is already installed at {}",
            target.display()
        );
        // Re-generate .onnx.json in case the generator improved
        // (e.g. language code, hop_length, inference params).
        if let Err(e) = generate_sidecar(&target, model_id, model) {
            eprintln!("  ⚠ could not update sidecar: {e}");
        }
        return Ok(());
    }

    std::fs::create_dir_all(&target)
        .map_err(|e| format!("could not create {target}: {e}", target = target.display()))?;

    // If a durations_url exists, download the already-patched ONNX directly.
    // This is much faster than downloading the full archive and patching.
    if let Some(ref dur_url) = model.durations_url {
        eprintln!("{}Downloading patched ONNX for {model_id}…", st.dim("↓ "));
        let status = std::process::Command::new("curl")
            .args([
                "-fsSL",
                "-o",
                &target.join("model.onnx").to_string_lossy(),
                dur_url,
            ])
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| format!("could not run curl: {e}"))?;
        if !status.success() {
            return Err("download failed".into());
        }
        eprintln!(
            "{}Installed {model_id} to {}\n  Restart speech-dispatcher, then: spd-say -o voicegarden-spd -y \"{voice}\" -e 'Hello'",
            st.green("✓ "),
            target.display(),
            voice = model_id.split('-').next().unwrap_or(model_id)
        );
        return Ok(());
    }

    let url = &model.url;
    let size_mb = model.filesize_mb;
    eprintln!(
        "{}Downloading {model_id} ({size_mb:.0} MB) from {url}…",
        st.dim("↓ ")
    );

    // Pipe curl -> tar for streaming download + extract
    let curl = std::process::Command::new("curl")
        .args(["-fsSL", url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;

    // Auto-detect compression from URL extension
    let tar_flags = if url.ends_with(".bz2") {
        "-xjf"
    } else if url.ends_with(".xz") {
        "-xJf"
    } else {
        "-xzf"
    };
    let tar = std::process::Command::new("tar")
        .args([tar_flags, "-", "-C", &target.to_string_lossy()])
        .stdin(curl.stdout.unwrap())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("could not run tar: {e}"))?;

    let output = tar
        .wait_with_output()
        .map_err(|e| format!("tar failed: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&target);
        return Err(format!("extraction failed (tar exit {})", output.status));
    }

    // Flatten single nested directory (sherpa-onnx archives often
    // contain a <model-id>/ subdirectory).
    let entries: Vec<_> = std::fs::read_dir(&target)
        .map_err(|e| format!("read dir: {e}"))?
        .filter_map(|e| e.ok())
        .collect();
    if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
        let nested = entries[0].path();
        for entry in std::fs::read_dir(&nested).map_err(|e| format!("read nested: {e}"))? {
            let entry = entry.map_err(|e| format!("entry: {e}"))?;
            let name = entry.file_name();
            let dest = target.join(&name);
            // If dest exists, remove it first
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::remove_dir_all(&dest);
            std::fs::rename(entry.path(), &dest).map_err(|e| format!("rename {name:?}: {e}"))?;
        }
        std::fs::remove_dir(&nested).ok();
    }

    // Generate a minimal .onnx.json if the model has a config.json
    // (vits/piper models). The floravox engine requires this for
    // voice discovery — it provides language + audio metadata.
    let _config_path = target.join("config.json");
    let _onnx_path = find_onnx_in_dir(&target);

    // For matcha models, download the standard hifigan vocoder.
    // The sherpa-onnx matcha archives don't bundle a vocoder; one
    // must be downloaded separately. `find_vocoder` in floravox-core
    // discovers it by name in the model directory.
    if model.model_type == "matcha" {
        let vocoder_name = "hifigan_v2.onnx";
        let vocoder_path = target.join(vocoder_name);
        if !vocoder_path.exists() {
            let vocoder_url = format!(
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/vocoder/{vocoder_name}"
            );
            eprintln!(
                "{}Downloading vocoder {vocoder_name} from {vocoder_url}…",
                st.dim("↓ ")
            );
            let status = std::process::Command::new("curl")
                .args(["-fsSL", "-o", &vocoder_path.to_string_lossy(), &vocoder_url])
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .map_err(|e| format!("could not run curl: {e}"))?;
            if !status.success() {
                eprintln!(
                    "{}  Matcha models need a hifigan/vocos vocoder.\n  Place one at {} to enable this voice.",
                    st.yellow("⚠ "),
                    vocoder_path.display()
                );
            }
        }
    }

    fn generate_sidecar(
        target: &std::path::Path,
        model_id: &str,
        model: &sherpa_onnx_models::ModelInfo,
    ) -> Result<(), String> {
        let config_path = target.join("config.json");
        let onnx_path = find_onnx_in_dir(target);
        if !config_path.exists() || onnx_path.is_none() {
            return Ok(());
        }
        let config =
            std::fs::read_to_string(&config_path).map_err(|e| format!("read config: {e}"))?;
        let cfg: serde_json::Value =
            serde_json::from_str(&config).map_err(|e| format!("parse config: {e}"))?;
        let onnx_path = onnx_path.unwrap();
        let json_path = onnx_path.with_extension("onnx.json");
        if json_path.exists() {
            return Ok(());
        }

        // Read tokens.txt, case-fold to lowercase, and write as phoneme_id_map.
        // This is critical for character-based models (Coqui) whose tokens.txt
        // uses uppercase letters — the CharFrontend lowercases input, so the
        // map must be lowercase too.
        let tokens_path = target.join("tokens.txt");
        let phoneme_id_map = tokens_txt_map_casefold(&tokens_path);
        let sample_rate = cfg
            .pointer("/audio/sample_rate")
            .and_then(|v| v.as_u64())
            .unwrap_or(22050);
        let hop_length = cfg.pointer("/audio/hop_length").and_then(|v| v.as_u64());
        let lang_code = model
            .language
            .first()
            .map(|l| l.lang_code.split('-').next().unwrap_or("en"))
            .unwrap_or_else(|| model_id.split('-').next().unwrap_or("en"));
        let noise_scale = cfg
            .pointer("/inference/noise_scale")
            .or_else(|| cfg.pointer("/noise_scale"))
            .or_else(|| cfg.pointer("/model_args/noise_scale"))
            .and_then(|v| v.as_f64());
        let length_scale = cfg
            .pointer("/inference/length_scale")
            .or_else(|| cfg.pointer("/length_scale"))
            .or_else(|| cfg.pointer("/model_args/length_scale"))
            .and_then(|v| v.as_f64());
        let mut minimal = serde_json::json!({
            "audio": {"sample_rate": sample_rate},
            "espeak": {"voice": lang_code},
            "dataset": "",
        });
        if let Some(hl) = hop_length {
            minimal["audio"]["hop_length"] = serde_json::json!(hl);
        }
        if noise_scale.is_some() || length_scale.is_some() {
            let mut inf = serde_json::json!({});
            if let Some(ns) = noise_scale {
                inf["noise_scale"] = serde_json::json!(ns);
            }
            if let Some(ls) = length_scale {
                inf["length_scale"] = serde_json::json!(ls);
            }
            minimal["inference"] = inf;
        }
        if let Some(map) = phoneme_id_map {
            minimal["phoneme_id_map"] = map;
        }
        std::fs::write(&json_path, serde_json::to_string_pretty(&minimal).unwrap())
            .map_err(|e| format!("write sidecar: {e}"))?;
        eprintln!(
            "  generated minimal {}",
            json_path.file_name().unwrap_or_default().to_string_lossy()
        );
        Ok(())
    }

    // After download + flatten, generate the sidecar
    if let Err(e) = generate_sidecar(&target, model_id, model) {
        eprintln!("  ⚠ sidecar: {e}");
    }

    eprintln!(
        "{}Installed {model_id} to {}\n  Restart speech-dispatcher, then: spd-say -o voicegarden-spd -e 'Hello'",
        st.green("✓ "),
        target.display()
    );
    Ok(())
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
                "install: `voicegarden-spd model install <model-id>` or download + extract into {}/<model-id>/",
                primary.display()
            ))
        );
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

/// Read tokens.txt and case-fold the keys to lowercase.
/// This is needed because the floravox engine's CharFrontend lowercases
/// input text, but some models (Coqui) have uppercase tokens.
fn tokens_txt_map_casefold(path: &std::path::Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut map: std::collections::BTreeMap<String, Vec<i64>> = std::collections::BTreeMap::new();
    for line in text.lines() {
        if let Some(idx) = line.rfind(' ') {
            let (sym, id) = (line[..idx].trim_end_matches('\r'), &line[idx + 1..]);
            if let Ok(id) = id.parse::<i64>() {
                let sym = sym.to_lowercase();
                if !sym.is_empty() {
                    map.entry(sym).or_default().push(id);
                }
            }
        }
    }
    if map.is_empty() {
        return None;
    }
    serde_json::to_value(map).ok()
}

/// Find the first .onnx file in a directory (non-recursive).
fn find_onnx_in_dir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("onnx") {
            return Some(path);
        }
        if path.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&path) {
                for sub_entry in sub.filter_map(Result::ok) {
                    let sub_path = sub_entry.path();
                    if sub_path.extension().and_then(|e| e.to_str()) == Some("onnx") {
                        return Some(sub_path);
                    }
                }
            }
        }
    }
    None
}
