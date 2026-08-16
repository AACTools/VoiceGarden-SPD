//! Fake-server integration test: spawn `sd_voicegarden` and drive the
//! real speech-dispatcher module protocol over its stdin/stdout pipes.
//!
//! This exercises the actual `libspeechd_module` machinery (INIT
//! handshake, SET parsing, LIST VOICES, SPEAK flow, event replies) —
//! without a speechd daemon or any TTS engine (a placeholder model means
//! synthesis fails silently, but the protocol framing must stay valid).
//!
//! Lines are `\n`-terminated; multi-line data blocks end with `.`.
//! Reading is byte-oriented because 705 AUDIO events carry binary data.

use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Max time a single protocol line may take before we give up.
const LINE_TIMEOUT: Duration = Duration::from_secs(20);

struct Module {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Drop for Module {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(b"QUIT\n");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

impl Module {
    fn spawn(extra_env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sd_voicegarden"));
        // Default HOME first so extra_env entries can override it.
        let home = std::env::temp_dir().join(format!("vgspd-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        cmd.env("HOME", &home);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn sd_voicegarden");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        // Reader thread: raw lines in, channel out — every read becomes
        // interruptible by timeout, so protocol bugs fail instead of hang.
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            child,
            stdin,
            lines: rx,
        }
    }

    fn send(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn send_block(&mut self, data: &str) {
        // data already contains \n line endings and a trailing "."
        self.stdin.write_all(data.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    /// Read one raw `\n`-terminated line (bytes → lossy String).
    fn raw_line(&mut self) -> String {
        self.lines
            .recv_timeout(LINE_TIMEOUT)
            .expect("timed out waiting for a protocol line")
    }

    /// True when a line is a `NNN-` continuation (code position only —
    /// voice-list entries like `200-piper-nl-rdh-low#0\tnl\t` contain
    /// hyphens in the payload and must NOT be treated as continuations).
    fn is_continuation(l: &str) -> bool {
        let b = l.as_bytes();
        b.len() > 3
            && b[0].is_ascii_digit()
            && b[1].is_ascii_digit()
            && b[2].is_ascii_digit()
            && b[3] == b'-'
    }

    /// Read one logical reply: multi-part `-` continuation lines joined
    /// with `\n` (e.g. `399-msg` + `399 ERR …`).
    fn line(&mut self) -> String {
        let first = self.raw_line();
        if !Self::is_continuation(&first) {
            return first;
        }
        let mut out = first;
        loop {
            let next = self.raw_line();
            let terminal = !Self::is_continuation(&next);
            out.push('\n');
            out.push_str(&next);
            if terminal {
                return out;
            }
        }
    }

    /// Collect logical lines until one terminates with `terminator`
    /// (deadline-bound). A joined multi-part reply matches when its last
    /// segment equals the terminator.
    fn until(&mut self, terminator: &str) -> Vec<String> {
        let deadline = Instant::now() + LINE_TIMEOUT;
        let suffix = format!("\n{terminator}");
        let mut lines = Vec::new();
        loop {
            if Instant::now() > deadline {
                panic!("timed out waiting for {terminator:?}; got so far: {lines:?}");
            }
            let l = self.line();
            let done = l == terminator || l.ends_with(&suffix);
            lines.push(l);
            if done {
                return lines;
            }
        }
    }
}

fn init(mut m: Module) -> Module {
    m.send("INIT");
    // line() joins `299-…` continuation lines with the terminal line.
    let reply = m.line();
    assert!(reply.starts_with("299-"), "INIT reply: {reply}");
    assert_eq!(
        reply.split('\n').next_back().unwrap(),
        "299 OK LOADED SUCCESSFULLY"
    );
    m
}

#[test]
fn handshake_list_voices_quit() {
    let m = Module::spawn(&[]);
    let mut m = init(m);

    m.send("LIST VOICES");
    let reply = m.line();
    // Empty install → 304 CANT LIST VOICES.
    assert_eq!(reply, "304 CANT LIST VOICES", "LIST VOICES: {reply}");

    m.send("SET");
    m.send_block("rate=50\npitch=0\nvolume=-20\nlanguage=en\nsynthesis_voice=NULL\n.\n");
    assert_eq!(m.line(), "203 OK RECEIVING SETTINGS");
    assert_eq!(m.line(), "203 OK SETTINGS RECEIVED");

    m.send("SET");
    m.send_block("rate=999\n.\n");
    assert_eq!(m.line(), "203 OK RECEIVING SETTINGS");
    assert_eq!(m.line(), "303 ERROR INVALID PARAMETER OR VALUE");

    m.send("AUDIO");
    m.send_block("audio_output_method=server\n.\n");
    assert_eq!(m.line(), "207 OK RECEIVING AUDIO SETTINGS");
    assert_eq!(m.line(), "203 OK AUDIO INITIALIZED");

    m.send("BOGUS");
    assert_eq!(m.line(), "300 ERR UNKNOWN COMMAND");

    m.send("LOGLEVEL");
    m.send_block("log_level=3\n.\n");
    assert_eq!(m.line(), "207 OK RECEIVING LOGLEVEL SETTINGS");
    assert_eq!(m.line(), "203 OK LOGLEVEL SET");

    m.send("QUIT");
    assert_eq!(m.line(), "210 OK QUIT");
    let _ = m.child.wait();
}

#[test]
fn speak_without_voices_reports_cant_speak() {
    let m = Module::spawn(&[]);
    let mut m = init(m);
    m.send("SET");
    m.send_block("rate=0\npitch=0\nvolume=0\nlanguage=NULL\nsynthesis_voice=NULL\n.\n");
    m.line();
    m.line();
    m.send("SPEAK");
    assert_eq!(m.line(), "202 OK RECEIVING MESSAGE");
    m.send_block("hello world\n.\n");
    // No voice available → 301 ERROR CANT SPEAK.
    assert_eq!(m.line(), "301 ERROR CANT SPEAK");
}

#[test]
fn speak_with_stub_voice_streams_protocol_events() {
    // Install a placeholder model directory so exactly one sherpa voice
    // exists. Synthesis fails (no real onnx files) but the protocol must
    // produce: 200 OK SPEAKING → 701 BEGIN → 702 END.
    let home = std::env::temp_dir().join(format!("vgspd-speak-{}", std::process::id()));
    let models = home.join(".rust-tts-wrapper/sherpaonnx/piper-nl-rdh-low");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join("placeholder"), b"").unwrap();

    let m = Module::spawn(&[("HOME", home.to_str().unwrap())]);
    let mut m = init(m);

    m.send("LIST VOICES");
    let reply = m.until("200 OK VOICE LIST SENT");
    assert!(
        reply
            .iter()
            .any(|l| l.starts_with("200-piper-nl-rdh-low#0\t")),
        "voices: {reply:?}"
    );

    m.send("SET");
    m.send_block("rate=0\npitch=0\nvolume=0\nlanguage=en\nsynthesis_voice=piper-nl-rdh-low#0\n.\n");
    m.line();
    m.line();

    m.send("SPEAK");
    assert_eq!(m.line(), "202 OK RECEIVING MESSAGE");
    m.send_block("hello <mark name=\"m1\"/> world\n.\n");
    assert_eq!(m.line(), "200 OK SPEAKING");
    assert_eq!(m.line(), "701 BEGIN");
    assert_eq!(m.line(), "702 END");
}

#[test]
fn stop_is_acknowledged_between_utterances() {
    let home = std::env::temp_dir().join(format!("vgspd-stop-{}", std::process::id()));
    let models = home.join(".rust-tts-wrapper/sherpaonnx/piper-nl-rdh-low");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join("placeholder"), b"").unwrap();

    let m = Module::spawn(&[("HOME", home.to_str().unwrap())]);
    let mut m = init(m);
    m.send("SET");
    m.send_block("rate=0\npitch=0\nvolume=0\nlanguage=en\nsynthesis_voice=piper-nl-rdh-low#0\n.\n");
    m.line();
    m.line();

    m.send("SPEAK");
    m.line(); // 202
    m.send_block("first\n.\n");
    m.line(); // 200 OK SPEAKING
    m.line(); // 701 BEGIN
    m.line(); // 702 END

    // STOP outside of speech is simply accepted (module_stop returns 0,
    // the library has no STOP reply).
    m.send("STOP");
    m.send("QUIT");
    assert_eq!(m.line(), "210 OK QUIT");
}
