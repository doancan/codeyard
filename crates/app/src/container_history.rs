//! Numeric resource samples, counter-safe rates, and container browser state.
use crate::container_monitor::Usage;
use std::time::Instant;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Cpu,
    Memory,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Resources,
    Details,
    Logs,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Running,
    Stopped,
}
impl StatusFilter {
    pub fn matches(self, state: &str) -> bool {
        match self {
            Self::All => true,
            Self::Running => state == "running",
            Self::Stopped => matches!(state, "exited" | "created" | "dead"),
        }
    }
}
#[derive(Default)]
pub struct Browser {
    pub log_view: crate::container_logs::View,
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub query: String,
    pub field: Option<gpui::Entity<crate::input::TextField>>,
    pub subscription: Option<gpui::Subscription>,
    pub status_filter: StatusFilter,
    pub sidebar_scroll: gpui::ScrollHandle,
    // Freeze ranking values until the user explicitly requests another sort.
    pub sort_usage: Vec<Usage>,
    pub sort: Sort,
    pub selected: Option<String>,
    pub tab: Tab,
    pub minutes: u64,
    pub detail: Option<serde_json::Value>,
    pub logs: String,
    pub loading: bool,
    pub error: Option<String>,
    pub request: u64,
    pub terminal_target: Option<String>,
}

#[derive(Clone)]
pub struct Sample {
    pub at: Instant,
    pub cpu: Option<f32>,
    pub memory: Option<f32>,
    pub network: Option<(f64, f64)>,
    pub disk: Option<(f64, f64)>,
    network_total: Option<(f64, f64)>,
    disk_total: Option<(f64, f64)>,
}
impl Sample {
    pub fn new(at: Instant, usage: &Usage, previous: Option<&Self>) -> Self {
        let network_total = pair(&usage.network);
        let disk_total = pair(&usage.disk_io);
        let rate = |current: Option<(f64, f64)>, old: Option<(f64, f64)>| {
            let elapsed = previous.map(|p| at.duration_since(p.at).as_secs_f64())?;
            let (a, b) = current?;
            let (x, y) = old?;
            (elapsed > 0. && a >= x && b >= y).then_some(((a - x) / elapsed, (b - y) / elapsed))
        };
        Self {
            at,
            cpu: Usage::percent(&usage.cpu),
            memory: Usage::percent(&usage.memory_percent),
            network: rate(network_total, previous.and_then(|p| p.network_total)),
            disk: rate(disk_total, previous.and_then(|p| p.disk_total)),
            network_total,
            disk_total,
        }
    }
}
pub fn bytes(text: &str) -> Option<f64> {
    let text = text.trim();
    let cut = text.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(text.len());
    let n = text[..cut].parse::<f64>().ok()?;
    let multiplier = match text[cut..].trim() {
        "B" | "" => 1.,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.,
        "MiB" => 1048576.,
        "GiB" => 1073741824.,
        "TiB" => 1099511627776.,
        _ => return None,
    };
    (n.is_finite() && n >= 0.).then_some(n * multiplier)
}
fn pair(text: &str) -> Option<(f64, f64)> {
    let (a, b) = text.split_once('/')?;
    Some((bytes(a)?, bytes(b)?))
}
pub fn speed(value: Option<(f64, f64)>) -> String {
    let number = |n: f64| {
        if n >= 1e6 {
            format!("{:.1} MB/s", n / 1e6)
        } else if n >= 1e3 {
            format!("{:.1} kB/s", n / 1e3)
        } else {
            format!("{n:.0} B/s")
        }
    };
    value.map(|(a, b)| format!("{} / {}", number(a), number(b))).unwrap_or_else(|| "—".into())
}
pub fn project(container: &crate::docker::Container) -> Option<&str> {
    container.labels.split(',').find_map(|p| p.strip_prefix("com.docker.compose.project=")).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates_use_elapsed_time_and_do_not_spike_on_counter_reset() {
        let at = Instant::now();
        let mut usage = Usage { network: "1MB / 2MB".into(), disk_io: "1MiB / 0B".into(), ..Usage::default() };
        let first = Sample::new(at, &usage, None);
        assert!(first.network.is_none());
        usage.network = "3MB / 6MB".into();
        let second = Sample::new(at + std::time::Duration::from_secs(2), &usage, Some(&first));
        assert_eq!(second.network, Some((1e6, 2e6)));
        usage.network = "0B / 0B".into();
        assert!(Sample::new(at + std::time::Duration::from_secs(3), &usage, Some(&second)).network.is_none());
        assert_eq!(bytes("1.5 GiB"), Some(1.5 * 1073741824.));
        assert!(bytes("NaNMB").is_none());
    }
}
