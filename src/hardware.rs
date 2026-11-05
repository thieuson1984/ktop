//! Bounded, best-effort Linux hwmon and DRM reads; no vendor libraries or subprocesses.

use std::fs::{self, File};
use std::io::{self, Read};
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
        Self::collect_from(Path::new("/sys/class/hwmon"), Path::new("/sys/class/drm"))
    }

    fn collect_from(hwmon_root: &Path, drm_root: &Path) -> Self {
        let mut groups: [Vec<String>; 4] = std::array::from_fn(|_| Vec::new());
        for directory in numbered_directories(hwmon_root, "hwmon", MAX_HWMON) {
            if groups
                .iter()
                .all(|group| group.len() >= MAX_SENSORS_PER_KIND)
            {
                break;
            }
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
        let gpus = numbered_directories(drm_root, "card", MAX_CARDS)
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

fn channels(directory: &Path) -> Vec<(SensorKind, String)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut channels: Vec<_> = entries
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let (kind, channel) = sensor_channel(name.to_str()?)?;
            Some((kind, channel.to_owned()))
        })
        .collect();
    channels.sort_unstable();
    channels.dedup();
    channels
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
    // A numeric input remains readable on some failed/disabled devices. The
    // hwmon ABI says those readings must not be trusted.
    for (suffix, expected) in [("enable", 1_u8), ("fault", 0_u8)] {
        match read_text_result(&directory.join(format!("{channel}_{suffix}"))) {
            Ok(value) if value.trim().parse::<u8>().ok() == Some(expected) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return None,
        }
    }
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
    let hwmons = numbered_directories(&device.join("hwmon"), "hwmon", 4);
    let frequency = gpu_frequency(card, &device, &hwmons)
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| "unavailable".to_owned());
    let mut line = format!("{vendor} {name}: busy {busy} | VRAM {memory} | freq {frequency}");
    // DRM's own hwmon link supplies useful temperature/power even if this driver
    // exposes no utilization or VRAM counters (common without vendor libraries).
    let mut temperature = None;
    let mut power = None;
    for hwmon in &hwmons {
        for (kind, channel) in channels(hwmon) {
            if kind == SensorKind::Temperature && temperature.is_none() {
                temperature = read_sensor(hwmon, kind, &channel);
            } else if kind == SensorKind::Power && power.is_none() {
                power = read_sensor(hwmon, kind, &channel);
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
        line.push_str(" | ");
        line.push_str(&value);
    }
    if let Some(value) = power {
        line.push_str(" | ");
        line.push_str(&value);
    }
    line
}

fn gpu_frequency(card: &Path, device: &Path, hwmons: &[PathBuf]) -> Option<u64> {
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
    // amdgpu's hwmon gfx clock is in Hz, unlike the MHz DRM interfaces.
    // Do not mistake another driver's freq1 channel (possibly memory) for gfx.
    for hwmon in hwmons {
        if read_text(&hwmon.join("name")).as_deref().map(str::trim) != Some("amdgpu") {
            continue;
        }
        if let Some(hz) = read_u64(&hwmon.join("freq1_input")) {
            return Some(hz / 1_000_000);
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
    read_text_result(path).ok()
}

fn read_text_result(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    // Read one byte beyond the bound so a truncated value cannot become valid.
    File::open(path)?
        .take(MAX_VALUE_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() > MAX_VALUE_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sysfs value exceeds read limit",
        ));
    }
    Ok(text)
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
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let root = std::env::temp_dir().join(format!(
                    "ktop-hardware-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed),
                ));
                match fs::create_dir(&root) {
                    Ok(()) => return Self(root),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create synthetic sysfs fixture: {error}"),
                }
            }
        }

        fn put(&self, relative: &str, value: impl AsRef<[u8]>) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

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

    #[test]
    fn failed_disabled_or_unreadable_status_sensors_are_not_trusted() {
        let fixture = Fixture::new();
        fixture.put("hwmon0/temp1_input", "42000\n");
        let directory = fixture.0.join("hwmon0");
        assert_eq!(
            read_sensor(&directory, SensorKind::Temperature, "temp1").as_deref(),
            Some("42.0 C")
        );
        fixture.put("hwmon0/temp1_enable", "0\n");
        assert_eq!(
            read_sensor(&directory, SensorKind::Temperature, "temp1"),
            None
        );
        fixture.put("hwmon0/temp1_enable", "1\n");
        fixture.put("hwmon0/temp1_fault", "1\n");
        assert_eq!(
            read_sensor(&directory, SensorKind::Temperature, "temp1"),
            None
        );
        fixture.put("hwmon0/temp1_fault", "0\n");
        assert_eq!(
            read_sensor(&directory, SensorKind::Temperature, "temp1").as_deref(),
            Some("42.0 C")
        );
        fixture.put("hwmon0/temp1_fault", "invalid\n");
        assert_eq!(
            read_sensor(&directory, SensorKind::Temperature, "temp1"),
            None
        );
        fs::remove_file(directory.join("temp1_fault")).unwrap();
        fs::create_dir(directory.join("temp1_fault")).unwrap();
        assert_eq!(
            read_sensor(&directory, SensorKind::Temperature, "temp1"),
            None
        );
    }

    #[test]
    fn power_channel_is_deduplicated_and_falls_back_only_to_valid_input() {
        let fixture = Fixture::new();
        fixture.put("hwmon0/power1_input", "12500000\n");
        fixture.put("hwmon0/power1_average", "invalid\n");
        let directory = fixture.0.join("hwmon0");
        assert_eq!(channels(&directory).len(), 1);
        assert_eq!(
            read_sensor(&directory, SensorKind::Power, "power1").as_deref(),
            Some("12.50 W")
        );
        fixture.put("hwmon0/power1_average", "25000000\n");
        assert_eq!(
            read_sensor(&directory, SensorKind::Power, "power1").as_deref(),
            Some("25.00 W")
        );
    }

    #[test]
    fn amdgpu_hwmon_clock_uses_hertz_and_does_not_claim_other_driver_channels() {
        let fixture = Fixture::new();
        fixture.put("card0/device/hwmon/hwmon0/name", "amdgpu\n");
        fixture.put("card0/device/hwmon/hwmon0/freq1_input", "1234500000\n");
        let card = fixture.0.join("card0");
        let device = card.join("device");
        let hwmons = numbered_directories(&device.join("hwmon"), "hwmon", 4);
        assert_eq!(gpu_frequency(&card, &device, &hwmons), Some(1234));
        fixture.put("card0/device/hwmon/hwmon0/name", "other_driver\n");
        assert_eq!(gpu_frequency(&card, &device, &hwmons), None);
        fixture.put("card0/device/pp_dpm_sclk", "0: 500Mhz\n1: 900Mhz *\n");
        assert_eq!(gpu_frequency(&card, &device, &hwmons), Some(900));
    }

    #[test]
    fn intel_i915_and_xe_frequency_paths_are_read_as_megahertz() {
        let fixture = Fixture::new();
        fixture.put("card0/device/tile1/gt2/freq0/act_freq", "1350\n");
        let card = fixture.0.join("card0");
        let device = card.join("device");
        assert_eq!(gpu_frequency(&card, &device, &[]), Some(1350));
        fixture.put("card0/gt/gt0/rps_act_freq_mhz", "725\n");
        assert_eq!(gpu_frequency(&card, &device, &[]), Some(725));
    }

    #[test]
    fn removed_hardware_and_failed_counters_are_not_retained() {
        let fixture = Fixture::new();
        fixture.put("hwmon/hwmon0/name", "synthetic\n");
        fixture.put("hwmon/hwmon0/temp1_input", "42000\n");
        fixture.put("drm/card0/device/vendor", "0x1002\n");
        fixture.put("drm/card0/device/gpu_busy_percent", "101\n");
        fixture.put("drm/card0/device/mem_info_vram_used", "3\n");
        fixture.put("drm/card0/device/mem_info_vram_total", "2\n");
        let hwmon = fixture.0.join("hwmon");
        let drm = fixture.0.join("drm");
        let first = Hardware::collect_from(&hwmon, &drm);
        assert_eq!(first.sensors, ["synthetic/temp1: 42.0 C"]);
        assert_eq!(first.gpus.len(), 1);
        assert!(first.gpus[0].contains("busy unavailable | VRAM unavailable"));
        fs::remove_file(hwmon.join("hwmon0/temp1_input")).unwrap();
        fs::remove_dir_all(drm.join("card0")).unwrap();
        let next = Hardware::collect_from(&hwmon, &drm);
        assert!(next.sensors.is_empty() && next.gpus.is_empty());
    }

    #[test]
    fn oversized_and_non_utf8_sysfs_values_do_not_become_valid_prefixes() {
        let fixture = Fixture::new();
        fixture.put(
            "value",
            format!("1{}", " ".repeat(MAX_VALUE_BYTES as usize)),
        );
        assert_eq!(read_u64(&fixture.0.join("value")), None);
        fixture.put("value", b"42\xff");
        assert_eq!(read_u64(&fixture.0.join("value")), None);
    }
}
