use memmap2::Mmap;
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
};

// 1. Custom float parsing.
// `Measure-Command { cargo r --release }` gives `TotalSeconds: 63,8516366`
fn main() {
    let file = File::open("measurements-100m.txt").unwrap();
    let mem_map = unsafe { Mmap::map(&file).unwrap() };
    let mut stats: HashMap<Vec<u8>, (i16, i32, usize, i16)> = HashMap::new();
    for line in mem_map.split(|c| *c == b'\n') {
        if line.is_empty() {
            break;
        }
        let mut fields = line.rsplitn(2, |c| *c == b';');
        let temperature = fields.next().unwrap();
        let station = fields.next().unwrap();
        let temp = parse_temperature(temperature);
        let stats = match stats.get_mut(station) {
            Some(stats) => stats,
            None => stats
                .entry(station.to_vec())
                .or_insert((i16::MAX, 0, 0, i16::MIN)),
        };
        stats.0 = stats.0.min(temp);
        stats.1 += i32::from(temp);
        stats.2 += 1;
        stats.3 = stats.3.max(temp);
    }
    print!("{{");
    let stats = stats
        .into_iter()
        .map(|(k, v)| (unsafe { String::from_utf8_unchecked(k) }, v));
    let stats: BTreeMap<String, (i16, i32, usize, i16)> = BTreeMap::from_iter(stats);
    let mut stats = stats.into_iter().peekable();
    while let Some((station, (min, sum, count, max))) = stats.next() {
        let min = (min as f64) / 10.;
        let avg = sum as f64 / 10. / count as f64;
        let max = (max as f64) / 10.;
        print!("{station}={min:.1}/{avg:.1}/{max:.1}");
        if stats.peek().is_some() {
            print!(", ")
        }
    }
    print!("}}");
}

fn parse_temperature(t: &[u8]) -> i16 {
    let mut temperature: i16 = 0;
    let mut mul = 1;
    for &byte in t.iter().rev() {
        match byte {
            b'.' => continue,
            b'-' => {
                temperature = -temperature;
                break;
            }
            _ => {
                temperature += i16::from(byte - b'0') * mul;
                mul *= 10;
            }
        }
    }
    temperature
}
