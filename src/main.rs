use std::{
    fs,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;
use regex::Regex;
use serde::{Deserialize, Serialize};

// Wooting USB constants — from ShayBox/Wooting-Integrations and WootingKb/wooting-rgb-sdk
const WOOTING_VID: u16 = 0x31E3;

// HID usage pages that identify the Wooting configuration interface
const CFG_USAGE_PAGE: u16 = 0x1337; // Standard (older) devices
const CFG_V3_USAGE_PAGE: u16 = 0xFF55; // ARM-based devices (60HE+, Two HE ARM, etc.)

// HID report sizes for the CFG_V3 interface (from HID descriptor of 60HE+):
//   Report ID 1: Input=32 bytes, Output=32 bytes, Feature=7 bytes
//   We send commands as Output reports (device.write, 33 bytes incl. report ID)
//   and read responses as Input reports (device.read, 33 bytes incl. report ID).
const OUTPUT_REPORT_SIZE: usize = 33; // report_id(1) + 32 data bytes
const INPUT_REPORT_SIZE: usize = 33; // report_id(1) + 32 data bytes

// Standard (non-ARM) devices use feature reports, 8 bytes
const FEATURE_REPORT_SIZE: usize = 8; // report_id(1) + 7 data bytes
const RESPONSE_SIZE_STD: usize = 256;

// HID feature-report command IDs (Wooting USB protocol)
const CMD_INIT: u8 = 33; // WootDevInit
const CMD_GET_PROFILE_COUNT: u8 = 9; // GetDigitalProfilesCount
const CMD_GET_STORED_PROFILE: u8 = 11; // GetCurrentKeyboardProfileIndex (flash/default)
const CMD_ACTIVATE_PROFILE: u8 = 23; // ActivateProfile
                                     // ShayBox/Wooting-Profile-Switcher uses cmd 7 (ReloadProfile0), not 38
const CMD_RELOAD_PROFILE: u8 = 7;

// Response layout: [report_id][magic_low][magic_high][cmd_echo][status][data...]
// For multi-report (ARM): report_id is included in the 33-byte read
// For standard: no report_id prefix in the 256-byte read
const DATA_OFFSET_MULTI: usize = 5; // skip report_id(1) + magic(2) + cmd_echo(1) + status(1)
const DATA_OFFSET_STD: usize = 5; // header(5), no report_id prefix

// ---------------------------------------------------------------------------
// Config file (TOML)
// ---------------------------------------------------------------------------

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wootswitch")
        .join("config.toml")
}

#[derive(Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    profiles: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    /// Regex matched against Hyprland window class
    #[serde(default)]
    class: Option<String>,
    /// Regex matched against Hyprland window title
    #[serde(default)]
    title: Option<String>,
    /// 1-based profile number to switch to
    profile: u8,
}

fn load_config() -> FileConfig {
    let path = config_path();
    let content = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return FileConfig::default(),
    };
    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("wootswitch: config parse error in {}: {e}", path.display());
        FileConfig::default()
    })
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "wootswitch", about = "Wooting keyboard profile switcher")]
struct Args {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Print only the current profile number
    #[arg(short, long)]
    current: bool,

    /// List all detected Wooting HID interfaces (for debugging)
    #[arg(short = 'D', long)]
    list_devices: bool,

    /// Output as JSON
    #[arg(short, long)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Switch to profile N (1-based)
    Switch {
        /// Profile number (1-based)
        profile: u8,
    },
    /// Watch Hyprland events and auto-switch profiles based on config rules
    Watch,
    /// Diagnose HID communication with the keyboard
    Diagnose,
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ProfileEntry {
    number: u8,
    current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize)]
struct ProfileList {
    profiles: Vec<ProfileEntry>,
    current: u8,
}

// ---------------------------------------------------------------------------
// Keyboard HID abstraction
// ---------------------------------------------------------------------------

struct Keyboard {
    device: hidapi::HidDevice,
    /// True for ARM devices (usage_page 0xFF55): commands sent as output reports.
    uses_multi_report: bool,
    model: String,
}

impl Keyboard {
    fn find(api: &HidApi) -> Result<Self> {
        let info = api
            .device_list()
            .filter(|d| d.vendor_id() == WOOTING_VID)
            .find(|d| d.usage_page() == CFG_USAGE_PAGE || d.usage_page() == CFG_V3_USAGE_PAGE)
            .context(
                "No Wooting keyboard found. \
                 Make sure it is connected and Wootility is not open.",
            )?;

        let uses_multi_report = info.usage_page() == CFG_V3_USAGE_PAGE;
        let model = info
            .product_string()
            .unwrap_or("Unknown Wooting")
            .to_string();

        let device = info.open_device(api).with_context(|| {
            format!(
                "Failed to open {model}. \
                 Check that the udev rules are installed (programs.wootswitch.enable = true)."
            )
        })?;

        Ok(Self {
            device,
            uses_multi_report,
            model,
        })
    }

    /// Build the command packet.
    ///
    /// Multi-report ARM (output report, 33 bytes):
    ///   [0x01][0xD1][0xDA][cmd][p3][p2][p1][p0][0...0]
    /// Standard (feature report, 8 bytes):
    ///   [0x00][0xD0][0xDA][cmd][p3][p2][p1][p0]
    fn make_cmd(&self, cmd: u8, p0: u8, p1: u8, p2: u8, p3: u8) -> [u8; FEATURE_REPORT_SIZE] {
        let report_id: u8 = if self.uses_multi_report { 1 } else { 0 };
        let magic_low: u8 = if self.uses_multi_report { 0xD1 } else { 0xD0 };
        [report_id, magic_low, 0xDA, cmd, p3, p2, p1, p0]
    }

    fn data_offset(&self) -> usize {
        if self.uses_multi_report {
            DATA_OFFSET_MULTI
        } else {
            DATA_OFFSET_STD
        }
    }

    fn response_size(&self) -> usize {
        if self.uses_multi_report {
            INPUT_REPORT_SIZE // 33 bytes: report_id(1) + 32 data bytes
        } else {
            RESPONSE_SIZE_STD
        }
    }

    fn send(&self, cmd: u8, p0: u8, p1: u8, p2: u8, p3: u8) -> Result<Vec<u8>> {
        let pkt = self.make_cmd(cmd, p0, p1, p2, p3);
        self.device
            .send_feature_report(&pkt)
            .context("HID feature report write failed")?;

        let mut buf = vec![0u8; self.response_size()];
        let n = self
            .device
            .read_timeout(&mut buf, 1000)
            .context("HID response read timed out")?;
        if std::env::var("WOOTSWITCH_DEBUG").is_ok() {
            let show = n.min(16);
            eprintln!(
                "cmd={cmd:02x} p0={p0:02x} → {n} bytes: {:02x?}",
                &buf[..show]
            );
        }
        buf.truncate(n);
        Ok(buf)
    }

    fn init(&self) -> Result<()> {
        self.send(CMD_INIT, 0, 0, 0, 0)?;
        Ok(())
    }

    /// Returns the flash/default profile index (0-based).
    ///
    /// On ARM firmware (60HE+), this reflects the stored default profile, not
    /// the runtime-switched profile. Use state file for the active one.
    fn stored_profile(&self) -> Result<u8> {
        let resp = self.send(CMD_GET_STORED_PROFILE, 0, 0, 0, 0)?;
        if resp.len() > self.data_offset() {
            Ok(resp[self.data_offset()])
        } else {
            Ok(0)
        }
    }

    fn get_profile_count(&self) -> Result<u8> {
        let resp = self.send(CMD_GET_PROFILE_COUNT, 0, 0, 0, 0)?;
        let count = if resp.len() > self.data_offset() {
            resp[self.data_offset()]
        } else {
            0
        };
        // Fallback to 4 if the response is zero or unexpectedly large
        Ok(if (1..=8).contains(&count) { count } else { 4 })
    }

    /// Switch to the given 0-based profile index.
    fn switch_profile(&self, index: u8) -> Result<()> {
        self.send(CMD_ACTIVATE_PROFILE, index, 0, 0, 0)?;
        thread::sleep(Duration::from_millis(100));
        self.send(CMD_RELOAD_PROFILE, index, 0, 0, 0)?;
        thread::sleep(Duration::from_millis(100));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Local state tracking
// ---------------------------------------------------------------------------
// The 60HE+ ARM firmware's GetCurrentKeyboardProfileIndex returns the flash
// default profile, not the runtime-switched one. We track the last software
// switch in a state file so queries reflect what wootswitch actually did.

fn state_path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wootswitch")
        .join("profile")
}

fn read_state_profile() -> Option<u8> {
    let p = state_path();
    fs::read_to_string(&p).ok()?.trim().parse().ok()
}

fn write_state_profile(profile_num: u8) {
    let p = state_path();
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&p, profile_num.to_string());
}

// ---------------------------------------------------------------------------
// Device listing
// ---------------------------------------------------------------------------

fn print_all_devices(api: &HidApi) {
    println!("Wooting HID interfaces (VID {WOOTING_VID:#06x}):");
    let mut found = false;
    for d in api.device_list().filter(|d| d.vendor_id() == WOOTING_VID) {
        let model = d.product_string().unwrap_or("Unknown");
        let path = d.path().to_string_lossy();
        let cfg = match d.usage_page() {
            CFG_USAGE_PAGE => " ← config",
            CFG_V3_USAGE_PAGE => " ← config (ARM/multi-report)",
            _ => "",
        };
        println!(
            "  {model}  PID={:#06x}  usage_page={:#06x}{cfg}  @ {path}",
            d.product_id(),
            d.usage_page(),
        );
        found = true;
    }
    if !found {
        println!("  (none found)");
    }
}

// ---------------------------------------------------------------------------
// Hyprland IPC helpers
// ---------------------------------------------------------------------------

fn hyprland_socket(file: &str) -> Option<String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    Some(format!("{runtime}/hypr/{sig}/{file}"))
}

fn hyprland_active_window() -> Option<(String, String)> {
    use std::io::{Read, Write};
    let path = hyprland_socket(".socket.sock")?;
    let mut stream = UnixStream::connect(&path).ok()?;
    stream.write_all(b"j/activewindow").ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    let class = json_str_field(&buf, "class")?;
    let title = json_str_field(&buf, "title").unwrap_or_default();
    Some((class, title))
}

fn json_str_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = json.find(&needle)? + needle.len();
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

// ---------------------------------------------------------------------------
// Compiled rule
// ---------------------------------------------------------------------------

struct CompiledRule {
    class: Option<Regex>,
    title: Option<Regex>,
    profile: u8,
}

impl CompiledRule {
    fn matches(&self, class: &str, title: &str) -> bool {
        let class_ok = self
            .class
            .as_ref()
            .map(|r| r.is_match(class))
            .unwrap_or(true);
        let title_ok = self
            .title
            .as_ref()
            .map(|r| r.is_match(title))
            .unwrap_or(true);
        class_ok && title_ok
    }
}

fn compile_rules(rules: &[Rule]) -> Vec<CompiledRule> {
    rules
        .iter()
        .map(|r| {
            let class = r
                .class
                .as_deref()
                .map(Regex::new)
                .transpose()
                .unwrap_or_else(|e| {
                    eprintln!("wootswitch: invalid class regex: {e}");
                    None
                });
            let title = r
                .title
                .as_deref()
                .map(Regex::new)
                .transpose()
                .unwrap_or_else(|e| {
                    eprintln!("wootswitch: invalid title regex: {e}");
                    None
                });
            CompiledRule {
                class,
                title,
                profile: r.profile,
            }
        })
        .collect()
}

fn find_matching_profile(rules: &[CompiledRule], class: &str, title: &str) -> Option<u8> {
    rules
        .iter()
        .find(|r| r.matches(class, title))
        .map(|r| r.profile)
}

// ---------------------------------------------------------------------------
// Watch mode
// ---------------------------------------------------------------------------

fn run_diagnose(api: &HidApi) -> Result<()> {
    let kb = Keyboard::find(api)?;
    println!(
        "Device: {} (uses_multi_report={})",
        kb.model, kb.uses_multi_report
    );

    // 1. Try reading without sending anything (check for spontaneous reports)
    println!("\n--- Drain: reading spontaneous input reports (500ms) ---");
    let mut drain_buf = vec![0u8; 2048];
    let n = kb.device.read_timeout(&mut drain_buf, 500).unwrap_or(0);
    println!("  Got {n} bytes without sending anything");
    if n > 0 {
        println!("  Data: {:02x?}", &drain_buf[..n.min(32)]);
    }

    // 2. Try write() output report
    println!("\n--- write() output report (CMD_INIT) ---");
    let pkt_write = {
        let mut p = vec![0u8; OUTPUT_REPORT_SIZE];
        p[0] = 1; // report_id
        p[1] = 0xD1; // magic_low
        p[2] = 0xDA;
        p[3] = CMD_INIT;
        p
    };
    match kb.device.write(&pkt_write) {
        Ok(n) => println!("  write() OK, {n} bytes sent"),
        Err(e) => println!("  write() FAILED: {e}"),
    }
    let n = kb.device.read_timeout(&mut drain_buf, 1500).unwrap_or(0);
    println!("  read_timeout(1500ms) → {n} bytes");
    if n > 0 {
        println!("  Data: {:02x?}", &drain_buf[..n.min(32)]);
    }

    // 3. Try send_feature_report (8 bytes: 1 report_id + 7 data)
    println!("\n--- send_feature_report() (CMD_GET_PROFILE_COUNT) ---");
    let pkt_feat = [0x01u8, 0xD1, 0xDA, CMD_GET_PROFILE_COUNT, 0, 0, 0, 0];
    match kb.device.send_feature_report(&pkt_feat) {
        Ok(()) => println!("  send_feature_report() OK"),
        Err(e) => println!("  send_feature_report() FAILED: {e}"),
    }
    let n = kb.device.read_timeout(&mut drain_buf, 1500).unwrap_or(0);
    println!("  read_timeout(1500ms) → {n} bytes");
    if n > 0 {
        println!("  Data: {:02x?}", &drain_buf[..n.min(32)]);
    }

    // 4. Try write() for GET_PROFILE_COUNT
    println!("\n--- write() output report (CMD_GET_PROFILE_COUNT) ---");
    let pkt_cnt = {
        let mut p = vec![0u8; OUTPUT_REPORT_SIZE];
        p[0] = 1;
        p[1] = 0xD1;
        p[2] = 0xDA;
        p[3] = CMD_GET_PROFILE_COUNT;
        p
    };
    match kb.device.write(&pkt_cnt) {
        Ok(n) => println!("  write() OK, {n} bytes sent"),
        Err(e) => println!("  write() FAILED: {e}"),
    }
    // Read multiple times in case response is delayed
    for i in 0..5 {
        let n = kb.device.read_timeout(&mut drain_buf, 500).unwrap_or(0);
        println!("  read #{i}: {n} bytes");
        if n > 0 {
            println!("    Data: {:02x?}", &drain_buf[..n.min(32)]);
        }
    }

    Ok(())
}

fn run_watch(rules: Vec<CompiledRule>) -> Result<()> {
    if rules.is_empty() {
        bail!(
            "No rules defined. Add [[rules]] sections to {}",
            config_path().display()
        );
    }

    eprintln!(
        "wootswitch: watching Hyprland events ({} rules)",
        rules.len()
    );

    // Apply rule for current window on startup
    if let Some((class, title)) = hyprland_active_window() {
        apply_rule_if_needed(&rules, &class, &title);
    }

    loop {
        let Some(path) = hyprland_socket(".socket2.sock") else {
            bail!("Hyprland IPC not available — is Hyprland running?");
        };
        match UnixStream::connect(&path) {
            Ok(stream) => {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Some(("activewindow", data)) = line.split_once(">>") {
                        let (class, title) = data.split_once(',').unwrap_or((data, ""));
                        apply_rule_if_needed(&rules, class.trim(), title.trim());
                    }
                }
                eprintln!("wootswitch: Hyprland event socket closed, reconnecting…");
            }
            Err(e) => eprintln!("wootswitch: Hyprland IPC connect error: {e}"),
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn apply_rule_if_needed(rules: &[CompiledRule], class: &str, title: &str) {
    let Some(profile_num) = find_matching_profile(rules, class, title) else {
        return;
    };
    let current = read_state_profile();
    if current == Some(profile_num) {
        return; // already on the right profile
    }
    match switch_to(profile_num) {
        Ok(()) => eprintln!("wootswitch: switched to profile {profile_num} for class={class:?}"),
        Err(e) => eprintln!("wootswitch: failed to switch to profile {profile_num}: {e}"),
    }
}

fn switch_to(profile_num: u8) -> Result<()> {
    let api = HidApi::new().context("Failed to initialise HID API")?;
    let kb = Keyboard::find(&api)?;
    let count = kb.get_profile_count()?;
    let index = profile_num - 1;
    if index >= count {
        bail!("Profile {profile_num} does not exist (keyboard has {count} profiles)");
    }
    kb.init()?;
    kb.switch_profile(index)?;
    write_state_profile(profile_num);
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let args = Args::parse();
    let api = HidApi::new().context("Failed to initialise HID API")?;

    if args.list_devices {
        print_all_devices(&api);
        return Ok(());
    }

    let cfg = load_config();

    // ── Diagnose ──────────────────────────────────────────────────────────────
    if matches!(args.command, Some(Cmd::Diagnose)) {
        return run_diagnose(&api);
    }

    // ── Watch mode ────────────────────────────────────────────────────────────
    if matches!(args.command, Some(Cmd::Watch)) {
        let rules = compile_rules(&cfg.rules);
        return run_watch(rules);
    }

    // ── Switch to a specific profile ──────────────────────────────────────────
    if let Some(Cmd::Switch {
        profile: profile_num,
    }) = args.command
    {
        if profile_num == 0 {
            bail!("Profile number must be 1 or higher");
        }
        switch_to(profile_num)?;
        let kb = Keyboard::find(&api)?;
        if args.json {
            println!("{}", serde_json::json!({ "switched_to": profile_num }));
        } else {
            let name = cfg.profiles.get(&profile_num.to_string());
            match name {
                Some(n) => println!("{}: switched to profile {profile_num} ({n})", kb.model),
                None => println!("{}: switched to profile {profile_num}", kb.model),
            }
        }
        return Ok(());
    }

    let kb = Keyboard::find(&api)?;

    // Resolve current profile: prefer local state, fall back to firmware query.
    let current = read_state_profile().unwrap_or_else(|| kb.stored_profile().unwrap_or(0) + 1);

    // ── Print current profile only ────────────────────────────────────────────
    if args.current {
        if args.json {
            println!("{}", serde_json::json!({ "current": current }));
        } else {
            println!("{current}");
        }
        return Ok(());
    }

    // ── Default: list all profiles, marking the active one ───────────────────
    let count = kb.get_profile_count()?;

    let profiles: Vec<ProfileEntry> = (0..count)
        .map(|i| {
            let number = i + 1;
            ProfileEntry {
                number,
                current: number == current,
                name: cfg.profiles.get(&number.to_string()).cloned(),
            }
        })
        .collect();

    if args.json {
        let list = ProfileList { current, profiles };
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else {
        println!("{}", kb.model);
        for p in &profiles {
            let name_str = p
                .name
                .as_deref()
                .map(|n| format!(" — {n}"))
                .unwrap_or_default();
            if p.current {
                println!("  * Profile {}{name_str} (current)", p.number);
            } else {
                println!("    Profile {}{name_str}", p.number);
            }
        }
        println!(
            "\nConfig: {}",
            if config_path().exists() {
                config_path().display().to_string()
            } else {
                format!("{} (not found)", config_path().display())
            }
        );
    }

    Ok(())
}
