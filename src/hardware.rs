//! Bounded, best-effort Linux hwmon and DRM reads; no vendor libraries or subprocesses.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_DIRECTORY_ENTRIES: usize = 512;
const MAX_HWMON: usize = 64;
const MAX_CARDS: usize = 16;
const MAX_SENSORS_PER_KIND: usize = 24;
const MAX_VALUE_BYTES: u64 = 4096;

pub struct Hardware {
    pub sensors: Vec<String>,
    pub gpus: Vec<String>,
}

impl Hardware {
    pub fn collect() -> Self {
        let mut groups: [Vec<String>; 4] = std::array::from_fn(|_| Vec::new());
        for directory in numbered_directories(Path::new("/sys/class/hwmon"), "hwmon", MAX_HWMON) {
            let driver = read_text(&directory.join("name"))
                .map(|name| ascii_name(&name))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| {
                    directory
                        .file_name()
                        .map(|name| ascii_name(&name.to_string_lossy()))
                        .unwrap_or_default()
                });
            for (kind, channel) in channels(&directory) {
                let group = &mut groups[kind as usize];
                if group.len() >= MAX_SENSORS_PER_KIND {
                    continue;
                }
                let Some(value) = read_sensor(&directory, kind, &channel) else {
                    continue;
                };
                let label = read_text(&directory.join(format!("{channel}_label")))
                    .map(|label| ascii_name(&label))
                    .filter(|label| !label.is_empty());
                let label = label.map(|label| format!(" ({label})")).unwrap_or_default();
                group.push(format!("{driver}/{channel}{label}: {value}"));
            }
        }
        let sensors = groups.into_iter().flatten().collect();
        let gpus = numbered_directories(Path::new("/sys/class/drm"), "card", MAX_CARDS)
            .into_iter()
            .map(|card| gpu_line(&card))
            .collect();
        Self { sensors, gpus }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SensorKind {
    Temperature,
    Voltage,
    Fan,
    Power,
}

// Limit each directory traversal, including entries that are not useful to us.
fn numbered_directories(root: &Path, prefix: &str, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut directories: Vec<(u32, PathBuf)> = entries
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let number = numbered_name(name.to_str()?, prefix)?;
            Some((number, entry.path()))
        })
        .collect();
    directories.sort_unstable_by_key(|(number, _)| *number);
    directories.truncate(limit);
    directories.into_iter().map(|(_, path)| path).collect()
}

fn numbered_name(name: &str, prefix: &str) -> Option<u32> {
    let number = name.strip_prefix(prefix)?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

fn channels(directory: &Path) -> BTreeSet<(SensorKind, String)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return BTreeSet::new();
    };
    entries
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let (kind, channel) = sensor_channel(name.to_str()?)?;
            Some((kind, channel.to_owned()))
        })
        .collect()
}

fn sensor_channel(name: &str) -> Option<(SensorKind, &str)> {
    let (channel, suffix) = name.rsplit_once('_')?;
    for (prefix, kind) in [
        ("temp", SensorKind::Temperature),
        ("in", SensorKind::Voltage),
        ("fan", SensorKind::Fan),
        ("power", SensorKind::Power),
    ] {
        if numbered_name(channel, prefix).is_some()
            && (suffix == "input" || (kind == SensorKind::Power && suffix == "average"))
        {
            return Some((kind, channel));
        }
    }
    None
}

fn read_sensor(directory: &Path, kind: SensorKind, channel: &str) -> Option<String> {
    let input = || read_text(&directory.join(format!("{channel}_input")));
    if kind == SensorKind::Power {
        // Average and instantaneous readings describe the same channel. Prefer the
        // average, but retain a valid instantaneous reading if average is absent/bad.
        return read_text(&directory.join(format!("{channel}_average")))
            .and_then(|value| format_sensor(kind, &value))
            .or_else(|| input().and_then(|value| format_sensor(kind, &value)));
    }
    format_sensor(kind, &input()?)
}

fn format_sensor(kind: SensorKind, input: &str) -> Option<String> {
    match kind {
        SensorKind::Temperature => {
            let value: i64 = input.trim().parse().ok()?;
            // Below absolute zero is an invalid sensor value, not a temperature.
            (value >= -273_150).then(|| format!("{:.1} C", value as f64 / 1000.0))
        }
        SensorKind::Voltage => {
            let value: i64 = input.trim().parse().ok()?;
            Some(format!("{:.3} V", value as f64 / 1000.0))
        }
        SensorKind::Fan => {
            let value: u64 = input.trim().parse().ok()?;
            Some(format!("{value} RPM"))
        }
        SensorKind::Power => {
            let value: u64 = input.trim().parse().ok()?;
            Some(format!("{:.2} W", value as f64 / 1_000_000.0))
        }
    }
}

fn gpu_line(card: &Path) -> String {
    let device = card.join("device");
    let vendor = match read_text(&device.join("vendor")).as_deref().map(str::trim) {
        Some("0x1002") => "AMD",
        Some("0x8086") => "Intel",
        Some("0x10de") => "NVIDIA",
        _ => "GPU",
    };
    let name = card
        .file_name()
        .map(|name| ascii_name(&name.to_string_lossy()))
        .unwrap_or_default();
    let busy = read_u64(&device.join("gpu_busy_percent"))
        .filter(|value| *value <= 100)
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "unavailable".to_owned());
    let memory = match (
        read_u64(&device.join("mem_info_vram_used")),
        read_u64(&device.join("mem_info_vram_total")),
    ) {
        (Some(used), Some(total)) if total > 0 && used <= total => {
            format!(
                "{:.0}/{:.0} MiB",
                used as f64 / 1_048_576.0,
                total as f64 / 1_048_576.0
            )
        }
        _ => "unavailable".to_owned(),
    };
    let frequency = gpu_frequency(card, &device)
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| "unavailable".to_owned());
    let mut line = format!("{vendor} {name}: busy {busy} | VRAM {memory} | freq {frequency}");
    // DRM's own hwmon link supplies useful temperature/power even if this driver
    // exposes no utilization or VRAM counters (common without vendor libraries).
    let mut temperature = None;
    let mut power = None;
    for hwmon in numbered_directories(&device.join("hwmon"), "hwmon", 4) {
        for (kind, channel) in channels(&hwmon) {
            if kind == SensorKind::Temperature && temperature.is_none() {
                temperature = read_sensor(&hwmon, kind, &channel);
            } else if kind == SensorKind::Power && power.is_none() {
                power = read_sensor(&hwmon, kind, &channel);
            }
            if temperature.is_some() && power.is_some() {
                break;
            }
        }
        if temperature.is_some() && power.is_some() {
            break;
        }
    }
    if let Some(value) = temperature {
        line.push_str(&format!(" | {value}"));
    }
    if let Some(value) = power {
        line.push_str(&format!(" | {value}"));
    }
    line
}

fn gpu_frequency(card: &Path, device: &Path) -> Option<u64> {
    // i915: MHz, both the older card-level files and the per-GT interface.
    for relative in [
        "gt_act_freq_mhz",
        "gt_cur_freq_mhz",
        "gt/gt0/rps_act_freq_mhz",
        "gt/gt0/rps_cur_freq_mhz",
    ] {
        if let Some(value) = read_u64(&card.join(relative)) {
            return Some(value);
        }
    }
    // Xe exposes MHz beneath each tile/GT; show the first readable GT.
    for tile in numbered_directories(device, "tile", 4) {
        for gt in numbered_directories(&tile, "gt", 4) {
            for relative in ["freq0/act_freq", "freq0/cur_freq"] {
                if let Some(value) = read_u64(&gt.join(relative)) {
                    return Some(value);
                }
            }
        }
    }
    // amdgpu lists the supported states with an asterisk on the active state.
    active_dpm_frequency(&read_text(&device.join("pp_dpm_sclk"))?)
}

fn active_dpm_frequency(input: &str) -> Option<u64> {
    input.lines().find_map(|line| {
        let (_, state) = line.split_once(':')?;
        let mut fields = state.split_whitespace();
        let frequency = fields.next()?;
        if fields.next()? != "*" || fields.next().is_some() {
            return None;
        }
        frequency
            .strip_suffix("Mhz")
            .or_else(|| frequency.strip_suffix("MHz"))?
            .parse()
            .ok()
    })
}

fn read_u64(path: &Path) -> Option<u64> {
    read_text(path)?.trim().parse().ok()
}

fn read_text(path: &Path) -> Option<String> {
    let mut text = String::new();
    // Read one byte beyond the bound so a truncated value cannot become valid.
    File::open(path)
        .ok()?
        .take(MAX_VALUE_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= MAX_VALUE_BYTES as usize).then_some(text)
}

fn ascii_name(input: &str) -> String {
    input
        .trim()
        .chars()
        .take(40)
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drm_cards_exclude_connectors_and_render_nodes() {
        assert_eq!(numbered_name("card0", "card"), Some(0));
        assert_eq!(numbered_name("card12", "card"), Some(12));
        for name in [
            "card",
            "card0-HDMI-A-1",
            "renderD128",
            "card-1",
            "card4294967296",
        ] {
            assert_eq!(numbered_name(name, "card"), None);
        }
    }

    #[test]
    fn only_supported_sensor_files_are_channels() {
        assert_eq!(
            sensor_channel("temp1_input"),
            Some((SensorKind::Temperature, "temp1"))
        );
        assert_eq!(
            sensor_channel("in0_input"),
            Some((SensorKind::Voltage, "in0"))
        );
        assert_eq!(
            sensor_channel("power2_average"),
            Some((SensorKind::Power, "power2"))
        );
        assert_eq!(
            sensor_channel("power2_input"),
            Some((SensorKind::Power, "power2"))
        );
        for name in [
            "temp1_max",
            "temp1_label",
            "fan1_average",
            "in_input",
            "power1_cap",
        ] {
            assert_eq!(sensor_channel(name), None);
        }
    }

    #[test]
    fn sensor_units_and_signed_values_are_preserved() {
        assert_eq!(
            format_sensor(SensorKind::Temperature, "-12500\n").as_deref(),
            Some("-12.5 C")
        );
        assert_eq!(
            format_sensor(SensorKind::Voltage, "12000").as_deref(),
            Some("12.000 V")
        );
        assert_eq!(
            format_sensor(SensorKind::Voltage, "-5000").as_deref(),
            Some("-5.000 V")
        );
        assert_eq!(
            format_sensor(SensorKind::Power, "12500000").as_deref(),
            Some("12.50 W")
        );
        assert_eq!(
            format_sensor(SensorKind::Fan, "0").as_deref(),
            Some("0 RPM")
        );
        assert_eq!(
            format_sensor(SensorKind::Fan, "1234").as_deref(),
            Some("1234 RPM")
        );
    }

    #[test]
    fn invalid_sensor_reads_do_not_turn_into_zero() {
        for kind in [
            SensorKind::Temperature,
            SensorKind::Voltage,
            SensorKind::Fan,
            SensorKind::Power,
        ] {
            for input in ["", "unavailable", "42 extra", "18446744073709551616"] {
                assert_eq!(format_sensor(kind, input), None);
            }
        }
        assert_eq!(format_sensor(SensorKind::Temperature, "-273151"), None);
        assert_eq!(format_sensor(SensorKind::Fan, "-1"), None);
        assert_eq!(format_sensor(SensorKind::Power, "-1"), None);
    }

    #[test]
    fn amd_frequency_requires_an_active_state() {
        assert_eq!(
            active_dpm_frequency("0: 500Mhz\n1: 1200Mhz *\n"),
            Some(1200)
        );
        assert_eq!(active_dpm_frequency("0: 800MHz *\n"), Some(800));
        assert_eq!(active_dpm_frequency("0: 500Mhz\n"), None);
        assert_eq!(active_dpm_frequency("0: badMhz *\n"), None);
        assert_eq!(active_dpm_frequency("0: 800Mhz * extra\n"), None);
    }

    #[test]
    fn labels_are_bounded_ascii() {
        assert_eq!(ascii_name("  CPU\n\u{1b}温度  "), "CPU????");
        assert_eq!(ascii_name(&"x".repeat(100)).len(), 40);
    }
}
