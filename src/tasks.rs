use crate::{
    Sort, SortOrder,
    control::Identity,
    system::{Process, System},
};

#[derive(Clone, Copy)]
struct Rank {
    id: Identity,
    cpu: f64,
    rss: u64,
    state: char,
    threads: u64,
}
#[derive(Default)]
pub struct Tasks {
    rows: Vec<Rank>,
    pub selected: Option<Identity>,
    pub filter: String,
    initialized: bool,
}
impl Tasks {
    pub fn refresh(&mut self, system: &System, sort: SortOrder) {
        self.rows.clear();
        let pid_filter = self.filter.parse::<u32>().ok();
        self.rows.extend(
            system
                .processes
                .values()
                .filter(|p| {
                    self.filter.is_empty()
                        || pid_filter.map_or_else(
                            || contains_ascii(&p.name, &self.filter),
                            |pid| p.pid == pid,
                        )
                })
                .map(|p| Rank {
                    id: Identity::of(p),
                    cpu: p.cpu,
                    rss: p.rss,
                    state: p.state,
                    threads: p.threads,
                }),
        );
        self.rows.sort_unstable_by(|a, b| {
            let order = match sort.column {
                Sort::Cpu => a.cpu.total_cmp(&b.cpu).then_with(|| a.rss.cmp(&b.rss)),
                Sort::Memory => a.rss.cmp(&b.rss),
                Sort::Pid => a.id.pid.cmp(&b.id.pid),
                Sort::Name => system.processes[&a.id.pid]
                    .name
                    .bytes()
                    .map(|c| c.to_ascii_lowercase())
                    .cmp(
                        system.processes[&b.id.pid]
                            .name
                            .bytes()
                            .map(|c| c.to_ascii_lowercase()),
                    ),
                Sort::State => a.state.cmp(&b.state),
                Sort::Threads => a.threads.cmp(&b.threads),
            };
            let order = if sort.descending {
                order.reverse()
            } else {
                order
            };
            order.then_with(|| a.id.pid.cmp(&b.id.pid))
        });
        if !self.initialized {
            self.selected = self.rows.first().map(|r| r.id);
            self.initialized = true;
        }
        if self
            .selected
            .is_some_and(|id| !self.rows.iter().any(|r| r.id == id))
        {
            self.selected = None;
        }
        if self.rows.capacity() > 1024 && self.rows.len() < self.rows.capacity() / 4 {
            self.rows.shrink_to(self.rows.len().max(256));
        }
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn index(&self) -> Option<usize> {
        self.selected
            .and_then(|id| self.rows.iter().position(|r| r.id == id))
    }
    pub fn step(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.selected = None;
            return;
        }
        let index = self
            .index()
            .map(|i| i.saturating_add_signed(delta).min(self.rows.len() - 1))
            .unwrap_or(if delta < 0 { self.rows.len() - 1 } else { 0 });
        self.selected = Some(self.rows[index].id);
    }
    pub fn first(&mut self) {
        self.selected = self.rows.first().map(|r| r.id);
    }
    pub fn last(&mut self) {
        self.selected = self.rows.last().map(|r| r.id);
    }
    pub fn filtered(&mut self, system: &System, sort: SortOrder) {
        self.refresh(system, sort);
        self.first();
    }
    pub fn process<'a>(&self, system: &'a System) -> Option<&'a Process> {
        let id = self.selected?;
        system
            .processes
            .get(&id.pid)
            .filter(|p| p.start_ticks() == id.start)
    }
    pub fn at<'a>(&self, system: &'a System, index: usize) -> Option<&'a Process> {
        let id = self.rows.get(index)?.id;
        system
            .processes
            .get(&id.pid)
            .filter(|p| p.start_ticks() == id.start)
    }
    pub fn offset(&self, offset: usize, visible: usize) -> usize {
        let max = self.len().saturating_sub(visible.max(1));
        let mut offset = offset.min(max);
        if let Some(index) = self.index() {
            if index < offset {
                offset = index;
            }
            if index >= offset + visible.max(1) {
                offset = index + 1 - visible.max(1);
            }
        }
        offset
    }
}
fn contains_ascii(haystack: &str, needle: &str) -> bool {
    needle.is_empty()
        || haystack
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_column_reverses_with_stable_ties_and_selection() {
        let mut system = System::new();
        for (pid, name, cpu, rss, state, threads) in [
            (11, "beta", 2.0, 100, 'S', 2),
            (22, "Alpha", 8.0, 300, 'R', 6),
            (33, "alpha", 8.0, 300, 'T', 4),
            (44, "Gamma", 1.0, 200, 'S', 4),
        ] {
            let p = system.processes.entry(pid).or_default();
            p.pid = pid;
            p.name = name.into();
            p.cpu = cpu;
            p.rss = rss;
            p.state = state;
            p.threads = threads;
        }
        let mut tasks = Tasks::default();
        tasks.refresh(&system, SortOrder::new(Sort::Pid));
        let selected = tasks.selected;
        for (column, default, reversed) in [
            (Sort::Cpu, [22, 33, 11, 44], [44, 11, 22, 33]),
            (Sort::Memory, [22, 33, 44, 11], [11, 44, 22, 33]),
            (Sort::Pid, [11, 22, 33, 44], [44, 33, 22, 11]),
            (Sort::Name, [22, 33, 11, 44], [44, 11, 22, 33]),
            (Sort::State, [22, 11, 44, 33], [33, 11, 44, 22]),
            (Sort::Threads, [22, 33, 44, 11], [11, 33, 44, 22]),
        ] {
            let mut sort = SortOrder::new(column);
            for expected in [default, reversed] {
                tasks.refresh(&system, sort);
                assert_eq!(
                    tasks.rows.iter().map(|r| r.id.pid).collect::<Vec<_>>(),
                    expected,
                    "{} {}",
                    column.label(),
                    sort.direction()
                );
                assert_eq!(tasks.selected, selected);
                sort.select(column);
            }
        }
        tasks.filter = "ALPHA".into();
        let mut sort = SortOrder::new(Sort::Name);
        for _ in 0..2 {
            tasks.filtered(&system, sort);
            assert_eq!(tasks.at(&system, 0).unwrap().pid, 22);
            assert_eq!(tasks.at(&system, 1).unwrap().pid, 33);
            assert_eq!(tasks.len(), 2);
            sort.select(Sort::Name);
        }
    }
    #[test]
    fn selected_identity_survives_resort_but_not_exit() {
        let mut system = System::new();
        for (pid, cpu) in [(10, 20.0), (20, 10.0)] {
            let p = system.processes.entry(pid).or_default();
            p.pid = pid;
            p.cpu = cpu;
        }
        let mut tasks = Tasks::default();
        tasks.refresh(&system, SortOrder::new(Sort::Cpu));
        assert_eq!(tasks.selected.unwrap().pid, 10);
        system.processes.get_mut(&20).unwrap().cpu = 40.0;
        tasks.refresh(&system, SortOrder::new(Sort::Cpu));
        assert_eq!(tasks.index(), Some(1));
        system.processes.remove(&10);
        tasks.refresh(&system, SortOrder::new(Sort::Cpu));
        assert!(tasks.selected.is_none());
        tasks.step(1);
        assert_eq!(tasks.selected.unwrap().pid, 20);
    }
    #[test]
    fn filter_selects_exact_pid_or_case_insensitive_name() {
        let mut system = System::new();
        for (pid, name) in [(12, "Worker"), (123, "other")] {
            let p = system.processes.entry(pid).or_default();
            p.pid = pid;
            p.name = name.into();
        }
        let mut tasks = Tasks {
            filter: "12".into(),
            ..Default::default()
        };
        tasks.filtered(&system, SortOrder::new(Sort::Pid));
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks.selected.unwrap().pid, 12);
        tasks.filter = "WORK".into();
        tasks.filtered(&system, SortOrder::new(Sort::Pid));
        assert_eq!(tasks.selected.unwrap().pid, 12);
        tasks.filter = "absent".into();
        tasks.filtered(&system, SortOrder::new(Sort::Pid));
        assert!(tasks.selected.is_none());
    }
}
