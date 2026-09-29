#![feature(portable_simd)]
#![feature(slice_split_once)]

use libc::memchr;
use memmap2::Mmap;
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    hash::{BuildHasher, Hasher},
    iter::once,
    os::raw::{c_int, c_void},
    simd::{cmp::SimdPartialEq, u8x64},
};

const SEMICOLON: u8x64 = u8x64::splat(b';');

// 1. Custom hasher.
// `Measure-Command { cargo r --release }` gives `TotalSeconds: 22,8231286`
fn main() {
    let file = File::open("measurements-100m.txt").unwrap();
    let mem_map = unsafe { Mmap::map(&file).unwrap() };
    let mut stats: HashMap<Vec<u8>, (i16, i32, usize, i16), MyHasherBuilder> =
        HashMap::with_capacity_and_hasher(10_000, MyHasherBuilder);
    let mut at = 0;
    loop {
        let line = next_line(&mem_map, &mut at);
        if line.is_empty() {
            break;
        }
        let (station, temperature) = split_on_semicolon(line);
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

fn next_line<'a>(mem_map: &'a [u8], at: &mut usize) -> &'a [u8] {
    let rest = &mem_map[*at..];
    let next_new_line =
        // `memchr` returns a pointer to the location of the byte,
        // or a null pointer if no such byte is found
        unsafe { memchr(rest.as_ptr() as *const c_void, b'\n' as c_int, rest.len()) };
    let line = if next_new_line.is_null() {
        rest
    } else {
        // SAFETY: mem_char always returns a pointer in `rest`,
        // which is always valid.
        let len = unsafe { (next_new_line as *const u8).offset_from(rest.as_ptr()) };
        &rest[..len as usize]
    };
    // `+ 1` skips the `\n`
    *at += line.len() + 1;
    line
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

fn split_on_semicolon(line: &[u8]) -> (&[u8], &[u8]) {
    // station name is max 100 bytes + `;`
    // + max 5 bytes of temperature (-99.9)
    if line.len() > 64 {
        // slow path
        line.rsplit_once(|c| *c == b';').unwrap()
    } else {
        let simd_line = u8x64::load_or_default(line);
        let mask = SEMICOLON.simd_eq(simd_line);
        let index = mask
            .first_set()
            .expect("every line should have a semicolon");
        (&line[..index], &line[index + 1..])
    }
}

struct MyHasher(u64);

impl Hasher for MyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (chunks, remainder) = bytes.as_chunks::<8>();
        let mut last = [1u8; 8];
        last[..remainder.len()].copy_from_slice(remainder);
        for &chunk in chunks.iter().chain(once(&last)) {
            let mixed = self.0 as u128 * (u64::from_ne_bytes(chunk) as u128);
            self.0 = (mixed >> 64 ^ mixed) as u64;
        }
    }
}

struct MyHasherBuilder;

impl BuildHasher for MyHasherBuilder {
    type Hasher = MyHasher;

    fn build_hasher(&self) -> Self::Hasher {
        MyHasher(0xcbf29ce484222325)
    }
}
