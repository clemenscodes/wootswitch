use std::{fmt, thread, time::Duration};

use anyhow::{bail, Context, Result};
use hidapi::HidApi;
use serde::Serialize;

const WOOTING_VID: u16 = 0x31E3;
const CFG_USAGE_PAGE: u16 = 0x1337;
const CFG_V3_USAGE_PAGE: u16 = 0xFF55;

/// All responses share a 5-byte header: `[report_id][protocol_byte][0xDA][cmd_echo][status]`
const DATA_OFFSET: usize = 5;

/// `GetCurrentKeyboardProfileIndex` (cmd 11) response offsets into the data payload.
/// Standard: `data[0]` = runtime-active profile.
/// ARM: `data[0]` = flash-stored default (never changes via software);
///      `data[2]` = runtime-active profile (updated by `ActivateProfile`).
const STANDARD_RUNTIME_PROFILE_OFFSET: usize = 0;
const ARM_RUNTIME_PROFILE_OFFSET: usize = 2;

/// `GetProfileMetadata` (cmd 55) response layout after `DATA_OFFSET`:
///   `data[0]` = payload_length (`METADATA_ABSENT_PAYLOAD_LENGTH` = no profile at this slot)
///   `data[1]` = 0x00 padding
///   `data[2..]` = protobuf bytes (field 1, wire type 2 = length-delimited string)
const METADATA_PROTOBUF_START: usize = 2;
/// Payload length value returned by the firmware when no profile exists at the requested slot.
const METADATA_ABSENT_PAYLOAD_LENGTH: u8 = 0;

/// Within the protobuf bytes: `[tag][length][utf8 bytes]`
const PROTOBUF_FIELD_TAG: usize = 0;
const PROTOBUF_FIELD_LENGTH: usize = 1;
const PROTOBUF_FIELD_DATA: usize = 2;

/// Protobuf field-1 wire-type-2 tag: identifies the profile name string field.
const METADATA_NAME_TAG: u8 = 0x0a;

/// Protocol identifier bytes: second byte in every command packet, identifying the wire variant.
const STANDARD_PROTOCOL_BYTE: u8 = 0xD0;
const ARM_PROTOCOL_BYTE: u8 = 0xD1;

/// Third byte in every command packet — fixed framing marker for the Wooting command channel.
const WOOTING_COMMAND_MAGIC: u8 = 0xDA;

/// HID report IDs: 0 for standard (no explicit report ID), 1 for ARM.
const STANDARD_REPORT_ID: u8 = 0;
const ARM_REPORT_ID: u8 = 1;

/// Full response buffer sizes including the report ID byte.
const STANDARD_RESPONSE_SIZE: usize = 256;
/// ARM response: 1 report ID byte + 32 data bytes.
const ARM_RESPONSE_SIZE: usize = 33;

/// Byte positions within the 8-byte command packet for the profile slot argument.
/// ARM firmware reads the slot from byte 4; standard firmware from byte 7.
const PROFILE_SLOT_BYTE_ARM: usize = 4;
const PROFILE_SLOT_BYTE_STANDARD: usize = 7;

const MAX_PROFILES: u8 = 8;
const MIN_VALID_PROFILE_COUNT: u8 = 1;
/// Fallback profile count when the firmware does not report one (ARM devices).
const DEFAULT_PROFILE_COUNT: u8 = 4;

const HID_READ_TIMEOUT_MS: i32 = 1000;
const PROFILE_SWITCH_SETTLE_MS: u64 = 100;

/// Wire-protocol variant — encapsulates every per-device HID difference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// Older/standard devices (usage_page 0x1337)
    Standard,
    /// ARM-based devices: 60HE+, Two HE ARM, … (usage_page 0xFF55)
    Arm,
}

impl TryFrom<u16> for Protocol {
    type Error = ();

    fn try_from(usage_page: u16) -> std::result::Result<Self, ()> {
        match usage_page {
            CFG_USAGE_PAGE => Ok(Self::Standard),
            CFG_V3_USAGE_PAGE => Ok(Self::Arm),
            _ => Err(()),
        }
    }
}

impl Protocol {
    fn report_id(self) -> u8 {
        match self {
            Self::Standard => STANDARD_REPORT_ID,
            Self::Arm => ARM_REPORT_ID,
        }
    }

    fn protocol_byte(self) -> u8 {
        match self {
            Self::Standard => STANDARD_PROTOCOL_BYTE,
            Self::Arm => ARM_PROTOCOL_BYTE,
        }
    }

    fn response_size(self) -> usize {
        match self {
            Self::Standard => STANDARD_RESPONSE_SIZE,
            Self::Arm => ARM_RESPONSE_SIZE,
        }
    }

    fn build_packet(self, command: Command) -> [u8; 8] {
        [
            self.report_id(),
            self.protocol_byte(),
            WOOTING_COMMAND_MAGIC,
            u8::from(command),
            0,
            0,
            0,
            0,
        ]
    }

    /// ARM firmware reads the profile slot from byte 4; standard from byte 7.
    fn build_packet_for_profile(self, command: Command, index: ProfileIndex) -> [u8; 8] {
        let mut packet = self.build_packet(command);
        match self {
            Self::Standard => packet[PROFILE_SLOT_BYTE_STANDARD] = index.slot,
            Self::Arm => packet[PROFILE_SLOT_BYTE_ARM] = index.slot,
        }
        packet
    }
}

/// HID feature-report command IDs.
///
/// All 60 IDs (0–59) are present and sequential; no explicit discriminants needed.
/// Variants prefixed `Removed` existed in older firmware and are now no-ops or absent,
/// but their IDs are still reserved in the wire protocol.
///
/// Full reference: <https://gist.github.com/BigBrainAFK/0ba454a1efb43f7cb6301cda8838f432>
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // complete protocol surface; not every command is used by this binary
pub enum Command {
    Ping,
    GetVersion,
    ResetToBootloader,
    GetSerial,
    GetRgbProfileCount,
    RemovedGetCurrentRgbProfileIndex,
    RemovedGetRgbMainProfile,
    ReloadProfile0,
    SaveRgbProfile,
    GetDigitalProfilesCount,
    GetAnalogProfilesCount,
    GetCurrentKeyboardProfileIndex,
    GetDigitalProfile,
    GetAnalogProfileMainPart,
    GetAnalogProfileCurveChangeMapPart1,
    GetAnalogProfileCurveChangeMapPart2,
    GetNumberOfKeys,
    GetMainMappingProfile,
    GetFunctionMappingProfile,
    GetDeviceConfig,
    GetAnalogValues,
    KeysOff,
    KeysOn,
    ActivateProfile,
    GetDksProfile,
    DoSoftReset,
    RemovedGetRgbColorsPart1,
    RemovedGetRgbColorsPart2,
    RemovedGetRgbEffects,
    RefreshRgbColors,
    WootDevSingleColor,
    WootDevResetColor,
    WootDevResetAll,
    WootDevInit,
    RemovedGetRgbProfileBase,
    GetRgbProfileColorsPart1,
    GetRgbProfileColorsPart2,
    RemovedGetRgbProfileEffect,
    ReloadProfile,
    GetKeyboardProfile,
    GetGamepadMapping,
    GetGamepadProfile,
    SaveKeyboardProfile,
    ResetSettings,
    SetRawScanning,
    StartXinputDetection,
    StopXinputDetection,
    SaveDksProfile,
    GetMappingProfile,
    GetActuationProfile,
    GetRgbProfileCore,
    GetGlobalSettings,
    GetAkcProfile,
    SaveAkcProfile,
    GetRapidTriggerProfile,
    GetProfileMetadata,
    IsFlashChipConnected,
    GetRgbLayer,
    GetFlashStats,
    GetRgbBins,
}

impl From<Command> for u8 {
    fn from(command: Command) -> u8 {
        command as u8
    }
}

/// 0-based profile slot as used internally by the firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProfileIndex {
    slot: u8,
}

/// 1-based profile number as shown to the user.
///
/// Construct via `From<u8>`. The raw number is intentionally not exposed;
/// use `Display` to print it and `Serialize` to include it in JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ProfileNumber {
    number: u8,
}

impl From<u8> for ProfileNumber {
    fn from(number: u8) -> Self {
        ProfileNumber { number }
    }
}

impl ProfileNumber {
    /// Returns the next profile, wrapping from the last back to the first.
    pub fn wrapping_next(self, count: u8) -> ProfileNumber {
        ProfileNumber {
            number: (self.number % count) + 1,
        }
    }

    /// Returns the previous profile, wrapping from the first back to the last.
    pub fn wrapping_prev(self, count: u8) -> ProfileNumber {
        let number = if self.number == 1 {
            count
        } else {
            self.number - 1
        };
        ProfileNumber { number }
    }
}

impl From<ProfileIndex> for ProfileNumber {
    fn from(index: ProfileIndex) -> Self {
        ProfileNumber {
            number: index.slot + 1,
        }
    }
}

impl TryFrom<ProfileNumber> for ProfileIndex {
    type Error = anyhow::Error;

    fn try_from(profile: ProfileNumber) -> Result<Self> {
        if profile.number == 0 {
            bail!("Profile number must be 1 or higher");
        }
        let index = ProfileIndex {
            slot: profile.number - 1,
        };
        Ok(index)
    }
}

impl fmt::Display for ProfileNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.number)
    }
}

/// All profiles on the keyboard along with which one is currently active.
#[derive(Debug, Serialize)]
pub struct ProfileListing {
    profiles: Vec<Profile>,
    current: Option<ProfileNumber>,
}

impl ProfileListing {
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    #[allow(dead_code)]
    pub fn current(&self) -> Option<ProfileNumber> {
        self.current
    }
}

/// A keyboard profile: its number, name, and whether it is currently active.
#[derive(Debug, Serialize)]
pub struct Profile {
    number: ProfileNumber,
    current: bool,
    name: String,
}

#[allow(dead_code)]
impl Profile {
    pub fn number(&self) -> ProfileNumber {
        self.number
    }

    pub fn is_current(&self) -> bool {
        self.current
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Profile {} — {}", self.number, self.name)
    }
}

impl fmt::Display for ProfileListing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for profile in &self.profiles {
            if profile.current {
                writeln!(f, "* {profile} (current)")?;
            } else {
                writeln!(f, "  {profile}")?;
            }
        }
        Ok(())
    }
}

/// A raw HID response from the keyboard.
pub struct Response {
    bytes: Vec<u8>,
}

#[allow(dead_code)]
impl Response {
    /// The full raw bytes, including the response header.
    pub fn raw(&self) -> &[u8] {
        &self.bytes
    }

    /// The payload bytes after the 5-byte response header.
    pub fn data(&self) -> &[u8] {
        self.bytes.get(DATA_OFFSET..).unwrap_or_default()
    }
}

/// Parses a profile name out of the data payload of a `GetProfileMetadata` response.
///
/// Returns `None` if the slot is absent (`payload_length == 0`), the protobuf tag is
/// unexpected, or the name bytes are not valid UTF-8.
fn parse_profile_name(data: &[u8]) -> Option<String> {
    if data.first().copied() == Some(METADATA_ABSENT_PAYLOAD_LENGTH) {
        return None;
    }
    let protobuf = data.get(METADATA_PROTOBUF_START..)?;
    if protobuf.get(PROTOBUF_FIELD_TAG) != Some(&METADATA_NAME_TAG) {
        return None;
    }
    let name_length = usize::from(*protobuf.get(PROTOBUF_FIELD_LENGTH)?);
    let name_bytes = protobuf.get(PROTOBUF_FIELD_DATA..PROTOBUF_FIELD_DATA + name_length)?;
    String::from_utf8(name_bytes.to_vec()).ok()
}

pub struct Keyboard {
    device: hidapi::HidDevice,
    protocol: Protocol,
    model: String,
}

impl fmt::Display for Keyboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.model)
    }
}

impl Keyboard {
    pub fn find(api: &HidApi) -> Result<Self> {
        let (info, protocol) = api
            .device_list()
            .filter(|device| device.vendor_id() == WOOTING_VID)
            .find_map(|device| {
                Protocol::try_from(device.usage_page())
                    .ok()
                    .map(|proto| (device, proto))
            })
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
                 Check that the udev rules are installed \
                 (programs.wootswitch.enable = true)."
            )
        })?;

        let keyboard = Keyboard {
            device,
            protocol,
            model,
        };
        Ok(keyboard)
    }

    fn exchange(&self, packet: [u8; 8]) -> Result<Response> {
        self.device
            .send_feature_report(&packet)
            .context("HID feature report write failed")?;
        let mut buffer = vec![0; self.protocol.response_size()];
        let bytes_read = self
            .device
            .read_timeout(&mut buffer, HID_READ_TIMEOUT_MS)
            .context("HID response read timed out")?;
        buffer.truncate(bytes_read);
        let response = Response { bytes: buffer };
        Ok(response)
    }

    /// Send a command with no profile-index parameter and return the response.
    pub fn send(&self, command: Command) -> Result<Response> {
        self.exchange(self.protocol.build_packet(command))
    }

    fn send_for_profile(&self, command: Command, index: ProfileIndex) -> Result<Response> {
        self.exchange(self.protocol.build_packet_for_profile(command, index))
    }

    /// Send the WootDevInit command, required before switching profiles.
    pub fn init(&self) -> Result<()> {
        self.send(Command::WootDevInit)?;
        Ok(())
    }

    /// Returns the runtime-active profile.
    ///
    /// On ARM devices, `GetCurrentKeyboardProfileIndex` (cmd 11) includes both the
    /// flash-stored default at `data[0]` and the runtime-active profile at `data[2]`.
    /// We always read the runtime value.
    pub fn active_profile(&self) -> Result<ProfileNumber> {
        let response = self.send(Command::GetCurrentKeyboardProfileIndex)?;
        let runtime_offset = match self.protocol {
            Protocol::Arm => ARM_RUNTIME_PROFILE_OFFSET,
            Protocol::Standard => STANDARD_RUNTIME_PROFILE_OFFSET,
        };
        let firmware_slot = response
            .data()
            .get(runtime_offset)
            .copied()
            .context("GetCurrentKeyboardProfileIndex response too short")?;
        let index = ProfileIndex {
            slot: firmware_slot,
        };
        Ok(ProfileNumber::from(index))
    }

    fn profile_count(&self) -> Result<u8> {
        let response = self.send(Command::GetDigitalProfilesCount)?;
        let count = response.data().first().copied().unwrap_or_default();
        if (MIN_VALID_PROFILE_COUNT..=MAX_PROFILES).contains(&count) {
            return Ok(count);
        }
        // `GetDigitalProfilesCount` is unsupported on 60HE+ ARM (returns error 0x66).
        // Fall back: probe via GetProfileMetadata until we hit an absent slot.
        // `.last()` gives the highest slot that responded; adding 1 yields the count.
        // If no slots respond, fall back to DEFAULT_PROFILE_COUNT.
        let probed_count = (0..MAX_PROFILES)
            .take_while(|&slot| self.profile_name(ProfileIndex { slot }).is_some())
            .last()
            .map_or(DEFAULT_PROFILE_COUNT, |slot| slot + 1);
        Ok(probed_count)
    }

    /// Returns the name of the profile at the given index, or `None` if the slot is absent.
    pub fn profile_name(&self, index: ProfileIndex) -> Option<String> {
        let response = self
            .send_for_profile(Command::GetProfileMetadata, index)
            .ok()?;
        parse_profile_name(response.data())
    }

    /// Returns all profiles and the currently active one in a single operation.
    pub fn profiles(&self) -> Result<ProfileListing> {
        let current = self.active_profile().ok();
        let count = self.profile_count()?;
        let profiles = (0..count)
            .map(|slot| {
                let index = ProfileIndex { slot };
                let number = ProfileNumber::from(index);
                let name = self.profile_name(index).unwrap_or_default();
                Profile {
                    number,
                    current: current == Some(number),
                    name,
                }
            })
            .collect();
        let listing = ProfileListing { profiles, current };
        Ok(listing)
    }

    /// Initialise, activate, and reload the profile at `index`.
    ///
    /// Caller is responsible for bounds-checking `index` against `profile_count`.
    fn activate(&self, index: ProfileIndex) -> Result<Profile> {
        self.init()?;
        self.send_for_profile(Command::ActivateProfile, index)?;
        thread::sleep(Duration::from_millis(PROFILE_SWITCH_SETTLE_MS));
        self.send_for_profile(Command::ReloadProfile, index)?;
        thread::sleep(Duration::from_millis(PROFILE_SWITCH_SETTLE_MS));
        let number = ProfileNumber::from(index);
        let name = self.profile_name(index).unwrap_or_default();
        let switched = Profile {
            number,
            current: true,
            name,
        };
        Ok(switched)
    }

    /// Bounds-check, initialise, activate, and reload the given profile.
    ///
    /// Returns the profile that was switched to, including its name.
    pub fn switch_to(&self, profile: ProfileNumber) -> Result<Profile> {
        let count = self.profile_count()?;
        let index = ProfileIndex::try_from(profile)?;
        if index.slot >= count {
            bail!("Profile {profile} does not exist (keyboard has {count} profiles)");
        }
        self.activate(index)
    }

    /// Switch to the next profile, wrapping from the last back to the first.
    pub fn switch_next(&self) -> Result<Profile> {
        let count = self.profile_count()?;
        let current = self.active_profile()?;
        let index = ProfileIndex::try_from(current.wrapping_next(count))?;
        self.activate(index)
    }

    /// Switch to the previous profile, wrapping from the first back to the last.
    pub fn switch_prev(&self) -> Result<Profile> {
        let count = self.profile_count()?;
        let current = self.active_profile()?;
        let index = ProfileIndex::try_from(current.wrapping_prev(count))?;
        self.activate(index)
    }
}

/// Test-only constructors for [`Profile`] and [`ProfileListing`].
///
/// The fields of these types are private; this module lets other test modules
/// build instances without exposing constructors in production code.
#[cfg(test)]
pub(crate) mod testutil {
    use super::{Profile, ProfileListing, ProfileNumber};

    pub(crate) fn profile(number: u8, current: bool, name: &str) -> Profile {
        Profile {
            number: ProfileNumber::from(number),
            current,
            name: name.to_string(),
        }
    }

    pub(crate) fn listing(profiles: Vec<Profile>, current: Option<u8>) -> ProfileListing {
        ProfileListing {
            profiles,
            current: current.map(ProfileNumber::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Protocol detection ──────────────────────────────────────────────────

    #[test]
    fn protocol_detected_from_standard_usage_page() {
        assert_eq!(Protocol::try_from(CFG_USAGE_PAGE), Ok(Protocol::Standard));
    }

    #[test]
    fn protocol_detected_from_arm_usage_page() {
        assert_eq!(Protocol::try_from(CFG_V3_USAGE_PAGE), Ok(Protocol::Arm));
    }

    #[test]
    fn protocol_unknown_usage_page_is_err() {
        assert_eq!(Protocol::try_from(0x0001u16), Err(()));
    }

    // ── Command wire values ─────────────────────────────────────────────────
    // These are load-bearing protocol constants; a reorder silently breaks firmware comms.

    #[test]
    fn command_wire_values_match_protocol_spec() {
        assert_eq!(u8::from(Command::Ping), 0);
        assert_eq!(u8::from(Command::GetDigitalProfilesCount), 9);
        assert_eq!(u8::from(Command::GetCurrentKeyboardProfileIndex), 11);
        assert_eq!(u8::from(Command::ActivateProfile), 23);
        assert_eq!(u8::from(Command::WootDevInit), 33);
        assert_eq!(u8::from(Command::ReloadProfile), 38);
        assert_eq!(u8::from(Command::GetProfileMetadata), 55);
    }

    // ── Packet layout ───────────────────────────────────────────────────────

    #[test]
    fn standard_packet_has_correct_framing_bytes() {
        let packet = Protocol::Standard.build_packet(Command::Ping);
        assert_eq!(packet[0], STANDARD_REPORT_ID);
        assert_eq!(packet[1], STANDARD_PROTOCOL_BYTE);
        assert_eq!(packet[2], WOOTING_COMMAND_MAGIC);
        assert_eq!(packet[3], 0); // Ping == 0
        assert_eq!(&packet[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn arm_packet_has_correct_framing_bytes() {
        let packet = Protocol::Arm.build_packet(Command::Ping);
        assert_eq!(packet[0], ARM_REPORT_ID);
        assert_eq!(packet[1], ARM_PROTOCOL_BYTE);
        assert_eq!(packet[2], WOOTING_COMMAND_MAGIC);
        assert_eq!(packet[3], 0);
        assert_eq!(&packet[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn standard_profile_packet_places_slot_at_byte_7() {
        let index = ProfileIndex { slot: 3 };
        let packet = Protocol::Standard.build_packet_for_profile(Command::ActivateProfile, index);
        assert_eq!(packet[PROFILE_SLOT_BYTE_STANDARD], 3);
        assert_eq!(packet[PROFILE_SLOT_BYTE_ARM], 0); // ARM slot byte must be untouched
    }

    #[test]
    fn arm_profile_packet_places_slot_at_byte_4() {
        let index = ProfileIndex { slot: 3 };
        let packet = Protocol::Arm.build_packet_for_profile(Command::ActivateProfile, index);
        assert_eq!(packet[PROFILE_SLOT_BYTE_ARM], 3);
        assert_eq!(packet[PROFILE_SLOT_BYTE_STANDARD], 0); // standard slot byte must be untouched
    }

    // ── ProfileNumber ↔ ProfileIndex ────────────────────────────────────────

    #[test]
    fn profile_index_to_number_adds_one() {
        assert_eq!(
            ProfileNumber::from(ProfileIndex { slot: 0 }),
            ProfileNumber::from(1u8)
        );
        assert_eq!(
            ProfileNumber::from(ProfileIndex { slot: 7 }),
            ProfileNumber::from(8u8)
        );
    }

    #[test]
    fn profile_number_to_index_subtracts_one() {
        let index = ProfileIndex::try_from(ProfileNumber::from(1u8)).unwrap();
        assert_eq!(index.slot, 0);
        let index = ProfileIndex::try_from(ProfileNumber::from(8u8)).unwrap();
        assert_eq!(index.slot, 7);
    }

    #[test]
    fn profile_number_zero_is_rejected_as_index() {
        assert!(ProfileIndex::try_from(ProfileNumber::from(0u8)).is_err());
    }

    // ── ProfileNumber wrapping ──────────────────────────────────────────────

    #[test]
    fn wrapping_next_advances_by_one() {
        assert_eq!(
            ProfileNumber::from(2u8).wrapping_next(4),
            ProfileNumber::from(3u8)
        );
    }

    #[test]
    fn wrapping_next_rolls_over_from_last_to_first() {
        assert_eq!(
            ProfileNumber::from(4u8).wrapping_next(4),
            ProfileNumber::from(1u8)
        );
    }

    #[test]
    fn wrapping_prev_retreats_by_one() {
        assert_eq!(
            ProfileNumber::from(3u8).wrapping_prev(4),
            ProfileNumber::from(2u8)
        );
    }

    #[test]
    fn wrapping_prev_rolls_over_from_first_to_last() {
        assert_eq!(
            ProfileNumber::from(1u8).wrapping_prev(4),
            ProfileNumber::from(4u8)
        );
    }

    #[test]
    fn wrapping_next_and_prev_are_inverses() {
        let start = ProfileNumber::from(2u8);
        assert_eq!(start.wrapping_next(4).wrapping_prev(4), start);
        assert_eq!(start.wrapping_prev(4).wrapping_next(4), start);
    }

    // ── Response data slicing ───────────────────────────────────────────────

    #[test]
    fn response_data_strips_five_byte_header() {
        let bytes = vec![0u8, 1, 2, 3, 4, 10, 20, 30];
        let response = Response { bytes };
        assert_eq!(response.data(), &[10u8, 20, 30]);
    }

    #[test]
    fn response_data_is_empty_when_shorter_than_header() {
        let response = Response {
            bytes: vec![0u8, 1, 2],
        };
        assert_eq!(response.data(), &[] as &[u8]);
    }

    // ── Protobuf name parsing ───────────────────────────────────────────────

    /// Builds the data payload (post-header) for a present profile with the given name.
    fn metadata_data(name: &str) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        let name_len = u8::try_from(name_bytes.len()).expect("test name fits in u8");
        let mut data = Vec::new();
        data.push(name_len + 2); // payload_length: non-zero = slot occupied
        data.push(0x00); // padding
        data.push(METADATA_NAME_TAG); // 0x0A
        data.push(name_len); // field length
        data.extend_from_slice(name_bytes);
        data
    }

    #[test]
    fn parse_profile_name_returns_name_for_valid_payload() {
        assert_eq!(
            parse_profile_name(&metadata_data("Gaming")),
            Some("Gaming".to_string())
        );
    }

    #[test]
    fn parse_profile_name_returns_none_for_absent_slot() {
        // payload_length == 0 signals no profile at this slot
        let data = [
            METADATA_ABSENT_PAYLOAD_LENGTH,
            0x00,
            METADATA_NAME_TAG,
            4,
            b'T',
            b'e',
            b's',
            b't',
        ];
        assert_eq!(parse_profile_name(&data), None);
    }

    #[test]
    fn parse_profile_name_returns_none_for_wrong_protobuf_tag() {
        let mut data = metadata_data("Test");
        data[METADATA_PROTOBUF_START] = 0x0B; // wrong tag
        assert_eq!(parse_profile_name(&data), None);
    }

    #[test]
    fn parse_profile_name_returns_none_for_truncated_data() {
        assert_eq!(parse_profile_name(&[]), None);
        assert_eq!(parse_profile_name(&[1u8, 0, 0x0A]), None); // tag present but length missing
    }

    #[test]
    fn parse_profile_name_returns_none_for_invalid_utf8() {
        let mut data = metadata_data("ok");
        // Overwrite the name bytes with invalid UTF-8
        let start = METADATA_PROTOBUF_START + PROTOBUF_FIELD_DATA;
        data[start] = 0xFF;
        data[start + 1] = 0xFE;
        assert_eq!(parse_profile_name(&data), None);
    }

    // ── Display formatting ──────────────────────────────────────────────────

    #[test]
    fn profile_display_formats_number_and_name() {
        let profile = testutil::profile(2, false, "FPS");
        assert_eq!(profile.to_string(), "Profile 2 — FPS");
    }

    #[test]
    fn profile_listing_display_marks_active_profile() {
        let listing = testutil::listing(
            vec![
                testutil::profile(1, false, "Default"),
                testutil::profile(2, true, "Gaming"),
                testutil::profile(3, false, "Office"),
            ],
            Some(2),
        );
        assert_eq!(
            listing.to_string(),
            "  Profile 1 — Default\n* Profile 2 — Gaming (current)\n  Profile 3 — Office\n",
        );
    }

    #[test]
    fn profile_listing_display_with_no_active_profile() {
        let listing = testutil::listing(vec![testutil::profile(1, false, "Default")], None);
        assert_eq!(listing.to_string(), "  Profile 1 — Default\n");
    }
}
