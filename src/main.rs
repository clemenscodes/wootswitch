use std::{fs, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use clap::Parser;
use hidapi::HidApi;
use serde::Serialize;

// Wooting USB constants — from ShayBox/Wooting-Integrations and WootingKb/wooting-rgb-sdk
const WOOTING_VID: u16 = 0x31E3;

// HID usage pages that identify the Wooting configuration interface
const CFG_USAGE_PAGE: u16 = 0x1337; // Standard (older) devices
const CFG_V3_USAGE_PAGE: u16 = 0xFF55; // ARM-based devices (60HE+, Two HE ARM, etc.)

// Feature report is 8 bytes: [report_id][magic_low][magic_high][cmd][p3][p2][p1][p0]
const COMMAND_SIZE: usize = 8;
// From the official wooting-rgb-sdk:
//   V1: 128 bytes, V2: 256 bytes, V3 (ARM multi-report): 2046 bytes
const RESPONSE_SIZE_STD: usize = 256;
const RESPONSE_SIZE_V3: usize = 2046;

// HID feature-report command IDs (Wooting USB protocol)
const CMD_INIT: u8 = 33; // WootDevInit
const CMD_GET_PROFILE_COUNT: u8 = 9; // GetDigitalProfilesCount
const CMD_GET_STORED_PROFILE: u8 = 11; // GetCurrentKeyboardProfileIndex (flash/default)
const CMD_ACTIVATE_PROFILE: u8 = 23; // ActivateProfile
                                     // ShayBox/Wooting-Profile-Switcher uses cmd 7 (ReloadProfile0), not 38 (ReloadProfile)
const CMD_RELOAD_PROFILE: u8 = 7;

// Response layout for multi-report v2 devices:
// [report_id][magic_low][magic_high][cmd_echo][status][data_len][data...]
const DATA_OFFSET_MULTI: usize = 6; // report_id(1) + header(5)
const DATA_OFFSET_STD: usize = 5; // header(5), no report_id prefix

#[derive(Parser)]
#[command(name = "wootswitch", about = "Wooting keyboard profile switcher")]
struct Args {
    /// Profile number to switch to (1-based)
    profile: Option<u8>,

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

#[derive(Serialize)]
struct ProfileEntry {
    number: u8,
    current: bool,
}

#[derive(Serialize)]
struct ProfileList {
    profiles: Vec<ProfileEntry>,
    current: u8,
}

struct Keyboard {
    device: hidapi::HidDevice,
    /// True for ARM devices (usage_page 0xFF55): report ID = 1, magic low = 0xD1,
    /// and the response is prefixed with the report ID byte.
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

    /// Build the 8-byte HID feature-report command packet.
    ///
    /// Standard layout:  [0x00][0xD0][0xDA][cmd][p3][p2][p1][p0]
    /// Multi-report ARM: [0x01][0xD1][0xDA][cmd][p3][p2][p1][p0]
    fn make_cmd(&self, cmd: u8, p0: u8, p1: u8, p2: u8, p3: u8) -> [u8; COMMAND_SIZE] {
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
            RESPONSE_SIZE_V3
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
            let show = n.min(12);
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
    /// the runtime-switched profile. Use `state_profile()` for the active one.
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
        std::thread::sleep(Duration::from_millis(100));
        self.send(CMD_RELOAD_PROFILE, index, 0, 0, 0)?;
        std::thread::sleep(Duration::from_millis(100));
        Ok(())
    }
}

// ── Local state tracking ──────────────────────────────────────────────────────
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

// ── Device listing ────────────────────────────────────────────────────────────

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

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    let api = HidApi::new().context("Failed to initialise HID API")?;

    if args.list_devices {
        print_all_devices(&api);
        return Ok(());
    }

    let kb = Keyboard::find(&api)?;

    // ── Switch to a specific profile ──────────────────────────────────────────
    if let Some(profile_num) = args.profile {
        if profile_num == 0 {
            bail!("Profile number must be 1 or higher");
        }
        let index = profile_num - 1;
        let count = kb.get_profile_count()?;
        if index >= count {
            bail!("Profile {profile_num} does not exist (keyboard has {count} profiles)");
        }
        kb.init()?;
        kb.switch_profile(index)?;
        write_state_profile(profile_num);
        if args.json {
            println!("{}", serde_json::json!({ "switched_to": profile_num }));
        } else {
            println!("{}: switched to profile {profile_num}", kb.model);
        }
        return Ok(());
    }

    // Resolve current profile: prefer local state, fall back to firmware query.
    // ARM firmware (60HE+) returns the flash default, not the runtime profile.
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
        .map(|i| ProfileEntry {
            number: i + 1,
            current: (i + 1) == current,
        })
        .collect();

    if args.json {
        let list = ProfileList { current, profiles };
        println!("{}", serde_json::to_string_pretty(&list)?);
    } else {
        println!("{}", kb.model);
        for p in &profiles {
            if p.current {
                println!("  * Profile {} (current)", p.number);
            } else {
                println!("    Profile {}", p.number);
            }
        }
    }

    Ok(())
}
