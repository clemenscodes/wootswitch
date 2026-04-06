use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use hidapi::HidApi;
use serde::Serialize;

// Wooting USB constants — from ShayBox/Wooting-Integrations and WootingKb/wooting-rgb-sdk
const WOOTING_VID: u16 = 0x31E3;
const CFG_USAGE_PAGE: u16 = 0x1337;

const COMMAND_SIZE: usize = 8;
const RESPONSE_SIZE: usize = 256;

// HID feature-report command IDs (Wooting USB protocol)
const CMD_INIT: u8 = 33; // WootDevInit
const CMD_GET_PROFILE_COUNT: u8 = 9; // GetDigitalProfilesCount
const CMD_GET_CURRENT_PROFILE: u8 = 11; // GetCurrentKeyboardProfileIndex
const CMD_ACTIVATE_PROFILE: u8 = 23; // ActivateProfile
const CMD_RELOAD_PROFILE: u8 = 38; // ReloadProfile

// Response data starts at byte 5 for v2 devices (magic[2] + cmd[1] + unk[1] + len[1])
const DATA_OFFSET: usize = 5;

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
    model: String,
}

impl Keyboard {
    fn find(api: &HidApi) -> Result<Self> {
        let info = api
            .device_list()
            .filter(|d| d.vendor_id() == WOOTING_VID)
            .find(|d| d.usage_page() == CFG_USAGE_PAGE)
            .context(
                "No Wooting keyboard found. \
                 Make sure it is connected and Wootility is not open.",
            )?;

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

        Ok(Self { device, model })
    }

    /// Build the 8-byte HID feature-report command packet.
    ///
    /// Layout: [report_id=0x00] [0xD0] [0xDA] [cmd] [p3] [p2] [p1] [p0]
    fn make_cmd(cmd: u8, p0: u8, p1: u8, p2: u8, p3: u8) -> [u8; COMMAND_SIZE] {
        [0x00, 0xD0, 0xDA, cmd, p3, p2, p1, p0]
    }

    fn send(&self, cmd: u8, p0: u8, p1: u8, p2: u8, p3: u8) -> Result<[u8; RESPONSE_SIZE]> {
        let pkt = Self::make_cmd(cmd, p0, p1, p2, p3);
        self.device
            .send_feature_report(&pkt)
            .context("HID feature report write failed")?;

        let mut buf = [0u8; RESPONSE_SIZE];
        self.device
            .read_timeout(&mut buf, 1000)
            .context("HID response read timed out")?;
        Ok(buf)
    }

    fn init(&self) -> Result<()> {
        self.send(CMD_INIT, 0, 0, 0, 0)?;
        Ok(())
    }

    /// Returns the current profile as a 0-based index.
    fn get_current_profile(&self) -> Result<u8> {
        let resp = self.send(CMD_GET_CURRENT_PROFILE, 0, 0, 0, 0)?;
        Ok(resp[DATA_OFFSET])
    }

    /// Returns the total number of profiles configured on the keyboard.
    fn get_profile_count(&self) -> Result<u8> {
        let resp = self.send(CMD_GET_PROFILE_COUNT, 0, 0, 0, 0)?;
        let count = resp[DATA_OFFSET];
        // Fallback to 4 if the response is zero or unexpectedly large
        Ok(if (1..=8).contains(&count) { count } else { 4 })
    }

    /// Switch to the given 0-based profile index.
    fn switch_profile(&self, index: u8) -> Result<()> {
        self.send(CMD_ACTIVATE_PROFILE, index, 0, 0, 0)?;
        std::thread::sleep(Duration::from_millis(30));
        self.send(CMD_RELOAD_PROFILE, index, 0, 0, 0)?;
        Ok(())
    }
}

fn print_all_devices(api: &HidApi) {
    println!("Wooting HID interfaces (VID {WOOTING_VID:#06x}):");
    let mut found = false;
    for d in api.device_list().filter(|d| d.vendor_id() == WOOTING_VID) {
        let model = d.product_string().unwrap_or("Unknown");
        let path = d.path().to_string_lossy();
        println!(
            "  {model}  PID={:#06x}  usage_page={:#06x}  @ {path}",
            d.product_id(),
            d.usage_page(),
        );
        found = true;
    }
    if !found {
        println!("  (none found)");
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let api = HidApi::new().context("Failed to initialise HID API")?;

    if args.list_devices {
        print_all_devices(&api);
        return Ok(());
    }

    let kb = Keyboard::find(&api)?;
    kb.init()?;

    // Switch to a specific profile
    if let Some(profile_num) = args.profile {
        if profile_num == 0 {
            bail!("Profile number must be 1 or higher");
        }
        let index = profile_num - 1;
        let count = kb.get_profile_count()?;
        if index >= count {
            bail!("Profile {profile_num} does not exist (keyboard has {count} profiles)");
        }
        kb.switch_profile(index)?;
        if args.json {
            println!("{}", serde_json::json!({ "switched_to": profile_num }));
        } else {
            println!("{}: switched to profile {profile_num}", kb.model);
        }
        return Ok(());
    }

    // Print current profile only
    if args.current {
        let current = kb.get_current_profile()? + 1;
        if args.json {
            println!("{}", serde_json::json!({ "current": current }));
        } else {
            println!("{current}");
        }
        return Ok(());
    }

    // Default: list all profiles, marking the active one
    let current_idx = kb.get_current_profile()?;
    let count = kb.get_profile_count()?;

    let profiles: Vec<ProfileEntry> = (0..count)
        .map(|i| ProfileEntry {
            number: i + 1,
            current: i == current_idx,
        })
        .collect();

    if args.json {
        let list = ProfileList {
            current: current_idx + 1,
            profiles,
        };
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
