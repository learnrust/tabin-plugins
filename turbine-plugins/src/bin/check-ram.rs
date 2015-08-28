extern crate rustc_serialize;

extern crate docopt;
extern crate turbine_plugins;

use docopt::Docopt;

use turbine_plugins::ExitStatus;
use turbine_plugins::procfs::{RunningProcs, MemInfo};

static USAGE: &'static str = "
Usage: check-ram [options]
       check-ram -h | --help

Options:
    -h, --help             Show this help message

    -w, --warn=<percent>   Percent used to warn at      [default: 80]
    -c, --crit=<percent>   Percent used to critical at  [default: 95]

    --show-hogs=<count>    Show most RAM-hungry procs   [default: 0]
    -v, --verbose          Always show the hogs
";

#[derive(RustcDecodable, Debug)]
struct Args {
    flag_help: bool,
    flag_warn:  f64,
    flag_crit:  f64,
    flag_show_hogs: usize,
    flag_verbose: bool
}

fn compare_status(crit: f64, warn: f64, mem: MemInfo) -> ExitStatus {
    match mem.percent_used() {
        Ok(percent) => {
            if percent > crit {
                println!("check-ram critical: {:.1}% > {}%", percent, crit);
                ExitStatus::Critical
            } else if percent > warn {
                println!("check-ram warning: {:.1}% > {}%", percent, warn);
                ExitStatus::Warning
            } else {
                println!("check-ram okay: {:.1}% < {}%", percent, warn);
                ExitStatus::Ok
            }
        },
        Err(e) => {
            println!("check-ram UNEXPECTED ERROR: {:?}", e);
            ExitStatus::Unknown
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn main() {
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.decode())
        .unwrap_or_else(|e| e.exit());
    if args.flag_help {
        print!("{}", USAGE);
        return;
    }
    let mem = MemInfo::load();
    let status = compare_status(args.flag_crit, args.flag_warn, mem);
    if args.flag_show_hogs > 0 && (status != ExitStatus::Ok ||
                                   args.flag_verbose) {
        let per_proc = RunningProcs::currently_running().unwrap();
        let mut procs = per_proc.0.values().collect::<Vec<_>>();
        procs.sort_by(|l, r| r.rss.cmp(&l.rss));
        for process in procs.iter().take(args.flag_show_hogs) {
            println!("mem: {} {} pages", process.comm, process.rss)
        }
    };
    status.exit();
}

#[cfg(test)]
mod test {
    use docopt::Docopt;
    use super::{USAGE, Args, compare_status};
    use turbine_plugins::ExitStatus;
    use turbine_plugins::procfs::MemInfo;

    #[test]
    fn usage_is_valid() {
        let _: Args = Docopt::new(USAGE).and_then(|d| d.decode()).unwrap();
    }

    #[test]
    fn alerts_when_told_to() {
        let mem = MemInfo {
            total: Some(100),
            available: Some(15),  // 15% free means 85% used
            free: None,
            cached: None
        };
        let crit_threshold = 80.0;

        assert_eq!(compare_status(crit_threshold, 25.0, mem), ExitStatus::Critical);
    }
}
