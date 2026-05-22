use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, SecondsFormat};
use clap::{ArgAction, Parser};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use include_dir::{Dir, include_dir};
use rand::RngExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{self, BufRead, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static PAIN_AUDIO: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/audio/pain");
static SEXY_AUDIO: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/audio/sexy");
static HALO_AUDIO: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/audio/halo");
static LIZARD_AUDIO: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/audio/lizard");

const DECAY_HALF_LIFE: f64 = 30.0;
const DEFAULT_MIN_AMPLITUDE: f64 = 0.05;
const DEFAULT_COOLDOWN_MS: u64 = 750;
const DEFAULT_SPEED_RATIO: f64 = 1.0;
const DEFAULT_SENSOR_POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEFAULT_MAX_SAMPLE_BATCH: usize = 200;
const SENSOR_STARTUP_DELAY: Duration = Duration::from_millis(100);
const MAX_LOG_LINES: usize = 200;
const MIN_AMPLITUDE_STEP: f64 = 0.01;
const COOLDOWN_STEP_MS: u64 = 50;
const MIN_COOLDOWN_MS: u64 = 50;
const SPEED_STEP: f64 = 0.05;
const MIN_SPEED_RATIO: f64 = 0.25;
const MAX_SPEED_RATIO: f64 = 4.0;

type SharedLog = Arc<Mutex<VecDeque<String>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TuningPreset {
    Default,
    Fast,
}

impl TuningPreset {
    fn from_cli(cli: &Cli) -> Self {
        if cli.fast { Self::Fast } else { Self::Default }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Fast => "fast",
        }
    }

    fn runtime(self) -> RuntimeTuning {
        match self {
            Self::Default => RuntimeTuning::default(),
            Self::Fast => RuntimeTuning::default().apply_fast_overlay(),
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::Default => Self::Fast,
            Self::Fast => Self::Default,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TuiControl {
    Pause,
    SoundPack,
    TuningPreset,
    MinAmplitude,
    Cooldown,
    Speed,
    VolumeScaling,
    CustomSource,
}

const TUI_CONTROLS: [TuiControl; 8] = [
    TuiControl::Pause,
    TuiControl::SoundPack,
    TuiControl::TuningPreset,
    TuiControl::MinAmplitude,
    TuiControl::Cooldown,
    TuiControl::Speed,
    TuiControl::VolumeScaling,
    TuiControl::CustomSource,
];

impl TuiControl {
    fn label(self) -> &'static str {
        match self {
            Self::Pause => "Pause",
            Self::SoundPack => "Sound pack",
            Self::TuningPreset => "Tuning preset",
            Self::MinAmplitude => "Min amplitude",
            Self::Cooldown => "Cooldown",
            Self::Speed => "Speed",
            Self::VolumeScaling => "Volume scaling",
            Self::CustomSource => "Custom source",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "spank",
    version,
    about = "TUI that yells 'ow!' when you slap the laptop",
    long_about = "spank reads the Apple Silicon accelerometer directly via IOKit HID, opens a terminal UI, and plays audio responses when a slap or hit is detected.\n\nRequires sudo for IOKit HID access to the accelerometer. Use --stdio for JSON stdin/stdout integration mode."
)]
struct Cli {
    #[arg(
        short = 's',
        long,
        action = ArgAction::SetTrue,
        help = "Start the TUI or stdio mode with the sexy sound pack"
    )]
    sexy: bool,

    #[arg(
        short = 'H',
        long,
        action = ArgAction::SetTrue,
        help = "Start the TUI or stdio mode with the Halo sound pack"
    )]
    halo: bool,

    #[arg(
        short = 'l',
        long,
        action = ArgAction::SetTrue,
        help = "Start the TUI or stdio mode with the lizard sound pack"
    )]
    lizard: bool,

    #[arg(
        short = 'c',
        long,
        value_name = "DIR",
        help = "Start with a custom MP3 directory; editable in the TUI"
    )]
    custom: Option<PathBuf>,

    #[arg(
        long,
        value_name = "FILE",
        value_delimiter = ',',
        help = "Start with comma-separated custom MP3 files; editable in the TUI"
    )]
    custom_files: Vec<PathBuf>,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Start with fast tuning; toggleable in the TUI"
    )]
    fast: bool,

    #[arg(
        long,
        value_name = "FLOAT",
        help = "Initial minimum amplitude threshold (default: 0.05; adjustable in the TUI)"
    )]
    min_amplitude: Option<f64>,

    #[arg(
        long,
        value_name = "MS",
        help = "Initial cooldown between responses in milliseconds (default: 750; adjustable in the TUI)"
    )]
    cooldown: Option<u64>,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Use JSON stdin/stdout mode instead of the interactive TUI"
    )]
    stdio: bool,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Start with volume scaling enabled; toggleable in the TUI"
    )]
    volume_scaling: bool,

    #[arg(
        long,
        value_name = "RATIO",
        help = "Initial playback speed multiplier (default: 1.0; adjustable in the TUI)"
    )]
    speed: Option<f64>,
}

#[derive(Clone, Copy)]
struct RuntimeTuning {
    min_amplitude: f64,
    cooldown: Duration,
    poll_interval: Duration,
    max_batch: usize,
}

impl RuntimeTuning {
    fn default() -> Self {
        Self {
            min_amplitude: DEFAULT_MIN_AMPLITUDE,
            cooldown: Duration::from_millis(DEFAULT_COOLDOWN_MS),
            poll_interval: DEFAULT_SENSOR_POLL_INTERVAL,
            max_batch: DEFAULT_MAX_SAMPLE_BATCH,
        }
    }

    fn apply_fast_overlay(mut self) -> Self {
        self.poll_interval = Duration::from_millis(4);
        self.cooldown = Duration::from_millis(350);
        if self.min_amplitude > 0.18 {
            self.min_amplitude = 0.18;
        }
        if self.max_batch < 320 {
            self.max_batch = 320;
        }
        self
    }
}

#[derive(Clone)]
struct Settings {
    paused: bool,
    min_amplitude: f64,
    cooldown_ms: u64,
    stdio_mode: bool,
    volume_scaling: bool,
    speed_ratio: f64,
}

impl Settings {
    fn new(tuning: RuntimeTuning, cli: &Cli) -> Self {
        Self {
            paused: false,
            min_amplitude: tuning.min_amplitude,
            cooldown_ms: tuning.cooldown.as_millis() as u64,
            stdio_mode: cli.stdio,
            volume_scaling: cli.volume_scaling,
            speed_ratio: cli.speed.unwrap_or(DEFAULT_SPEED_RATIO),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlayMode {
    Random,
    Escalation,
}

#[derive(Clone)]
enum AudioFile {
    Embedded {
        display: String,
        bytes: &'static [u8],
    },
    Custom {
        path: PathBuf,
    },
}

impl AudioFile {
    fn display_path(&self) -> String {
        match self {
            Self::Embedded { display, .. } => display.clone(),
            Self::Custom { path } => path.display().to_string(),
        }
    }

    fn read_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Embedded { bytes, .. } => Ok(bytes.to_vec()),
            Self::Custom { path } => {
                fs::read(path).with_context(|| format!("reading {}", path.display()))
            }
        }
    }
}

struct SoundPack {
    name: &'static str,
    mode: PlayMode,
    files: Vec<AudioFile>,
}

impl SoundPack {
    fn embedded(
        name: &'static str,
        prefix: &'static str,
        dir: &'static Dir<'static>,
        mode: PlayMode,
    ) -> Result<Self> {
        let mut files = dir
            .files()
            .filter(|file| is_mp3(file.path()))
            .map(|file| AudioFile::Embedded {
                display: format!("{}/{}", prefix, file.path().display()),
                bytes: file.contents(),
            })
            .collect::<Vec<_>>();

        files.sort_by_key(AudioFile::display_path);
        if files.is_empty() {
            bail!("no audio files found in {prefix}");
        }

        Ok(Self { name, mode, files })
    }

    fn custom_dir(path: &Path) -> Result<Self> {
        let entries = fs::read_dir(path).with_context(|| format!("reading {}", path.display()))?;
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_file() && is_mp3(&entry.path()) {
                files.push(AudioFile::Custom { path: entry.path() });
            }
        }

        files.sort_by_key(AudioFile::display_path);
        if files.is_empty() {
            bail!("no MP3 audio files found in {}", path.display());
        }

        Ok(Self {
            name: "custom",
            mode: PlayMode::Random,
            files,
        })
    }

    fn custom_files(paths: &[PathBuf]) -> Result<Self> {
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            if !is_mp3(path) {
                bail!("custom file must be MP3: {}", path.display());
            }
            if !path.is_file() {
                bail!("custom file not found: {}", path.display());
            }
            files.push(AudioFile::Custom { path: path.clone() });
        }

        if files.is_empty() {
            bail!("no custom audio files provided");
        }

        files.sort_by_key(AudioFile::display_path);
        Ok(Self {
            name: "custom",
            mode: PlayMode::Random,
            files,
        })
    }
}

struct SlapTracker {
    score: f64,
    last_time: Option<SystemTime>,
    total: u64,
    half_life: f64,
    scale: f64,
}

impl SlapTracker {
    fn new(file_count: usize, cooldown: Duration) -> Self {
        Self::new_with_total(file_count, cooldown, 0)
    }

    fn new_with_total(file_count: usize, cooldown: Duration, total: u64) -> Self {
        let cooldown_secs = cooldown.as_secs_f64();
        let steady_state_max = 1.0 / (1.0 - 0.5_f64.powf(cooldown_secs / DECAY_HALF_LIFE));
        let scale = (steady_state_max - 1.0) / ((file_count as f64 + 1.0).ln());
        Self {
            score: 0.0,
            last_time: None,
            total,
            half_life: DECAY_HALF_LIFE,
            scale,
        }
    }

    fn record(&mut self, now: SystemTime) -> (u64, f64) {
        if let Some(last) = self.last_time
            && let Ok(elapsed) = now.duration_since(last)
        {
            self.score *= 0.5_f64.powf(elapsed.as_secs_f64() / self.half_life);
        }

        self.score += 1.0;
        self.last_time = Some(now);
        self.total += 1;
        (self.total, self.score)
    }

    fn file_index(&self, mode: PlayMode, file_count: usize, score: f64) -> usize {
        if mode == PlayMode::Random {
            return rand::rng().random_range(0..file_count);
        }

        let max_idx = file_count - 1;
        let idx = (file_count as f64 * (1.0 - (-(score - 1.0) / self.scale).exp())) as usize;
        idx.min(max_idx)
    }
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!("spank: {err:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let mut tuning = RuntimeTuning::default();
    if cli.fast {
        tuning = tuning.apply_fast_overlay();
    }
    if let Some(min_amplitude) = cli.min_amplitude {
        tuning.min_amplitude = min_amplitude;
    }
    if let Some(cooldown_ms) = cli.cooldown {
        tuning.cooldown = Duration::from_millis(cooldown_ms);
    }

    run(cli, tuning)
}

fn run(cli: Cli, tuning: RuntimeTuning) -> Result<()> {
    sensor::ensure_supported()?;

    if !is_root() {
        bail!("spank requires root privileges for accelerometer access, run with: sudo spank");
    }

    validate_cli(&cli, tuning)?;
    let settings = Arc::new(RwLock::new(Settings::new(tuning, &cli)));
    let mut sensor = sensor::SensorRuntime::start()?;

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = Arc::clone(&running);
        ctrlc::set_handler(move || {
            running.store(false, Ordering::SeqCst);
        })
        .context("installing signal handler")?;
    }

    if cli.stdio {
        let settings = Arc::clone(&settings);
        thread::spawn(move || {
            let stdin = io::stdin();
            let stdout = io::stdout();
            let mut out = stdout.lock();
            if let Err(err) = process_commands(stdin.lock(), &mut out, settings) {
                eprintln!("spank: stdin command reader failed: {err}");
            }
        });
    }

    thread::sleep(SENSOR_STARTUP_DELAY);
    if cli.stdio {
        let pack = select_sound_pack(&cli)?;
        listen_for_slaps_stdio(&mut sensor, &pack, tuning, settings, running, cli.fast)
    } else {
        run_tui(&mut sensor, &cli, tuning, settings, running)
    }
}

fn validate_cli(cli: &Cli, tuning: RuntimeTuning) -> Result<()> {
    let mut mode_count = 0;
    if cli.sexy {
        mode_count += 1;
    }
    if cli.halo {
        mode_count += 1;
    }
    if cli.lizard {
        mode_count += 1;
    }
    if cli.custom.is_some() || !cli.custom_files.is_empty() {
        mode_count += 1;
    }
    if mode_count > 1 {
        bail!(
            "--sexy, --halo, --lizard, and --custom/--custom-files are mutually exclusive; pick one"
        );
    }
    if !(0.0..=1.0).contains(&tuning.min_amplitude) {
        bail!("--min-amplitude must be between 0.0 and 1.0");
    }
    if tuning.cooldown.is_zero() {
        bail!("--cooldown must be greater than 0");
    }
    if cli.speed.unwrap_or(DEFAULT_SPEED_RATIO) <= 0.0 {
        bail!("--speed must be greater than 0");
    }
    Ok(())
}

fn select_sound_pack(cli: &Cli) -> Result<SoundPack> {
    if !cli.custom_files.is_empty() {
        return SoundPack::custom_files(&cli.custom_files);
    }
    if let Some(path) = &cli.custom {
        return SoundPack::custom_dir(path);
    }
    if cli.sexy {
        return SoundPack::embedded("sexy", "audio/sexy", &SEXY_AUDIO, PlayMode::Escalation);
    }
    if cli.halo {
        return SoundPack::embedded("halo", "audio/halo", &HALO_AUDIO, PlayMode::Random);
    }
    if cli.lizard {
        return SoundPack::embedded(
            "lizard",
            "audio/lizard",
            &LIZARD_AUDIO,
            PlayMode::Escalation,
        );
    }
    SoundPack::embedded("pain", "audio/pain", &PAIN_AUDIO, PlayMode::Random)
}

fn built_in_sound_packs() -> Result<Vec<SoundPack>> {
    Ok(vec![
        SoundPack::embedded("pain", "audio/pain", &PAIN_AUDIO, PlayMode::Random)?,
        SoundPack::embedded("sexy", "audio/sexy", &SEXY_AUDIO, PlayMode::Escalation)?,
        SoundPack::embedded("halo", "audio/halo", &HALO_AUDIO, PlayMode::Random)?,
        SoundPack::embedded(
            "lizard",
            "audio/lizard",
            &LIZARD_AUDIO,
            PlayMode::Escalation,
        )?,
    ])
}

fn initial_tui_sound_packs(cli: &Cli) -> Result<(Vec<SoundPack>, usize)> {
    let mut packs = built_in_sound_packs()?;
    if !cli.custom_files.is_empty() {
        let custom = SoundPack::custom_files(&cli.custom_files)?;
        let idx = upsert_custom_pack(&mut packs, custom);
        return Ok((packs, idx));
    }
    if let Some(path) = &cli.custom {
        let custom = SoundPack::custom_dir(path)?;
        let idx = upsert_custom_pack(&mut packs, custom);
        return Ok((packs, idx));
    }

    let selected = if cli.sexy {
        "sexy"
    } else if cli.halo {
        "halo"
    } else if cli.lizard {
        "lizard"
    } else {
        "pain"
    };
    let idx = packs
        .iter()
        .position(|pack| pack.name == selected)
        .unwrap_or_default();
    Ok((packs, idx))
}

fn upsert_custom_pack(packs: &mut Vec<SoundPack>, pack: SoundPack) -> usize {
    if let Some(idx) = packs.iter().position(|existing| existing.name == "custom") {
        packs[idx] = pack;
        idx
    } else {
        packs.push(pack);
        packs.len() - 1
    }
}

fn load_custom_source(source: &str) -> Result<SoundPack> {
    let source = source.trim();
    if source.is_empty() {
        bail!("custom source cannot be empty");
    }

    let paths = source
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    if paths.len() > 1 {
        return SoundPack::custom_files(&paths);
    }

    let path = PathBuf::from(source);
    if path.is_dir() {
        SoundPack::custom_dir(&path)
    } else {
        SoundPack::custom_files(&[path])
    }
}

#[derive(Clone)]
struct SlapEvent {
    timestamp: DateTime<Local>,
    slap_number: u64,
    amplitude: f64,
    severity: String,
    file: String,
}

struct SlapOutcome {
    event: SlapEvent,
    file: AudioFile,
}

struct SlapEngine {
    tracker: SlapTracker,
    detector: Detector,
    last_event_time: f64,
    last_yell: Option<SystemTime>,
}

impl SlapEngine {
    fn new(pack: &SoundPack, tuning: RuntimeTuning) -> Self {
        Self {
            tracker: SlapTracker::new(pack.files.len(), tuning.cooldown),
            detector: Detector::new(),
            last_event_time: 0.0,
            last_yell: None,
        }
    }

    fn reconfigure_tracker(&mut self, pack: &SoundPack, cooldown_ms: u64) {
        let total = self.tracker.total;
        self.tracker = SlapTracker::new_with_total(
            pack.files.len(),
            Duration::from_millis(cooldown_ms),
            total,
        );
    }

    fn poll(
        &mut self,
        sensor: &mut sensor::SensorRuntime,
        pack: &SoundPack,
        tuning: RuntimeTuning,
        settings: &Settings,
    ) -> Option<SlapOutcome> {
        let samples = sensor.poll(tuning.poll_interval);
        if settings.paused || samples.is_empty() {
            return None;
        }

        let mut samples = samples;
        if samples.len() > tuning.max_batch {
            samples = samples.split_off(samples.len() - tuning.max_batch);
        }

        let now_secs = unix_now_secs();
        let n_samples = samples.len();
        for (idx, sample) in samples.iter().enumerate() {
            let t_sample = now_secs - (n_samples - idx - 1) as f64 / self.detector.fs as f64;
            self.detector
                .process(sample.x, sample.y, sample.z, t_sample);
        }

        let event = self.detector.events.last().cloned()?;
        if (event.time - self.last_event_time).abs() < f64::EPSILON {
            return None;
        }
        self.last_event_time = event.time;

        let now = SystemTime::now();
        if let Some(last) = self.last_yell {
            let elapsed = now.duration_since(last).unwrap_or_default();
            if elapsed <= Duration::from_millis(settings.cooldown_ms) {
                return None;
            }
        }
        if event.amplitude < settings.min_amplitude {
            return None;
        }

        self.last_yell = Some(now);
        let (num, score) = self.tracker.record(now);
        let idx = self.tracker.file_index(pack.mode, pack.files.len(), score);
        let file = pack.files[idx].clone();
        let display_path = file.display_path();

        Some(SlapOutcome {
            event: SlapEvent {
                timestamp: Local::now(),
                slap_number: num,
                amplitude: event.amplitude,
                severity: event.severity.to_string(),
                file: display_path,
            },
            file,
        })
    }
}

fn listen_for_slaps_stdio(
    sensor: &mut sensor::SensorRuntime,
    pack: &SoundPack,
    tuning: RuntimeTuning,
    settings: Arc<RwLock<Settings>>,
    running: Arc<AtomicBool>,
    fast_mode: bool,
) -> Result<()> {
    let mut engine = SlapEngine::new(pack, tuning);
    let preset_label = if fast_mode { "fast" } else { "default" };

    println!(
        "spank: listening for slaps in {} mode with {} tuning... (ctrl+c to quit)",
        pack.name, preset_label
    );
    if settings.read().expect("settings lock poisoned").stdio_mode {
        println!("{}", json!({ "status": "ready" }));
    }

    while running.load(Ordering::SeqCst) {
        let settings_snapshot = settings.read().expect("settings lock poisoned").clone();
        if let Some(outcome) = engine.poll(sensor, pack, tuning, &settings_snapshot) {
            println!(
                "{}",
                json!({
                    "timestamp": outcome.event.timestamp.to_rfc3339_opts(SecondsFormat::Nanos, true),
                    "slapNumber": outcome.event.slap_number,
                    "amplitude": outcome.event.amplitude,
                    "severity": outcome.event.severity,
                    "file": outcome.event.file,
                })
            );
            spawn_audio(
                outcome.file,
                outcome.event.amplitude,
                Arc::clone(&settings),
                None,
            );
        };
    }

    println!("\nbye!");
    Ok(())
}

struct TuiApp {
    started_at: DateTime<Local>,
    total_slaps: u64,
    last_event: Option<SlapEvent>,
    log: VecDeque<String>,
    selected_control: usize,
    custom_source: Option<String>,
    custom_input: Option<String>,
}

impl TuiApp {
    fn new(pack: &SoundPack, preset: TuningPreset, custom_source: Option<String>) -> Self {
        let mut log = VecDeque::new();
        push_log(
            &mut log,
            format!(
                "listening in {} mode with {} tuning",
                pack.name,
                preset.label()
            ),
        );
        Self {
            started_at: Local::now(),
            total_slaps: 0,
            last_event: None,
            log,
            selected_control: 0,
            custom_source,
            custom_input: None,
        }
    }

    fn record_slap(&mut self, event: SlapEvent) {
        self.total_slaps = event.slap_number;
        push_log(
            &mut self.log,
            format!(
                "{} slap #{} [{} amp={:.5}g] -> {}",
                event.timestamp.format("%H:%M:%S"),
                event.slap_number,
                event.severity,
                event.amplitude,
                event.file
            ),
        );
        self.last_event = Some(event);
    }

    fn drain_shared_log(&mut self, shared: &SharedLog) {
        let mut shared = shared.lock().expect("audio log lock poisoned");
        while let Some(line) = shared.pop_front() {
            push_log(&mut self.log, line);
        }
    }

    fn selected_control(&self) -> TuiControl {
        TUI_CONTROLS[self.selected_control]
    }

    fn select_next_control(&mut self) {
        self.selected_control = (self.selected_control + 1) % TUI_CONTROLS.len();
    }

    fn select_previous_control(&mut self) {
        self.selected_control =
            (self.selected_control + TUI_CONTROLS.len() - 1) % TUI_CONTROLS.len();
    }

    fn log_action(&mut self, message: impl Into<String>) {
        let message = message.into();
        push_log(
            &mut self.log,
            format!("{} {}", Local::now().format("%H:%M:%S"), message),
        );
    }
}

struct TuiSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TuiSession {
    fn start() -> Result<Self> {
        enable_raw_mode().context("enabling raw terminal mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).context("entering terminal UI")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("creating terminal UI")?;
        Ok(Self { terminal })
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn run_tui(
    sensor: &mut sensor::SensorRuntime,
    cli: &Cli,
    mut tuning: RuntimeTuning,
    settings: Arc<RwLock<Settings>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let mut preset = TuningPreset::from_cli(cli);
    let (mut packs, mut active_pack_idx) = initial_tui_sound_packs(cli)?;
    let custom_source = initial_custom_source(cli);
    let mut engine = SlapEngine::new(&packs[active_pack_idx], tuning);
    let mut app = TuiApp::new(&packs[active_pack_idx], preset, custom_source);
    let audio_log = Arc::new(Mutex::new(VecDeque::new()));
    let mut tui = TuiSession::start()?;

    while running.load(Ordering::SeqCst) {
        handle_tui_input(
            &mut app,
            &mut packs,
            &mut active_pack_idx,
            &mut preset,
            &mut tuning,
            &settings,
            &mut engine,
            &running,
        )
        .context("reading terminal input")?;

        let settings_snapshot = settings.read().expect("settings lock poisoned").clone();
        let active_pack = &packs[active_pack_idx];
        if let Some(outcome) = engine.poll(sensor, active_pack, tuning, &settings_snapshot) {
            app.record_slap(outcome.event.clone());
            spawn_audio(
                outcome.file,
                outcome.event.amplitude,
                Arc::clone(&settings),
                Some(Arc::clone(&audio_log)),
            );
        }

        app.drain_shared_log(&audio_log);
        tui.terminal
            .draw(|frame| {
                draw_tui(
                    frame,
                    &app,
                    &packs,
                    active_pack_idx,
                    &settings_snapshot,
                    tuning,
                    preset,
                )
            })
            .context("drawing terminal UI")?;
    }

    Ok(())
}

fn initial_custom_source(cli: &Cli) -> Option<String> {
    if !cli.custom_files.is_empty() {
        Some(
            cli.custom_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(","),
        )
    } else {
        cli.custom.as_ref().map(|path| path.display().to_string())
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_tui_input(
    app: &mut TuiApp,
    packs: &mut Vec<SoundPack>,
    active_pack_idx: &mut usize,
    preset: &mut TuningPreset,
    tuning: &mut RuntimeTuning,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
    running: &Arc<AtomicBool>,
) -> Result<()> {
    while event::poll(Duration::ZERO).context("polling terminal input")? {
        if let TerminalEvent::Key(key) = event::read().context("reading terminal event")? {
            handle_tui_key(
                key,
                app,
                packs,
                active_pack_idx,
                preset,
                tuning,
                settings,
                engine,
                running,
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_tui_key(
    key: KeyEvent,
    app: &mut TuiApp,
    packs: &mut Vec<SoundPack>,
    active_pack_idx: &mut usize,
    preset: &mut TuningPreset,
    tuning: &mut RuntimeTuning,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
    running: &Arc<AtomicBool>,
) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
        running.store(false, Ordering::SeqCst);
        return;
    }

    if app.custom_input.is_some() {
        handle_custom_input_key(key, app, packs, active_pack_idx, settings, engine);
        return;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
            running.store(false, Ordering::SeqCst);
        }
        KeyCode::Up => app.select_previous_control(),
        KeyCode::Down | KeyCode::Tab => app.select_next_control(),
        KeyCode::BackTab => app.select_previous_control(),
        KeyCode::Left => adjust_selected_control(
            app,
            packs,
            active_pack_idx,
            preset,
            tuning,
            settings,
            engine,
            -1,
        ),
        KeyCode::Right => adjust_selected_control(
            app,
            packs,
            active_pack_idx,
            preset,
            tuning,
            settings,
            engine,
            1,
        ),
        KeyCode::Enter => activate_selected_control(
            app,
            packs,
            active_pack_idx,
            preset,
            tuning,
            settings,
            engine,
        ),
        KeyCode::Char(' ') | KeyCode::Char('p') | KeyCode::Char('P') => {
            toggle_pause(settings, app);
        }
        KeyCode::Char('v') | KeyCode::Char('V') => {
            toggle_volume_scaling(settings, app);
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            switch_sound_pack(app, packs, active_pack_idx, settings, engine, 1);
        }
        KeyCode::Char('f') | KeyCode::Char('F') => {
            toggle_tuning_preset(
                app,
                preset,
                tuning,
                settings,
                engine,
                &packs[*active_pack_idx],
            );
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            app.custom_input = Some(app.custom_source.clone().unwrap_or_default());
        }
        _ => {}
    }
}

fn handle_custom_input_key(
    key: KeyEvent,
    app: &mut TuiApp,
    packs: &mut Vec<SoundPack>,
    active_pack_idx: &mut usize,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
) {
    match key.code {
        KeyCode::Esc => {
            app.custom_input = None;
            app.log_action("custom source edit canceled");
        }
        KeyCode::Enter => {
            let source = app.custom_input.take().unwrap_or_default();
            match load_custom_source(&source) {
                Ok(pack) => {
                    let idx = upsert_custom_pack(packs, pack);
                    *active_pack_idx = idx;
                    app.custom_source = Some(source.trim().to_string());
                    let cooldown_ms = settings.read().expect("settings lock poisoned").cooldown_ms;
                    engine.reconfigure_tracker(&packs[*active_pack_idx], cooldown_ms);
                    app.log_action(format!(
                        "custom source loaded: {} files",
                        packs[*active_pack_idx].files.len()
                    ));
                }
                Err(err) => {
                    app.custom_input = Some(source);
                    app.log_action(format!("custom source error: {err:#}"));
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(buffer) = app.custom_input.as_mut() {
                buffer.pop();
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(buffer) = app.custom_input.as_mut() {
                buffer.push(ch);
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn activate_selected_control(
    app: &mut TuiApp,
    packs: &mut [SoundPack],
    active_pack_idx: &mut usize,
    preset: &mut TuningPreset,
    tuning: &mut RuntimeTuning,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
) {
    match app.selected_control() {
        TuiControl::Pause => toggle_pause(settings, app),
        TuiControl::SoundPack => {
            switch_sound_pack(app, packs, active_pack_idx, settings, engine, 1)
        }
        TuiControl::TuningPreset => {
            toggle_tuning_preset(
                app,
                preset,
                tuning,
                settings,
                engine,
                &packs[*active_pack_idx],
            );
        }
        TuiControl::MinAmplitude => adjust_min_amplitude(settings, app, 1),
        TuiControl::Cooldown => {
            adjust_cooldown(settings, app, engine, &packs[*active_pack_idx], 1);
        }
        TuiControl::Speed => adjust_speed(settings, app, 1),
        TuiControl::VolumeScaling => toggle_volume_scaling(settings, app),
        TuiControl::CustomSource => {
            app.custom_input = Some(app.custom_source.clone().unwrap_or_default());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn adjust_selected_control(
    app: &mut TuiApp,
    packs: &mut [SoundPack],
    active_pack_idx: &mut usize,
    preset: &mut TuningPreset,
    tuning: &mut RuntimeTuning,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
    direction: isize,
) {
    match app.selected_control() {
        TuiControl::Pause => toggle_pause(settings, app),
        TuiControl::SoundPack => {
            switch_sound_pack(app, packs, active_pack_idx, settings, engine, direction);
        }
        TuiControl::TuningPreset => {
            toggle_tuning_preset(
                app,
                preset,
                tuning,
                settings,
                engine,
                &packs[*active_pack_idx],
            );
        }
        TuiControl::MinAmplitude => adjust_min_amplitude(settings, app, direction),
        TuiControl::Cooldown => {
            adjust_cooldown(settings, app, engine, &packs[*active_pack_idx], direction);
        }
        TuiControl::Speed => adjust_speed(settings, app, direction),
        TuiControl::VolumeScaling => toggle_volume_scaling(settings, app),
        TuiControl::CustomSource => {
            app.custom_input = Some(app.custom_source.clone().unwrap_or_default());
        }
    }
}

fn toggle_pause(settings: &Arc<RwLock<Settings>>, app: &mut TuiApp) {
    let paused = {
        let mut settings = settings.write().expect("settings lock poisoned");
        settings.paused = !settings.paused;
        settings.paused
    };
    app.log_action(format!("pause -> {}", on_off(paused)));
}

fn toggle_volume_scaling(settings: &Arc<RwLock<Settings>>, app: &mut TuiApp) {
    let enabled = {
        let mut settings = settings.write().expect("settings lock poisoned");
        settings.volume_scaling = !settings.volume_scaling;
        settings.volume_scaling
    };
    app.log_action(format!("volume scaling -> {}", on_off(enabled)));
}

fn switch_sound_pack(
    app: &mut TuiApp,
    packs: &[SoundPack],
    active_pack_idx: &mut usize,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
    direction: isize,
) {
    if packs.is_empty() {
        return;
    }
    let next_idx =
        (*active_pack_idx as isize + direction).rem_euclid(packs.len() as isize) as usize;
    *active_pack_idx = next_idx;
    let cooldown_ms = settings.read().expect("settings lock poisoned").cooldown_ms;
    engine.reconfigure_tracker(&packs[next_idx], cooldown_ms);
    app.log_action(format!(
        "sound pack -> {} ({} files)",
        packs[next_idx].name,
        packs[next_idx].files.len()
    ));
}

fn toggle_tuning_preset(
    app: &mut TuiApp,
    preset: &mut TuningPreset,
    tuning: &mut RuntimeTuning,
    settings: &Arc<RwLock<Settings>>,
    engine: &mut SlapEngine,
    pack: &SoundPack,
) {
    *preset = preset.toggled();
    *tuning = preset.runtime();
    let cooldown_ms = {
        let mut settings = settings.write().expect("settings lock poisoned");
        settings.min_amplitude = tuning.min_amplitude;
        settings.cooldown_ms = tuning.cooldown.as_millis() as u64;
        settings.cooldown_ms
    };
    engine.reconfigure_tracker(pack, cooldown_ms);
    app.log_action(format!("tuning preset -> {}", preset.label()));
}

fn adjust_min_amplitude(settings: &Arc<RwLock<Settings>>, app: &mut TuiApp, direction: isize) {
    let value = {
        let mut settings = settings.write().expect("settings lock poisoned");
        settings.min_amplitude =
            (settings.min_amplitude + direction as f64 * MIN_AMPLITUDE_STEP).clamp(0.0, 1.0);
        settings.min_amplitude
    };
    app.log_action(format!("min amplitude -> {value:.4}"));
}

fn adjust_cooldown(
    settings: &Arc<RwLock<Settings>>,
    app: &mut TuiApp,
    engine: &mut SlapEngine,
    pack: &SoundPack,
    direction: isize,
) {
    let value = {
        let mut settings = settings.write().expect("settings lock poisoned");
        if direction < 0 {
            settings.cooldown_ms = settings
                .cooldown_ms
                .saturating_sub(COOLDOWN_STEP_MS)
                .max(MIN_COOLDOWN_MS);
        } else {
            settings.cooldown_ms = settings.cooldown_ms.saturating_add(COOLDOWN_STEP_MS);
        }
        settings.cooldown_ms
    };
    engine.reconfigure_tracker(pack, value);
    app.log_action(format!("cooldown -> {value}ms"));
}

fn adjust_speed(settings: &Arc<RwLock<Settings>>, app: &mut TuiApp, direction: isize) {
    let value = {
        let mut settings = settings.write().expect("settings lock poisoned");
        settings.speed_ratio = (settings.speed_ratio + direction as f64 * SPEED_STEP)
            .clamp(MIN_SPEED_RATIO, MAX_SPEED_RATIO);
        settings.speed_ratio
    };
    app.log_action(format!("speed -> {value:.2}x"));
}

fn draw_tui(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    packs: &[SoundPack],
    active_pack_idx: usize,
    settings: &Settings,
    tuning: RuntimeTuning,
    preset: TuningPreset,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(11),
            Constraint::Min(8),
            Constraint::Length(4),
        ])
        .split(frame.area());

    let active_pack = &packs[active_pack_idx];
    draw_header(frame, root[0], app, active_pack, settings, preset);

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(root[1]);

    draw_status_panel(frame, panels[0], app, active_pack, preset);
    draw_controls_panel(
        frame,
        panels[1],
        app,
        packs,
        active_pack_idx,
        settings,
        tuning,
        preset,
    );
    draw_last_event_panel(frame, panels[2], app);
    draw_log_panel(frame, root[2], app);
    draw_footer(frame, root[3], app);
}

fn draw_header(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    pack: &SoundPack,
    settings: &Settings,
    preset: TuningPreset,
) {
    let status = if settings.paused {
        "paused"
    } else {
        "listening"
    };
    let color = if settings.paused {
        Color::Yellow
    } else {
        Color::Green
    };
    let line = Line::from(vec![
        Span::styled(
            "spank",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            status,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  mode={} tuning={} started={}",
            pack.name,
            preset.label(),
            app.started_at.format("%H:%M:%S")
        )),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_status_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    pack: &SoundPack,
    preset: TuningPreset,
) {
    let lines = vec![
        Line::from(format!("Slaps: {}", app.total_slaps)),
        Line::from(format!("Mode: {}", pack.name)),
        Line::from(format!("Files: {}", pack.files.len())),
        Line::from(format!("Tuning: {}", preset.label())),
        Line::from(format!(
            "Uptime: {}",
            format_duration(Local::now() - app.started_at)
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Status").borders(Borders::ALL)),
        area,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_controls_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    packs: &[SoundPack],
    active_pack_idx: usize,
    settings: &Settings,
    tuning: RuntimeTuning,
    preset: TuningPreset,
) {
    let items = TUI_CONTROLS
        .iter()
        .enumerate()
        .map(|(idx, control)| {
            let selected = idx == app.selected_control;
            let marker = if selected { "> " } else { "  " };
            let line = Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    control.label(),
                    Style::default().add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
                Span::raw(format!(
                    ": {}",
                    control_value(
                        *control,
                        app,
                        packs,
                        active_pack_idx,
                        settings,
                        tuning,
                        preset,
                    )
                )),
            ]);
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().title("Controls").borders(Borders::ALL)),
        area,
    );
}

fn control_value(
    control: TuiControl,
    app: &TuiApp,
    packs: &[SoundPack],
    active_pack_idx: usize,
    settings: &Settings,
    tuning: RuntimeTuning,
    preset: TuningPreset,
) -> String {
    match control {
        TuiControl::Pause => on_off(settings.paused).to_string(),
        TuiControl::SoundPack => format!(
            "{} ({}/{})",
            packs[active_pack_idx].name,
            active_pack_idx + 1,
            packs.len()
        ),
        TuiControl::TuningPreset => format!(
            "{} poll={}ms batch={}",
            preset.label(),
            tuning.poll_interval.as_millis(),
            tuning.max_batch
        ),
        TuiControl::MinAmplitude => format!("{:.4}", settings.min_amplitude),
        TuiControl::Cooldown => format!("{}ms", settings.cooldown_ms),
        TuiControl::Speed => format!("{:.2}x", settings.speed_ratio),
        TuiControl::VolumeScaling => on_off(settings.volume_scaling).to_string(),
        TuiControl::CustomSource => app
            .custom_source
            .as_deref()
            .unwrap_or("not loaded")
            .to_string(),
    }
}

fn draw_last_event_panel(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let lines = if let Some(event) = &app.last_event {
        vec![
            Line::from(format!("Time: {}", event.timestamp.format("%H:%M:%S"))),
            Line::from(format!("Number: {}", event.slap_number)),
            Line::from(format!("Severity: {}", event.severity)),
            Line::from(format!("Amplitude: {:.5}g", event.amplitude)),
            Line::from(format!("File: {}", event.file)),
        ]
    } else {
        vec![Line::from("No slaps detected yet")]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Last Slap").borders(Borders::ALL)),
        area,
    );
}

fn draw_log_panel(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let visible = area.height.saturating_sub(2) as usize;
    let items = app
        .log
        .iter()
        .rev()
        .take(visible)
        .rev()
        .map(|line| ListItem::new(line.as_str()))
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().title("Events").borders(Borders::ALL)),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let line = if let Some(buffer) = &app.custom_input {
        Line::from(vec![
            Span::styled("custom source", Style::default().fg(Color::Cyan)),
            Span::raw(": "),
            Span::raw(buffer.as_str()),
            Span::raw("  "),
            Span::styled("enter", Style::default().fg(Color::Cyan)),
            Span::raw(" load  "),
            Span::styled("esc", Style::default().fg(Color::Cyan)),
            Span::raw(" cancel  "),
            Span::styled("ctrl-c", Style::default().fg(Color::Cyan)),
            Span::raw(" quit"),
        ])
    } else {
        Line::from(vec![
            Span::styled("up/down", Style::default().fg(Color::Cyan)),
            Span::raw(" select  "),
            Span::styled("left/right", Style::default().fg(Color::Cyan)),
            Span::raw(" adjust  "),
            Span::styled("enter", Style::default().fg(Color::Cyan)),
            Span::raw(" apply/edit  "),
            Span::styled("q/esc/ctrl-c", Style::default().fg(Color::Cyan)),
            Span::raw(" quit  "),
            Span::styled("space/p", Style::default().fg(Color::Cyan)),
            Span::raw(" pause  "),
            Span::styled("m/f/v/e", Style::default().fg(Color::Cyan)),
            Span::raw(" mode/preset/volume/custom"),
        ])
    };
    frame.render_widget(
        Paragraph::new(line)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn spawn_audio(
    file: AudioFile,
    amplitude: f64,
    settings: Arc<RwLock<Settings>>,
    log: Option<SharedLog>,
) {
    thread::spawn(move || {
        if let Err(err) = play_audio(file, amplitude, settings) {
            if let Some(log) = log {
                let mut log = log.lock().expect("audio log lock poisoned");
                push_log(&mut log, format!("audio error: {err:#}"));
            } else {
                eprintln!("spank: {err:#}");
            }
        }
    });
}

fn push_log(log: &mut VecDeque<String>, line: String) {
    if log.len() >= MAX_LOG_LINES {
        log.pop_front();
    }
    log.push_back(line);
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn format_duration(duration: chrono::Duration) -> String {
    let seconds = duration.num_seconds().max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn play_audio(file: AudioFile, amplitude: f64, settings: Arc<RwLock<Settings>>) -> Result<()> {
    let bytes = file.read_bytes()?;
    let cursor = Cursor::new(bytes);
    let mut output =
        rodio::DeviceSinkBuilder::open_default_sink().context("opening default audio output")?;
    output.log_on_drop(false);
    let player = rodio::play(output.mixer(), cursor)
        .with_context(|| format!("playing {}", file.display_path()))?;
    let settings = settings.read().expect("settings lock poisoned").clone();

    if settings.volume_scaling {
        player.set_volume(amplitude_to_gain(amplitude) as f32);
    }
    if settings.speed_ratio > 0.0 {
        player.set_speed(settings.speed_ratio as f32);
    }

    player.sleep_until_end();
    Ok(())
}

fn amplitude_to_volume(amplitude: f64) -> f64 {
    const MIN_AMP: f64 = 0.05;
    const MAX_AMP: f64 = 0.80;
    const MIN_VOL: f64 = -3.0;
    const MAX_VOL: f64 = 0.0;

    if amplitude <= MIN_AMP {
        return MIN_VOL;
    }
    if amplitude >= MAX_AMP {
        return MAX_VOL;
    }

    let mut t = (amplitude - MIN_AMP) / (MAX_AMP - MIN_AMP);
    t = (1.0 + t * 99.0).ln() / 100.0_f64.ln();
    MIN_VOL + t * (MAX_VOL - MIN_VOL)
}

fn amplitude_to_gain(amplitude: f64) -> f64 {
    2.0_f64.powf(amplitude_to_volume(amplitude))
}

#[derive(Deserialize)]
struct StdinCommand {
    cmd: String,
    amplitude: Option<f64>,
    cooldown: Option<u64>,
    speed: Option<f64>,
}

fn process_commands<R, W>(
    reader: R,
    writer: &mut W,
    settings: Arc<RwLock<Settings>>,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
{
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let command = match serde_json::from_str::<StdinCommand>(line) {
            Ok(command) => command,
            Err(err) => {
                if settings.read().expect("settings lock poisoned").stdio_mode {
                    writeln!(
                        writer,
                        "{}",
                        json!({ "error": format!("invalid command: {err}") })
                    )?;
                }
                continue;
            }
        };

        match command.cmd.as_str() {
            "pause" => {
                settings.write().expect("settings lock poisoned").paused = true;
                if settings.read().expect("settings lock poisoned").stdio_mode {
                    writeln!(writer, "{}", json!({ "status": "paused" }))?;
                }
            }
            "resume" => {
                settings.write().expect("settings lock poisoned").paused = false;
                if settings.read().expect("settings lock poisoned").stdio_mode {
                    writeln!(writer, "{}", json!({ "status": "resumed" }))?;
                }
            }
            "set" => {
                let snapshot = {
                    let mut settings = settings.write().expect("settings lock poisoned");
                    if let Some(amplitude) = command.amplitude
                        && amplitude > 0.0
                        && amplitude <= 1.0
                    {
                        settings.min_amplitude = amplitude;
                    }
                    if let Some(cooldown) = command.cooldown
                        && cooldown > 0
                    {
                        settings.cooldown_ms = cooldown;
                    }
                    if let Some(speed) = command.speed
                        && speed > 0.0
                    {
                        settings.speed_ratio = speed;
                    }
                    settings.clone()
                };
                if snapshot.stdio_mode {
                    writeln!(
                        writer,
                        "{}",
                        json!({
                            "status": "settings_updated",
                            "amplitude": snapshot.min_amplitude,
                            "cooldown": snapshot.cooldown_ms,
                            "speed": snapshot.speed_ratio,
                        })
                    )?;
                }
            }
            "volume-scaling" => {
                let snapshot = {
                    let mut settings = settings.write().expect("settings lock poisoned");
                    settings.volume_scaling = !settings.volume_scaling;
                    settings.clone()
                };
                if snapshot.stdio_mode {
                    writeln!(
                        writer,
                        "{}",
                        json!({
                            "status": "volume_scaling_toggled",
                            "volume_scaling": snapshot.volume_scaling,
                        })
                    )?;
                }
            }
            "status" => {
                let snapshot = settings.read().expect("settings lock poisoned").clone();
                if snapshot.stdio_mode {
                    writeln!(
                        writer,
                        "{}",
                        json!({
                            "status": "ok",
                            "paused": snapshot.paused,
                            "amplitude": snapshot.min_amplitude,
                            "cooldown": snapshot.cooldown_ms,
                            "volume_scaling": snapshot.volume_scaling,
                            "speed": snapshot.speed_ratio,
                        })
                    )?;
                }
            }
            other => {
                if settings.read().expect("settings lock poisoned").stdio_mode {
                    writeln!(
                        writer,
                        "{}",
                        json!({ "error": format!("unknown command: {other}") })
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn is_mp3(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("mp3"))
        .unwrap_or(false)
}

fn unix_now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(unix)]
fn is_root() -> bool {
    (unsafe { libc::geteuid() }) == 0
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

#[derive(Clone, Copy)]
struct Sample {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Clone)]
struct Event {
    time: f64,
    severity: &'static str,
    amplitude: f64,
}

struct Detector {
    sample_count: usize,
    fs: usize,
    latest_raw: [f64; 3],
    latest_mag: f64,
    hp_alpha: f64,
    hp_prev_raw: [f64; 3],
    hp_prev_out: [f64; 3],
    hp_ready: bool,
    sta: [f64; 3],
    lta: [f64; 3],
    sta_n: [usize; 3],
    lta_n: [usize; 3],
    sta_lta_on: [f64; 3],
    sta_lta_off: [f64; 3],
    sta_lta_active: [bool; 3],
    cusum_pos: f64,
    cusum_neg: f64,
    cusum_mu: f64,
    cusum_k: f64,
    cusum_h: f64,
    kurt_buf: RingFloat,
    kurtosis: f64,
    peak_buf: RingFloat,
    rms: f64,
    peak: f64,
    crest: f64,
    mad_sigma: f64,
    rms_window: RingFloat,
    events: Vec<Event>,
    last_event_t: f64,
    kurt_dec: usize,
    rms_dec: usize,
}

impl Detector {
    fn new() -> Self {
        let fs = 100;
        Self {
            sample_count: 0,
            fs,
            latest_raw: [0.0; 3],
            latest_mag: 0.0,
            hp_alpha: 0.95,
            hp_prev_raw: [0.0; 3],
            hp_prev_out: [0.0; 3],
            hp_ready: false,
            sta: [0.0; 3],
            lta: [1e-10; 3],
            sta_n: [3, 15, 50],
            lta_n: [100, 500, 2000],
            sta_lta_on: [3.0, 2.5, 2.0],
            sta_lta_off: [1.5, 1.3, 1.2],
            sta_lta_active: [false; 3],
            cusum_pos: 0.0,
            cusum_neg: 0.0,
            cusum_mu: 0.0,
            cusum_k: 0.0005,
            cusum_h: 0.01,
            kurt_buf: RingFloat::new(100),
            kurtosis: 3.0,
            peak_buf: RingFloat::new(200),
            rms: 0.0,
            peak: 0.0,
            crest: 1.0,
            mad_sigma: 0.0,
            rms_window: RingFloat::new(fs),
            events: Vec::new(),
            last_event_t: 0.0,
            kurt_dec: 0,
            rms_dec: 0,
        }
    }

    fn process(&mut self, ax: f64, ay: f64, az: f64, t_now: f64) -> f64 {
        self.sample_count += 1;
        self.latest_raw = [ax, ay, az];
        self.latest_mag = (ax * ax + ay * ay + az * az).sqrt();

        if !self.hp_ready {
            self.hp_prev_raw = [ax, ay, az];
            self.hp_ready = true;
            return 0.0;
        }

        let a = self.hp_alpha;
        let hx = a * (self.hp_prev_out[0] + ax - self.hp_prev_raw[0]);
        let hy = a * (self.hp_prev_out[1] + ay - self.hp_prev_raw[1]);
        let hz = a * (self.hp_prev_out[2] + az - self.hp_prev_raw[2]);
        self.hp_prev_raw = [ax, ay, az];
        self.hp_prev_out = [hx, hy, hz];
        let mag = (hx * hx + hy * hy + hz * hz).sqrt();

        self.rms_window.push(mag);
        self.rms_dec += 1;
        if self.rms_dec >= self.fs.saturating_div(10).max(1) {
            self.rms_dec = 0;
            let vals = self.rms_window.slice();
            if !vals.is_empty() {
                let sum = vals.iter().map(|v| v * v).sum::<f64>();
                self.rms = (sum / vals.len() as f64).sqrt();
            }
        }

        let mut detections = Vec::new();
        let energy = mag * mag;
        for i in 0..3 {
            self.sta[i] += (energy - self.sta[i]) / self.sta_n[i] as f64;
            self.lta[i] += (energy - self.lta[i]) / self.lta_n[i] as f64;
            let ratio = self.sta[i] / (self.lta[i] + 1e-30);
            let was_active = self.sta_lta_active[i];
            if ratio > self.sta_lta_on[i] && !was_active {
                self.sta_lta_active[i] = true;
                detections.push("STA/LTA");
            } else if ratio < self.sta_lta_off[i] {
                self.sta_lta_active[i] = false;
            }
        }

        self.cusum_mu += 0.0001 * (mag - self.cusum_mu);
        self.cusum_pos = 0.0_f64.max(self.cusum_pos + mag - self.cusum_mu - self.cusum_k);
        self.cusum_neg = 0.0_f64.max(self.cusum_neg - mag + self.cusum_mu - self.cusum_k);
        if self.cusum_pos > self.cusum_h {
            detections.push("CUSUM");
            self.cusum_pos = 0.0;
        }
        if self.cusum_neg > self.cusum_h {
            detections.push("CUSUM");
            self.cusum_neg = 0.0;
        }

        self.kurt_buf.push(mag);
        self.kurt_dec += 1;
        if self.kurt_dec >= 10 && self.kurt_buf.len() >= 50 {
            self.kurt_dec = 0;
            let buf = self.kurt_buf.slice();
            let n = buf.len() as f64;
            let mean = buf.iter().sum::<f64>() / n;
            let mut m2 = 0.0;
            let mut m4 = 0.0;
            for value in &buf {
                let diff = value - mean;
                let d2 = diff * diff;
                m2 += d2;
                m4 += d2 * d2;
            }
            m2 /= n;
            m4 /= n;
            self.kurtosis = m4 / (m2 * m2 + 1e-30);
            if self.kurtosis > 6.0 {
                detections.push("KURTOSIS");
            }
        }

        self.peak_buf.push(mag);
        if self.peak_buf.len() >= 50 && self.sample_count.is_multiple_of(10) {
            let buf = self.peak_buf.slice();
            let mut sorted = buf.clone();
            insertion_sort(&mut sorted);
            let n = sorted.len();
            let median = sorted[n / 2];

            let mut devs = sorted
                .iter()
                .map(|value| (value - median).abs())
                .collect::<Vec<_>>();
            insertion_sort(&mut devs);
            let mad = devs[n / 2];
            let sigma = 1.4826 * mad + 1e-30;
            self.mad_sigma = sigma;

            let mut sum = 0.0;
            let mut peak = 0.0_f64;
            for value in &buf {
                sum += value * value;
                peak = peak.max(value.abs());
            }
            self.rms = (sum / n as f64).sqrt();
            self.peak = peak;
            self.crest = peak / (self.rms + 1e-30);

            let dev = (mag - median).abs() / sigma;
            if dev > 2.0 {
                detections.push("PEAK");
            }
        }

        if !detections.is_empty() && (t_now - self.last_event_t) > 0.01 {
            self.last_event_t = t_now;
            self.classify(&detections, t_now, mag);
        }

        mag
    }

    fn classify(&mut self, detections: &[&'static str], t: f64, amplitude: f64) {
        let sources = detections.iter().copied().collect::<HashSet<_>>();
        let source_count = sources.len();
        let severity = if source_count >= 4 && amplitude > 0.05 {
            "CHOC_MAJEUR"
        } else if source_count >= 3 && amplitude > 0.02 {
            "CHOC_MOYEN"
        } else if sources.contains("PEAK") && amplitude > 0.005 {
            "MICRO_CHOC"
        } else if (sources.contains("STA/LTA") || sources.contains("CUSUM")) && amplitude > 0.003 {
            "VIBRATION"
        } else if amplitude > 0.001 {
            "VIB_LEGERE"
        } else {
            "MICRO_VIB"
        };

        self.events.push(Event {
            time: t,
            severity,
            amplitude,
        });
        if self.events.len() > 500 {
            let drop_count = self.events.len() - 500;
            self.events.drain(0..drop_count);
        }
    }
}

struct RingFloat {
    data: Vec<f64>,
    pos: usize,
    full: bool,
}

impl RingFloat {
    fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity],
            pos: 0,
            full: false,
        }
    }

    fn push(&mut self, value: f64) {
        self.data[self.pos] = value;
        self.pos += 1;
        if self.pos >= self.data.len() {
            self.pos = 0;
            self.full = true;
        }
    }

    fn len(&self) -> usize {
        if self.full { self.data.len() } else { self.pos }
    }

    fn slice(&self) -> Vec<f64> {
        let len = self.len();
        if !self.full {
            return self.data[..len].to_vec();
        }

        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(&self.data[self.pos..]);
        out.extend_from_slice(&self.data[..self.pos]);
        out
    }
}

fn insertion_sort(values: &mut [f64]) {
    for i in 1..values.len() {
        let key = values[i];
        let mut j = i;
        while j > 0 && values[j - 1] > key {
            values[j] = values[j - 1];
            j -= 1;
        }
        values[j] = key;
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod sensor {
    use super::{Sample, VecDeque};
    use anyhow::{Context, Result, bail};
    use std::ffi::CString;
    use std::os::raw::{c_char, c_double, c_int, c_void};
    use std::ptr;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    const PAGE_VENDOR: i64 = 0xFF00;
    const USAGE_ACCEL: i64 = 3;
    const IMU_REPORT_LEN: usize = 22;
    const IMU_DECIMATION: usize = 8;
    const IMU_DATA_OFFSET: usize = 6;
    const REPORT_BUF_SIZE: usize = 4096;
    const REPORT_INTERVAL_US: i32 = 1000;
    const ACCEL_SCALE: f64 = 65536.0;
    const RING_CAP: usize = 8000;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const CF_NUMBER_SINT32_TYPE: i32 = 3;
    const CF_NUMBER_SINT64_TYPE: i32 = 4;

    type KernReturn = i32;
    type IOReturn = i32;
    type IOOptionBits = u32;
    type IOObject = u32;
    type IOIterator = IOObject;
    type IORegistryEntry = IOObject;
    type IOService = IOObject;
    type MachPort = u32;
    type CFIndex = isize;
    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFNumberRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFMutableDictionaryRef = *mut c_void;
    type CFAllocatorRef = *const c_void;
    type CFRunLoopRef = *mut c_void;
    type IOHIDDeviceRef = *mut c_void;
    type IOHIDReportType = i32;
    type Boolean = u8;

    type IOHIDReportCallback = extern "C" fn(
        context: *mut c_void,
        result: IOReturn,
        sender: *mut c_void,
        report_type: IOHIDReportType,
        report_id: u32,
        report: *mut u8,
        report_length: CFIndex,
    );

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
        fn IOServiceGetMatchingServices(
            main_port: MachPort,
            matching: CFDictionaryRef,
            existing: *mut IOIterator,
        ) -> KernReturn;
        fn IOIteratorNext(iterator: IOIterator) -> IOObject;
        fn IOObjectRelease(object: IOObject) -> KernReturn;
        fn IORegistryEntryCreateCFProperty(
            entry: IORegistryEntry,
            key: CFStringRef,
            allocator: CFAllocatorRef,
            options: IOOptionBits,
        ) -> CFTypeRef;
        fn IORegistryEntrySetCFProperty(
            entry: IORegistryEntry,
            property_name: CFStringRef,
            property: CFTypeRef,
        ) -> KernReturn;
        fn IOHIDDeviceCreate(allocator: CFAllocatorRef, service: IOService) -> IOHIDDeviceRef;
        fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
        fn IOHIDDeviceRegisterInputReportCallback(
            device: IOHIDDeviceRef,
            report: *mut u8,
            report_length: CFIndex,
            callback: IOHIDReportCallback,
            context: *mut c_void,
        );
        fn IOHIDDeviceScheduleWithRunLoop(
            device: IOHIDDeviceRef,
            run_loop: CFRunLoopRef,
            run_loop_mode: CFStringRef,
        );
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: CFStringRef;

        fn CFStringCreateWithCString(
            allocator: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFNumberCreate(
            allocator: CFAllocatorRef,
            the_type: c_int,
            value_ptr: *const c_void,
        ) -> CFNumberRef;
        fn CFNumberGetValue(
            number: CFNumberRef,
            the_type: c_int,
            value_ptr: *mut c_void,
        ) -> Boolean;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopRunInMode(
            mode: CFStringRef,
            seconds: c_double,
            return_after_source_handled: Boolean,
        ) -> i32;
        fn CFRelease(cf: CFTypeRef);
    }

    #[derive(Default)]
    struct SensorState {
        samples: VecDeque<Sample>,
        accel_decimation: usize,
    }

    static SENSOR_STATE: OnceLock<Mutex<SensorState>> = OnceLock::new();

    pub struct SensorRuntime {
        report_buffers: Vec<Box<[u8; REPORT_BUF_SIZE]>>,
        devices: Vec<IOHIDDeviceRef>,
    }

    impl SensorRuntime {
        pub fn start() -> Result<Self> {
            SENSOR_STATE.get_or_init(|| Mutex::new(SensorState::default()));
            wake_spu_drivers().context("waking SPU drivers")?;

            let mut runtime = Self {
                report_buffers: Vec::new(),
                devices: Vec::new(),
            };
            runtime
                .register_hid_devices()
                .context("registering HID devices")?;

            if runtime.devices.is_empty() {
                bail!("no AppleSPUHIDDevice accelerometer found");
            }

            Ok(runtime)
        }

        pub fn poll(&mut self, timeout: Duration) -> Vec<Sample> {
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, timeout.as_secs_f64(), 0);
            }

            let state = SENSOR_STATE.get().expect("sensor state initialized");
            let mut state = state.lock().expect("sensor state lock poisoned");
            state.samples.drain(..).collect()
        }

        fn register_hid_devices(&mut self) -> Result<()> {
            let matching = io_service_matching("AppleSPUHIDDevice")?;
            let mut iterator: IOIterator = 0;
            let kr = unsafe {
                IOServiceGetMatchingServices(0, matching as CFDictionaryRef, &mut iterator)
            };
            if kr != 0 {
                bail!("IOServiceGetMatchingServices returned {kr}");
            }

            loop {
                let service = unsafe { IOIteratorNext(iterator) };
                if service == 0 {
                    break;
                }

                let result = self.maybe_register_accelerometer(service);
                unsafe {
                    IOObjectRelease(service);
                }
                result?;
            }

            if iterator != 0 {
                unsafe {
                    IOObjectRelease(iterator);
                }
            }

            Ok(())
        }

        fn maybe_register_accelerometer(&mut self, service: IOService) -> Result<()> {
            let usage_page = prop_int(service, "PrimaryUsagePage").unwrap_or_default();
            let usage = prop_int(service, "PrimaryUsage").unwrap_or_default();
            if usage_page != PAGE_VENDOR || usage != USAGE_ACCEL {
                return Ok(());
            }

            let hid = unsafe { IOHIDDeviceCreate(ptr::null(), service) };
            if hid.is_null() {
                return Ok(());
            }

            let kr = unsafe { IOHIDDeviceOpen(hid, 0) };
            if kr != 0 {
                return Ok(());
            }

            let mut report_buf = Box::new([0_u8; REPORT_BUF_SIZE]);
            let report_ptr = report_buf.as_mut_ptr();
            unsafe {
                IOHIDDeviceRegisterInputReportCallback(
                    hid,
                    report_ptr,
                    REPORT_BUF_SIZE as CFIndex,
                    accel_callback,
                    ptr::null_mut(),
                );
                IOHIDDeviceScheduleWithRunLoop(hid, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            }

            self.report_buffers.push(report_buf);
            self.devices.push(hid);
            Ok(())
        }
    }

    impl Drop for SensorRuntime {
        fn drop(&mut self) {
            for device in self.devices.drain(..) {
                if !device.is_null() {
                    unsafe {
                        CFRelease(device as CFTypeRef);
                    }
                }
            }
        }
    }

    pub fn ensure_supported() -> Result<()> {
        Ok(())
    }

    extern "C" fn accel_callback(
        _context: *mut c_void,
        _result: IOReturn,
        _sender: *mut c_void,
        _report_type: IOHIDReportType,
        _report_id: u32,
        report: *mut u8,
        report_length: CFIndex,
    ) {
        if report.is_null() || report_length as usize != IMU_REPORT_LEN {
            return;
        }

        let Some(state) = SENSOR_STATE.get() else {
            return;
        };
        let mut state = state.lock().expect("sensor state lock poisoned");
        state.accel_decimation += 1;
        if state.accel_decimation < IMU_DECIMATION {
            return;
        }
        state.accel_decimation = 0;

        let report = unsafe { std::slice::from_raw_parts(report, report_length as usize) };
        let x = read_i32_le(report, IMU_DATA_OFFSET) as f64 / ACCEL_SCALE;
        let y = read_i32_le(report, IMU_DATA_OFFSET + 4) as f64 / ACCEL_SCALE;
        let z = read_i32_le(report, IMU_DATA_OFFSET + 8) as f64 / ACCEL_SCALE;

        if state.samples.len() >= RING_CAP {
            state.samples.pop_front();
        }
        state.samples.push_back(Sample { x, y, z });
    }

    fn wake_spu_drivers() -> Result<()> {
        let matching = io_service_matching("AppleSPUHIDDriver")?;
        let mut iterator: IOIterator = 0;
        let kr =
            unsafe { IOServiceGetMatchingServices(0, matching as CFDictionaryRef, &mut iterator) };
        if kr != 0 {
            bail!("IOServiceGetMatchingServices returned {kr}");
        }

        loop {
            let service = unsafe { IOIteratorNext(iterator) };
            if service == 0 {
                break;
            }

            set_int_property(service, "SensorPropertyReportingState", 1);
            set_int_property(service, "SensorPropertyPowerState", 1);
            set_int_property(service, "ReportInterval", REPORT_INTERVAL_US);
            unsafe {
                IOObjectRelease(service);
            }
        }

        if iterator != 0 {
            unsafe {
                IOObjectRelease(iterator);
            }
        }

        Ok(())
    }

    fn prop_int(service: IOService, key: &str) -> Option<i64> {
        let key = cf_string(key).ok()?;
        let value = unsafe { IORegistryEntryCreateCFProperty(service, key, ptr::null(), 0) };
        unsafe {
            CFRelease(key as CFTypeRef);
        }
        if value.is_null() {
            return None;
        }

        let mut out64 = 0_i64;
        let ok64 = unsafe {
            CFNumberGetValue(
                value as CFNumberRef,
                CF_NUMBER_SINT64_TYPE,
                &mut out64 as *mut _ as *mut c_void,
            )
        } != 0;
        if ok64 {
            unsafe {
                CFRelease(value);
            }
            return Some(out64);
        }

        let mut out32 = 0_i32;
        let ok32 = unsafe {
            CFNumberGetValue(
                value as CFNumberRef,
                CF_NUMBER_SINT32_TYPE,
                &mut out32 as *mut _ as *mut c_void,
            )
        } != 0;
        unsafe {
            CFRelease(value);
        }
        ok32.then_some(out32 as i64)
    }

    fn set_int_property(service: IOService, key: &str, value: i32) {
        let Ok(key) = cf_string(key) else {
            return;
        };
        let number = unsafe {
            CFNumberCreate(
                ptr::null(),
                CF_NUMBER_SINT32_TYPE,
                &value as *const _ as *const c_void,
            )
        };
        if !number.is_null() {
            unsafe {
                IORegistryEntrySetCFProperty(service, key, number);
                CFRelease(number as CFTypeRef);
            }
        }
        unsafe {
            CFRelease(key as CFTypeRef);
        }
    }

    fn cf_string(value: &str) -> Result<CFStringRef> {
        let c_string = CString::new(value)?;
        let cf = unsafe {
            CFStringCreateWithCString(ptr::null(), c_string.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        if cf.is_null() {
            bail!("CFStringCreateWithCString failed for {value}");
        }
        Ok(cf)
    }

    fn io_service_matching(name: &str) -> Result<CFMutableDictionaryRef> {
        let name = CString::new(name)?;
        let matching = unsafe { IOServiceMatching(name.as_ptr()) };
        if matching.is_null() {
            bail!("IOServiceMatching returned null");
        }
        Ok(matching)
    }

    fn read_i32_le(bytes: &[u8], offset: usize) -> i32 {
        let mut raw = [0_u8; 4];
        raw.copy_from_slice(&bytes[offset..offset + 4]);
        i32::from_le_bytes(raw)
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
mod sensor {
    use super::Sample;
    use anyhow::{Result, bail};
    use std::time::Duration;

    pub struct SensorRuntime;

    impl SensorRuntime {
        pub fn start() -> Result<Self> {
            bail!("spank only supports macOS on Apple Silicon");
        }

        pub fn poll(&mut self, _timeout: Duration) -> Vec<Sample> {
            Vec::new()
        }
    }

    pub fn ensure_supported() -> Result<()> {
        bail!("spank only supports macOS on Apple Silicon");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_settings(stdio_mode: bool) -> Arc<RwLock<Settings>> {
        Arc::new(RwLock::new(Settings {
            paused: false,
            min_amplitude: DEFAULT_MIN_AMPLITUDE,
            cooldown_ms: DEFAULT_COOLDOWN_MS,
            stdio_mode,
            volume_scaling: false,
            speed_ratio: DEFAULT_SPEED_RATIO,
        }))
    }

    #[test]
    fn pause_command_updates_state() {
        let settings = test_settings(true);
        let mut output = Vec::new();
        process_commands(
            io::Cursor::new(r#"{"cmd":"pause"}"#),
            &mut output,
            Arc::clone(&settings),
        )
        .unwrap();

        assert!(settings.read().unwrap().paused);
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["status"], "paused");
    }

    #[test]
    fn set_command_updates_values() {
        let settings = test_settings(true);
        let mut output = Vec::new();
        process_commands(
            io::Cursor::new(r#"{"cmd":"set","amplitude":0.2,"cooldown":1000,"speed":0.8}"#),
            &mut output,
            Arc::clone(&settings),
        )
        .unwrap();

        let snapshot = settings.read().unwrap();
        assert_eq!(snapshot.min_amplitude, 0.2);
        assert_eq!(snapshot.cooldown_ms, 1000);
        assert_eq!(snapshot.speed_ratio, 0.8);
    }

    #[test]
    fn no_output_when_stdio_disabled() {
        let settings = test_settings(false);
        let mut output = Vec::new();
        process_commands(
            io::Cursor::new(r#"{"cmd":"pause"}"#),
            &mut output,
            Arc::clone(&settings),
        )
        .unwrap();

        assert!(settings.read().unwrap().paused);
        assert!(output.is_empty());
    }

    #[test]
    fn amplitude_to_volume_is_monotonic() {
        let mut prev = amplitude_to_volume(0.05);
        for step in 1..=15 {
            let amp = 0.05 + step as f64 * 0.05;
            let current = amplitude_to_volume(amp);
            assert!(current >= prev - 1e-9);
            prev = current;
        }
        assert_eq!(amplitude_to_volume(0.01), -3.0);
        assert_eq!(amplitude_to_volume(0.80), 0.0);
    }

    #[test]
    fn detector_detects_impulse() {
        let mut detector = Detector::new();
        for i in 0..200 {
            detector.process(0.0, 0.0, -1.0, i as f64 * 0.01);
        }
        detector.process(0.5, 0.5, -1.0, 2.01);
        assert!(!detector.events.is_empty());
    }
}
