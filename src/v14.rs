#![feature(portable_simd)]
#![feature(slice_split_once)]

use libc::memchr;
use memmap2::Mmap;
use std::{
    borrow::Borrow,
    collections::btree_map::Entry,
    collections::{BTreeMap, HashMap},
    fs::File,
    hash::{BuildHasher, Hash, Hasher},
    os::raw::{c_int, c_void},
    ptr::{copy, slice_from_raw_parts_mut},
    simd::{cmp::SimdPartialEq, u8x64},
    slice::from_raw_parts,
    sync::mpsc::sync_channel,
    thread::{available_parallelism, scope},
};

const SEMICOLON: u8x64 = u8x64::splat(b';');
const NEWLINE: u8x64 = u8x64::splat(b'\n');

// 1. Parallelism was added for data processing.
// `Measure-Command { cargo r --release }` gives `TotalSeconds: 5,4082363`
fn main() {
    let file = File::open("measurements-100m.txt").unwrap();
    let mem_map = unsafe { Mmap::map(&file).unwrap() };
    let mut stats = BTreeMap::new();
    scope(|scope| {
        let n_threads = available_parallelism().unwrap().get();
        let mut at = 0;
        // `mpsc::sync_channel` is just bounded `mpsc::channel`
        let (tx, rx) = sync_channel(n_threads);
        let chunk_size = mem_map.len() / n_threads;
        for _ in 0..n_threads {
            let start = at;
            let mut end = (at + chunk_size).min(mem_map.len());
            if end != mem_map.len() {
                let new_line_at = find_new_line(&mem_map[end..]);
                end = end + new_line_at + 1;
            };
            at = end;
            let tx = tx.clone();
            let mem_map = &mem_map[start..end];
            scope.spawn(move || tx.send(process(mem_map)));
        }
        drop(tx);
        for chunk in rx {
            for (k, v) in chunk {
                match stats.entry(unsafe { String::from_utf8_unchecked(k.as_ref().to_vec()) }) {
                    Entry::Vacant(entry) => {
                        entry.insert(v);
                    }
                    Entry::Occupied(entry) => {
                        let stat = entry.into_mut();
                        stat.0 = stat.0.min(v.0); // min
                        stat.1 += v.1; // sum
                        stat.2 += v.2; // count
                        stat.3 = stat.3.max(v.3); // max
                    }
                }
            }
        }
    });
    print!("{{");
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

fn process(mem_map: &[u8]) -> HashMap<StrVec, (i16, i32, usize, i16), MyHasherBuilder> {
    let mut stats: HashMap<StrVec, (i16, i32, usize, i16), MyHasherBuilder> =
        HashMap::with_capacity_and_hasher(10_000, MyHasherBuilder);
    let mut at = 0;
    while at < mem_map.len() {
        let new_line_at = at + find_new_line(unsafe { mem_map.get_unchecked(at..) });
        let line = &mem_map[at..new_line_at];
        at = new_line_at + 1;
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
    stats
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

unsafe impl Send for StrVec {}

fn find_new_line(mem_map: &[u8]) -> usize {
    let simd_line = if let Some((arr, _)) = mem_map.split_first_chunk::<64>() {
        u8x64::from_array(*arr)
    } else {
        u8x64::load_or_default(mem_map)
    };
    let mask = NEWLINE.simd_eq(simd_line);
    if let Some(index) = mask.first_set() {
        index
    } else {
        // new line wasn't found in the first 64 bytes
        let rest = unsafe { mem_map.get_unchecked(64..) };
        let next_new_line =
        // `memchr` returns a pointer to the location of the byte,
        // or a null pointer if no such byte is found
        unsafe { memchr(rest.as_ptr() as *const c_void, b'\n' as c_int, rest.len()) };
        // shouldn't reach this point, handled outside this function
        assert!(!next_new_line.is_null());
        // SAFETY: mem_char always returns a pointer in `rest`,
        // which is always valid.
        let len = unsafe { (next_new_line as *const u8).offset_from(rest.as_ptr()) };
        64 + len as usize
    }
}

fn parse_temperature(t: &[u8]) -> i16 {
    let t_len = t.len();
    // If first char is '-' → t[0] != b'-' is false → 0 * 2 - 1 = -1 → negative,
    // otherwise → true → 1 * 2 - 1 = 1 → positive.
    let sign = i16::from(t[0] != b'-') * 2 - 1;
    // offset to skip the minus sign when reading digits
    let skip = if t[0] == b'-' { 1 } else { 0 };
    // If the string is 4 bytes ("12.3") the digit of
    // the integer part must be multiplied by 100.
    // Otherwise 3 bytes ("4.5"), it's just 10.
    let mul = if t_len - skip == 4 { 100 } else { 10 };
    // first digit becomes 100 ("12.3") or 40 ("4.5")
    let t1 = mul * i16::from(t[skip] - b'0');
    // if it's a two digit value ("4.5") `t2` is ignored and second digit is `t3`,
    // otherwise it gives 20 ("12.3").
    // t_len = 5 ("-12.3"), t_len - 3 = 2, hence t[2] = 2.
    // t_len = 4 ("12.3"), t_len - 3 = 1, hence t[1] = 2.
    let t2 = if mul == 10 { 0 } else { 1 } * 10 * i16::from(t[t_len - 3] - b'0');
    // the last digit after "."
    let t3 = i16::from(t[t_len - 1] - b'0');
    sign * (t1 + t2 + t3)
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
        // `rotate_right`: (x >> n) | (x << (BITS - n))
        self.0 ^ self.0.rotate_right(33) ^ self.0.rotate_right(15)
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut word = [0u64; 2];
        unsafe {
            copy(
                bytes.as_ptr(),
                word.as_mut_ptr() as *mut u8,
                bytes.len().min(16),
            );
        }
        self.0 = word[0] ^ word[1];
    }
}

struct MyHasherBuilder;

impl BuildHasher for MyHasherBuilder {
    type Hasher = MyHasher;

    fn build_hasher(&self) -> Self::Hasher {
        MyHasher(0xcbf29ce484222325)
    }
}
