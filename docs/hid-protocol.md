# Wooting HID Protocol — Reverse Engineering Notes

This document captures what was learned by reverse-engineering the Wooting HID command
protocol, primarily from a Wooting 60HE+ (ARM firmware, PID `0x1320`, usage_page `0xFF55`)
and cross-referencing with the community gist by BigBrainAFK:
<https://gist.github.com/BigBrainAFK/0ba454a1efb43f7cb6301cda8838f432>

---

## Device discovery

All Wooting devices share vendor ID `0x31E3`. Each physical keyboard exposes several HID
interfaces with different `usage_page` values. Only one interface accepts configuration
commands; the rest are input-only.

| `usage_page` | Interface purpose              |
|-------------|-------------------------------|
| `0x0001`    | Standard keyboard (key events) |
| `0x000C`    | Consumer control (media keys)  |
| `0xFF54`    | Analog values (periodic frames)|
| `0x1337`    | Config/command — Standard firmware |
| `0xFF55`    | Config/command — ARM firmware  |

The correct interface to open is the one with `usage_page == 0x1337` (standard) or
`usage_page == 0xFF55` (ARM). These two variants are modelled by the `Protocol` enum.

---

## Wire variants

Two distinct wire formats exist, identified by the `usage_page` of the config interface.

| Variant  | `usage_page` | Known devices                            |
|----------|-------------|------------------------------------------|
| Standard | `0x1337`    | Wooting One, Two, 60HE (original)        |
| ARM      | `0xFF55`    | Wooting 60HE+, Two HE ARM, and later     |

---

## Command packet format (8 bytes)

Every command is sent as an 8-byte HID **feature report**.

```
Byte 0: report_id     — 0x00 (Standard) | 0x01 (ARM)
Byte 1: protocol_byte — 0xD0 (Standard) | 0xD1 (ARM)
Byte 2: 0xDA          — fixed command channel framing marker
Byte 3: command_id    — see command table below
Byte 4: argument 3    — profile slot for ARM profile commands; otherwise 0
Byte 5: argument 2    — always 0 in current usage
Byte 6: argument 1    — always 0 in current usage
Byte 7: argument 0    — profile slot for Standard profile commands; otherwise 0
```

### Profile slot argument placement

The profile slot argument is placed at **different byte positions** depending on the
firmware variant:

| Variant  | Profile slot byte |
|----------|------------------|
| Standard | byte 7 (arg 0)   |
| ARM      | byte 4 (arg 3)   |

Commands that do not take a profile argument leave bytes 4–7 as zero.

---

## Response format

Responses are read via a blocking HID **input report read**.

| Variant  | Response buffer size |
|----------|---------------------|
| Standard | 256 bytes            |
| ARM      | 33 bytes (1 report ID + 32 data bytes) |

All responses share a 5-byte header:

```
Byte 0: report_id
Byte 1: protocol_byte (echo)
Byte 2: 0xDA (echo)
Byte 3: command_id echo
Byte 4: status (0x88 = success, 0x66 = unsupported/error)
Byte 5+: payload data
```

`DATA_OFFSET = 5` — all payload reads skip the first 5 bytes.

---

## Command reference

All 60 IDs (0–59) are present and sequential. IDs marked *(removed)* existed in older
firmware and are now no-ops or absent, but their wire IDs remain reserved.

| ID | Name                                    |
|----|-----------------------------------------|
| 0  | Ping                                    |
| 1  | GetVersion                              |
| 2  | ResetToBootloader                       |
| 3  | GetSerial                               |
| 4  | GetRgbProfileCount                      |
| 5  | *(removed)* GetCurrentRgbProfileIndex   |
| 6  | *(removed)* GetRgbMainProfile           |
| 7  | ReloadProfile0                          |
| 8  | SaveRgbProfile                          |
| 9  | GetDigitalProfilesCount                 |
| 10 | GetAnalogProfilesCount                  |
| 11 | GetCurrentKeyboardProfileIndex          |
| 12 | GetDigitalProfile                       |
| 13 | GetAnalogProfileMainPart                |
| 14 | GetAnalogProfileCurveChangeMapPart1     |
| 15 | GetAnalogProfileCurveChangeMapPart2     |
| 16 | GetNumberOfKeys                         |
| 17 | GetMainMappingProfile                   |
| 18 | GetFunctionMappingProfile               |
| 19 | GetDeviceConfig                         |
| 20 | GetAnalogValues                         |
| 21 | KeysOff                                 |
| 22 | KeysOn                                  |
| 23 | ActivateProfile                         |
| 24 | GetDksProfile                           |
| 25 | DoSoftReset                             |
| 26 | *(removed)* GetRgbColorsPart1           |
| 27 | *(removed)* GetRgbColorsPart2           |
| 28 | *(removed)* GetRgbEffects               |
| 29 | RefreshRgbColors                        |
| 30 | WootDevSingleColor                      |
| 31 | WootDevResetColor                       |
| 32 | WootDevResetAll                         |
| 33 | WootDevInit                             |
| 34 | *(removed)* GetRgbProfileBase           |
| 35 | GetRgbProfileColorsPart1                |
| 36 | GetRgbProfileColorsPart2                |
| 37 | *(removed)* GetRgbProfileEffect         |
| 38 | ReloadProfile                           |
| 39 | GetKeyboardProfile                      |
| 40 | GetGamepadMapping                       |
| 41 | GetGamepadProfile                       |
| 42 | SaveKeyboardProfile                     |
| 43 | ResetSettings                           |
| 44 | SetRawScanning                          |
| 45 | StartXinputDetection                    |
| 46 | StopXinputDetection                     |
| 47 | SaveDksProfile                          |
| 48 | GetMappingProfile                       |
| 49 | GetActuationProfile                     |
| 50 | GetRgbProfileCore                       |
| 51 | GetGlobalSettings                       |
| 52 | GetAkcProfile                           |
| 53 | SaveAkcProfile                          |
| 54 | GetRapidTriggerProfile                  |
| 55 | GetProfileMetadata                      |
| 56 | IsFlashChipConnected                    |
| 57 | GetRgbLayer                             |
| 58 | GetFlashStats                           |
| 59 | GetRgbBins                              |

---

## Command details

### WootDevInit (33)

Must be sent before any profile-switching command. Initialises the command session.
No arguments. No meaningful payload in the response.

### GetCurrentKeyboardProfileIndex (11)

Returns the active profile slot. **ARM and Standard differ critically here.**

**Standard:** `data[0]` = runtime-active profile slot (0-based).

**ARM:** The response contains two values:
- `data[0]` = flash-stored default profile (never changes via software; reflects the
  physical profile stored on the keyboard)
- `data[2]` = **runtime-active profile** (updated in RAM by `ActivateProfile`)

On ARM, always read `data[2]`. Reading `data[0]` will return a stale value that does not
reflect software-initiated profile switches.

### GetDigitalProfilesCount (9)

Returns the number of configured profiles as `data[0]`.

**ARM caveat:** This command returns status `0x66` (unsupported) on ARM firmware (confirmed
on 60HE+). The count must be probed instead: send `GetProfileMetadata` for slots 0, 1, 2, …
in order and stop at the first slot that returns `payload_length == 0`. The number of
successful probes is the profile count.

If probing also fails (all slots return zero), a safe fallback of 4 profiles is used.

### ActivateProfile (23)

Switches the active profile in RAM without persisting to flash. The profile slot argument
goes in **byte 4** on ARM and **byte 7** on Standard.

Must be preceded by `WootDevInit`. A 100 ms settle delay after sending is recommended
before sending `ReloadProfile`.

### ReloadProfile (38)

Reloads the profile's LED and key configuration from flash into the active state. Send
after `ActivateProfile` with the same slot. Same argument placement rules as
`ActivateProfile`.

### GetProfileMetadata (55)

Returns protobuf-encoded metadata for the requested profile slot. The profile slot
argument goes in byte 4 (ARM) or byte 7 (Standard).

Response payload layout after `DATA_OFFSET`:

```
data[0]:   payload_length — 0 means no profile exists at this slot
data[1]:   0x00 padding
data[2..]: protobuf bytes
```

The protobuf encodes a single field-1 string (wire type 2 = length-delimited):

```
protobuf[0]: 0x0A — field 1, wire type 2 tag
protobuf[1]: name_length — byte count of the UTF-8 name
protobuf[2..2+name_length]: profile name (UTF-8)
```

---

## Profile switching sequence

To switch to profile N (1-based):

1. Validate that N is within the profile count.
2. Send `WootDevInit` (cmd 33).
3. Send `ActivateProfile` (cmd 23) with profile slot `N - 1`.
4. Wait 100 ms.
5. Send `ReloadProfile` (cmd 38) with profile slot `N - 1`.
6. Wait 100 ms.

The two sleep delays were determined empirically; the firmware needs settling time between
the activate and reload commands, and after reload before the change is fully visible.
