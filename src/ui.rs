//! Small cell-based renderer. Geometry and styling never share a string with
//! untrusted procfs/sysfs data; each glyph occupies exactly one terminal column.
use crate::{
    Sort,
    hardware::Hardware,
    system::{Process, System},
};
use std::{fmt::Write as _, time::Duration};

#[derive(Clone, Copy, Default, PartialEq)]
enum Style {
    #[default]
    Text,
    Muted,
    Border,
    Title,
    Accent,
    Good,
    Warning,
    Error,
    Active,
}
impl Style {
    fn ansi(self) -> &'static str {
        match self {
            Self::Text => "\x1b[0m",
            Self::Muted => "\x1b[0;2m",
            Self::Border => "\x1b[0;90m",
            Self::Title => "\x1b[0;1m",
            Self::Accent => "\x1b[0;1;36m",
            Self::Good => "\x1b[0;32m",
            Self::Warning => "\x1b[0;33m",
            Self::Error => "\x1b[0;31m",
            Self::Active => "\x1b[0;1;7m",
        }
    }
}
#[derive(Clone, Copy)]
struct Cell {
    glyph: char,
    style: Style,
}
impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: ' ',
            style: Style::Text,
        }
    }
}
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}
impl Rect {
    fn width(self) -> usize {
        self.w.saturating_sub(4)
    }
}
#[derive(Clone, Copy)]
pub(super) struct View {
    pub sort: Sort,
    pub offset: usize,
    pub page: u8,
    pub paused: bool,
    pub interval: Duration,
}

pub(super) struct Frame {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    color: bool,
}
impl Frame {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
            color: std::env::var_os("NO_COLOR").is_none(),
        }
    }
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.cells.resize(width * height, Cell::default());
    }
    fn put(&mut self, x: usize, y: usize, glyph: char, style: Style) {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x] = Cell { glyph, style };
        }
    }
    fn text(&mut self, x: usize, y: usize, width: usize, text: &str, style: Style) {
        let width = width.min(self.width.saturating_sub(x));
        if width == 0 || y >= self.height {
            return;
        }
        let mut chars = text.chars();
        for i in 0..width {
            let Some(c) = chars.next() else {
                return;
            };
            // Printable ASCII is the only dynamic content allowed. Unicode is
            // reserved for the renderer's own single-column drawing glyphs.
            self.put(
                x + i,
                y,
                if c.is_ascii_graphic() || c == ' ' {
                    c
                } else {
                    '?'
                },
                style,
            );
        }
        if chars.next().is_some() {
            self.put(x + width - 1, y, '…', style);
        }
    }
    fn right(&mut self, x: usize, y: usize, width: usize, text: &str, style: Style) {
        let len = text.chars().count().min(width);
        self.text(x + width.saturating_sub(len), y, len, text, style);
    }
    fn line(&mut self, x: usize, y: usize, width: usize, glyph: char) {
        for i in 0..width {
            self.put(x + i, y, glyph, Style::Border);
        }
    }
    fn card(&mut self, r: Rect, title: &str, aside: &str) {
        if r.w < 4 || r.h < 3 {
            return;
        }
        self.line(r.x + 1, r.y, r.w - 2, '─');
        self.line(r.x + 1, r.y + r.h - 1, r.w - 2, '─');
        for y in r.y + 1..r.y + r.h - 1 {
            self.put(r.x, y, '│', Style::Border);
            self.put(r.x + r.w - 1, y, '│', Style::Border);
        }
        for (x, y, glyph) in [
            (r.x, r.y, '╭'),
            (r.x + r.w - 1, r.y, '╮'),
            (r.x, r.y + r.h - 1, '╰'),
            (r.x + r.w - 1, r.y + r.h - 1, '╯'),
        ] {
            self.put(x, y, glyph, Style::Border);
        }
        let title_width = (title.len() + 2).min(r.w - 4);
        self.text(
            r.x + 2,
            r.y,
            title_width,
            &format!(" {title} "),
            Style::Title,
        );
        if !aside.is_empty() && r.w > title_width + aside.len() + 8 {
            self.right(
                r.x + 2 + title_width,
                r.y,
                r.w - title_width - 4,
                &format!(" {aside} "),
                Style::Muted,
            );
        }
    }
    fn pair(&mut self, r: Rect, row: usize, label: &str, value: &str, style: Style) {
        if row >= r.h.saturating_sub(2) {
            return;
        }
        let width = r.width();
        let value_width = value.chars().count().min(width);
        self.text(
            r.x + 2,
            r.y + row + 1,
            width.saturating_sub(value_width + 2),
            label,
            Style::Muted,
        );
        self.right(r.x + 2, r.y + row + 1, width, value, style);
    }
    fn meter(&mut self, x: usize, y: usize, width: usize, value: f64) {
        let used = (value.clamp(0.0, 100.0) * width as f64 / 100.0).round() as usize;
        for i in 0..width {
            self.put(
                x + i,
                y,
                if i < used { '━' } else { '─' },
                if i < used {
                    usage_style(value)
                } else {
                    Style::Border
                },
            );
        }
    }
    fn columns<const N: usize>(&self, y: usize, height: usize) -> [Rect; N] {
        let available = self.width.saturating_sub(2 + N.saturating_sub(1));
        let base = available / N;
        let extra = available % N;
        let mut x = 1;
        std::array::from_fn(|i| {
            let w = base + usize::from(i < extra);
            let r = Rect { x, y, w, h: height };
            x += w + 1;
            r
        })
    }
    pub fn render(&mut self, s: &System, h: &Hardware, view: View) {
        self.cells.fill(Cell::default());
        if self.width < 41 || self.height < 12 {
            self.text(
                1,
                1,
                self.width.saturating_sub(2),
                "ktop - terminal too small",
                Style::Title,
            );
            self.text(
                1,
                3,
                self.width.saturating_sub(2),
                "Resize to 42 columns x 12 rows",
                Style::Muted,
            );
            self.text(
                1,
                self.height.saturating_sub(2),
                self.width.saturating_sub(2),
                "q Quit",
                Style::Accent,
            );
            return;
        }
        self.header(s, view);
        let start = if self.height >= 18 { 3 } else { 2 };
        match view.page {
            2 => self.hardware(s, h, view, start),
            3 => self.process_page(s, view, start),
            _ => self.overview(s, h, view, start),
        }
        let footer = if self.width >= 100 {
            "q Quit   1-3 Views   c/m/p Sort   j/k Scroll   Space Pause   +/- Refresh"
        } else if self.width >= 70 {
            "q Quit  1-3 Views  c/m/p Sort  j/k Scroll  Space Pause  +/- Rate"
        } else {
            "q Quit   1-3 Views   j/k Scroll"
        };
        self.text(2, self.height - 1, self.width - 4, footer, Style::Muted);
        if let Some(error) = &s.error {
            // Keep failures visible regardless of the selected page or card density.
            let y = if self.height >= 18 {
                2
            } else {
                self.height - 1
            };
            self.text(
                1,
                y,
                self.width - 2,
                &format!("WARNING: {error}"),
                Style::Error,
            );
        }
    }
    fn header(&mut self, s: &System, view: View) {
        self.text(1, 0, 4, "ktop", Style::Accent);
        if self.width >= 70 {
            self.text(7, 0, 32, "SYSTEM MONITOR", Style::Muted);
        }
        let status = format!(
            "{}  {:.1}s ",
            if view.paused { "PAUSED" } else { "LIVE" },
            view.interval.as_secs_f64()
        );
        self.right(
            self.width.saturating_sub(21),
            0,
            20,
            &status,
            if view.paused {
                Style::Warning
            } else {
                Style::Good
            },
        );
        let mut x = 1;
        for (page, label) in [(1, "Overview"), (2, "Hardware"), (3, "Processes")] {
            let tab = if view.page == page {
                format!("[{page} {label}]")
            } else {
                format!(" {page} {label} ")
            };
            self.text(
                x,
                1,
                tab.len(),
                &tab,
                if view.page == page {
                    Style::Active
                } else {
                    Style::Muted
                },
            );
            x += tab.len() + 1;
        }
        if self.height >= 18 {
            let meta = format!(
                "UP {}d {:02}h {:02}m  |  LOAD {}  |  {} processes",
                s.uptime / 86400,
                s.uptime / 3600 % 24,
                s.uptime / 60 % 60,
                s.load,
                s.processes.len()
            );
            self.text(2, 2, self.width - 4, &meta, Style::Muted);
        }
    }
    fn overview(&mut self, s: &System, h: &Hardware, view: View, y: usize) {
        if self.width < 72 || self.height < 22 {
            let height = if self.height < 16 {
                3
            } else if self.height < 22 {
                4
            } else {
                6
            };
            let r = Rect {
                x: 1,
                y,
                w: self.width - 2,
                h: height,
            };
            self.card(r, "SYSTEM", "");
            let cpu = s.cpu.first().copied().unwrap_or(0.0);
            self.pair(
                r,
                0,
                "CPU / RAM",
                &format!("{cpu:.1}% / {:.1}%", percent(s.memory_used, s.memory_total)),
                Style::Accent,
            );
            if height >= 4 {
                self.pair(
                    r,
                    1,
                    "Memory",
                    &format!("{} / {}", bytes(s.memory_used), bytes(s.memory_total)),
                    Style::Text,
                );
            }
            if height >= 6 {
                let root = s
                    .disk_space
                    .map(|(used, total)| format!("{} / {}", bytes(used), bytes(total)))
                    .unwrap_or_else(|| "unavailable".into());
                self.pair(r, 2, "Root", &root, Style::Text);
                let (rx, tx) = network_rates(s);
                self.pair(
                    r,
                    3,
                    "Net RX / TX",
                    &format!("{} / {}/s", bytes(rx), bytes(tx)),
                    Style::Text,
                );
            }
            self.processes(
                s,
                view,
                Rect {
                    x: 1,
                    y: y + height + 1,
                    w: self.width - 2,
                    h: self.height.saturating_sub(y + height + 2),
                },
            );
            return;
        }
        self.summary(s, y);
        let secondary_y = y + 7;
        let secondary_height = if self.height >= 30 { 8 } else { 5 };
        if self.width >= 110 {
            let [cores, hardware, traffic] = self.columns::<3>(secondary_y, secondary_height);
            self.cores(s, cores);
            self.hardware_brief(h, hardware);
            self.traffic(s, traffic);
        } else {
            let [hardware, traffic] = self.columns::<2>(secondary_y, secondary_height);
            self.hardware_brief(h, hardware);
            self.traffic(s, traffic);
        }
        let processes_y = secondary_y + secondary_height + 1;
        self.processes(
            s,
            view,
            Rect {
                x: 1,
                y: processes_y,
                w: self.width - 2,
                h: self.height.saturating_sub(processes_y + 1),
            },
        );
    }
    fn summary(&mut self, s: &System, y: usize) {
        let cards = self.columns::<3>(y, 6);
        let cpu = s.cpu.first().copied().unwrap_or(0.0);
        let memory = percent(s.memory_used, s.memory_total);
        for (i, r) in cards.iter().copied().enumerate() {
            let (title, value, detail, amount, note) = match i {
                0 => (
                    "PROCESSOR",
                    format!("{cpu:.1}%"),
                    format!(
                        "{} cores | {}",
                        s.cpu.len().saturating_sub(1),
                        s.mhz
                            .map(|v| format!("{v} MHz"))
                            .unwrap_or_else(|| "clock N/A".into())
                    ),
                    cpu,
                    format!("Load {}", s.load),
                ),
                1 => (
                    "MEMORY",
                    format!("{memory:.1}%"),
                    format!("{} / {}", bytes(s.memory_used), bytes(s.memory_total)),
                    memory,
                    format!(
                        "Swap {} / {}",
                        bytes_short(s.swap_used),
                        bytes_short(s.swap_total)
                    ),
                ),
                _ => match s.disk_space {
                    Some((used, total)) => (
                        "STORAGE",
                        format!("{:.1}%", percent(used, total)),
                        format!("{} / {}", bytes(used), bytes(total)),
                        percent(used, total),
                        "Root filesystem /".into(),
                    ),
                    None => (
                        "STORAGE",
                        "N/A".into(),
                        "Root filesystem".into(),
                        0.0,
                        "Counters unavailable".into(),
                    ),
                },
            };
            self.card(r, title, "");
            self.text(r.x + 2, r.y + 1, r.width(), &value, Style::Title);
            self.text(r.x + 2, r.y + 2, r.width(), &detail, Style::Muted);
            self.meter(r.x + 2, r.y + 3, r.width(), amount);
            self.text(r.x + 2, r.y + 4, r.width(), &note, Style::Muted);
        }
    }
    fn cores(&mut self, s: &System, r: Rect) {
        self.card(r, "CPU CORES", "busy %");
        let available = r.h - 2;
        let cores = s.cpu.get(1..).unwrap_or(&[]);
        let per_row = if cores.len() > available && r.width() >= 32 {
            2
        } else {
            1
        };
        let capacity = available * per_row;
        let count = if cores.len() > capacity {
            capacity.saturating_sub(per_row)
        } else {
            cores.len()
        };
        for (i, value) in cores.iter().take(count).enumerate() {
            let width = r.width() / per_row;
            let x = r.x + 2 + (i % per_row) * width;
            let y = r.y + 1 + i / per_row;
            self.text(x, y, 4, &format!("{:02}", i), Style::Muted);
            self.meter(x + 4, y, width.saturating_sub(12), *value);
            self.right(
                x,
                y,
                width.saturating_sub(2),
                &format!("{value:4.1}%"),
                usage_style(*value),
            );
        }
        if count < cores.len() {
            self.text(
                r.x + 2,
                r.y + r.h - 2,
                r.width(),
                &format!("+{} additional cores", cores.len() - count),
                Style::Muted,
            );
        }
    }
    fn hardware_brief(&mut self, h: &Hardware, r: Rect) {
        self.card(r, "HARDWARE", "2 Details");
        let thermal = first_sensor(h, " C");
        let voltage = first_sensor(h, " V");
        let power = first_sensor(h, " W");
        // Narrow cards use separate lines instead of clipping a whole GPU sentence.
        if let Some(gpu) = h.gpus.first() {
            let (name, metrics) = gpu.split_once(": ").unwrap_or(("GPU", gpu));
            self.pair(
                r,
                0,
                name,
                metrics.split(" | ").next().unwrap_or("unavailable"),
                Style::Text,
            );
            if r.h >= 8 {
                for (i, field) in metrics.split(" | ").skip(1).take(2).enumerate() {
                    let (name, value) = field.split_once(' ').unwrap_or((field, ""));
                    self.pair(r, i + 1, name, value, Style::Text);
                }
            }
        } else {
            self.pair(r, 0, "GPU", "unavailable", Style::Muted);
        }
        let start = if r.h >= 8 { 3 } else { 1 };
        self.pair(
            r,
            start,
            "Temperature",
            thermal.unwrap_or("unavailable"),
            Style::Text,
        );
        self.pair(
            r,
            start + 1,
            "Voltage",
            voltage.unwrap_or("unavailable"),
            Style::Text,
        );
        if r.h >= 8 {
            self.pair(
                r,
                start + 2,
                "Power",
                power.unwrap_or("unavailable"),
                Style::Text,
            );
        }
    }
    fn traffic(&mut self, s: &System, r: Rect) {
        self.card(r, "TRAFFIC", "bytes / s");
        let (rx, tx) = network_rates(s);
        self.pair(r, 0, "NET RX", &format!("{}/s", bytes(rx)), Style::Text);
        self.pair(r, 1, "NET TX", &format!("{}/s", bytes(tx)), Style::Text);
        let read = s.disks.iter().map(|d| d.read).sum::<f64>() as u64;
        let write = s.disks.iter().map(|d| d.write).sum::<f64>() as u64;
        if r.h >= 8 {
            self.line(r.x + 2, r.y + 3, r.width(), '─');
            self.pair(
                r,
                3,
                "DISK READ",
                &format!("{}/s", bytes(read)),
                Style::Text,
            );
            self.pair(
                r,
                4,
                "DISK WRITE",
                &format!("{}/s", bytes(write)),
                Style::Text,
            );
            self.pair(
                r,
                5,
                "Devices",
                &format!("{} net / {} disk", s.network.len(), s.disks.len()),
                Style::Muted,
            );
        } else {
            self.pair(
                r,
                2,
                "DISK R/W",
                &format!("{}/{}", bytes(read), bytes(write)),
                Style::Text,
            );
        }
    }
    fn process_page(&mut self, s: &System, view: View, y: usize) {
        let top = if self.width >= 72 && self.height >= 24 {
            self.summary(s, y);
            y + 7
        } else {
            y
        };
        self.processes(
            s,
            view,
            Rect {
                x: 1,
                y: top,
                w: self.width - 2,
                h: self.height.saturating_sub(top + 1),
            },
        );
    }
    fn processes(&mut self, s: &System, view: View, r: Rect) {
        if r.h < 4 {
            return;
        }
        let capacity = r.h - 4;
        let count = s.processes.len();
        let end = (view.offset + capacity).min(count);
        let aside = format!(
            "{}-{} / {}",
            if count == 0 { 0 } else { view.offset + 1 },
            end,
            count
        );
        self.card(r, "PROCESSES", &aside);
        let wide = r.width() >= 64;
        let name_width = r.width().saturating_sub(if wide { 41 } else { 29 });
        let x = r.x + 2;
        let name_x = x + 9;
        let cpu_x = name_x + name_width + if wide { 5 } else { 2 };
        let rss_x = cpu_x + 9;
        let header_y = r.y + 1;
        self.right(
            x,
            header_y,
            7,
            "PID",
            if view.sort == Sort::Pid {
                Style::Accent
            } else {
                Style::Muted
            },
        );
        self.text(name_x, header_y, name_width, "NAME", Style::Muted);
        self.right(
            cpu_x,
            header_y,
            7,
            "CPU %",
            if view.sort == Sort::Cpu {
                Style::Accent
            } else {
                Style::Muted
            },
        );
        self.right(
            rss_x,
            header_y,
            9,
            "RSS",
            if view.sort == Sort::Memory {
                Style::Accent
            } else {
                Style::Muted
            },
        );
        if wide {
            self.text(name_x + name_width + 2, header_y, 1, "S", Style::Muted);
            self.right(rss_x + 11, header_y, 5, "THR", Style::Muted);
        }
        self.line(r.x + 1, r.y + 2, r.w - 2, '─');
        let mut processes: Vec<&Process> = s.processes.values().collect();
        processes.sort_unstable_by(|a, b| match view.sort {
            Sort::Cpu => b
                .cpu
                .total_cmp(&a.cpu)
                .then_with(|| b.rss.cmp(&a.rss))
                .then_with(|| a.pid.cmp(&b.pid)),
            Sort::Memory => b.rss.cmp(&a.rss).then_with(|| a.pid.cmp(&b.pid)),
            Sort::Pid => a.pid.cmp(&b.pid),
        });
        for (i, p) in processes
            .into_iter()
            .skip(view.offset)
            .take(capacity)
            .enumerate()
        {
            let y = r.y + 3 + i;
            self.right(x, y, 7, &p.pid.to_string(), Style::Muted);
            self.text(name_x, y, name_width, &p.name, Style::Text);
            self.right(
                cpu_x,
                y,
                7,
                &format!("{:.1}", p.cpu),
                if p.cpu >= 1.0 {
                    usage_style(p.cpu)
                } else {
                    Style::Text
                },
            );
            self.right(rss_x, y, 9, &bytes(p.rss), Style::Text);
            if wide {
                self.put(
                    name_x + name_width + 2,
                    y,
                    p.state,
                    if p.state == 'R' {
                        Style::Good
                    } else {
                        Style::Muted
                    },
                );
                self.right(rss_x + 11, y, 5, &p.threads.to_string(), Style::Muted);
            }
        }
        let hint = format!(" {} sort | 100% = one core ", view.sort.label());
        self.text(r.x + 2, r.y + r.h - 1, r.width(), &hint, Style::Muted);
        if count == 0 {
            self.text(x, r.y + 3, r.width(), "No readable processes", Style::Muted);
        }
    }
    fn hardware(&mut self, s: &System, h: &Hardware, view: View, y: usize) {
        let top = if self.width >= 72 && self.height >= 24 {
            let cards = self.columns::<3>(y, 5);
            for (r, (title, suffix)) in
                cards
                    .into_iter()
                    .zip([("TEMPERATURE", " C"), ("VOLTAGE", " V"), ("POWER", " W")])
            {
                self.card(r, title, "");
                if let Some(sensor) = h.sensors.iter().find(|s| s.ends_with(suffix)) {
                    let (label, value) = sensor.rsplit_once(": ").unwrap_or(("Sensor", sensor));
                    self.text(r.x + 2, r.y + 1, r.width(), value, Style::Title);
                    self.text(r.x + 2, r.y + 2, r.width(), label, Style::Muted);
                    let count = h.sensors.iter().filter(|s| s.ends_with(suffix)).count();
                    self.text(
                        r.x + 2,
                        r.y + 3,
                        r.width(),
                        &format!("{count} readable channels"),
                        Style::Muted,
                    );
                } else {
                    self.text(r.x + 2, r.y + 1, r.width(), "unavailable", Style::Muted);
                }
            }
            y + 6
        } else {
            y
        };
        let r = Rect {
            x: 1,
            y: top,
            w: self.width - 2,
            h: self.height.saturating_sub(top + 1),
        };
        let capacity = r.h.saturating_sub(3);
        let total = hardware_row_count(s, h);
        self.card(
            r,
            "HARDWARE INVENTORY",
            &format!("{} / {total}", (view.offset + 1).min(total)),
        );
        let mut row = 0;
        let mut index = 0;
        for gpu in &h.gpus {
            for field in gpu.split(" | ") {
                let (label, value) = if field.ends_with(" C") {
                    ("Temperature", field)
                } else if field.ends_with(" W") {
                    ("Power", field)
                } else {
                    field
                        .split_once(": ")
                        .or_else(|| field.split_once(' '))
                        .unwrap_or((field, ""))
                };
                if index >= view.offset && row < capacity {
                    self.pair(r, row, label, value, Style::Text);
                    row += 1;
                }
                index += 1;
            }
        }
        for sensor in &h.sensors {
            if index >= view.offset && row < capacity {
                let (label, value) = sensor.rsplit_once(": ").unwrap_or((sensor, ""));
                self.pair(r, row, label, value, Style::Text);
                row += 1;
            }
            index += 1;
        }
        for (core, usage) in s.cpu.iter().skip(1).enumerate() {
            if index >= view.offset && row < capacity {
                self.pair(
                    r,
                    row,
                    &format!("CPU {core:02} usage"),
                    &format!("{usage:.1}%"),
                    usage_style(*usage),
                );
                row += 1;
            }
            index += 1;
        }
        if total == 0 {
            self.text(
                r.x + 2,
                r.y + 1,
                r.width(),
                "No readable hardware counters",
                Style::Muted,
            );
        }
        let hint = format!(
            " j/k Scroll | {} GPU | {} sensors ",
            h.gpus.len(),
            h.sensors.len()
        );
        self.text(r.x + 2, r.y + r.h - 1, r.width(), &hint, Style::Muted);
    }
    pub fn write_ansi(&self, output: &mut String) {
        if self.width == 0 {
            return;
        }
        for (y, row) in self.cells.chunks(self.width).enumerate() {
            let _ = write!(output, "\x1b[{};1H", y + 1);
            let mut style = Style::Text;
            output.push_str("\x1b[0m");
            for cell in row {
                if self.color && style != cell.style {
                    output.push_str(cell.style.ansi());
                    style = cell.style;
                }
                output.push(cell.glyph);
            }
            // Clear the reserved final column, including stale cells after resize,
            // without erasing a whole row before its replacement is ready.
            output.push_str("\x1b[0m\x1b[K");
        }
        output.push_str("\x1b[0m\x1b[J");
    }
    pub fn write_plain(&self, output: &mut String) {
        if self.width == 0 {
            return;
        }
        for row in self.cells.chunks(self.width) {
            let end = row
                .iter()
                .rposition(|c| c.glyph != ' ')
                .map_or(0, |i| i + 1);
            for cell in &row[..end] {
                output.push(cell.glyph);
            }
            output.push('\n');
        }
    }
}

pub(super) fn hardware_row_count(s: &System, h: &Hardware) -> usize {
    s.cpu.len().saturating_sub(1)
        + h.sensors.len()
        + h.gpus
            .iter()
            .map(|gpu| gpu.split(" | ").count())
            .sum::<usize>()
}
fn first_sensor<'a>(h: &'a Hardware, suffix: &str) -> Option<&'a str> {
    h.sensors
        .iter()
        .find(|s| s.ends_with(suffix))
        .and_then(|s| s.rsplit_once(": ").map(|(_, value)| value))
}
fn network_rates(s: &System) -> (u64, u64) {
    (
        s.network.iter().map(|n| n.read).sum::<f64>() as u64,
        s.network.iter().map(|n| n.write).sum::<f64>() as u64,
    )
}
fn usage_style(value: f64) -> Style {
    if value >= 90.0 {
        Style::Warning
    } else {
        Style::Accent
    }
}
fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    }
}
fn scaled_bytes(value: u64) -> (f64, usize) {
    let (mut n, mut unit) = (value as f64, 0);
    while n >= 1024.0 && unit < 4 {
        n /= 1024.0;
        unit += 1;
    }
    (n, unit)
}
fn bytes(value: u64) -> String {
    let (n, unit) = scaled_bytes(value);
    format!("{n:.1} {}", ["B", "KiB", "MiB", "GiB", "TiB"][unit])
}
fn bytes_short(value: u64) -> String {
    let (n, unit) = scaled_bytes(value);
    format!("{n:.1}{}", ["B", "K", "M", "G", "T"][unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_labels_cannot_supply_terminal_commands_or_wide_glyphs() {
        let mut frame = Frame::new(24, 1);
        frame.text(0, 0, 24, "bad\x1b[2J\n\t\u{202e}温度", Style::Text);
        let mut plain = String::new();
        frame.write_plain(&mut plain);
        assert_eq!(plain, "bad?[2J?????\n");
        let mut ansi = String::new();
        frame.write_ansi(&mut ansi);
        assert!(!ansi.contains("\x1b[2J"));
    }
    #[test]
    fn clipping_cannot_overwrite_a_neighboring_card() {
        let mut frame = Frame::new(20, 3);
        frame.text(1, 1, 5, "an overlong name", Style::Text);
        frame.put(6, 1, '│', Style::Border);
        assert_eq!(frame.cells[20 + 6].glyph, '│');
        assert_eq!(frame.cells[20 + 5].glyph, '…');
        assert_eq!(frame.cells[20 + 7].glyph, ' ');
    }
    #[test]
    fn process_columns_keep_borders_and_padding_at_responsive_boundaries() {
        let mut system = System::new();
        let p = system.processes.entry(424242).or_default();
        p.pid = 424242;
        p.name = "long worker name that must not displace numeric columns".into();
        p.rss = 16 * 1024 * 1024 * 1024;
        p.cpu = 1234.5;
        p.state = 'R';
        p.threads = 128;
        let view = View {
            sort: Sort::Cpu,
            offset: 0,
            page: 3,
            paused: false,
            interval: Duration::from_secs(1),
        };
        for width in [41, 42, 63, 71, 72, 79, 109, 110, 111] {
            let mut frame = Frame::new(width, 8);
            frame.processes(
                &system,
                view,
                Rect {
                    x: 1,
                    y: 0,
                    w: width - 2,
                    h: 6,
                },
            );
            for y in [1, 3] {
                let row = &frame.cells[y * width..(y + 1) * width];
                assert_eq!(row[1].glyph, '│', "left border at width {width}");
                assert_eq!(row[width - 2].glyph, '│', "right border at width {width}");
                assert_eq!(
                    row[width - 3].glyph,
                    ' ',
                    "numeric padding at width {width}"
                );
            }
            let mut plain = String::new();
            frame.write_plain(&mut plain);
            assert!(plain.contains("424242"));
            assert!(plain.contains("1234.5"));
            assert!(plain.contains("16.0 GiB"));
        }
    }
}
