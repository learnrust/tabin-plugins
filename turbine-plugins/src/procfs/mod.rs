//! Structs and impls for the various files from the /proc filesystem
//!
//! Each file gets a struct to represent its data, with an associated `load`
//! function.

use std::collections::HashMap;
use std::collections::hash_map;
use std::fs::{self, File};
use std::io::{self, Read};
use std::ops::{Div, Sub};
use std::result::Result as StdResult;
use std::str::FromStr;
use std::fmt;
use std::num;

use regex::Regex;

pub mod pid;

wrapped_enum!{
    #[derive(Debug)]
    /// ProcFs errors
    pub enum ProcFsError {
        /// Errors originating in IO
        Io(io::Error),
        /// Error pulling all required data out of procfs
        InsufficientData(String),
        /// Happens when we try to parse a float from something in procfs
        InvalidData(num::ParseFloatError),
    }
}

/// All the results are results with `ProcFsError`s
pub type Result<T> = StdResult<T, ProcFsError>;

type Usages<'a> = Vec<ProcUsage<'a>>;
pub struct ProcUsages<'a>(pub Usages<'a>);

/// Represent the percent CPU utilization of a specific process over a specific
/// time period
pub struct ProcUsage<'a> {
    /// The process we're reporting on
    pub proc_stat: &'a pid::Stat,
    /// Percent time spent in user mode
    pub upercent: f64,
    /// Percent time spent in system mode
    pub spercent: f64,
    /// upercent + spercent
    pub total: f64
}

type ProcMap = HashMap<i32, pid::Stat>;
/// All the processes that are running
pub struct RunningProcs(pub ProcMap);

impl RunningProcs {
    /// Load the currently running processes from /proc/[pid]/*
    pub fn currently_running() -> Result<RunningProcs> {
        let mut procs = ProcMap::new();
        let is_digit = Regex::new(r"^[0-9]+$").unwrap();
        for entry in try!(fs::read_dir("/proc")) {
            if let Some(fname) = try!(entry).path().file_name() {
                let fname_str = fname.to_str().unwrap();
                if is_digit.is_match(fname_str) {
                    let stat = try!(pid::Stat::from_pid(fname_str));
                    procs.insert(stat.pid, stat);
                }
            }
        }
        Ok(RunningProcs(procs))
    }

    fn iter(&self) -> hash_map::Iter<i32, pid::Stat> {
        self.0.iter()
    }

    /// Collect CPU usage for each proc as compared to the total active CPU
    /// time for the system.
    ///
    /// The value for `total_cpu` should probably be the result of subtracting
    /// two `Calculations::total()`s from each other.
    pub fn percent_cpu_util_since<'a>(
        &self,
        start: &'a RunningProcs,
        total_cpu: f64
    ) -> ProcUsages<'a> {
        let me = &self.0;
        let mut usages = Usages::new();
        for (_start_pid, start_ps) in start.iter() {
            if let Some(end_ps) = me.get(&start_ps.pid) {
                let user = 100.0 * (end_ps.utime - start_ps.utime) as f64 / total_cpu as f64;
                let sys = 100.0 * (end_ps.stime - start_ps.stime) as f64 / total_cpu as f64;
                usages.push(ProcUsage {
                    proc_stat: &start_ps,
                    upercent: user,
                    spercent: sys,
                    total: user + sys
                })
            }
        }
        ProcUsages(usages)
    }
}

////////////////////////////////////////////////////////////////////////////////
// System-Level totals

/// A kind of CPU usage
///
/// Corresponds to the fields and functions in `Calculations`, q.v. for the
/// definitions.
#[derive(RustcDecodable, Debug)]
pub enum WorkSource {
    Active, ActivePlusIoWait, ActiveMinusNice,
    User, Nice, System, Irq, SoftIrq, Steal, Guest, GuestNice,
    Idle, IoWait,
}

impl fmt::Display for WorkSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::WorkSource::*;
        let s = match *self {
            Active => "active",
            ActivePlusIoWait => "active+iowait",
            ActiveMinusNice => "active-nice",
            User => "user",
            Nice => "nice",
            System => "system",
            Idle => "idle",
            IoWait => "iowait",
            Irq => "irq",
            SoftIrq => "softirq",
            Steal => "steal",
            Guest => "guest",
            GuestNice => "guestnice"
        };
        f.write_str(s)
    }
}

/// The number of calculations that have occured on this computer in a time
/// period
#[derive(PartialEq, Debug)]
pub struct Calculations {
    /// Time spent in user mode.
    pub user: f64,
    /// Time spent in user mode with low priority (nice).
    pub nice: f64,
    /// Time spent in system mode.
    pub system: f64,
    /// Time spent not doing anything in particular
    pub idle: f64,
    /// Time forced to be idle waiting for I/O to complete.
    pub iowait: f64,
    /// Time spent servicing hardware interrupts
    pub irq: f64,
    /// Time spent servicing software interrupts
    pub softirq: f64,
    /// Stolen time, which is the time spent in other operating systems when
    /// running in a virtualized environment
    pub steal: f64,
    /// Time spent running a virtual CPU for a guest operating system. AKA time
    /// we lent out to virtual machines.
    pub guest: f64,
    /// Time spent running a niced guest
    pub guest_nice: Option<f64>,
}

impl Calculations {
    /// Read /proc/stat and return its contents as a string
    fn read_procstat() -> Result<String> {
        let mut fh = try!(File::open("/proc/stat"));
        let mut contents = String::new();
        try!(fh.read_to_string(&mut contents));
        Ok(contents)
    }

    /// Build a new `Calculations` for *total* CPU jiffies from the /proc/stat
    /// pseudofile. See `load_per_cpu` for per-cpu metrics.
    pub fn load() -> Result<Calculations> {
        let contents = try!(Self::read_procstat());
        Self::from_str(&contents)
    }

    /// Build a list of per-cpu metrics from the /proc/stat file
    ///
    /// This does not include the `total` line, use `load()` for that.
    pub fn load_per_cpu() -> Result<Vec<Calculations>> {
        let contents = Self::read_procstat().unwrap();
        contents.lines().skip(1).filter(|line| line.starts_with("cpu"))
            .map(Calculations::from_line)
            .collect::<StdResult<Vec<_>, _>>()
    }

    /// Parse the entire /proc/stat file into a single `Calculations` object
    /// for total CPU
    fn from_str(contents: &str) -> Result<Calculations> {
        let mut calcs = try!(contents.lines().take(1).map(Self::from_line)
                             .collect::<StdResult<Vec<_>, _>>());
        Ok(calcs.remove(0))
    }

    /// Convert a single line from /proc/stat
    fn from_line(line: &str) -> Result<Calculations> {
        assert!(line.starts_with("cpu"));

        let usages = try!(line.split(' ').skip(1)
                          .filter(|part| part.len() > 0)
                          .map(|part| part.parse())
                          .collect::<StdResult<Vec<_>, _>>());
        Ok(Calculations {
            user: usages[0],
            nice: usages[1],
            system: usages[2],
            idle: usages[3],
            iowait: usages[4],
            irq: usages[5],
            softirq: usages[6],
            steal: usages[7],
            guest: usages[8],
            guest_nice: usages.get(9).cloned(),
        })
    }

    /// Jiffies spent non-idle
    ///
    /// This includes all processes in user space, kernel space, and time
    /// stolen by other VMs.
    pub fn active(&self) -> f64 {
        self.user + self.nice // user processes
            + self.system + self.irq + self.softirq // kernel and interrupts
            + self.steal  // other VMs stealing our precious time
    }

    /// Jiffies spent with nothing to do
    pub fn idle(&self) -> f64 {
        self.idle + self.iowait
    }

    /// Jiffies spent running child VMs
    ///
    /// This is included in `active()` by the nature of the way that
    /// /proc/stats reports things, so don't add this to that when totalling.
    #[allow(dead_code)]  // this mostly exists as documentation of what `guest` means
    pub fn virt(&self) -> f64 {
        self.guest + self.guest_nice.unwrap_or(0.0)
    }

    /// All jiffies since the kernel started tracking
    pub fn total(&self) -> f64 {
        self.active() + self.idle()
    }

    /// Return how much cpu time the specific worksource took
    ///
    /// The number returned is between 0 and 100
    pub fn percent_util_since(&self, kind: &WorkSource, start: &Calculations) -> f64 {
        let (start_val, end_val) = match *kind {
            WorkSource::Active => (start.active(), self.active()),
            WorkSource::ActivePlusIoWait => (start.active() + start.iowait,
                                             self.active() + self.iowait),
            WorkSource::ActiveMinusNice => (start.active() - start.nice,
                                            self.active() - self.nice),
            WorkSource::User => (start.user, self.user),
            WorkSource::IoWait => (start.iowait, self.iowait),
            WorkSource::Nice => (start.nice, self.nice),
            WorkSource::System => (start.system, self.system),
            WorkSource::Idle => (start.idle, self.idle),
            WorkSource::Irq => (start.irq, self.irq),
            WorkSource::SoftIrq => (start.softirq, self.softirq),
            WorkSource::Steal => (start.steal, self.steal),
            WorkSource::Guest => (start.guest, self.guest),
            WorkSource::GuestNice => (start.guest_nice.unwrap_or(0f64), self.guest_nice.unwrap_or(0f64)),
        };
        let total = (end_val - start_val) /
            (self.total() - start.total());
        total * 100f64
    }

}

impl Sub for Calculations {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Calculations {
            user: self.user - rhs.user,
            nice: self.nice - rhs.nice,
            system: self.system - rhs.system,
            idle: self.idle - rhs.idle,
            iowait: self.iowait - rhs.iowait,
            irq: self.irq - rhs.irq,
            softirq: self.softirq - rhs.softirq,
            steal: self.steal - rhs.steal,
            guest: self.guest - rhs.guest,
            guest_nice: match (self.guest_nice, rhs.guest_nice) {
                (Some(lhs), Some(rhs)) => Some(lhs - rhs),
                _ => None
            }
        }
    }
}

impl<'a> Sub for &'a Calculations {
    type Output = Calculations;
    fn sub(self, rhs: Self) -> Calculations {
        Calculations {
            user: self.user - rhs.user,
            nice: self.nice - rhs.nice,
            system: self.system - rhs.system,
            idle: self.idle - rhs.idle,
            iowait: self.iowait - rhs.iowait,
            irq: self.irq - rhs.irq,
            softirq: self.softirq - rhs.softirq,
            steal: self.steal - rhs.steal,
            guest: self.guest - rhs.guest,
            guest_nice: match (self.guest_nice, rhs.guest_nice) {
                (Some(lhs), Some(rhs)) => Some(lhs - rhs),
                _ => None
            }
        }
    }
}

impl fmt::Display for Calculations {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let total = self.total();
        let p = |v: f64| 100.0 * v / total;
        write!(f, "user={:.1} system={:.1} nice={:.1} irq={:.1} softirq={:.1} | idle={:.1} iowait={:.1} | steal={:.1} guest={:.1} guest_nice={}",
               p(self.user), p(self.system), p(self.nice), p(self.irq), p(self.softirq),
               p(self.idle), p(self.iowait),
               p(self.steal), p(self.guest), self.guest_nice.map_or(
                   "unknown".into(), |v| format!("{}", v)))
    }
}

////////////////////////////////////////////////////////////////////////////////
// Memory

/// A struct that represents overall memory usage on the system.
#[derive(PartialEq, Eq, Debug)]
pub struct MemInfo {
    pub total: Option<usize>,
    pub available: Option<usize>,
    pub free: Option<usize>,
    pub cached: Option<usize>
}

impl MemInfo {
    /// Read the data from /proc/meminfo into a `MemInfo`
    pub fn load() -> MemInfo {
        let contents = match File::open("/proc/meminfo") {
            Ok(ref mut content) => {
                let mut s = String::new();
                let _ = content.read_to_string(&mut s);
                s
            }
            Err(e) => panic!("Unable to /proc/meminfo: {:?}", e)
        };

        MemInfo::from_str(&contents)
    }
    /// Try to figure out how much memory is being used
    ///
    /// Since linux kernel 3.16 this just performs
    ///
    /// ```text
    ///     available / total * 100
    /// ```
    ///
    /// Before that we approximate available as Free + Cached, even though that
    /// is [almost certain to be incorrect][].
    ///
    /// [almost certain to be incorrect]: https://github.com/torvalds/linux/commit/34e431b0ae398fc54ea69ff85ec700722c9da773
    pub fn percent_free(&self) -> Result<f64> {
        match *self {
            MemInfo { total: Some(t), available: Some(a), .. } => {
                Ok( a as f64 / t as f64 * 100.0 )
            }
            MemInfo { total: Some(t), free: Some(f), cached: Some(c), ..} => {
                Ok( (f + c) as f64 / t as f64 * 100.0 )
            },
            _ => { Err(ProcFsError::InsufficientData(
                format!("/proc/meminfo is missing one of total, available, free, or cached: {:?}",
                        self)))
            }
        }
    }

    /// The inverse of `MemInfo::percent_free`
    pub fn percent_used(&self) -> Result<f64> {
        let free = try!(self.percent_free());
        Ok(100f64 - free)
    }

    /// Convert the contents of a string like /proc/meminfo into a MemInfo
    /// object
    fn from_str(meminfo: &str) -> Self {
        let mut word = String::new();
        let mut info = MemInfo {
            total: None,
            free: None,
            available: None,
            cached: None
        };
        let mut amount: usize;
        enum Currently {
            Total, Free, Available, Cached, Unknown, None
        };
        let mut currently = Currently::None;

        for chr in meminfo.chars() {
            // println!("'a'<= {}: {}, {} <= 'Z': {}",
            //        chr, 'A' <= chr, chr, chr <= 'z');
            match chr {
                c if 'A' <= c && c <= 'z' => {
                    word.push(chr);
                },
                ':' => {
                    match &word[..] {
                        "MemTotal" => currently = Currently::Total,
                        "MemAvailable" => currently = Currently::Available,
                        "MemFree" => currently = Currently::Free,
                        "Cached" => currently = Currently::Cached,
                        _ => currently = Currently::Unknown
                    };
                    word.clear();
                }
                x if '0' <= x && x <= '9' => {
                    word.push(chr);
                },
                ' ' | '\n' => {
                    if word.is_empty() { continue };
                    if word == "kB" { word.clear(); continue; };

                    amount = match word.parse() {
                        Ok(amount) => amount,
                        Err(e) => panic!(r#"Unable to parse number from "{}": {:?}"#, word, e)
                    };
                    word.clear();

                    match currently {
                        Currently::Total => info.total = Some(amount),
                        Currently::Free => info.free = Some(amount),
                        Currently::Available => info.available = Some(amount),
                        Currently::Cached => info.cached = Some(amount),
                        Currently::Unknown => { /* don't care */ },
                        Currently::None => {
                            panic!(
                                "Unexpectedly parsed a number before figuring out where I am: {}",
                                amount)
                        }
                    }
                }
                _ => { /* Don't care about other chars */ }
            }
        }

        info
    }
}

/// The load average of the system
///
/// Load average is number of jobs in the run queue (state R) or waiting for
/// disk I/O (state D) averaged over 1, 5, and 15 minutes.
#[derive(PartialEq, PartialOrd, Debug)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64
}

impl LoadAvg {
    /// Load from the /proc/loadavg file
    pub fn load() -> Result<LoadAvg> {
        let mut fh = try!(File::open("/proc/loadavg"));
        let mut contents = String::new();
        try!(fh.read_to_string(&mut contents));
        Self::from_str(&contents)
    }
}

impl Div<usize> for LoadAvg {
    type Output = LoadAvg;

    /// Divide by an integer. Useful to divide by the number of CPUs
    fn div(self, rhs: usize) -> LoadAvg {
        LoadAvg {
            one: self.one / rhs as f64,
            five: self.five / rhs as f64,
            fifteen: self.fifteen / rhs as f64,
        }
    }
}

impl FromStr for LoadAvg {
    type Err = ProcFsError;

    fn from_str(contents: &str) -> Result<LoadAvg> {
        let pat = Regex::new(r"[ ,]").unwrap();
        let fields = try!(pat.split(contents).take(3).map(|load| load.parse())
                          .collect::<StdResult<Vec<f64>, _>>());
        Ok(LoadAvg {
            one: fields[0],
            five: fields[1],
            fifteen: fields[2]
        })
    }
}

impl fmt::Display for LoadAvg {
    fn fmt(&self, f: &mut fmt::Formatter) -> StdResult<(), fmt::Error> {
        try!(write!(f, "{:.1} {:.1} {:.1}", self.one, self.five, self.fifteen));
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Testing

#[cfg(test)]
mod unit {
    use super::{Calculations, MemInfo, LoadAvg};
    use super::pid;
    use std::str::FromStr;

    #[test]
    fn can_parse_stat_for_system() {
        let c = Calculations::from_str(
"cpu  100 55 66 77 88 1 9 0 0 0
cpu0 415415 55 55572 37240261 7008 1 1803 0 0 0
intr 17749885 52 10 0 0 0 0 0 0 0 0 0 0 149 0 0 0 0 0 0 493792 10457659 2437665
ctxt 310
btime 143
");
        assert_eq!(c.unwrap(), Calculations {
            user: 100.0,
            nice: 55.0,
            system: 66.0,
            idle: 77.0,
            iowait: 88.0,
            irq: 1.0,
            softirq: 9.0,
            steal: 0.0,
            guest: 0.0,
            guest_nice: Some(0.0),
        });
    }

    #[test]
    fn can_parse_stat_for_process() {
        let stat = "1 (init) S 0 1 1 0 -1 4219136 40326 5752369 36 3370 16 41 25846 8061 20 0 1 0 5 34381824 610 18446744073709551615 1 1 0 0 0 0 0 4096 536962595 18446744073709551615 0 0 17 0 0 0 7 0 0 0 0 0 0 0 0 0 0";
        stat.parse::<pid::Stat>().unwrap();
    }

    #[test]
    fn parse_meminfo() {
        assert_eq!(
            MemInfo::from_str(
"Useless: 898
MemTotal: 500
MemAvailable: 20
MemFree: 280
Meaningless: 777
Cached: 200
"),
            MemInfo {
                total: Some(500),
                available: Some(20),
                free: Some(280),
                cached: Some(200)
            }
            )
    }

    #[test]
    fn meminfo_percent_free() {
        let mem = MemInfo {
            total: Some(100),
            available: Some(30),
            free: None,
            cached: None
        };
        assert_eq!(mem.percent_free().unwrap(), 30.0);

        let mem = MemInfo {
            total: Some(100),
            available: None,
            free: Some(25),
            cached: Some(20)
        };
        assert_eq!(mem.percent_free().unwrap(), 45.0);
    }

    #[test]
    fn loadavg_can_parse_space_str() {
        let avg = LoadAvg::from_str("0.1 1.5 21 5/23 938").unwrap();
        assert_eq!(avg, LoadAvg {
            one: 0.1_f64,
            five: 1.5_f64,
            fifteen: 21_f64
        });
    }

    #[test]
    fn loadavg_can_parse_comma_str() {
        let avg = LoadAvg::from_str("0.1,1.5,21").unwrap();
        assert_eq!(avg, LoadAvg {
            one: 0.1_f64,
            five: 1.5_f64,
            fifteen: 21_f64
        });
    }

    #[test]
    fn loadavg_display() {
        let string = format!("{}", LoadAvg { one: 0.888, five: 1.0, fifteen: 0.1 });
        // ooh, rounding
        assert_eq!(&string, "0.9 1.0 0.1");
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod integration {
    use super::{RunningProcs, MemInfo, LoadAvg};

    #[test]
    fn can_read_all_procs() {
        let procs = RunningProcs::currently_running().unwrap();
        assert!(procs.0.len() > 0);
    }

    #[test]
    fn meminfo_can_load() {
        let info = MemInfo::load();
        assert_eq!(info.percent_free().unwrap() + info.percent_used().unwrap(), 100.0)
    }

    #[test]
    fn loadavg_can_load() {
        LoadAvg::load().unwrap();
    }
}