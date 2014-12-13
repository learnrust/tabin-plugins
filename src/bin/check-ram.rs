extern crate serialize;

extern crate docopt;

use std::io::File;
use std::str::from_str;

use docopt::Docopt;

static USAGE: &'static str = "
Usage: check-ram [options]

Options:
    -h, --help             Show this help message

    -w, --warn=<percent>   Percent to warn at     [default: 80]
    -c, --crit=<percent>   Percent to critical at [default: 95]
";

#[deriving(Decodable, Show)]
struct Args {
    flag_help: bool,
    flag_warn:  f64,
    flag_crit:  f64,
}

#[deriving(Show)]
pub struct MemInfo {
    total: Option<uint>,
    available: Option<uint>,
    free: Option<uint>,
    cached: Option<uint>
}

#[deriving(Show)]
enum RamError {
    InsufficientData(String)
}

impl MemInfo {
    /// Try to figure out how much memory is being used
    ///
    /// Since linux kernel 3.16 this just performs
    ///
    ///     (total - available) / total * 100
    ///
    /// Before that we approximate available as Free + Cached, even though that
    /// is [almost certain to be incorrect][].
    ///
    /// [almost certain to be incorrect]: https://github.com/torvalds/linux/commit/34e431b0ae398fc54ea69ff85ec700722c9da773
    fn percent_free(&self) -> Result<f64, RamError> {
        match *self {
            MemInfo { total: Some(t), available: Some(a), .. } => {
                Ok( (t - a) as f64 / t as f64 * 100f64 )
            }
            MemInfo { total: Some(t), free: Some(f), cached: Some(c), ..} => {
                Ok( (t - (f + c)) as f64 / t as f64) },
            _ => { Err(RamError::InsufficientData(format!("/proc/meminfo is missing one of total, available, free, or cached: {}", self)))}
        }
    }

    fn percent_used(&self) -> Result<f64, RamError> {
        let free = try!(self.percent_free());
        Ok(100f64 - free)
    }
}

fn read_mem() -> MemInfo {
    let contents = match File::open(&Path::new("/proc/meminfo")) {
        Ok(ref mut content) => content.read_to_end().unwrap(),
        Err(e) => panic!("Unable to /proc/meminfo: {}", e)
    };
    let contents = String::from_utf8(contents).unwrap();

    let mut word = String::new();
    let mut info = MemInfo {
        total: None,
        free: None,
        available: None,
        cached: None
    };
    let mut amount: uint;
    enum Currently {
        Total, Free, Available, Cached, Unknown, None
    };
    let mut currently = Currently::None;

    for chr in contents.chars() {
        // println!("'a'<= {}: {}, {} <= 'Z': {}",
        //        chr, 'A' <= chr, chr, chr <= 'z');
        match chr {
            c if 'A' <= c && c <= 'z' => {
                word.push(chr);
            },
            ':' => {
                match word.as_slice() {
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
                if word.as_slice() == "kB" { word.clear(); continue; };

                amount = match from_str(word.as_slice()) {
                    Some(amount) => amount,
                    None => panic!(r#"Unable to parse number from "{}""#, word)
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

fn main() {
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.decode())
        .unwrap_or_else(|e| e.exit());
    if args.flag_help {
        print!("{}", USAGE);
        return;
    }
    let mem = read_mem();
    println!("Got some meminfo: {}", mem);
    match mem.percent_used() {
        Ok(percent) => if percent > args.flag_crit {
            println!("check-ram critical: {:.1}% > {}%", percent, args.flag_crit);
            std::os::set_exit_status(2);
        } else if percent > args.flag_warn {
            println!("check-ram warning: {:.1}% > {}%", percent, args.flag_warn)
        } else {
            println!("check-ram okay: {:.1}% < {}%", percent, args.flag_warn)
        },
        Err(e) => {
            println!("check-ram UNEXPECTED ERROR: {}", e);
            std::os::set_exit_status(3)
        }
    }
}
