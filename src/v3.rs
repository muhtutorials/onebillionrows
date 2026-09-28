use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader},
    str::from_utf8_unchecked,
};

// 1. `lines` method isn't used to avoid `String` conversion.
// 2. `temperature` is converted to `f64` without string validation (unchecked).
// 3. Station names (hashmap keys in `stats`) are stored as `Vec<u8>` to avoid string validation.
//    They are only converted to `String` while creating `BTreeMap`,
//    and also without string validation (unchecked).
// `Measure-Command { cargo r --release }` gives `TotalSeconds: 72,6348177`.
fn main() {
    let file = File::open("measurements-100m.txt").unwrap();
    let file = BufReader::new(file);
    let mut stats: HashMap<Vec<u8>, (f64, f64, usize, f64)> = HashMap::new();
    for line in file.split(b'\n') {
        if line.is_empty() {
            break;
        }
        let line = line.unwrap();
        let mut fields = line.rsplitn(2, |c| *c == b';');
        let temperature = fields.next().unwrap();
        let station = fields.next().unwrap();
        // SAFETY: we know that input data is valid UTF-8.
        let temperature: f64 = unsafe { from_utf8_unchecked(temperature).parse().unwrap() };
        let stats = match stats.get_mut(station) {
            Some(stats) => stats,
            None => stats
                .entry(station.to_vec())
                .or_insert((f64::MAX, 0., 0, f64::MIN)),
        };
        stats.0 = stats.0.min(temperature);
        stats.1 += temperature;
        stats.2 += 1;
        stats.3 = stats.3.max(temperature);
    }
    print!("{{");
    let stats = stats
        .into_iter()
        // SAFETY: we know that input data is valid UTF-8.
        .map(|(k, v)| (unsafe { String::from_utf8_unchecked(k) }, v));
    let stats: BTreeMap<String, (f64, f64, usize, f64)> = BTreeMap::from_iter(stats);
    let mut stats = stats.into_iter().peekable();
    while let Some((station, (min, sum, count, max))) = stats.next() {
        print!("{station}={min}/{:.1}/{max}", sum / count as f64);
        if stats.peek().is_some() {
            print!(", ")
        }
    }
    print!("}}");
}
