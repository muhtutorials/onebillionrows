#![feature(portable_simd)]
#![feature(slice_split_once)]

use libc::memchr;
use memmap2::Mmap;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap},
    fs::File,
    hash::{BuildHasher, Hash, Hasher},
    iter::once,
    os::raw::{c_int, c_void},
    ptr::slice_from_raw_parts_mut,
    simd::{cmp::SimdPartialEq, u8x64},
    slice::from_raw_parts,
};

const SEMICOLON: u8x64 = u8x64::splat(b';');

// 1. `StrVec` is used as key in `HashMap`.
// `Measure-Command { cargo r --release }` gives `TotalSeconds: 34,9894971`
fn main() {
    let file = File::open("measurements-100m.txt").unwrap();
    let mem_map = unsafe { Mmap::map(&file).unwrap() };
    let mut stats: HashMap<StrVec, (i16, i32, usize, i16), MyHasherBuilder> =
        HashMap::with_capacity_and_hasher(10_000, MyHasherBuilder);
    let mut at = 0;
    loop {
        let line = next_line(&mem_map, &mut at);
        if line.is_empty() {
            break;
        }
        let (station, temperature) = split_on_semicolon(line);
        let stats = match stats.get_mut(station) {
            Some(stats) => stats,
            None => stats
                .entry(StrVec::new(station))
                .or_insert((i16::MAX, 0, 0, i16::MIN)),
        };
        let temp = parse_temperature(temperature);
        stats.0 = stats.0.min(temp);
        stats.1 += i32::from(temp);
        stats.2 += 1;
        stats.3 = stats.3.max(temp);
    }
    print!("{{");
    let stats = stats
        .iter()
        .map(|(k, v)| (unsafe { str::from_utf8_unchecked(k.as_ref()) }, *v));
    let stats: BTreeMap<&str, (i16, i32, usize, i16)> = BTreeMap::from_iter(stats);
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

// `StrVec` is a union type with the size of 16 bytes, where the first byte is
// used as a tag. If the string is between 1-15 bytes long, then it's stored
// inside 2-16 bytes of the union. The first byte is set to `0xFF` to indicate
// that it's `inlined`. If the string exceeds 15 bytes, then it's stored on
// the heep, and its pointer is `heap.1` (*mut u8), and its length is `heap.0`
// (usize). The first byte will be `0x00`, because city names are not long
// enough to have a length that takes up the whole usize (8 bytes) value.
// `0x00` indicates that it's `heap`.
union StrVec {
    // String is stored in `[1..]`. The first byte is set to `0xFF`.
    inlined: [u8; 16],
    // String is stored on the heap. `usize` is string's length,
    // `*mut u8` is string's pointer. The first byte is `0x00`.
    heap: (usize, *mut u8),
}

impl StrVec {
    fn new(s: &[u8]) -> Self {
        if s.len() < 16 {
            let mut inlined = [0u8; 16];
            inlined[0] = 0xFF;
            inlined[1..][..s.len()].copy_from_slice(s);
            Self { inlined }
        } else {
            // `into_raw` consumes the `Box<[T]>` and returns a `*mut [T]` — a raw fat
            // pointer — without deallocating. Now we own a raw heap allocation of
            // exactly `len` elements and must eventually reconstruct
            // a Box/Vec/slice from it to free it.
            // Without `into_boxed_slice`, we couldn't safely turn a `Vec` into a
            // raw fat pointer, because we'd lose track of the capacity needed for
            // deallocation. `Box<[T]>` gives us a well-defined allocation whose size
            // is exactly `len`, so a `*mut [T]` fully describes it — no hidden capacity.
            let ptr = Box::into_raw(s.to_vec().into_boxed_slice());
            Self {
                heap: (ptr.len().to_be(), ptr as *mut u8),
            }
        }
    }
}

impl Drop for StrVec {
    fn drop(&mut self) {
        unsafe {
            if self.inlined[0] != 0xFF {
                let fat_ptr = slice_from_raw_parts_mut(self.heap.1, self.heap.0);
                let _ = Box::from_raw(fat_ptr);
            }
        }
    }
}

impl AsRef<[u8]> for StrVec {
    fn as_ref(&self) -> &[u8] {
        unsafe {
            if self.inlined[0] == 0xFF {
                let end = memchr(self.inlined.as_ptr() as *const c_void, 0x00, 16);
                if end.is_null() {
                    &self.inlined[1..]
                } else {
                    let len = (end as *const u8).offset_from(self.inlined.as_ptr());
                    &self.inlined[1..len as usize]
                }
            } else {
                let ptr = self.heap.1;
                let len = usize::from_be(self.heap.0);
                from_raw_parts(ptr, len)
            }
        }
    }
}

impl PartialEq for StrVec {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Eq for StrVec {}

impl Hash for StrVec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl Borrow<[u8]> for StrVec {
    fn borrow(&self) -> &[u8] {
        self.as_ref()
    }
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
