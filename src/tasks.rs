use crate::{
    Sort,
    control::Identity,
    system::{Process, System},
};

#[derive(Clone, Copy)]
struct Rank {
    id: Identity,
    cpu: f64,
    rss: u64,
}
#[derive(Default)]
pub struct Tasks {
    rows: Vec<Rank>,
    pub selected: Option<Identity>,
    pub filter: String,
    initialized: bool,
}
impl Tasks {
    pub fn refresh(&mut self, system: &System, sort: Sort) {
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
                }),
        );
        self.rows.sort_unstable_by(|a, b| match sort {
            Sort::Cpu => b
                .cpu
                .total_cmp(&a.cpu)
                .then_with(|| b.rss.cmp(&a.rss))
                .then_with(|| a.id.pid.cmp(&b.id.pid)),
            Sort::Memory => b.rss.cmp(&a.rss).then_with(|| a.id.pid.cmp(&b.id.pid)),
            Sort::Pid => a.id.pid.cmp(&b.id.pid),
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
    pub fn filtered(&mut self, system: &System, sort: Sort) {
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
    fn selected_identity_survives_resort_but_not_exit() {
        let mut system = System::new();
        for (pid, cpu) in [(10, 20.0), (20, 10.0)] {
            let p = system.processes.entry(pid).or_default();
            p.pid = pid;
            p.cpu = cpu;
        }
        let mut tasks = Tasks::default();
        tasks.refresh(&system, Sort::Cpu);
        assert_eq!(tasks.selected.unwrap().pid, 10);
        system.processes.get_mut(&20).unwrap().cpu = 40.0;
        tasks.refresh(&system, Sort::Cpu);
        assert_eq!(tasks.index(), Some(1));
        system.processes.remove(&10);
        tasks.refresh(&system, Sort::Cpu);
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
        tasks.filtered(&system, Sort::Pid);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks.selected.unwrap().pid, 12);
        tasks.filter = "WORK".into();
        tasks.filtered(&system, Sort::Pid);
        assert_eq!(tasks.selected.unwrap().pid, 12);
        tasks.filter = "absent".into();
        tasks.filtered(&system, Sort::Pid);
        assert!(tasks.selected.is_none());
    }
}
