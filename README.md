# wootswitch

A Rust CLI for switching Wooting keyboard profiles directly via HID — no Wootility required.

## Usage

```
wootswitch [OPTIONS] [COMMAND]

Commands:
  switch <N>   Switch to profile N (1-based)

Options:
  -c, --current       Print only the current profile number
  -D, --list-devices  List all detected Wooting HID interfaces (for debugging)
  -j, --json          Output as JSON
  -h, --help          Print help
```

### Examples

```sh
# List all profiles with the active one marked
wootswitch

# Switch to profile 2
wootswitch switch 2

# Print the current profile number
wootswitch --current

# JSON output (useful for scripting and status bars)
wootswitch --json
wootswitch --current --json
```

### JSON output

`wootswitch --json` outputs the full profile listing:

```json
{
  "profiles": [
    { "number": 1, "current": false, "name": "Gaming" },
    { "number": 2, "current": true,  "name": "Office" },
    { "number": 3, "current": false, "name": "Media" }
  ],
  "current": 2
}
```

`wootswitch switch 2 --json` outputs:

```json
{ "switched_to": 2 }
```

`wootswitch --current --json` outputs:

```json
{ "current": 2 }
```

## Supported devices

Any Wooting keyboard. Two firmware variants are supported:

| Variant  | HID `usage_page` | Known devices                        |
|----------|-----------------|--------------------------------------|
| Standard | `0x1337`        | Wooting One, Two, 60HE (original)    |
| ARM      | `0xFF55`        | Wooting 60HE+, Two HE ARM, and later |

Profile names are read directly from the keyboard firmware via `GetProfileMetadata`
(command 55). No state files, no config files, no Wootility dependency.

## Installation

### NixOS (flake)

Add to your flake inputs and enable the NixOS module:

```nix
inputs.wootswitch.url = "github:clemenscodes/wootswitch";

# In your NixOS configuration:
imports = [ inputs.wootswitch.nixosModules.default ];
programs.wootswitch.enable = true;
```

The module installs the binary and configures udev rules so the device is accessible
without root.

### From source

Requires Rust (see `rust-toolchain.toml`) and `libudev`.

```sh
cargo build --release
```

## udev rules

Raw HID access to Wooting devices requires a udev rule. The NixOS module installs this
automatically. For other distributions, add:

```
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="31e3", TAG+="uaccess"
```

Then reload rules: `sudo udevadm control --reload && sudo udevadm trigger`.

## Protocol

See [`docs/hid-protocol.md`](docs/hid-protocol.md) for a full account of the reverse-engineered
Wooting HID wire protocol, including packet format, command IDs, ARM vs. Standard differences,
and the profile switching sequence.
