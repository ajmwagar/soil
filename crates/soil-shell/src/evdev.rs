//! The phone's keypad, read into `soil:present/input` key events.
//!
//! A framebuffer host has no window system to deliver keys, so it reads the
//! kernel's input device itself. The names emitted here are the normalized
//! ones the surface contract defines; the Spore maps them to whatever its own
//! UI calls a key.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use soil_webgpu::{InputEvent, InputKey};

/// `struct input_event` is 24 bytes on 64-bit Linux: a 16-byte `timeval`, then
/// type, code and value.
const EVENT_BYTES: usize = 24;
const EV_KEY: u16 = 1;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;

/// The queue the guest polls. `soil-webgpu` owns the same handle.
pub type Queue = Arc<Mutex<VecDeque<InputEvent>>>;

/// Read `path` on a background thread, pushing every key press and release.
///
/// Reads block, so the thread costs nothing between keys. A read error ends the
/// thread: a device that disappears is not recoverable, and pretending
/// otherwise would leave a guest waiting on keys that can never arrive.
pub fn spawn(path: &Path, queue: Queue) -> Result<()> {
    let mut device =
        File::open(path).with_context(|| format!("open input device {}", path.display()))?;
    std::thread::Builder::new()
        .name("soil-evdev".into())
        .spawn(move || {
            let mut shift = false;
            loop {
                let mut event = [0u8; EVENT_BYTES];
                if device.read_exact(&mut event).is_err() {
                    return;
                }
                let kind = u16::from_ne_bytes([event[16], event[17]]);
                let code = u16::from_ne_bytes([event[18], event[19]]);
                let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
                if kind != EV_KEY {
                    continue;
                }
                match code {
                    KEY_LEFTSHIFT | KEY_RIGHTSHIFT => {
                        shift = value != 0;
                        continue;
                    }
                    _ => {}
                }
                // 2 is autorepeat. The UI runs its own long-press timers, so a
                // repeat would arrive as a second press rather than a hold.
                if value != 0 && value != 1 {
                    continue;
                }
                let Some(key) = key_name(code) else {
                    if std::env::var_os("SOIL_INPUT_TRACE").is_some() {
                        eprintln!("soil-evdev: code={code} value={value} (unmapped)");
                    }
                    continue;
                };
                if std::env::var_os("SOIL_INPUT_TRACE").is_some() {
                    eprintln!("soil-evdev: code={code} name={key} value={value}");
                }
                let event = InputEvent::Key(InputKey {
                    key: key.to_string(),
                    text: None,
                    pressed: value == 1,
                    shift,
                    control: false,
                    alt: false,
                    meta: false,
                });
                queue.lock().expect("input queue poisoned").push_back(event);
            }
        })
        .context("spawn the input reader")?;
    Ok(())
}

/// A Linux key code as a normalized `soil:present/input` name.
///
/// The codes are the ones this phone's keypad actually advertises
/// (`/sys/class/input/event0/device/capabilities/key`), not a guess at a
/// generic keyboard: `KEY_NUMERIC_STAR`/`POUND` for `*`/`#`, `KEY_PHONE` for
/// the green key, `KEY_MICMUTE` for push-to-talk, `KEY_NUMERIC_A`/`B` and
/// `KEY_APPSELECT` for the three unlabelled soft keys.
pub fn key_name(code: u16) -> Option<&'static str> {
    Some(match code {
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        14 => "backspace",
        28 | 96 => "enter",
        103 => "arrow-up",
        105 => "arrow-left",
        106 => "arrow-right",
        108 => "arrow-down",
        // KEY_BACK: the keypad's back arrow, beside the d-pad.
        158 => "back",
        168 => "end-call",
        169 => "call",
        172 => "home",
        114 => "volume-down",
        115 => "volume-up",
        248 => "push-to-talk",
        522 => "*",
        523 => "#",
        524 => "soft-left",
        525 => "soft-right",
        580 => "menu",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code the keypad advertises. Taken from the device's own
    /// `capabilities/key` bitmask: a key the hardware can send that this table
    /// does not name is input the phone silently swallows.
    const KEYPAD_CODES: &[u16] = &[
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 14, 28, 103, 105, 106, 108, 158, 169, 172, 248, 522,
        523, 524, 525, 580,
    ];

    #[test]
    fn every_key_the_keypad_advertises_has_a_name() {
        for &code in KEYPAD_CODES {
            assert!(key_name(code).is_some(), "keypad code {code} is unmapped");
        }
    }

    #[test]
    fn digits_are_the_digits_the_keys_print() {
        // KEY_1..KEY_0 are contiguous and descend through the printed digits.
        for (offset, digit) in "1234567890".chars().enumerate() {
            let code = 2 + offset as u16;
            assert_eq!(key_name(code), Some(digit.to_string().as_str()), "code {code}");
        }
    }

    #[test]
    fn unknown_codes_are_reported_not_guessed() {
        for code in [1, 12, 13, 99, 200, 999] {
            assert_eq!(key_name(code), None, "code {code} should not map");
        }
    }
}
