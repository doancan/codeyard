//! Runs the spike measurements (#97) and prints a Markdown report. See ../README.md.
//!
//!   spike-measure <exthost> <out-dir> <name>=<file.wasm>...
//!
//! For every component: compile time and size, cold start (spawn → `ready` → first reply), warm call latency,
//! idle memory. The last component is also hot-reloaded from .wasm (Cranelift included) and from .cwasm.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const RUNS: usize = 20;
const CALLS: usize = 2000;

struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Host {
    fn spawn(exe: &Path, file: &Path) -> Host {
        let mut child = Command::new(exe)
            .arg("run")
            .arg(file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn exthost");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Host { child, stdin, stdout }
    }

    fn line(&mut self) -> String {
        let mut s = String::new();
        self.stdout.read_line(&mut s).expect("read");
        assert!(!s.is_empty(), "exthost exited");
        s.trim_end().to_owned()
    }

    fn send(&mut self, cmd: &str) -> String {
        writeln!(self.stdin, "{cmd}").unwrap();
        self.stdin.flush().unwrap();
        self.line()
    }

    fn quit(mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.child.wait();
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

/// (resident, private) in MB. Private is what the process alone costs: `phys_footprint` on macOS (Activity Monitor's
/// Memory), private dirty + swap on Linux, the private working set on Windows.
#[cfg(target_os = "linux")]
fn memory(pid: u32) -> (f64, Option<f64>) {
    let s = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
    let kb = |key: &str| {
        s.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()).unwrap_or(0.0)
    };
    (kb("Rss:") / 1024.0, Some((kb("Private_Dirty:") + kb("Swap:")) / 1024.0))
}

#[cfg(windows)]
fn memory(pid: u32) -> (f64, Option<f64>) {
    let script = format!(
        "$p = Get-Process -Id {pid}; $c = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter \"IDProcess={pid}\"; \"$($p.WorkingSet64) $($c.WorkingSetPrivate)\""
    );
    let o = Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    let mut it = s.split_whitespace().map(|v| v.parse::<f64>().unwrap_or(0.0) / 1048576.0);
    (it.next().unwrap_or(0.0), it.next())
}

#[cfg(target_os = "macos")]
fn memory(pid: u32) -> (f64, Option<f64>) {
    let ps = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    let rss = String::from_utf8_lossy(&ps.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0;
    let fp = Command::new("footprint").args(["-p", &pid.to_string()]).output().ok().and_then(|o| {
        let s = String::from_utf8_lossy(&o.stdout).into_owned();
        // "... Footprint: 12 MB (16384 bytes per page)"
        let line = s.lines().find(|l| l.contains("Footprint:"))?;
        let rest = line.split("Footprint:").nth(1)?.trim();
        let mut it = rest.split_whitespace();
        let n: f64 = it.next()?.parse().ok()?;
        Some(match it.next()? {
            "KB" => n / 1024.0,
            "GB" => n * 1024.0,
            _ => n,
        })
    });
    (rss, fp)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exe = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let parts: Vec<(String, PathBuf)> = args[3..]
        .iter()
        .map(|a| {
            let (n, p) = a.split_once('=').expect("name=file");
            (n.to_owned(), PathBuf::from(p))
        })
        .collect();

    println!("| Component | .wasm | .cwasm | Compile (median) | Cold start p50 / p95 | First reply after ready | Warm call p50 / p95 | Idle resident | Idle private |");
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut last = None;
    for (name, _) in &parts {
        let cwasm = out.join(format!("{name}.cwasm"));
        // Keep the .wasm next to the .cwasm: a sandboxed exthost reads only that folder.
        let wasm = &out.join(format!("{name}.wasm"));
        std::fs::copy(&parts.iter().find(|(n, _)| n == name).unwrap().1, wasm).unwrap();
        let bytes = std::fs::read(wasm).unwrap();
        let mut compile = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            let mut c = Command::new(&exe).arg("compile").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
            let mut stdin = c.stdin.take().unwrap();
            let b = bytes.clone();
            let w = std::thread::spawn(move || stdin.write_all(&b));
            let o = c.wait_with_output().unwrap();
            w.join().unwrap().unwrap();
            assert!(o.status.success(), "compile {name}");
            compile.push(ms(t.elapsed()));
            std::fs::write(&cwasm, &o.stdout).unwrap();
        }
        let csize = std::fs::metadata(&cwasm).unwrap().len();

        let mut cold = Vec::new();
        let mut first = Vec::new();
        for _ in 0..RUNS {
            let t = Instant::now();
            let mut h = Host::spawn(&exe, &cwasm);
            assert_eq!(h.line(), "ready");
            let ready = t.elapsed();
            let r = h.send("call world");
            assert!(r.starts_with("ok "), "{r}");
            cold.push(ms(t.elapsed()));
            first.push(ms(t.elapsed() - ready));
            h.quit();
        }

        let mut h = Host::spawn(&exe, &cwasm);
        assert_eq!(h.line(), "ready");
        let mut warm = Vec::with_capacity(CALLS);
        for i in 0..CALLS {
            let t = Instant::now();
            h.send(&format!("call https://example.com/{i} key = {i}"));
            warm.push(ms(t.elapsed()));
        }
        std::thread::sleep(Duration::from_secs(2));
        let (rss, fp) = memory(h.child.id());
        h.quit();

        println!(
            "| {name} | {:.2} MB | {:.2} MB | {:.0} ms | {:.1} / {:.1} ms | {:.2} ms | {:.3} / {:.3} ms | {rss:.1} MB | {} |",
            bytes.len() as f64 / 1e6,
            csize as f64 / 1e6,
            pct(&mut compile, 0.5),
            pct(&mut cold.clone(), 0.5),
            pct(&mut cold, 0.95),
            pct(&mut first, 0.5),
            pct(&mut warm.clone(), 0.5),
            pct(&mut warm, 0.95),
            fp.map_or("n/a".into(), |f| format!("{f:.1} MB")),
        );
        last = Some((name.clone(), wasm.clone(), cwasm));
    }

    let (name, wasm, cwasm) = last.unwrap();
    let mut h = Host::spawn(&exe, &cwasm);
    assert_eq!(h.line(), "ready");
    let mut reload = |file: &Path| {
        let mut v: Vec<f64> = (0..RUNS)
            .map(|_| {
                let t = Instant::now();
                let r = h.send(&format!("reload {}", file.display()));
                assert!(r.starts_with("reloaded "), "{r}");
                assert!(h.send("call again").starts_with("ok "));
                ms(t.elapsed())
            })
            .collect();
        (pct(&mut v.clone(), 0.5), pct(&mut v, 0.95))
    };
    let (w50, w95) = reload(&wasm);
    let (c50, c95) = reload(&cwasm);
    h.quit();
    println!();
    println!("| Hot reload of {name} | p50 | p95 |");
    println!("| --- | --- | --- |");
    println!("| from .wasm (Cranelift compile + instantiate + activate) | {w50:.0} ms | {w95:.0} ms |");
    println!("| from cached .cwasm (instantiate + activate) | {c50:.1} ms | {c95:.1} ms |");
}
