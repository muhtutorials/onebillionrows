use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
};

// The slowest version of the program without any optimizations.
// `Measure-Command { cargo r --release }` gives `TotalSeconds: 89,5372158`.
fn main() {
    let file = File::open("measurements-100m.txt").unwrap();
    let file = BufReader::new(file);
    let mut stats: BTreeMap<String, (f64, f64, usize, f64)> = BTreeMap::new();
    for line in file.lines() {
        let line = line.unwrap();
        let (station, temperature) = line.split_once(';').unwrap();
        let temperature: f64 = temperature.parse().unwrap();
        let stats = stats
            .entry(station.to_string())
            .or_insert((f64::MAX, 0., 0, f64::MIN));
        stats.0 = stats.0.min(temperature);
        stats.1 += temperature;
        stats.2 += 1;
        stats.3 = stats.3.max(temperature);
    }
    print!("{{");
    let mut stats = stats.into_iter().peekable();
    while let Some((station, (min, sum, count, max))) = stats.next() {
        print!("{station}={min}/{:.1}/{max}", sum / count as f64);
        if stats.peek().is_some() {
            print!(", ")
        }
    }
    print!("}}");
}
